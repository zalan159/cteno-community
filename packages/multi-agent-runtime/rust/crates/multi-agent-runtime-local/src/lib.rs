use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use multi_agent_protocol::{
    MultiAgentProvider, RoleSpec, WorkspaceEvent, WorkspaceSpec, WorkspaceState,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const RUNTIME_DIR: &str = ".multi-agent-runtime";

#[derive(Debug, Error)]
pub enum LocalPersistenceError {
    #[error("workspace spec has no cwd; local persistence requires a workspace directory")]
    MissingWorkspaceDirectory,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProviderBinding {
    pub role_id: String,
    pub provider_conversation_id: String,
    pub kind: ProviderConversationKind,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConversationKind {
    Session,
    Thread,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProviderState {
    pub workspace_id: String,
    pub provider: MultiAgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_conversation_id: Option<String>,
    pub member_bindings: std::collections::BTreeMap<String, PersistedProviderBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct LocalWorkspacePersistence {
    root: PathBuf,
}

impl LocalWorkspacePersistence {
    pub fn from_spec(spec: &WorkspaceSpec) -> Result<Self, LocalPersistenceError> {
        let cwd = spec
            .cwd
            .as_deref()
            .ok_or(LocalPersistenceError::MissingWorkspaceDirectory)?;
        Ok(Self::from_workspace(cwd, &spec.id))
    }

    pub fn from_workspace(cwd: impl AsRef<Path>, workspace_id: &str) -> Self {
        Self {
            root: cwd.as_ref().join(RUNTIME_DIR).join(workspace_id),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn initialize_workspace(&self, spec: &WorkspaceSpec) -> Result<(), LocalPersistenceError> {
        fs::create_dir_all(self.roles_dir())?;
        self.write_json(self.workspace_spec_path(), spec)?;
        self.write_json(self.state_path(), &WorkspaceStateSeed::from_spec(spec))?;
        self.write_json(
            self.provider_state_path(),
            &PersistedProviderState {
                workspace_id: spec.id.clone(),
                provider: spec.provider,
                root_conversation_id: None,
                member_bindings: Default::default(),
                metadata: None,
                updated_at: now_string(),
            },
        )?;
        for role in &spec.roles {
            let role_dir = self.roles_dir().join(&role.id);
            fs::create_dir_all(&role_dir)?;
            fs::write(role_dir.join("AGENT.md"), render_agent_markdown(spec, role))?;
        }
        Ok(())
    }

    pub fn ensure_workspace_initialized(
        &self,
        spec: &WorkspaceSpec,
    ) -> Result<(), LocalPersistenceError> {
        if !self.workspace_spec_path().exists() {
            self.initialize_workspace(spec)?;
        }
        Ok(())
    }

    pub fn persist_runtime(
        &self,
        state: &WorkspaceState,
        events: &[WorkspaceEvent],
        provider_state: &PersistedProviderState,
    ) -> Result<(), LocalPersistenceError> {
        fs::create_dir_all(&self.root)?;
        self.write_json(self.state_path(), state)?;
        self.write_json(self.provider_state_path(), provider_state)?;
        if !events.is_empty() {
            self.append_jsonl(self.events_path(), events)?;
        }
        Ok(())
    }

    pub fn load_workspace_spec(&self) -> Result<WorkspaceSpec, LocalPersistenceError> {
        self.read_json(self.workspace_spec_path())
    }

    pub fn load_workspace_state(&self) -> Result<WorkspaceState, LocalPersistenceError> {
        self.read_json(self.state_path())
    }

    pub fn load_provider_state(&self) -> Result<PersistedProviderState, LocalPersistenceError> {
        self.read_json(self.provider_state_path())
    }

    pub fn load_events(&self) -> Result<Vec<WorkspaceEvent>, LocalPersistenceError> {
        if !self.events_path().exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(self.events_path())?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            events.push(serde_json::from_str(trimmed)?);
        }
        Ok(events)
    }

    pub fn delete_workspace(&self) -> Result<(), LocalPersistenceError> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        Ok(())
    }

    pub fn roles_dir(&self) -> PathBuf {
        self.root.join("roles")
    }

    pub fn workspace_spec_path(&self) -> PathBuf {
        self.root.join("workspace.json")
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn provider_state_path(&self) -> PathBuf {
        self.root.join("provider-state.json")
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    fn write_json<T: Serialize>(
        &self,
        path: PathBuf,
        value: &T,
    ) -> Result<(), LocalPersistenceError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(value)?)?;
        Ok(())
    }

    fn read_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: PathBuf,
    ) -> Result<T, LocalPersistenceError> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn append_jsonl<T: Serialize>(
        &self,
        path: PathBuf,
        values: &[T],
    ) -> Result<(), LocalPersistenceError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for value in values {
            serde_json::to_writer(&mut file, value)?;
            file.write_all(b"\n")?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceStateSeed {
    workspace_id: String,
    note: &'static str,
}

impl WorkspaceStateSeed {
    fn from_spec(spec: &WorkspaceSpec) -> Self {
        Self {
            workspace_id: spec.id.clone(),
            note: "Workspace initialized. Runtime state will be updated after the first event batch.",
        }
    }
}

fn render_agent_markdown(spec: &WorkspaceSpec, role: &RoleSpec) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# {}", role.name));
    lines.push(String::new());
    if let Some(description) = role.description.as_ref() {
        lines.push(description.clone());
        lines.push(String::new());
    } else {
        lines.push(role.agent.description.clone());
        lines.push(String::new());
    }

    lines.push("## Workspace".to_string());
    lines.push(format!("- Workspace: {}", spec.name));
    lines.push(format!("- Role ID: {}", role.id));
    if let Some(output_root) = role.output_root.as_ref() {
        lines.push(format!("- Output root: {}", output_root));
    }
    lines.push(String::new());

    lines.push("## Instructions".to_string());
    lines.push(role.agent.prompt.clone());
    lines.push(String::new());

    if let Some(tools) = role.agent.tools.as_ref() {
        if !tools.is_empty() {
            lines.push("## Tools".to_string());
            for tool in tools {
                lines.push(format!("- {}", tool));
            }
            lines.push(String::new());
        }
    }

    if let Some(tools) = role.agent.disallowed_tools.as_ref() {
        if !tools.is_empty() {
            lines.push("## Disallowed Tools".to_string());
            for tool in tools {
                lines.push(format!("- {}", tool));
            }
            lines.push(String::new());
        }
    }

    if let Some(skills) = role.agent.skills.as_ref() {
        if !skills.is_empty() {
            lines.push("## Skills".to_string());
            for skill in skills {
                lines.push(format!("- {}", skill));
            }
            lines.push(String::new());
        }
    }

    if let Some(model) = role.agent.model.as_ref() {
        lines.push("## Model".to_string());
        lines.push(model.clone());
        lines.push(String::new());
    }

    if let Some(permission_mode) = role.agent.permission_mode.as_ref() {
        lines.push("## Permission Mode".to_string());
        lines.push(format!("{permission_mode:?}"));
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string() + "\n"
}

fn now_string() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use multi_agent_protocol::{
        WorkspaceActivityKind, WorkspaceEvent, WorkspaceInstanceParams, WorkspaceVisibility,
        create_claude_workspace_profile, create_coding_studio_template, instantiate_workspace,
    };

    use super::*;

    #[test]
    fn initializes_workspace_files_and_agent_docs() {
        let template = create_coding_studio_template();
        let temp = std::env::temp_dir().join(format!(
            "multi-agent-runtime-local-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        let instance = WorkspaceInstanceParams {
            id: "local-files".to_string(),
            name: "Local Files".to_string(),
            cwd: Some(temp.to_string_lossy().to_string()),
        };
        let profile = create_claude_workspace_profile(None);
        let spec = instantiate_workspace(&template, &instance, &profile);
        let persistence = LocalWorkspacePersistence::from_spec(&spec).unwrap();

        persistence.initialize_workspace(&spec).unwrap();

        assert!(persistence.workspace_spec_path().exists());
        assert!(persistence.state_path().exists());
        assert!(persistence.roles_dir().join("prd/AGENT.md").exists());

        let agent_md = fs::read_to_string(persistence.roles_dir().join("prd/AGENT.md")).unwrap();
        assert!(agent_md.contains("# PRD"));
        assert!(agent_md.contains("## Instructions"));

        let _ = persistence.delete_workspace();
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn persists_and_loads_state_events_and_provider_bindings() {
        let template = create_coding_studio_template();
        let temp = std::env::temp_dir().join(format!(
            "multi-agent-runtime-local-state-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        let instance = WorkspaceInstanceParams {
            id: "persisted".to_string(),
            name: "Persisted".to_string(),
            cwd: Some(temp.to_string_lossy().to_string()),
        };
        let profile = create_claude_workspace_profile(None);
        let spec = instantiate_workspace(&template, &instance, &profile);
        let persistence = LocalWorkspacePersistence::from_spec(&spec).unwrap();
        persistence.initialize_workspace(&spec).unwrap();

        let state = WorkspaceState {
            workspace_id: spec.id.clone(),
            status: multi_agent_protocol::WorkspaceStatus::Running,
            provider: spec.provider,
            session_id: Some("root-session".to_string()),
            started_at: Some(now_string()),
            roles: spec
                .roles
                .iter()
                .cloned()
                .map(|role| (role.id.clone(), role))
                .collect(),
            members: Default::default(),
            dispatches: Default::default(),
            activities: vec![multi_agent_protocol::WorkspaceActivity {
                activity_id: uuid::Uuid::new_v4(),
                workspace_id: spec.id.clone(),
                kind: WorkspaceActivityKind::UserMessage,
                visibility: WorkspaceVisibility::Public,
                text: "hello".to_string(),
                created_at: now_string(),
                role_id: None,
                member_id: None,
                dispatch_id: None,
                task_id: None,
            }],
            workflow_runtime: multi_agent_protocol::WorkspaceWorkflowRuntimeState {
                mode: multi_agent_protocol::WorkspaceMode::GroupChat,
                active_vote_window: None,
                active_request_message: None,
                active_node_id: None,
                active_stage_id: None,
            },
        };
        let provider_state = PersistedProviderState {
            workspace_id: spec.id.clone(),
            provider: spec.provider,
            root_conversation_id: Some("root-session".to_string()),
            member_bindings: BTreeMap::from([(
                "prd".to_string(),
                PersistedProviderBinding {
                    role_id: "prd".to_string(),
                    provider_conversation_id: "member-session".to_string(),
                    kind: ProviderConversationKind::Session,
                    updated_at: now_string(),
                },
            )]),
            metadata: None,
            updated_at: now_string(),
        };
        let events = vec![WorkspaceEvent::ActivityPublished {
            timestamp: now_string(),
            workspace_id: spec.id.clone(),
            activity: state.activities[0].clone(),
        }];

        persistence
            .persist_runtime(&state, &events, &provider_state)
            .unwrap();

        assert_eq!(persistence.load_workspace_spec().unwrap().id, spec.id);
        assert_eq!(
            persistence.load_workspace_state().unwrap().workspace_id,
            spec.id
        );
        assert_eq!(
            persistence
                .load_provider_state()
                .unwrap()
                .root_conversation_id,
            Some("root-session".to_string())
        );
        assert_eq!(persistence.load_events().unwrap().len(), 1);

        let _ = persistence.delete_workspace();
        let _ = fs::remove_dir_all(temp);
    }
}
