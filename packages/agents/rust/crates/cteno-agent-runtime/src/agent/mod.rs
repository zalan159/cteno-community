//! Agent Module
//!
//! Provides sub-agent execution capabilities. Agents defined in AGENT.md files
//! can be registered as tools callable by the parent agent. Each sub-agent runs
//! its own independent ReAct loop with tool isolation and timeout control.

pub mod executor;
