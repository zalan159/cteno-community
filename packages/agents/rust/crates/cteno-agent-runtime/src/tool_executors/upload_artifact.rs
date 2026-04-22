//! Upload Artifact Tool Executor
//!
//! Uploads a local file to server-managed object storage (Aliyun OSS currently),
//! using short-lived STS credentials issued by Happy Server.
//!
//! Default mode is background to avoid blocking the agent loop on large files.

use crate::runs::{RunLogSink, RunManager};
use crate::tool::ToolExecutor;
use crate::tool_executors::path_resolver;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Value};
use sha1::Sha1;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type HmacSha1 = Hmac<Sha1>;

#[derive(Clone)]
pub struct UploadArtifactExecutor {
    run_manager: Arc<RunManager>,
    data_dir: PathBuf,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
struct StsCreds {
    access_key_id: String,
    access_key_secret: String,
    security_token: String,
    expiration: String,
}

#[derive(Debug, Clone)]
struct InitiateResp {
    file_id: String,
    bucket: String,
    endpoint: String,
    object_key: String,
    expires_at_ms: i64,
    sts: StsCreds,
}

impl UploadArtifactExecutor {
    pub fn new(run_manager: Arc<RunManager>, data_dir: PathBuf) -> Self {
        Self {
            run_manager,
            data_dir,
            http: reqwest::Client::new(),
        }
    }

    fn resolve_file_path(input: &Value) -> Result<PathBuf, String> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let workdir = input.get("workdir").and_then(|v| v.as_str());
        path_resolver::resolve_file_path(path, workdir)
    }

    fn happy_server_url() -> String {
        crate::hooks::resolved_happy_server_url()
    }

    fn machine_auth_path(&self) -> PathBuf {
        self.data_dir.join("machine_auth.json")
    }

    fn load_happy_auth_token(&self) -> Result<String, String> {
        let path = self.machine_auth_path();
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "Failed to read machine auth cache '{}': {}",
                path.display(),
                e
            )
        })?;
        let v: Value = serde_json::from_str(&raw).map_err(|e| {
            format!(
                "Failed to parse machine auth cache '{}': {}",
                path.display(),
                e
            )
        })?;
        v.get("token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                "Missing token in machine_auth.json; is the machine logged in?".to_string()
            })
    }

    async fn initiate_on_server(
        &self,
        token: &str,
        filename: &str,
        mime: &str,
        size: u64,
        ttl_days: i64,
        session_id: Option<&str>,
    ) -> Result<InitiateResp, String> {
        let url = format!(
            "{}/v1/files/initiate",
            Self::happy_server_url().trim_end_matches('/')
        );
        let mut body = json!({
            "filename": filename,
            "mime": mime,
            "size": size,
            "ttlDays": ttl_days,
        });
        if let Some(sid) = session_id {
            body["sessionId"] = serde_json::Value::String(sid.to_string());
        }
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to call /v1/files/initiate: {}", e))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("initiate failed ({}): {}", status, text));
        }

        let v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("Failed to parse initiate response: {} ({})", e, text))?;

        let file_id = v
            .get("fileId")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "initiate response missing fileId".to_string())?
            .to_string();
        let bucket = v
            .get("bucket")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "initiate response missing bucket".to_string())?
            .to_string();
        let endpoint = v
            .get("endpoint")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "initiate response missing endpoint".to_string())?
            .to_string();
        let object_key = v
            .get("objectKey")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "initiate response missing objectKey".to_string())?
            .to_string();
        let expires_at_ms = v
            .get("expiresAt")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| "initiate response missing expiresAt".to_string())?;

        let sts = v
            .get("sts")
            .ok_or_else(|| "initiate response missing sts".to_string())?;
        let access_key_id = sts
            .get("accessKeyId")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "sts missing accessKeyId".to_string())?
            .to_string();
        let access_key_secret = sts
            .get("accessKeySecret")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "sts missing accessKeySecret".to_string())?
            .to_string();
        let security_token = sts
            .get("securityToken")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "sts missing securityToken".to_string())?
            .to_string();
        let expiration = sts
            .get("expiration")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "sts missing expiration".to_string())?
            .to_string();

        Ok(InitiateResp {
            file_id,
            bucket,
            endpoint,
            object_key,
            expires_at_ms,
            sts: StsCreds {
                access_key_id,
                access_key_secret,
                security_token,
                expiration,
            },
        })
    }

    async fn complete_on_server(
        &self,
        token: &str,
        file_id: &str,
        size: u64,
        mime: &str,
    ) -> Result<(), String> {
        let url = format!(
            "{}/v1/files/complete",
            Self::happy_server_url().trim_end_matches('/')
        );
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&json!({
                "fileId": file_id,
                "size": size,
                "mime": mime,
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to call /v1/files/complete: {}", e))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("complete failed ({}): {}", status, text));
        }
        Ok(())
    }

    async fn download_url_on_server(
        &self,
        token: &str,
        file_id: &str,
    ) -> Result<(String, i64), String> {
        let url = format!(
            "{}/v1/files/{}/download",
            Self::happy_server_url().trim_end_matches('/'),
            file_id
        );
        let resp = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Failed to call /v1/files/:id/download: {}", e))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("download-url failed ({}): {}", status, text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("Failed to parse download response: {} ({})", e, text))?;
        let url = v
            .get("url")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "download response missing url".to_string())?
            .to_string();
        let expires_at = v.get("expiresAt").and_then(|x| x.as_i64()).unwrap_or(0);
        Ok((url, expires_at))
    }

    fn oss_base_url(bucket: &str, endpoint: &str, object_key: &str) -> String {
        // Access Point endpoints already include the identifier, don't prefix with bucket
        let is_access_point = endpoint.contains("oss-accesspoint");
        if is_access_point {
            format!("https://{}/{}", endpoint, object_key)
        } else {
            format!("https://{}.{}/{}", bucket, endpoint, object_key)
        }
    }

    fn oss_canonical_resource(
        bucket: &str,
        endpoint: &str,
        object_key: &str,
        query: &str,
    ) -> String {
        // Access Point uses a special bucket alias format in canonical resource
        // The alias is not the original bucket name but an internal OSS identifier
        // For simplicity, we'll use standard bucket endpoint instead of Access Point
        let is_access_point = endpoint.contains("oss-accesspoint");
        if is_access_point {
            // Access Point signature is complex - recommend using standard endpoint instead
            // For now, try using bucket name (may need to be replaced with actual alias)
            if query.is_empty() {
                format!("/{}/{}", bucket, object_key)
            } else {
                format!("/{}/{}?{}", bucket, object_key, query)
            }
        } else if query.is_empty() {
            format!("/{}/{}", bucket, object_key)
        } else {
            format!("/{}/{}?{}", bucket, object_key, query)
        }
    }

    fn oss_date_header() -> String {
        // RFC1123, explicitly GMT
        chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string()
    }

    fn oss_canonical_headers(security_token: &str) -> String {
        // Lowercase + sorted; only one header for now.
        format!("x-oss-security-token:{}\n", security_token)
    }

    fn oss_string_to_sign(
        method: &str,
        content_md5: &str,
        content_type: &str,
        date: &str,
        canonical_headers: &str,
        canonical_resource: &str,
    ) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}{}",
            method, content_md5, content_type, date, canonical_headers, canonical_resource
        )
    }

    fn oss_authorization(
        access_key_id: &str,
        access_key_secret: &str,
        string_to_sign: &str,
    ) -> Result<String, String> {
        let mut mac = HmacSha1::new_from_slice(access_key_secret.as_bytes())
            .map_err(|e| format!("Failed to init HMAC: {}", e))?;
        mac.update(string_to_sign.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = B64.encode(sig);
        Ok(format!("OSS {}:{}", access_key_id, sig_b64))
    }

    async fn oss_initiate_multipart(
        &self,
        creds: &StsCreds,
        bucket: &str,
        endpoint: &str,
        object_key: &str,
    ) -> Result<String, String> {
        let url = format!(
            "{}?uploads",
            Self::oss_base_url(bucket, endpoint, object_key)
        );
        let date = Self::oss_date_header();
        let canonical_headers = Self::oss_canonical_headers(&creds.security_token);
        let canonical_resource =
            Self::oss_canonical_resource(bucket, endpoint, object_key, "uploads");
        let string_to_sign = Self::oss_string_to_sign(
            "POST",
            "",
            "",
            &date,
            &canonical_headers,
            &canonical_resource,
        );
        let auth = Self::oss_authorization(
            &creds.access_key_id,
            &creds.access_key_secret,
            &string_to_sign,
        )?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "Date",
            HeaderValue::from_str(&date).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&auth).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "x-oss-security-token",
            HeaderValue::from_str(&creds.security_token).map_err(|e| e.to_string())?,
        );

        let resp = self
            .http
            .post(url)
            .headers(headers)
            .body("")
            .send()
            .await
            .map_err(|e| format!("OSS initiate multipart failed: {}", e))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "OSS initiate multipart failed ({}): {}",
                status, body
            ));
        }

        // Extract <UploadId>...</UploadId> from XML response.
        let re = regex::Regex::new(r"<UploadId>([^<]+)</UploadId>").unwrap();
        let upload_id = re
            .captures(&body)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .ok_or_else(|| format!("Failed to parse OSS UploadId from response: {}", body))?;

        Ok(upload_id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn oss_upload_part(
        &self,
        creds: &StsCreds,
        bucket: &str,
        endpoint: &str,
        object_key: &str,
        upload_id: &str,
        part_number: u32,
        bytes: Vec<u8>,
    ) -> Result<String, String> {
        let url = format!(
            "{}?partNumber={}&uploadId={}",
            Self::oss_base_url(bucket, endpoint, object_key),
            part_number,
            urlencoding::encode(upload_id)
        );
        let date = Self::oss_date_header();
        let canonical_headers = Self::oss_canonical_headers(&creds.security_token);
        let canonical_resource = Self::oss_canonical_resource(
            bucket,
            endpoint,
            object_key,
            &format!("partNumber={}&uploadId={}", part_number, upload_id),
        );
        let string_to_sign = Self::oss_string_to_sign(
            "PUT",
            "",
            "application/octet-stream",
            &date,
            &canonical_headers,
            &canonical_resource,
        );
        let auth = Self::oss_authorization(
            &creds.access_key_id,
            &creds.access_key_secret,
            &string_to_sign,
        )?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "Date",
            HeaderValue::from_str(&date).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&auth).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "x-oss-security-token",
            HeaderValue::from_str(&creds.security_token).map_err(|e| e.to_string())?,
        );

        let resp = self
            .http
            .put(url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("OSS upload part failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OSS upload part failed ({}): {}", status, body));
        }

        let etag = resp
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if etag.trim().is_empty() {
            return Err("OSS upload part missing ETag header".to_string());
        }
        Ok(etag)
    }

    async fn oss_complete_multipart(
        &self,
        creds: &StsCreds,
        bucket: &str,
        endpoint: &str,
        object_key: &str,
        upload_id: &str,
        parts: &[(u32, String)],
    ) -> Result<(), String> {
        let url = format!(
            "{}?uploadId={}",
            Self::oss_base_url(bucket, endpoint, object_key),
            urlencoding::encode(upload_id)
        );
        let date = Self::oss_date_header();
        let canonical_headers = Self::oss_canonical_headers(&creds.security_token);
        let canonical_resource = Self::oss_canonical_resource(
            bucket,
            endpoint,
            object_key,
            &format!("uploadId={}", upload_id),
        );

        let mut xml = String::new();
        xml.push_str("<CompleteMultipartUpload>");
        for (num, etag) in parts {
            xml.push_str("<Part>");
            xml.push_str(&format!("<PartNumber>{}</PartNumber>", num));
            // ETag should include quotes for OSS; keep whatever server returns.
            xml.push_str(&format!("<ETag>{}</ETag>", etag));
            xml.push_str("</Part>");
        }
        xml.push_str("</CompleteMultipartUpload>");

        let string_to_sign = Self::oss_string_to_sign(
            "POST",
            "",
            "application/xml",
            &date,
            &canonical_headers,
            &canonical_resource,
        );
        let auth = Self::oss_authorization(
            &creds.access_key_id,
            &creds.access_key_secret,
            &string_to_sign,
        )?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "Date",
            HeaderValue::from_str(&date).map_err(|e| e.to_string())?,
        );
        headers.insert("Content-Type", HeaderValue::from_static("application/xml"));
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&auth).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "x-oss-security-token",
            HeaderValue::from_str(&creds.security_token).map_err(|e| e.to_string())?,
        );

        let resp = self
            .http
            .post(url)
            .headers(headers)
            .body(xml)
            .send()
            .await
            .map_err(|e| format!("OSS complete multipart failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "OSS complete multipart failed ({}): {}",
                status, body
            ));
        }
        Ok(())
    }

    async fn oss_put_object(
        &self,
        creds: &StsCreds,
        bucket: &str,
        endpoint: &str,
        object_key: &str,
        bytes: Vec<u8>,
        mime: &str,
    ) -> Result<(), String> {
        let url = Self::oss_base_url(bucket, endpoint, object_key);
        let date = Self::oss_date_header();
        let canonical_headers = Self::oss_canonical_headers(&creds.security_token);
        let canonical_resource = Self::oss_canonical_resource(bucket, endpoint, object_key, "");
        let string_to_sign = Self::oss_string_to_sign(
            "PUT",
            "",
            mime,
            &date,
            &canonical_headers,
            &canonical_resource,
        );
        let auth = Self::oss_authorization(
            &creds.access_key_id,
            &creds.access_key_secret,
            &string_to_sign,
        )?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "Date",
            HeaderValue::from_str(&date).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&auth).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "Content-Type",
            HeaderValue::from_str(mime).map_err(|e| e.to_string())?,
        );
        headers.insert(
            "x-oss-security-token",
            HeaderValue::from_str(&creds.security_token).map_err(|e| e.to_string())?,
        );

        let resp = self
            .http
            .put(url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("OSS put object failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OSS put object failed ({}): {}", status, body));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_file(
        &self,
        sink: &RunLogSink,
        token: &str,
        file_path: &Path,
        filename: &str,
        mime: &str,
        size: u64,
        ttl_days: i64,
        session_id: Option<&str>,
    ) -> Result<(String, String, i64), String> {
        sink.line(&format!(
            "[artifact-upload] initiating filename='{}' mime='{}' size={} ttl_days={}",
            filename, mime, size, ttl_days
        ))
        .await;

        let init = self
            .initiate_on_server(token, filename, mime, size, ttl_days, session_id)
            .await?;

        sink.line(&format!(
            "[artifact-upload] initiated file_id={} bucket={} endpoint={} object_key={}",
            init.file_id, init.bucket, init.endpoint, init.object_key
        ))
        .await;

        // Heuristic: multipart for > 16MB.
        const PART_SIZE: usize = 16 * 1024 * 1024;

        if (size as usize) <= PART_SIZE {
            let bytes = tokio::fs::read(file_path)
                .await
                .map_err(|e| format!("Failed to read file: {}", e))?;
            self.oss_put_object(
                &init.sts,
                &init.bucket,
                &init.endpoint,
                &init.object_key,
                bytes,
                mime,
            )
            .await?;
        } else {
            let upload_id = self
                .oss_initiate_multipart(&init.sts, &init.bucket, &init.endpoint, &init.object_key)
                .await?;
            sink.line(&format!(
                "[artifact-upload] multipart started upload_id={}",
                upload_id
            ))
            .await;

            let mut file = tokio::fs::File::open(file_path)
                .await
                .map_err(|e| format!("Failed to open file: {}", e))?;
            let mut part_number: u32 = 1;
            let mut uploaded: u64 = 0;
            let mut parts: Vec<(u32, String)> = Vec::new();

            loop {
                let mut buf = vec![0u8; PART_SIZE];
                let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf)
                    .await
                    .map_err(|e| format!("Failed to read file chunk: {}", e))?;
                if n == 0 {
                    break;
                }
                buf.truncate(n);

                let etag = self
                    .oss_upload_part(
                        &init.sts,
                        &init.bucket,
                        &init.endpoint,
                        &init.object_key,
                        &upload_id,
                        part_number,
                        buf,
                    )
                    .await?;
                parts.push((part_number, etag));

                uploaded += n as u64;
                sink.line(&format!(
                    "[artifact-upload] uploaded part={} uploaded_bytes={}/{}",
                    part_number, uploaded, size
                ))
                .await;

                part_number += 1;
            }

            self.oss_complete_multipart(
                &init.sts,
                &init.bucket,
                &init.endpoint,
                &init.object_key,
                &upload_id,
                &parts,
            )
            .await?;
            sink.line("[artifact-upload] multipart complete").await;
        }

        self.complete_on_server(token, &init.file_id, size, mime)
            .await?;
        sink.line(&format!(
            "[artifact-upload] server complete ok file_id={}",
            init.file_id
        ))
        .await;

        let (url, url_expires_at) = self.download_url_on_server(token, &init.file_id).await?;
        sink.line(&format!(
            "[artifact-upload] download url issued expires_at_ms={}",
            url_expires_at
        ))
        .await;

        Ok((init.file_id, url, url_expires_at))
    }
}

#[async_trait]
impl ToolExecutor for UploadArtifactExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let file_path = Self::resolve_file_path(&input)?;
        if !file_path.exists() {
            return Err(format!("File not found: {}", file_path.display()));
        }
        if !file_path.is_file() {
            return Err(format!("Not a file: {}", file_path.display()));
        }

        let meta = std::fs::metadata(&file_path)
            .map_err(|e| format!("Failed to stat file '{}': {}", file_path.display(), e))?;
        let size = meta.len();
        if size == 0 {
            return Err("File is empty".to_string());
        }
        if size > 2_000_000_000 {
            return Err(format!("File too large (max 2GB): {} bytes", size));
        }

        let ttl_days = input.get("ttl_days").and_then(|v| v.as_i64()).unwrap_or(7);
        if ttl_days != 7 && ttl_days != 30 {
            return Err("ttl_days must be 7 or 30".to_string());
        }

        let filename = input
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                file_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string()
            });

        let mime = input
            .get("mime")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let background = input
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let notify = input
            .get("notify")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let hard_timeout_secs = input
            .get("hard_timeout_secs")
            .and_then(|v| v.as_u64())
            .or(Some(0));
        let hard_timeout_secs = hard_timeout_secs.and_then(|v| if v == 0 { None } else { Some(v) });

        let token = self.load_happy_auth_token()?;

        // Extract session_id early — used both for server association and background run ownership.
        let session_id_for_server = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // Use a sink even for sync mode to keep consistent logs.
        // (We don't currently have a run record in sync mode.)
        if !background {
            let tmp =
                std::env::temp_dir().join(format!("cteno_upload_{}.log", uuid::Uuid::new_v4()));
            let sink = RunLogSink::new(&tmp).await?;

            let (file_id, url, url_expires_at) = self
                .upload_file(
                    &sink,
                    &token,
                    &file_path,
                    &filename,
                    &mime,
                    size,
                    ttl_days,
                    session_id_for_server.as_deref(),
                )
                .await?;

            return Ok(serde_json::to_string_pretty(&json!({
                "status": "uploaded",
                "file": {
                    "file_id": file_id,
                    "name": filename,
                    "mime": mime,
                    "size": size,
                    "ttl_days": ttl_days,
                    "download_url": url,
                    "download_url_expires_at": url_expires_at,
                }
            }))
            .unwrap_or_default());
        }

        let session_id = session_id_for_server
            .clone()
            .ok_or_else(|| "background=true requires internal __session_id".to_string())?;

        let run_manager = self.run_manager.clone();
        let me = self.clone();
        let file_path_clone = file_path.clone();
        let filename_clone = filename.clone();
        let mime_clone = mime.clone();
        let token_clone = token.clone();
        let size_clone = size;
        let ttl_days_clone = ttl_days;
        let session_id_clone = session_id.clone();

        // Create the run record immediately, then execute upload in background.
        let record = run_manager
            .start_task_run(
                &session_id,
                "upload_artifact",
                notify,
                hard_timeout_secs,
                move |sink, _run_id| async move {
                    match me
                        .upload_file(
                            &sink,
                            &token_clone,
                            &file_path_clone,
                            &filename_clone,
                            &mime_clone,
                            size_clone,
                            ttl_days_clone,
                            Some(&session_id_clone),
                        )
                        .await
                    {
                        Ok((file_id, url, _url_expires_at)) => {
                            sink.line(&format!(
                                "[artifact-upload-complete] file_id={} url={}",
                                file_id, url
                            ))
                            .await;
                            Ok(0)
                        }
                        Err(err) => {
                            sink.line(&format!(
                                "[artifact-upload-failed] reason={}",
                                err.replace('\n', " ")
                            ))
                            .await;
                            Err(err)
                        }
                    }
                },
            )
            .await?;

        Ok(format!(
            "文件 '{}' 正在后台上传（{} bytes）。上传完成后下载链接会自动发送给用户。",
            filename, size
        ))
    }

    fn supports_background(&self) -> bool {
        true
    }
}
