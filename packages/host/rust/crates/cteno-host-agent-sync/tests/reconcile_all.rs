//! Integration test: run `reconcile_all` across all four vendors at once and
//! verify each vendor's native layout appears correctly.

use std::collections::BTreeMap;

use cteno_host_agent_sync::{
    reconcile_all, ClaudeSyncer, CodexSyncer, CtenoSyncer, GeminiSyncer, McpSpec, McpTransport,
    PersonaSpec, SkillSpec, VendorSyncer,
};
use tempfile::TempDir;

fn sample_mcp() -> McpSpec {
    McpSpec {
        name: "cteno-memory".into(),
        command: "cteno-memory-mcp".into(),
        args: vec!["--project-dir".into(), "/proj".into()],
        env: BTreeMap::new(),
        transport: McpTransport::Stdio,
        host_managed: true,
    }
}

#[test]
fn reconcile_all_writes_every_vendor_layout() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path();
    std::fs::write(
        project.join("AGENTS.md"),
        "You are a helpful engineer. Follow the repo conventions.",
    )
    .unwrap();

    // Authoritative persona source.
    let persona_src = project.join(".cteno/agents/reviewer.md");
    std::fs::create_dir_all(persona_src.parent().unwrap()).unwrap();
    std::fs::write(
        &persona_src,
        "---\nname: reviewer\ndescription: code reviewer\n---\nReview changes carefully.",
    )
    .unwrap();

    // Authoritative skill source.
    let skill_src = project.join(".cteno/skills/web-search");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(
        skill_src.join("SKILL.md"),
        "---\nname: web-search\n---\nSearch the web.",
    )
    .unwrap();

    // Codex adapter with a sandboxed config path.
    let codex_cfg = tmp.path().join("codex-config.toml");
    let codex = CodexSyncer::with_config_path(codex_cfg.clone());
    let claude = ClaudeSyncer::new();
    let gemini = GeminiSyncer::new();
    let cteno = CtenoSyncer::new();

    let vendors: &[&dyn VendorSyncer] = &[&claude, &codex, &gemini, &cteno];
    let report = reconcile_all(
        project,
        &project.join("AGENTS.md"),
        &[sample_mcp()],
        &[PersonaSpec {
            name: "reviewer".into(),
            description: "code reviewer".into(),
            markdown: "".into(),
            source_path: persona_src.clone(),
        }],
        &[SkillSpec {
            name: "web-search".into(),
            source_dir: skill_src.clone(),
        }],
        vendors,
    )
    .unwrap();

    // Claude outputs
    assert!(
        project.join(".mcp.json").exists(),
        "claude .mcp.json missing"
    );
    let claude_mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap()).unwrap();
    assert!(claude_mcp["mcpServers"]["cteno-memory"].is_object());
    assert_eq!(
        std::fs::read_to_string(project.join(".claude/agents/reviewer.md"))
            .unwrap()
            .trim(),
        std::fs::read_to_string(&persona_src).unwrap().trim()
    );
    assert_eq!(
        std::fs::read_to_string(project.join(".claude/skills/web-search/SKILL.md"))
            .unwrap()
            .trim(),
        "---\nname: web-search\n---\nSearch the web."
    );
    assert_eq!(
        std::fs::read_to_string(project.join("CLAUDE.md"))
            .unwrap()
            .trim(),
        "You are a helpful engineer. Follow the repo conventions."
    );

    // Gemini outputs
    let g_settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".gemini/settings.json")).unwrap(),
    )
    .unwrap();
    assert!(g_settings["mcpServers"]["cteno-memory"].is_object());
    assert!(project.join(".gemini/agents/reviewer.md").exists());
    assert!(project.join(".gemini/skills/web-search").exists());
    assert!(project.join("GEMINI.md").exists());

    // Codex outputs (user-scoped config)
    let codex_back = std::fs::read_to_string(&codex_cfg).unwrap();
    assert!(
        codex_back.contains("[mcp_servers.cteno-memory]"),
        "{codex_back}"
    );

    // Cteno outputs — only PROMPT.md symlink
    assert!(project.join(".cteno/PROMPT.md").exists());
    assert_eq!(
        std::fs::read_to_string(project.join(".cteno/PROMPT.md"))
            .unwrap()
            .trim(),
        "You are a helpful engineer. Follow the repo conventions."
    );

    // Sanity: report saw several writes.
    assert!(
        report.wrote.len() >= 8,
        "expected many writes, got {}",
        report.wrote.len()
    );
}
