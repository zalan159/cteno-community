//! End-to-end: spawn the `cteno-memory-mcp` binary, talk to it through a real
//! rmcp stdio client, exercise each of the four tools. This mirrors how any
//! vendor CLI (Claude / Codex / Gemini / Cteno) will invoke us.

use rmcp::{
    model::CallToolRequestParam,
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_cteno-memory-mcp")
}

fn mk(name: &'static str, body: Value) -> CallToolRequestParam {
    let arguments = match body {
        Value::Object(m) => Some(m),
        _ => Some(Map::new()),
    };
    CallToolRequestParam {
        name: name.into(),
        arguments,
        meta: None,
        task: None,
    }
}

fn extract_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn spawn_client(
    proj: &TempDir,
    global: &TempDir,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(bin_path()).configure(|cmd| {
            cmd.arg("--project-dir")
                .arg(proj.path())
                .arg("--global-dir")
                .arg(global.path());
        }))?;
    Ok(().serve(transport).await?)
}

#[tokio::test]
async fn roundtrip_save_recall_read_list() -> anyhow::Result<()> {
    let proj = TempDir::new()?;
    let global = TempDir::new()?;
    let client = spawn_client(&proj, &global).await?;

    let tools = client.list_all_tools().await?;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in ["memory_save", "memory_recall", "memory_read", "memory_list"] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}, got {names:?}"
        );
    }

    client
        .call_tool(mk(
            "memory_save",
            json!({
                "file_path": "knowledge/rust",
                "content": "Rust borrow checker enforces ownership at compile time."
            }),
        ))
        .await?;

    client
        .call_tool(mk(
            "memory_save",
            json!({
                "file_path": "notes/general",
                "content": "Global fact: the speed of light is finite.",
                "scope": "global"
            }),
        ))
        .await?;

    let recall = client
        .call_tool(mk(
            "memory_recall",
            json!({"query": "borrow ownership light"}),
        ))
        .await?;
    let text = extract_text(&recall);
    assert!(
        text.contains("[project]"),
        "recall missing [project]: {text}"
    );
    assert!(text.contains("[global]"), "recall missing [global]: {text}");
    assert!(text.contains("ownership") || text.contains("light"));

    let list = client.call_tool(mk("memory_list", json!({}))).await?;
    let ltext = extract_text(&list);
    assert!(ltext.contains("[project] knowledge/rust.md"), "{ltext}");
    assert!(ltext.contains("[global] notes/general.md"), "{ltext}");

    let read = client
        .call_tool(mk(
            "memory_read",
            json!({
                "file_path": "knowledge/rust",
                "scope": "project"
            }),
        ))
        .await?;
    assert!(extract_text(&read).contains("borrow checker"));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn save_rejects_parent_traversal() -> anyhow::Result<()> {
    let proj = TempDir::new()?;
    let global = TempDir::new()?;
    let client = spawn_client(&proj, &global).await?;

    let result = client
        .call_tool(mk(
            "memory_save",
            json!({
                "file_path": "../../etc/escape",
                "content": "should fail"
            }),
        ))
        .await;

    match result {
        Err(_) => { /* protocol-level error is acceptable */ }
        Ok(call) => assert_eq!(
            call.is_error,
            Some(true),
            "path traversal should error, got {call:?}"
        ),
    }

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn recall_respects_type_filter() -> anyhow::Result<()> {
    let proj = TempDir::new()?;
    let global = TempDir::new()?;
    let client = spawn_client(&proj, &global).await?;

    client
        .call_tool(mk(
            "memory_save",
            json!({
                "file_path": "log/a",
                "content": "alpha common marker",
                "type": "user"
            }),
        ))
        .await?;
    client
        .call_tool(mk(
            "memory_save",
            json!({
                "file_path": "log/b",
                "content": "alpha common marker",
                "type": "feedback"
            }),
        ))
        .await?;

    let hit = client
        .call_tool(mk(
            "memory_recall",
            json!({"query": "alpha common", "type": "feedback"}),
        ))
        .await?;
    let text = extract_text(&hit);
    assert!(
        text.contains("log/b.md"),
        "type filter should keep feedback entry: {text}"
    );
    assert!(
        !text.contains("log/a.md"),
        "type filter should exclude user entry: {text}"
    );

    client.cancel().await?;
    Ok(())
}
