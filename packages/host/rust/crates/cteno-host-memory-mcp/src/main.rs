//! `cteno-memory-mcp` — stdio MCP server binary.
//!
//! Spawned by the host once per active project. Each agent session (Claude /
//! Codex / Gemini / Cteno) connects to the same binary through its vendor
//! configuration file; all four end up reading/writing the same Markdown bank.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use cteno_host_memory_mcp::{MemoryCore, MemoryServer};
use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(
    name = "cteno-memory-mcp",
    version,
    about = "Cross-vendor memory MCP server"
)]
struct Cli {
    /// Project root. Project-scope memory lives under `{project_dir}/.cteno/memory/`.
    /// Omit to disable project scope (global-only mode).
    #[arg(long, env = "CTENO_MEMORY_PROJECT_DIR")]
    project_dir: Option<PathBuf>,

    /// Global memory directory. Defaults to `~/.cteno/memory/`.
    #[arg(long, env = "CTENO_MEMORY_GLOBAL_DIR")]
    global_dir: Option<PathBuf>,
}

fn default_global_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve user home directory")?;
    Ok(home.join(".cteno").join("memory"))
}

fn expand(p: PathBuf) -> PathBuf {
    PathBuf::from(shellexpand::tilde(&p.to_string_lossy()).into_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    // stderr-only logging: MCP server speaks JSON-RPC over stdout, so any stdout
    // chatter would corrupt the transport.
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_env("CTENO_MEMORY_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    let cli = Cli::parse();
    let project_dir = cli.project_dir.map(expand);
    let global_dir = match cli.global_dir {
        Some(p) => expand(p),
        None => default_global_dir()?,
    };

    std::fs::create_dir_all(&global_dir)
        .with_context(|| format!("create global dir {global_dir:?}"))?;
    if let Some(p) = &project_dir {
        let project_mem = p.join(".cteno").join("memory");
        std::fs::create_dir_all(&project_mem)
            .with_context(|| format!("create project memory dir {project_mem:?}"))?;
    }

    tracing::info!(
        project_dir = ?project_dir,
        global_dir = ?global_dir,
        "cteno-memory-mcp starting on stdio"
    );

    let core = Arc::new(MemoryCore::new(project_dir, global_dir));
    let service = MemoryServer::new(core)
        .serve(stdio())
        .await
        .context("serve on stdio")?;
    service.waiting().await.context("service waiting")?;
    Ok(())
}
