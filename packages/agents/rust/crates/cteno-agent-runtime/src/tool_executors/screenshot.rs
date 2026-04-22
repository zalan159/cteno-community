//! Screenshot Tool Executor
//!
//! Standalone screen capture tool. Captures the desktop, saves PNG to workdir,
//! uploads to OSS, and returns text result with image_url for frontend rendering.
//! LLM never receives base64 image data from this tool.
//!
//! Shares a `CoordScale` with `ComputerUseExecutor` so that taking a
//! screenshot via either tool correctly updates the coordinate mapping
//! used by mouse/keyboard actions.

use crate::tool::ToolExecutor;
use crate::tool_executors::oss_upload::OssUploader;
use async_trait::async_trait;
use image::codecs::png::PngEncoder;
use image::ImageEncoder;
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use xcap::Monitor;

/// Maximum screenshot dimension (width or height).
/// Screenshots larger than this (in logical pixels) are downscaled.
const MAX_SCREENSHOT_DIM: u32 = 2048;

/// Coordinate scale factors: multiply image-space coords by these to get
/// logical-screen-space coords. `(1.0, 1.0)` means no scaling needed.
pub struct CoordScale {
    pub x: f64,
    pub y: f64,
}

impl Default for CoordScale {
    fn default() -> Self {
        Self { x: 1.0, y: 1.0 }
    }
}

/// Capture the desktop screenshot, encode to PNG, and update coord_scale.
/// Returns (png_bytes, enigo_w, enigo_h, target_w, target_h).
/// This is a shared function used by both ScreenshotExecutor and ComputerUseExecutor.
pub fn capture_screenshot(
    coord_scale: &Arc<Mutex<CoordScale>>,
) -> Result<(Vec<u8>, u32, u32, u32, u32), String> {
    let (png_bytes, enigo_w, enigo_h, target_w, target_h, scale_factor) = {
        let monitors = Monitor::all().map_err(|e| format!("Failed to list monitors: {}", e))?;
        let monitor = monitors.first().ok_or("No monitor found")?;

        let img = monitor
            .capture_image()
            .map_err(|e| format!("Failed to capture screenshot: {}", e))?;

        let (pixel_w, pixel_h) = (img.width(), img.height());
        let scale_factor = monitor.scale_factor().unwrap_or(1.0) as f64;

        // Determine the coordinate space that enigo operates in:
        // - macOS: logical points (physical / scale_factor) because CGEvent uses points
        // - Windows: physical pixels because SetCursorPos/GetSystemMetrics use pixels
        //   for DPI-aware processes (Tauri is per-monitor DPI aware)
        #[cfg(target_os = "macos")]
        let (enigo_w, enigo_h) = (
            (pixel_w as f64 / scale_factor) as u32,
            (pixel_h as f64 / scale_factor) as u32,
        );
        #[cfg(not(target_os = "macos"))]
        let (enigo_w, enigo_h) = (pixel_w, pixel_h);

        // Downscale for LLM if enigo-space resolution exceeds our limit
        let (target_w, target_h) = if enigo_w > MAX_SCREENSHOT_DIM || enigo_h > MAX_SCREENSHOT_DIM {
            let ratio = MAX_SCREENSHOT_DIM as f64 / enigo_w.max(enigo_h) as f64;
            (
                (enigo_w as f64 * ratio) as u32,
                (enigo_h as f64 * ratio) as u32,
            )
        } else {
            (enigo_w, enigo_h)
        };

        let img = if target_w != pixel_w || target_h != pixel_h {
            image::imageops::resize(
                &img,
                target_w,
                target_h,
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };

        // Encode to PNG
        let mut buf = Cursor::new(Vec::new());
        PngEncoder::new(&mut buf)
            .write_image(
                img.as_raw(),
                target_w,
                target_h,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("Failed to encode PNG: {}", e))?;

        (
            buf.into_inner(),
            enigo_w,
            enigo_h,
            target_w,
            target_h,
            scale_factor,
        )
    };
    // monitors (and all non-Send types) are now dropped

    // Update coordinate scale for subsequent actions (shared with ComputerUseExecutor).
    // image-space coord * scale = enigo-space coord
    {
        let mut s = coord_scale.lock().unwrap();
        s.x = enigo_w as f64 / target_w as f64;
        s.y = enigo_h as f64 / target_h as f64;
        log::info!(
            "Screenshot: scale_factor={:.2}, enigo_space={}x{}, image={}x{}, coord_scale=({:.3}, {:.3})",
            scale_factor, enigo_w, enigo_h, target_w, target_h, s.x, s.y
        );
    }

    Ok((png_bytes, enigo_w, enigo_h, target_w, target_h))
}

pub struct ScreenshotExecutor {
    data_dir: PathBuf,
    /// Shared coordinate scale — updated on each screenshot, read by ComputerUseExecutor.
    coord_scale: Arc<Mutex<CoordScale>>,
    /// OSS uploader for uploading screenshots to cloud storage.
    oss_uploader: Arc<OssUploader>,
}

impl ScreenshotExecutor {
    pub fn new(
        data_dir: PathBuf,
        coord_scale: Arc<Mutex<CoordScale>>,
        oss_uploader: Arc<OssUploader>,
    ) -> Self {
        Self {
            data_dir,
            coord_scale,
            oss_uploader,
        }
    }
}

#[async_trait]
impl ToolExecutor for ScreenshotExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        // Capture screenshot
        let (png_bytes, _, _, target_w, target_h) = capture_screenshot(&self.coord_scale)?;

        crate::tool_executors::computer_use::show_overlay(&["screenshot"]);

        // Save PNG to workdir/.screenshots/
        let screenshots_dir = if let Some(workdir) = input.get("workdir").and_then(|v| v.as_str()) {
            PathBuf::from(workdir).join(".screenshots")
        } else {
            self.data_dir.join("screenshots")
        };
        tokio::fs::create_dir_all(&screenshots_dir)
            .await
            .map_err(|e| format!("Failed to create screenshots dir: {}", e))?;

        let filename = format!(
            "screenshot_{}.png",
            chrono::Utc::now().format("%Y%m%d_%H%M%S")
        );
        let save_path = screenshots_dir.join(&filename);
        tokio::fs::write(&save_path, &png_bytes)
            .await
            .map_err(|e| format!("Failed to write screenshot: {}", e))?;

        log::info!(
            "[Screenshot] Saved {}x{} to {} ({}KB)",
            target_w,
            target_h,
            save_path.display(),
            png_bytes.len() / 1024
        );

        // Upload to OSS (graceful degradation on failure)
        let image_url = match self
            .oss_uploader
            .upload_bytes_and_get_url(&png_bytes, &filename, "image/png", 7)
            .await
        {
            Ok(url) => {
                log::info!("[Screenshot] Uploaded to OSS: {}", url);
                Some(url)
            }
            Err(e) => {
                log::warn!(
                    "[Screenshot] OSS upload failed (degrading gracefully): {}",
                    e
                );
                None
            }
        };

        // Return JSON with image_url for frontend rendering, no base64 images array.
        // LLM only sees text metadata; frontend renders via image_url.
        let mut result = json!({
            "type": "screenshot",
            "screen_size": [target_w, target_h],
            "image_path": save_path.display().to_string(),
        });
        if let Some(url) = image_url {
            result["image_url"] = json!(url);
        }

        Ok(result.to_string())
    }
}
