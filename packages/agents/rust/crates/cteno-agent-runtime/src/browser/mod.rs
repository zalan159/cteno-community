//! Browser Automation Module
//!
//! Interactive browser control via Chrome DevTools Protocol (CDP).
//! Provides a BrowserManager that manages per-session Chrome instances,
//! and tools for navigation, interaction, screenshots, and tab management.

pub mod adapter;
pub mod ax_tree;
pub mod cdp;
pub mod chrome;
pub mod manager;
pub mod network;
pub mod trace;
pub mod xpath;

pub use manager::BrowserManager;
