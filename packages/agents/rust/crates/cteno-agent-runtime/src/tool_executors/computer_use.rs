//! Computer Use Tool Executor
//!
//! Desktop-level screen capture and mouse/keyboard simulation.
//! Uses xcap for screenshots and enigo for input simulation.
//! Screenshots are uploaded to OSS and returned as URLs.
//!
//! **Coordinate scaling**: When the screenshot is downscaled (logical resolution
//! exceeds MAX_SCREENSHOT_DIM), all subsequent coordinate-based actions
//! automatically map from image-space coordinates back to logical screen
//! coordinates. The LLM always works in image-space; enigo always works in
//! logical-screen-space.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use base64::Engine;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub use crate::tool_executors::screenshot::CoordScale;

pub struct ComputerUseExecutor {
    /// Shared with ScreenshotExecutor so screenshot updates are visible here.
    coord_scale: Arc<Mutex<CoordScale>>,
}

impl ComputerUseExecutor {
    pub fn new(_data_dir: PathBuf, coord_scale: Arc<Mutex<CoordScale>>) -> Self {
        Self { coord_scale }
    }

    /// Map a coordinate pair from image-space to logical-screen-space.
    fn map_coords(&self, x: i32, y: i32) -> (i32, i32) {
        let s = self.coord_scale.lock().unwrap();
        let mapped_x = (x as f64 * s.x).round() as i32;
        let mapped_y = (y as f64 * s.y).round() as i32;
        (mapped_x, mapped_y)
    }

    /// Map coordinates back from logical-screen-space to image-space.
    fn unmap_coords(&self, x: i32, y: i32) -> (i32, i32) {
        let s = self.coord_scale.lock().unwrap();
        let img_x = (x as f64 / s.x).round() as i32;
        let img_y = (y as f64 / s.y).round() as i32;
        (img_x, img_y)
    }
}

#[async_trait]
impl ToolExecutor for ComputerUseExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: action")?;

        match action {
            "screenshot" => {
                let (png_bytes, _, _, target_w, target_h) =
                    crate::tool_executors::screenshot::capture_screenshot(&self.coord_scale)?;
                show_overlay(&["screenshot"]);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
                Ok(json!({
                    "type": "screenshot",
                    "screen_size": [target_w, target_h],
                    "images": [{"type": "base64", "media_type": "image/png", "data": b64}]
                })
                .to_string())
            }
            "click" => self.do_click(&input, Button::Left, false),
            "double_click" => self.do_click(&input, Button::Left, true),
            "right_click" => self.do_click(&input, Button::Right, false),
            "type" => self.do_type(&input),
            "keypress" => do_keypress(&input),
            "scroll" => self.do_scroll(&input),
            "drag" => self.do_drag(&input),
            "move" => self.do_move(&input),
            "cursor_position" => self.get_cursor_position(),
            _ => Err(format!("Unknown action: {}", action)),
        }
    }
}

impl ComputerUseExecutor {
    fn do_click(&self, input: &Value, button: Button, double: bool) -> Result<String, String> {
        let (img_x, img_y) = get_xy(input)?;
        let (x, y) = self.map_coords(img_x, img_y);
        let mut enigo = create_enigo()?;

        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| format!("Failed to move mouse: {}", e))?;

        thread::sleep(Duration::from_millis(50));

        enigo
            .button(button, Direction::Click)
            .map_err(|e| format!("Failed to click: {}", e))?;

        if double {
            thread::sleep(Duration::from_millis(50));
            enigo
                .button(button, Direction::Click)
                .map_err(|e| format!("Failed to double-click: {}", e))?;
        }

        let action_name = if double {
            "double_click"
        } else {
            match button {
                Button::Right => "right_click",
                _ => "click",
            }
        };

        let xs = x.to_string();
        let ys = y.to_string();
        show_overlay(&[action_name, &xs, &ys]);

        Ok(format!(
            "{} at image({}, {}) -> screen({}, {})",
            action_name, img_x, img_y, x, y
        ))
    }

    fn do_type(&self, input: &Value) -> Result<String, String> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: text")?;

        // Show overlay at cursor position (type action uses current cursor location)
        if let Ok(enigo_tmp) = create_enigo() {
            if let Ok((cx, cy)) = enigo_tmp.location() {
                let xs = cx.to_string();
                let ys = cy.to_string();
                show_overlay(&["type", &xs, &ys, text]);
            }
        }

        let mut enigo = create_enigo()?;

        enigo
            .text(text)
            .map_err(|e| format!("Failed to type text: {}", e))?;

        Ok(format!("Typed {} characters", text.len()))
    }

    fn do_scroll(&self, input: &Value) -> Result<String, String> {
        let (img_x, img_y) = get_xy(input)?;
        let (x, y) = self.map_coords(img_x, img_y);
        let scroll_x = input.get("scroll_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let scroll_y = input.get("scroll_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let mut enigo = create_enigo()?;

        // Move to position first
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| format!("Failed to move mouse: {}", e))?;

        thread::sleep(Duration::from_millis(50));

        if scroll_y != 0 {
            // enigo scroll: positive = up, but we want positive = down (matching OpenAI convention)
            enigo
                .scroll(-scroll_y, Axis::Vertical)
                .map_err(|e| format!("Failed to scroll vertically: {}", e))?;
        }

        if scroll_x != 0 {
            enigo
                .scroll(scroll_x, Axis::Horizontal)
                .map_err(|e| format!("Failed to scroll horizontally: {}", e))?;
        }

        let xs = x.to_string();
        let ys = y.to_string();
        let dxs = scroll_x.to_string();
        let dys = scroll_y.to_string();
        show_overlay(&["scroll", &xs, &ys, &dxs, &dys]);

        Ok(format!(
            "Scrolled at image({}, {}) -> screen({}, {}): dx={}, dy={}",
            img_x, img_y, x, y, scroll_x, scroll_y
        ))
    }

    fn do_drag(&self, input: &Value) -> Result<String, String> {
        let img_start_x = input
            .get("start_x")
            .and_then(|v| v.as_i64())
            .ok_or("Missing required parameter: start_x")? as i32;
        let img_start_y = input
            .get("start_y")
            .and_then(|v| v.as_i64())
            .ok_or("Missing required parameter: start_y")? as i32;
        let img_end_x = input
            .get("end_x")
            .and_then(|v| v.as_i64())
            .ok_or("Missing required parameter: end_x")? as i32;
        let img_end_y = input
            .get("end_y")
            .and_then(|v| v.as_i64())
            .ok_or("Missing required parameter: end_y")? as i32;

        let (start_x, start_y) = self.map_coords(img_start_x, img_start_y);
        let (end_x, end_y) = self.map_coords(img_end_x, img_end_y);

        let mut enigo = create_enigo()?;

        // Move to start
        enigo
            .move_mouse(start_x, start_y, Coordinate::Abs)
            .map_err(|e| format!("Failed to move mouse: {}", e))?;

        thread::sleep(Duration::from_millis(50));

        // Press button
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|e| format!("Failed to press button: {}", e))?;

        thread::sleep(Duration::from_millis(50));

        // Move to end
        enigo
            .move_mouse(end_x, end_y, Coordinate::Abs)
            .map_err(|e| format!("Failed to move mouse: {}", e))?;

        thread::sleep(Duration::from_millis(50));

        // Release button
        enigo
            .button(Button::Left, Direction::Release)
            .map_err(|e| format!("Failed to release button: {}", e))?;

        let x1s = start_x.to_string();
        let y1s = start_y.to_string();
        let x2s = end_x.to_string();
        let y2s = end_y.to_string();
        show_overlay(&["drag", &x1s, &y1s, &x2s, &y2s]);

        Ok(format!(
            "Dragged from image({}, {}) -> screen({}, {}) to image({}, {}) -> screen({}, {})",
            img_start_x, img_start_y, start_x, start_y, img_end_x, img_end_y, end_x, end_y
        ))
    }

    fn do_move(&self, input: &Value) -> Result<String, String> {
        let (img_x, img_y) = get_xy(input)?;
        let (x, y) = self.map_coords(img_x, img_y);
        let mut enigo = create_enigo()?;

        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| format!("Failed to move mouse: {}", e))?;

        let xs = x.to_string();
        let ys = y.to_string();
        show_overlay(&["move", &xs, &ys]);

        Ok(format!(
            "Moved cursor to image({}, {}) -> screen({}, {})",
            img_x, img_y, x, y
        ))
    }

    /// Return cursor position in **image-space** coordinates so the LLM
    /// can relate it to what it sees in the screenshot.
    fn get_cursor_position(&self) -> Result<String, String> {
        let enigo = create_enigo()?;

        let (screen_x, screen_y) = enigo
            .location()
            .map_err(|e| format!("Failed to get cursor position: {}", e))?;

        let (img_x, img_y) = self.unmap_coords(screen_x, screen_y);

        Ok(json!({
            "x": img_x,
            "y": img_y,
            "screen_x": screen_x,
            "screen_y": screen_y,
        })
        .to_string())
    }
}

/// Spawn the screen_overlay helper to show a visual indicator for the action.
/// Fire-and-forget: we don't wait for it to finish (it auto-fades and terminates).
#[cfg(target_os = "macos")]
pub(crate) fn show_overlay(args: &[&str]) {
    // Dev: CARGO_MANIFEST_DIR/helpers/screen_overlay
    // Prod: bundled in resource_dir/helpers/screen_overlay
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helpers/screen_overlay");
    let overlay_bin = if dev_path.exists() {
        dev_path
    } else {
        // Production: resource dir is next to the executable in macOS .app bundle
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.parent()
                    .map(|d| d.join("../Resources/helpers/screen_overlay"))
            })
            .unwrap_or(dev_path)
    };

    if !overlay_bin.exists() {
        log::warn!("screen_overlay binary not found at {:?}", overlay_bin);
        return;
    }

    let string_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        let _ = std::process::Command::new(&overlay_bin)
            .args(&string_args)
            .spawn();
    });
}

/// On non-macOS platforms, overlay visualization is not yet implemented.
#[cfg(not(target_os = "macos"))]
pub(crate) fn show_overlay(_args: &[&str]) {
    // No-op: visual overlay not available on this platform
}

fn get_xy(input: &Value) -> Result<(i32, i32), String> {
    let x = input
        .get("x")
        .and_then(|v| v.as_i64())
        .ok_or("Missing required parameter: x")? as i32;
    let y = input
        .get("y")
        .and_then(|v| v.as_i64())
        .ok_or("Missing required parameter: y")? as i32;
    Ok((x, y))
}

fn create_enigo() -> Result<Enigo, String> {
    Enigo::new(&Settings::default()).map_err(|e| format!("Failed to create Enigo instance: {}", e))
}

fn parse_key(key_str: &str) -> Result<Key, String> {
    match key_str.to_lowercase().as_str() {
        // Modifiers
        "meta" | "cmd" | "command" | "super" => Ok(Key::Meta),
        "ctrl" | "control" => Ok(Key::Control),
        "alt" | "option" => Ok(Key::Alt),
        "shift" => Ok(Key::Shift),

        // Navigation
        "enter" | "return" => Ok(Key::Return),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" => Ok(Key::Backspace),
        "delete" => Ok(Key::Delete),
        "space" => Ok(Key::Space),

        // Arrow keys
        "up" => Ok(Key::UpArrow),
        "down" => Ok(Key::DownArrow),
        "left" => Ok(Key::LeftArrow),
        "right" => Ok(Key::RightArrow),

        // Function keys
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" => Ok(Key::PageUp),
        "pagedown" => Ok(Key::PageDown),

        // F keys
        "f1" => Ok(Key::F1),
        "f2" => Ok(Key::F2),
        "f3" => Ok(Key::F3),
        "f4" => Ok(Key::F4),
        "f5" => Ok(Key::F5),
        "f6" => Ok(Key::F6),
        "f7" => Ok(Key::F7),
        "f8" => Ok(Key::F8),
        "f9" => Ok(Key::F9),
        "f10" => Ok(Key::F10),
        "f11" => Ok(Key::F11),
        "f12" => Ok(Key::F12),

        // CapsLock
        "capslock" => Ok(Key::CapsLock),

        // Single character
        s if s.len() == 1 => Ok(Key::Unicode(s.chars().next().unwrap())),

        _ => Err(format!("Unknown key: {}", key_str)),
    }
}

fn do_keypress(input: &Value) -> Result<String, String> {
    let keys = input
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or("Missing required parameter: keys")?;

    if keys.is_empty() {
        return Err("keys array is empty".to_string());
    }

    let mut enigo = create_enigo()?;
    let mut parsed_keys: Vec<Key> = Vec::new();

    for key_val in keys {
        let key_str = key_val.as_str().ok_or("Each key must be a string")?;
        parsed_keys.push(parse_key(key_str)?);
    }

    // Press all modifier keys first, then the final key, then release in reverse
    let (modifiers, regular): (Vec<&Key>, Vec<&Key>) = parsed_keys
        .iter()
        .partition(|k| matches!(k, Key::Meta | Key::Control | Key::Alt | Key::Shift));

    // Press modifiers
    for key in &modifiers {
        enigo
            .key(**key, Direction::Press)
            .map_err(|e| format!("Failed to press key: {}", e))?;
    }

    // Press and release regular keys
    for key in &regular {
        enigo
            .key(**key, Direction::Click)
            .map_err(|e| format!("Failed to press key: {}", e))?;
    }

    // Release modifiers in reverse
    for key in modifiers.iter().rev() {
        enigo
            .key(**key, Direction::Release)
            .map_err(|e| format!("Failed to release key: {}", e))?;
    }

    let key_names: Vec<String> = keys
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let keys_label = key_names.join("+");
    show_overlay(&["keypress", &keys_label]);

    Ok(format!("Pressed keys: {}", keys_label))
}
