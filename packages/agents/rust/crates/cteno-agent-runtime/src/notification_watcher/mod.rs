//! macOS Notification Watcher
//!
//! Monitors the macOS notification center database for new notifications
//! and delivers them to subscribed Persona chat sessions.

pub mod macos_db;
pub mod models;
pub mod store;
pub mod watcher;

pub use models::{NotificationApp, NotificationRecord, NotificationSubscription};
pub use store::NotificationStore;
pub use watcher::NotificationWatcher;
