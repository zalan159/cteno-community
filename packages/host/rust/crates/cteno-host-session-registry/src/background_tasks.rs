use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    str::FromStr,
    sync::{Arc, RwLock},
};

const ALLOWED_TASK_TYPES: &[&str] = &[
    "agent",
    "bash",
    "workflow",
    "remote_agent",
    "teammate",
    "scheduled_job",
    "background_session",
    "other",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundTaskCategory {
    ExecutionTask,
    ScheduledJob,
    BackgroundSession,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
    Unknown,
}

impl FromStr for BackgroundTaskStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "paused" => Ok(Self::Paused),
            "unknown" => Ok(Self::Unknown),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskRecord {
    pub task_id: String,
    pub session_id: String,
    pub vendor: String,
    pub category: BackgroundTaskCategory,
    pub task_type: String,
    pub description: Option<String>,
    pub summary: Option<String>,
    pub status: BackgroundTaskStatus,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub tool_use_id: Option<String>,
    pub output_file: Option<String>,
    pub vendor_extra: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskFilter {
    pub session_id: Option<String>,
    pub category: Option<BackgroundTaskCategory>,
    pub status: Option<BackgroundTaskStatus>,
}

pub trait ScheduledJobSource: Send + Sync {
    fn list_scheduled_jobs(&self) -> Vec<BackgroundTaskRecord>;
}

pub trait BackgroundSessionSource: Send + Sync {
    fn list_background_sessions(&self) -> Vec<BackgroundTaskRecord>;
}

#[derive(Default)]
struct NoopScheduledJobSource;

impl ScheduledJobSource for NoopScheduledJobSource {
    fn list_scheduled_jobs(&self) -> Vec<BackgroundTaskRecord> {
        Vec::new()
    }
}

#[derive(Default)]
struct NoopBackgroundSessionSource;

impl BackgroundSessionSource for NoopBackgroundSessionSource {
    fn list_background_sessions(&self) -> Vec<BackgroundTaskRecord> {
        Vec::new()
    }
}

pub struct BackgroundTaskRegistry {
    records: DashMap<String, BackgroundTaskRecord>,
    scheduled_job_source: RwLock<Arc<dyn ScheduledJobSource>>,
    background_session_source: RwLock<Arc<dyn BackgroundSessionSource>>,
}

impl BackgroundTaskRegistry {
    pub fn new() -> Self {
        Self::with_sources(
            Arc::new(NoopScheduledJobSource),
            Arc::new(NoopBackgroundSessionSource),
        )
    }

    pub fn with_sources(
        scheduled_job_source: Arc<dyn ScheduledJobSource>,
        background_session_source: Arc<dyn BackgroundSessionSource>,
    ) -> Self {
        Self {
            records: DashMap::new(),
            scheduled_job_source: RwLock::new(scheduled_job_source),
            background_session_source: RwLock::new(background_session_source),
        }
    }

    pub fn set_scheduled_job_source(&self, source: Arc<dyn ScheduledJobSource>) {
        let mut slot = self
            .scheduled_job_source
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = source;
    }

    pub fn set_background_session_source(&self, source: Arc<dyn BackgroundSessionSource>) {
        let mut slot = self
            .background_session_source
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = source;
    }

    pub fn upsert(&self, mut record: BackgroundTaskRecord) {
        record.task_type = normalize_task_type(&record.task_type);
        self.records.insert(record.task_id.clone(), record);
    }

    pub fn update_status(
        &self,
        task_id: &str,
        status: BackgroundTaskStatus,
        completed_at: Option<i64>,
        summary: Option<String>,
    ) {
        if let Some(mut record) = self.records.get_mut(task_id) {
            record.status = status;
            record.completed_at = completed_at;
            if let Some(summary) = summary {
                record.summary = Some(summary);
            }
        }
    }

    pub fn get(&self, task_id: &str) -> Option<BackgroundTaskRecord> {
        self.records.get(task_id).map(|entry| entry.clone())
    }

    pub fn list(&self, filter: BackgroundTaskFilter) -> Vec<BackgroundTaskRecord> {
        let mut records = Vec::new();

        if includes_execution_records(&filter) {
            records.extend(
                self.records
                    .iter()
                    .filter(|entry| {
                        entry.value().category == BackgroundTaskCategory::ExecutionTask
                            && matches_filter(entry.value(), &filter)
                    })
                    .map(|entry| entry.value().clone()),
            );
        }

        if includes_scheduled_jobs(&filter) {
            records.extend(
                self.scheduled_job_source()
                    .list_scheduled_jobs()
                    .into_iter()
                    .filter(|record| matches_filter(record, &filter)),
            );
        }

        if includes_background_sessions(&filter) {
            records.extend(
                self.background_session_source()
                    .list_background_sessions()
                    .into_iter()
                    .filter(|record| matches_filter(record, &filter)),
            );
        }

        records.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        records
    }

    pub fn has_running_for_session(&self, session_id: &str) -> bool {
        self.records.iter().any(|entry| {
            entry.session_id == session_id && entry.status == BackgroundTaskStatus::Running
        })
    }

    pub fn remove(&self, task_id: &str) {
        self.records.remove(task_id);
    }

    fn scheduled_job_source(&self) -> Arc<dyn ScheduledJobSource> {
        self.scheduled_job_source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn background_session_source(&self) -> Arc<dyn BackgroundSessionSource> {
        self.background_session_source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Default for BackgroundTaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn includes_execution_records(filter: &BackgroundTaskFilter) -> bool {
    filter.category.as_ref().is_none()
        || matches!(
            filter.category.as_ref(),
            Some(BackgroundTaskCategory::ExecutionTask)
        )
}

fn includes_scheduled_jobs(filter: &BackgroundTaskFilter) -> bool {
    filter.category.as_ref().is_none()
        || matches!(
            filter.category.as_ref(),
            Some(BackgroundTaskCategory::ScheduledJob)
        )
}

fn includes_background_sessions(filter: &BackgroundTaskFilter) -> bool {
    filter.category.as_ref().is_none()
        || matches!(
            filter.category.as_ref(),
            Some(BackgroundTaskCategory::BackgroundSession)
        )
}

fn normalize_task_type(task_type: &str) -> String {
    let normalized = task_type.trim().to_ascii_lowercase();
    if ALLOWED_TASK_TYPES.contains(&normalized.as_str()) {
        normalized
    } else {
        "other".to_string()
    }
}

fn matches_filter(record: &BackgroundTaskRecord, filter: &BackgroundTaskFilter) -> bool {
    filter
        .session_id
        .as_ref()
        .is_none_or(|session_id| record.session_id == *session_id)
        && filter
            .category
            .as_ref()
            .is_none_or(|category| record.category == *category)
        && filter
            .status
            .as_ref()
            .is_none_or(|status| record.status == *status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    struct StaticScheduledJobSource {
        records: Vec<BackgroundTaskRecord>,
    }

    impl ScheduledJobSource for StaticScheduledJobSource {
        fn list_scheduled_jobs(&self) -> Vec<BackgroundTaskRecord> {
            self.records.clone()
        }
    }

    struct StaticBackgroundSessionSource {
        records: Vec<BackgroundTaskRecord>,
    }

    impl BackgroundSessionSource for StaticBackgroundSessionSource {
        fn list_background_sessions(&self) -> Vec<BackgroundTaskRecord> {
            self.records.clone()
        }
    }

    #[test]
    fn upsert_is_idempotent_and_get_returns_latest_record() {
        let registry = BackgroundTaskRegistry::new();
        let mut record = sample_record("task-1", "session-a", BackgroundTaskStatus::Running);
        registry.upsert(record.clone());

        record.summary = Some("updated".into());
        record.vendor_extra = json!({ "step": 2 });
        registry.upsert(record.clone());

        assert_eq!(registry.list(BackgroundTaskFilter::default()).len(), 1);
        assert_eq!(registry.get("task-1"), Some(record));
    }

    #[test]
    fn update_status_to_completed_sets_completed_at_and_summary() {
        let registry = BackgroundTaskRegistry::new();
        registry.upsert(sample_record(
            "task-1",
            "session-a",
            BackgroundTaskStatus::Running,
        ));

        registry.update_status(
            "task-1",
            BackgroundTaskStatus::Completed,
            Some(1_700_000_123_456),
            Some("done".into()),
        );

        let record = registry.get("task-1").expect("task should exist");
        assert_eq!(record.status, BackgroundTaskStatus::Completed);
        assert_eq!(record.completed_at, Some(1_700_000_123_456));
        assert_eq!(record.summary.as_deref(), Some("done"));
    }

    #[test]
    fn has_running_for_session_tracks_running_to_completed_transition() {
        let registry = BackgroundTaskRegistry::new();
        registry.upsert(sample_record(
            "task-1",
            "session-a",
            BackgroundTaskStatus::Running,
        ));
        registry.upsert(sample_record(
            "task-2",
            "session-b",
            BackgroundTaskStatus::Completed,
        ));

        assert!(registry.has_running_for_session("session-a"));
        assert!(!registry.has_running_for_session("missing"));

        registry.update_status(
            "task-1",
            BackgroundTaskStatus::Completed,
            Some(42),
            Some("finished".into()),
        );

        assert!(!registry.has_running_for_session("session-a"));
    }

    #[test]
    fn list_merges_execution_records_with_scheduled_job_source_and_filters_by_category() {
        let registry = BackgroundTaskRegistry::with_sources(
            Arc::new(StaticScheduledJobSource {
                records: vec![scheduled_job_record(
                    "task-3",
                    "session-a",
                    BackgroundTaskStatus::Completed,
                )],
            }),
            Arc::new(StaticBackgroundSessionSource { records: vec![] }),
        );
        let execution_running = sample_record("task-1", "session-a", BackgroundTaskStatus::Running);
        let execution_completed =
            sample_record("task-2", "session-b", BackgroundTaskStatus::Completed);

        registry.upsert(execution_running.clone());
        registry.upsert(execution_completed.clone());

        assert_eq!(
            registry.list(BackgroundTaskFilter::default()),
            vec![
                execution_running.clone(),
                execution_completed.clone(),
                scheduled_job_record("task-3", "session-a", BackgroundTaskStatus::Completed),
            ]
        );

        assert_eq!(
            registry.list(BackgroundTaskFilter {
                category: Some(BackgroundTaskCategory::ExecutionTask),
                ..BackgroundTaskFilter::default()
            }),
            vec![execution_running.clone(), execution_completed]
        );

        assert_eq!(
            registry.list(BackgroundTaskFilter {
                category: Some(BackgroundTaskCategory::ScheduledJob),
                ..BackgroundTaskFilter::default()
            }),
            vec![scheduled_job_record(
                "task-3",
                "session-a",
                BackgroundTaskStatus::Completed,
            )]
        );

        assert_eq!(
            registry.list(BackgroundTaskFilter {
                session_id: Some("session-a".into()),
                category: Some(BackgroundTaskCategory::ScheduledJob),
                status: Some(BackgroundTaskStatus::Completed),
            }),
            vec![scheduled_job_record(
                "task-3",
                "session-a",
                BackgroundTaskStatus::Completed,
            )]
        );
    }

    #[test]
    fn unknown_task_type_is_coerced_to_other_on_upsert() {
        let registry = BackgroundTaskRegistry::new();
        let mut record = sample_record("task-1", "session-a", BackgroundTaskStatus::Running);
        record.task_type = "surprise".into();

        registry.upsert(record);

        assert_eq!(
            registry.get("task-1").map(|task| task.task_type),
            Some("other".into())
        );
    }

    #[test]
    fn serde_uses_camel_case_field_names_and_enum_values() {
        let value = serde_json::to_value(BackgroundTaskRecord {
            tool_use_id: Some("call-1".into()),
            output_file: Some("/tmp/out.log".into()),
            category: BackgroundTaskCategory::BackgroundSession,
            status: BackgroundTaskStatus::Completed,
            ..sample_record("task-1", "session-a", BackgroundTaskStatus::Running)
        })
        .expect("serialize record");

        assert_eq!(value["taskId"], json!("task-1"));
        assert_eq!(value["toolUseId"], json!("call-1"));
        assert_eq!(value["outputFile"], json!("/tmp/out.log"));
        assert_eq!(value["category"], json!("backgroundSession"));
        assert_eq!(value["status"], json!("completed"));
    }

    #[test]
    fn background_session_filter_uses_source_only() {
        let registry = BackgroundTaskRegistry::new();
        let mut record = sample_record("task-1", "session-a", BackgroundTaskStatus::Running);
        record.category = BackgroundTaskCategory::BackgroundSession;
        record.task_type = "background_session".into();
        registry.upsert(record);

        assert!(registry
            .list(BackgroundTaskFilter {
                category: Some(BackgroundTaskCategory::BackgroundSession),
                ..BackgroundTaskFilter::default()
            })
            .is_empty());
    }

    fn sample_record(
        task_id: &str,
        session_id: &str,
        status: BackgroundTaskStatus,
    ) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            task_id: task_id.into(),
            session_id: session_id.into(),
            vendor: "cteno".into(),
            category: BackgroundTaskCategory::ExecutionTask,
            task_type: "agent".into(),
            description: Some("task".into()),
            summary: Some("initial".into()),
            status,
            started_at: 1_700_000_000_000,
            completed_at: None,
            tool_use_id: None,
            output_file: None,
            vendor_extra: json!({ "raw": true }),
        }
    }

    fn scheduled_job_record(
        task_id: &str,
        session_id: &str,
        status: BackgroundTaskStatus,
    ) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            category: BackgroundTaskCategory::ScheduledJob,
            task_type: "scheduled_job".into(),
            ..sample_record(task_id, session_id, status)
        }
    }
}
