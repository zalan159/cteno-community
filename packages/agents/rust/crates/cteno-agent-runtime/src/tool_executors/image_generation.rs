//! Image Generation Tool Executor
//!
//! Generates images via Happy Server image proxy in background mode only.
//! Always returns immediately with a run_id, then downloads image and sends notification when complete.

use crate::runs::RunManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn happy_server_url() -> String {
    crate::hooks::resolved_happy_server_url()
}

struct ImageGenRequest {
    model: String,
    prompt: String,
    negative_prompt: Option<String>,
    size: Option<String>,
    seed: Option<i32>,
}

pub struct ImageGenerationExecutor {
    run_manager: Arc<RunManager>,
    app_data_dir: PathBuf,
}

impl ImageGenerationExecutor {
    pub fn new(run_manager: Arc<RunManager>, app_data_dir: PathBuf) -> Self {
        Self {
            run_manager,
            app_data_dir,
        }
    }

    fn read_auth_token(&self) -> Result<String, String> {
        let path = self.app_data_dir.join("machine_auth.json");
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "Image generation requires Happy Server connection. Failed to read auth: {}",
                e
            )
        })?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse machine auth cache: {}", e))?;
        parsed
            .get("token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "No auth token found in machine_auth.json".to_string())
    }

    fn parse_request(&self, input: &Value) -> Result<ImageGenRequest, String> {
        let prompt = input["prompt"]
            .as_str()
            .ok_or("Missing 'prompt' parameter")?
            .to_string();

        if prompt.trim().is_empty() {
            return Err("Prompt cannot be empty".to_string());
        }

        let model = input["model"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "qwen-image-max".to_string());

        let negative_prompt = input["negative_prompt"].as_str().map(|s| s.to_string());
        let size = input["size"].as_str().map(|s| s.to_string());
        let seed = input["seed"].as_i64().map(|n| n as i32);

        Ok(ImageGenRequest {
            model,
            prompt,
            negative_prompt,
            size,
            seed,
        })
    }
}

#[async_trait]
impl ToolExecutor for ImageGenerationExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "image_generation tool requires internal __session_id".to_string())?;

        self.execute_background(input, Some(session_id)).await
    }

    fn supports_background(&self) -> bool {
        true
    }

    async fn execute_background(
        &self,
        input: Value,
        session_id: Option<String>,
    ) -> Result<String, String> {
        let session_id = session_id.ok_or("Missing session_id for background execution")?;
        let req = self.parse_request(&input)?;

        let notify = input
            .get("notify")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let hard_timeout_secs = input.get("timeout").and_then(|v| v.as_u64()).or(Some(300));

        let workdir = input
            .get("workdir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let auth_token = self.read_auth_token()?;
        let server_url = happy_server_url();

        let record = self
            .run_manager
            .start_task_run(
                &session_id,
                "image_generation",
                notify,
                hard_timeout_secs,
                move |sink, _run_id| async move {
                    let client = reqwest::Client::new();

                    sink.line("[图像生成] 开始处理...").await;
                    sink.line(&format!("提示词: {}", req.prompt)).await;
                    sink.line(&format!("模型: {}", req.model)).await;

                    // Call Happy Server image proxy
                    let url = format!("{}/v1/image/generate", server_url.trim_end_matches('/'));
                    let mut body = json!({
                        "model": req.model,
                        "prompt": req.prompt,
                    });
                    if let Some(ref neg) = req.negative_prompt {
                        body["negative_prompt"] = json!(neg);
                    }
                    if let Some(ref size) = req.size {
                        body["size"] = json!(size);
                    }
                    if let Some(seed) = req.seed {
                        body["seed"] = json!(seed);
                    }

                    let resp = client
                        .post(&url)
                        .bearer_auth(&auth_token)
                        .json(&body)
                        .timeout(std::time::Duration::from_secs(120))
                        .send()
                        .await
                        .map_err(|e| format!("图像代理请求失败: {}", e))?;

                    let status = resp.status();
                    let resp_text = resp.text().await.unwrap_or_default();

                    if !status.is_success() {
                        let err = format!("图像代理错误 ({}): {}", status, resp_text);
                        sink.line(&format!("[错误] {}", err)).await;
                        return Err(err);
                    }

                    let resp_json: Value = serde_json::from_str(&resp_text)
                        .map_err(|e| format!("解析代理响应失败: {}", e))?;

                    let image_url = resp_json["url"]
                        .as_str()
                        .ok_or("代理响应中未找到图片 URL")?
                        .to_string();

                    sink.line(&format!("[完成] 图片 URL: {}", image_url)).await;

                    // Download image to local directory
                    sink.line("[下载] 正在保存图片到本地...").await;

                    let http_client = reqwest::Client::new();
                    match http_client.get(&image_url).send().await {
                        Ok(resp) => {
                            if !resp.status().is_success() {
                                let err = format!("下载失败: HTTP {}", resp.status());
                                sink.line(&format!("[错误] {}", err)).await;
                                return Err(err);
                            }

                            match resp.bytes().await {
                                Ok(bytes) => {
                                    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                    let filename = format!("image_{}.png", timestamp);
                                    let filepath = workdir.join(&filename);

                                    match tokio::fs::write(&filepath, bytes).await {
                                        Ok(_) => {
                                            sink.line(&format!(
                                                "[保存成功] 文件路径: {}",
                                                filepath.display()
                                            ))
                                            .await;
                                            sink.line("\n✅ 图像生成并保存成功！").await;
                                            sink.line(&format!("提示词: {}", req.prompt)).await;
                                            sink.line(&format!("文件: {}", filepath.display()))
                                                .await;
                                            Ok(0)
                                        }
                                        Err(e) => {
                                            let err = format!("保存文件失败: {}", e);
                                            sink.line(&format!("[错误] {}", err)).await;
                                            Err(err)
                                        }
                                    }
                                }
                                Err(e) => {
                                    let err = format!("读取图片数据失败: {}", e);
                                    sink.line(&format!("[错误] {}", err)).await;
                                    Err(err)
                                }
                            }
                        }
                        Err(e) => {
                            let err = format!("下载图片失败: {}", e);
                            sink.line(&format!("[错误] {}", err)).await;
                            Err(err)
                        }
                    }
                },
            )
            .await?;

        Ok(format!(
            "图像生成任务已启动！\nrun_id: {}\n\n完成后图片将自动保存到当前目录，我会通知您。",
            record.run_id
        ))
    }
}
