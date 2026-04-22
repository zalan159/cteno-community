//! Lightweight OSS Upload Helper
//!
//! Uploads a local file to Aliyun OSS via Happy Server STS credentials
//! and returns a signed download URL. Used by ReadExecutor for image vision support.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Value};
use sha1::Sha1;
use std::path::{Path, PathBuf};

type HmacSha1 = Hmac<Sha1>;

struct StsCreds {
    access_key_id: String,
    access_key_secret: String,
    security_token: String,
}

struct InitiateResp {
    file_id: String,
    bucket: String,
    endpoint: String,
    object_key: String,
    sts: StsCreds,
}

pub struct OssUploader {
    http: reqwest::Client,
    data_dir: PathBuf,
}

impl OssUploader {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            http: reqwest::Client::new(),
            data_dir,
        }
    }

    fn happy_server_url() -> String {
        crate::hooks::resolved_happy_server_url()
    }

    fn load_happy_auth_token(&self) -> Result<String, String> {
        let path = self.data_dir.join("machine_auth.json");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read machine auth '{}': {}", path.display(), e))?;
        let v: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse machine auth: {}", e))?;
        v.get("token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "Missing token in machine_auth.json".to_string())
    }

    async fn initiate(
        &self,
        token: &str,
        filename: &str,
        mime: &str,
        size: u64,
        ttl_days: i64,
    ) -> Result<InitiateResp, String> {
        let url = format!(
            "{}/v1/files/initiate",
            Self::happy_server_url().trim_end_matches('/')
        );
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&json!({ "filename": filename, "mime": mime, "size": size, "ttlDays": ttl_days }))
            .send()
            .await
            .map_err(|e| format!("initiate failed: {}", e))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("initiate failed ({}): {}", status, text));
        }

        let v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("Failed to parse initiate response: {}", e))?;

        let sts = v.get("sts").ok_or("initiate missing sts")?;
        Ok(InitiateResp {
            file_id: v["fileId"].as_str().ok_or("missing fileId")?.to_string(),
            bucket: v["bucket"].as_str().ok_or("missing bucket")?.to_string(),
            endpoint: v["endpoint"]
                .as_str()
                .ok_or("missing endpoint")?
                .to_string(),
            object_key: v["objectKey"]
                .as_str()
                .ok_or("missing objectKey")?
                .to_string(),
            sts: StsCreds {
                access_key_id: sts["accessKeyId"]
                    .as_str()
                    .ok_or("missing accessKeyId")?
                    .to_string(),
                access_key_secret: sts["accessKeySecret"]
                    .as_str()
                    .ok_or("missing accessKeySecret")?
                    .to_string(),
                security_token: sts["securityToken"]
                    .as_str()
                    .ok_or("missing securityToken")?
                    .to_string(),
            },
        })
    }

    async fn complete(
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
            .json(&json!({ "fileId": file_id, "size": size, "mime": mime }))
            .send()
            .await
            .map_err(|e| format!("complete failed: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("complete failed ({}): {}", status, text));
        }
        Ok(())
    }

    async fn download_url(&self, token: &str, file_id: &str) -> Result<String, String> {
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
            .map_err(|e| format!("download-url failed: {}", e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("download-url failed ({}): {}", status, text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("Failed to parse download response: {}", e))?;
        v.get("url")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "download response missing url".to_string())
    }

    fn oss_base_url(bucket: &str, endpoint: &str, object_key: &str) -> String {
        if endpoint.contains("oss-accesspoint") {
            format!("https://{}/{}", endpoint, object_key)
        } else {
            format!("https://{}.{}/{}", bucket, endpoint, object_key)
        }
    }

    fn oss_canonical_resource(bucket: &str, endpoint: &str, object_key: &str) -> String {
        let _ = endpoint; // Access Point handling simplified for put-only
        format!("/{}/{}", bucket, object_key)
    }

    fn oss_date_header() -> String {
        chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string()
    }

    fn oss_authorization(
        access_key_id: &str,
        access_key_secret: &str,
        string_to_sign: &str,
    ) -> Result<String, String> {
        let mut mac = HmacSha1::new_from_slice(access_key_secret.as_bytes())
            .map_err(|e| format!("HMAC init failed: {}", e))?;
        mac.update(string_to_sign.as_bytes());
        let sig = mac.finalize().into_bytes();
        Ok(format!("OSS {}:{}", access_key_id, B64.encode(sig)))
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
        let canonical_headers = format!("x-oss-security-token:{}\n", creds.security_token);
        let canonical_resource = Self::oss_canonical_resource(bucket, endpoint, object_key);
        let string_to_sign = format!(
            "PUT\n\n{}\n{}\n{}{}",
            mime, date, canonical_headers, canonical_resource
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
            .map_err(|e| format!("OSS put failed: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OSS put failed ({}): {}", status, body));
        }
        Ok(())
    }

    /// Upload in-memory bytes to OSS and return a signed download URL.
    /// Used by screenshot tools that already have PNG bytes in memory.
    pub async fn upload_bytes_and_get_url(
        &self,
        bytes: &[u8],
        filename: &str,
        mime: &str,
        ttl_days: i64,
    ) -> Result<String, String> {
        let token = self.load_happy_auth_token()?;
        let size = bytes.len() as u64;

        let init = self
            .initiate(&token, filename, mime, size, ttl_days)
            .await?;
        self.oss_put_object(
            &init.sts,
            &init.bucket,
            &init.endpoint,
            &init.object_key,
            bytes.to_vec(),
            mime,
        )
        .await?;
        self.complete(&token, &init.file_id, size, mime).await?;
        let url = self.download_url(&token, &init.file_id).await?;

        log::info!(
            "[OssUploader] Uploaded {} ({} bytes) → file_id={}",
            filename,
            size,
            init.file_id
        );
        Ok(url)
    }

    /// Upload a local file to OSS and return a signed download URL.
    /// Images are always < 20MB so only single PUT is used (no multipart).
    pub async fn upload_and_get_url(
        &self,
        path: &Path,
        mime: &str,
        ttl_days: i64,
    ) -> Result<String, String> {
        let token = self.load_happy_auth_token()?;
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("image");
        let size = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("Failed to stat file: {}", e))?
            .len();

        let init = self
            .initiate(&token, filename, mime, size, ttl_days)
            .await?;

        let bytes = tokio::fs::read(path)
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
        self.complete(&token, &init.file_id, size, mime).await?;
        let url = self.download_url(&token, &init.file_id).await?;

        log::info!(
            "[OssUploader] Uploaded {} → file_id={}",
            path.display(),
            init.file_id
        );
        Ok(url)
    }
}
