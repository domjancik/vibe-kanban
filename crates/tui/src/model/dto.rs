use std::collections::HashMap;

use chrono::{DateTime, Utc};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    scratch::{DraftFollowUpData, WorkspaceNotesData},
};
use executors::{
    executor_discovery::ExecutorDiscoveredOptions,
    logs::NormalizedEntry,
    profile::{ExecutorConfig, ExecutorConfigs, ExecutorProfileId},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    pub success: bool,
    pub data: Option<T>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceStreamState {
    pub workspaces: HashMap<String, db::models::workspace::WorkspaceWithStatus>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceSummary {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub remote_project_id: Option<Uuid>,
    pub latest_session_id: Option<Uuid>,
    pub has_pending_approval: bool,
    pub files_changed: Option<usize>,
    pub lines_added: Option<usize>,
    pub lines_removed: Option<usize>,
    pub latest_process_completed_at: Option<DateTime<Utc>>,
    pub latest_process_status: Option<ExecutionProcessStatus>,
    pub has_running_dev_server: bool,
    pub has_unseen_turns: bool,
    pub pr_status: Option<MergeStatus>,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceSummaryResponse {
    pub summaries: Vec<WorkspaceSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSummaryRequest {
    pub archived: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiffStreamState {
    pub entries: HashMap<String, HashMap<String, PatchType>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExecutionProcessesState {
    pub execution_processes: HashMap<String, ExecutionProcess>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogEntriesState {
    pub entries: Vec<PatchType>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScratchStreamState {
    pub scratch: Option<ScratchRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScratchRecord {
    pub payload: ScratchPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScratchPayload {
    DraftFollowUp(DraftFollowUpData),
    WorkspaceNotes(WorkspaceNotesData),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateScratchRequest {
    pub payload: UpdateScratchPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UpdateScratchPayload {
    DraftFollowUp(DraftFollowUpData),
    WorkspaceNotes(WorkspaceNotesData),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct QueuedMessage {
    pub session_id: Uuid,
    pub data: DraftFollowUpData,
    pub queued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueueStatus {
    #[default]
    Empty,
    Queued {
        message: QueuedMessage,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateWorkspaceRequest {
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateSessionRequest {
    pub workspace_id: Uuid,
    pub executor: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateSessionRequest {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenEditorRequest {
    pub editor_type: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowUpRequest {
    pub prompt: String,
    pub executor_config: ExecutorConfig,
    pub retry_process_id: Option<Uuid>,
    pub force_when_dirty: Option<bool>,
    pub perform_git_reset: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserSystemInfo {
    pub config: UserConfig,
    #[serde(flatten)]
    pub profiles: ExecutorConfigs,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserConfig {
    pub executor_profile: ExecutorProfileId,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutorDiscoveryStreamState {
    pub options: ExecutorDiscoveredOptions,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct RepoBranchStatus {
    pub repo_id: Uuid,
    pub repo_name: String,
    #[serde(flatten)]
    pub status: BranchStatus,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct BranchStatus {
    pub commits_behind: Option<usize>,
    pub commits_ahead: Option<usize>,
    pub has_uncommitted_changes: Option<bool>,
    pub head_oid: Option<String>,
    pub uncommitted_count: Option<usize>,
    pub untracked_count: Option<usize>,
    pub target_branch_name: String,
    pub remote_commits_behind: Option<usize>,
    pub remote_commits_ahead: Option<usize>,
    pub merges: Vec<Merge>,
    pub is_rebase_in_progress: bool,
    pub conflict_op: Option<String>,
    pub conflicted_files: Vec<String>,
    pub is_target_remote: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Merge {
    Direct {},
    Pr(PrMerge),
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrMerge {
    pub pr_info: PullRequestInfo,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestInfo {
    pub status: MergeStatus,
    #[serde(alias = "number")]
    pub pr_number: i64,
    #[serde(alias = "url")]
    pub pr_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeStatus {
    Open,
    Merged,
    Closed,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "content")]
pub enum PatchType {
    NormalizedEntry(NormalizedEntry),
    Stdout(String),
    Stderr(String),
    Diff(LocalDiff),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDiff {
    pub change: DiffChangeKind,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
    pub content_omitted: bool,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
    pub repo_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    PermissionChange,
}
