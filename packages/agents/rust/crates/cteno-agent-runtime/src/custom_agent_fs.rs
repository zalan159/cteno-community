use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct CustomAgentFileSpec {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub allowed_tools: Option<Vec<String>>,
    pub excluded_tools: Option<Vec<String>>,
    pub permission_mode: Option<String>,
}

pub fn write_custom_agent_dir(
    target_dir: &Path,
    spec: &CustomAgentFileSpec,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(target_dir)
        .map_err(|e| format!("Failed to create agent dir: {}", e))?;

    let md_path = target_dir.join("AGENT.md");
    std::fs::write(&md_path, render_custom_agent_markdown(spec))
        .map_err(|e| format!("Failed to write AGENT.md: {}", e))?;

    Ok(md_path)
}

pub fn delete_custom_agent_dir(target_dir: &Path) -> Result<(), String> {
    if target_dir.exists() {
        std::fs::remove_dir_all(target_dir)
            .map_err(|e| format!("Failed to remove agent dir {}: {}", target_dir.display(), e))?;
    }
    Ok(())
}

pub fn render_custom_agent_markdown(spec: &CustomAgentFileSpec) -> String {
    let mut frontmatter = format!(
        "---\nname: \"{}\"\ndescription: \"{}\"\nversion: \"1.0.0\"\ntype: \"autonomous\"\n",
        spec.name.replace('"', "\\\""),
        spec.description.replace('"', "\\\""),
    );

    if let Some(model) = spec.model.as_deref() {
        frontmatter.push_str(&format!("model: \"{}\"\n", model));
    }
    if let Some(ref tools) = spec.tools {
        frontmatter.push_str(&format!("tools: {:?}\n", tools));
    }
    if let Some(ref skills) = spec.skills {
        frontmatter.push_str(&format!("skills: {:?}\n", skills));
    }
    if let Some(ref allowed_tools) = spec.allowed_tools {
        frontmatter.push_str(&format!("allowed_tools: {:?}\n", allowed_tools));
    }
    if let Some(ref excluded_tools) = spec.excluded_tools {
        frontmatter.push_str(&format!("excluded_tools: {:?}\n", excluded_tools));
    }
    if let Some(permission_mode) = spec.permission_mode.as_deref() {
        frontmatter.push_str(&format!("permission_mode: \"{}\"\n", permission_mode));
    }
    frontmatter.push_str("---\n\n");

    if spec.instructions.is_empty() {
        format!("{}# {}\n", frontmatter, spec.name)
    } else {
        format!("{}{}", frontmatter, spec.instructions)
    }
}
