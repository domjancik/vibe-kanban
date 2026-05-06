use std::{collections::HashMap, path::PathBuf};

use chrono::{DateTime, Utc};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
    scratch::{DraftFollowUpData, WorkspaceNotesData},
    session::Session,
    workspace::{Workspace, WorkspaceWithStatus},
    workspace_repo::RepoWithTargetBranch,
};
use executors::{
    executor_discovery::ExecutorDiscoveredOptions,
    executors::BaseCodingAgent,
    logs::{ActionType, NormalizedEntry, NormalizedEntryError, NormalizedEntryType, ToolStatus},
    model_selector::PermissionPolicy,
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
    pub workspaces: HashMap<String, WorkspaceWithStatus>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceSummary {
    pub workspace_id: Uuid,
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
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
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
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UpdateScratchPayload {
    DraftFollowUp(DraftFollowUpData),
    WorkspaceNotes(WorkspaceNotesData),
}

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

#[derive(Debug, Clone, Deserialize)]
pub struct RepoBranchStatus {
    pub repo_id: Uuid,
    pub repo_name: String,
    #[serde(flatten)]
    pub status: BranchStatus,
}

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

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestInfo {
    pub status: MergeStatus,
    pub pr_number: i64,
    pub pr_url: String,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    Chat,
    Changes,
    Logs,
    Git,
    Terminal,
    Notes,
}

impl Pane {
    pub fn all() -> [Self; 6] {
        [
            Self::Chat,
            Self::Changes,
            Self::Logs,
            Self::Git,
            Self::Terminal,
            Self::Notes,
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Changes => "Changes",
            Self::Logs => "Logs",
            Self::Git => "Git",
            Self::Terminal => "Terminal",
            Self::Notes => "Notes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    WorkspaceList,
    Main,
    Detail,
    Composer,
}

#[derive(Debug, Default)]
pub struct WorkspaceBundle {
    pub workspace: Option<Workspace>,
    pub repos: Vec<RepoWithTargetBranch>,
    pub sessions: Vec<Session>,
    pub summaries: HashMap<Uuid, WorkspaceSummary>,
    pub git_status: Vec<RepoBranchStatus>,
    pub process_map: HashMap<Uuid, ExecutionProcess>,
    pub log_entries: Vec<PatchType>,
    pub diffs: Vec<LocalDiff>,
    pub notes: String,
    pub notes_dirty: bool,
    pub last_notes_edit: Option<std::time::Instant>,
    pub selected_session_id: Option<Uuid>,
    pub selected_process_id: Option<Uuid>,
    pub selected_diff_index: usize,
    pub log_scroll: u16,
    pub terminal: TerminalState,
}

pub struct TerminalState {
    pub connected: bool,
    pub input_mode: bool,
    pub parser: vt100::Parser,
    pub size: (u16, u16),
    pub error: Option<String>,
}

impl std::fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalState")
            .field("connected", &self.connected)
            .field("input_mode", &self.input_mode)
            .field("size", &self.size)
            .field("error", &self.error)
            .finish()
    }
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            connected: false,
            input_mode: false,
            parser: vt100::Parser::new(24, 80, 5_000),
            size: (80, 24),
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamKind {
    ActiveWorkspaces,
    ArchivedWorkspaces,
    Diffs(Uuid),
    Notes(Uuid),
    Processes(Uuid),
    Logs(Uuid),
}

#[derive(Debug, Clone)]
pub enum NetEvent {
    UserSystemLoaded(UserSystemInfo),
    ActiveWorkspaces(WorkspaceStreamState),
    ArchivedWorkspaces(WorkspaceStreamState),
    Summaries {
        archived: bool,
        data: Vec<WorkspaceSummary>,
    },
    WorkspaceLoaded(Workspace),
    SessionsLoaded {
        workspace_id: Uuid,
        sessions: Vec<Session>,
    },
    ReposLoaded {
        workspace_id: Uuid,
        repos: Vec<RepoWithTargetBranch>,
    },
    GitStatusLoaded {
        workspace_id: Uuid,
        statuses: Vec<RepoBranchStatus>,
    },
    NotesLoaded {
        workspace_id: Uuid,
        notes: String,
    },
    DiffsUpdated {
        workspace_id: Uuid,
        diffs: Vec<LocalDiff>,
    },
    ProcessesUpdated {
        session_id: Uuid,
        processes: HashMap<Uuid, ExecutionProcess>,
    },
    LogsUpdated {
        process_id: Uuid,
        entries: Vec<PatchType>,
    },
    ExecutorOptionsUpdated {
        executor: BaseCodingAgent,
        options: ExecutorDiscoveredOptions,
    },
    ConversationHistoryLoaded {
        session_id: Uuid,
        process_id: Uuid,
        entries: Vec<PatchType>,
    },
    ConversationBootstrapComplete {
        session_id: Uuid,
    },
    ConversationBackfillComplete {
        session_id: Uuid,
    },
    DraftLoaded {
        scratch_id: Uuid,
        draft: Option<DraftFollowUpData>,
    },
    QueueLoaded {
        session_id: Uuid,
        status: QueueStatus,
    },
    NotesSaved(Uuid),
    TerminalConnected(Uuid),
    TerminalOutput(Uuid, Vec<u8>),
    TerminalError(Uuid, String),
    ActionOk(String),
    Error(String),
    StreamClosed(StreamKind),
}

pub fn diff_title(diff: &LocalDiff) -> String {
    diff.new_path
        .clone()
        .or_else(|| diff.old_path.clone())
        .unwrap_or_else(|| "<unknown>".to_string())
}

pub fn format_patch_entry(entry: &PatchType) -> String {
    match entry {
        PatchType::Stdout(s) => s.clone(),
        PatchType::Stderr(s) => s.clone(),
        PatchType::Diff(diff) => format!(
            "{} {}",
            match diff.change {
                DiffChangeKind::Added => "added",
                DiffChangeKind::Deleted => "deleted",
                DiffChangeKind::Modified => "modified",
                DiffChangeKind::Renamed => "renamed",
                DiffChangeKind::Copied => "copied",
                DiffChangeKind::PermissionChange => "chmod",
            },
            diff_title(diff)
        ),
        PatchType::NormalizedEntry(entry) => format_normalized_entry(entry),
    }
}

pub fn format_normalized_entry(entry: &NormalizedEntry) -> String {
    let prefix = match &entry.entry_type {
        NormalizedEntryType::UserMessage => "user",
        NormalizedEntryType::AssistantMessage => "assistant",
        NormalizedEntryType::SystemMessage => "system",
        NormalizedEntryType::Thinking => "thinking",
        NormalizedEntryType::Loading => "loading",
        NormalizedEntryType::UserFeedback { .. } => "feedback",
        NormalizedEntryType::ErrorMessage { error_type } => match error_type {
            NormalizedEntryError::SetupRequired => "setup",
            NormalizedEntryError::Other => "error",
        },
        NormalizedEntryType::NextAction { failed, .. } => {
            if *failed {
                "next action failed"
            } else {
                "next action"
            }
        }
        NormalizedEntryType::ToolUse {
            tool_name,
            action_type,
            status,
        } => return format_tool_use(tool_name, action_type, status, &entry.content),
        NormalizedEntryType::TokenUsageInfo(info) => {
            return format!(
                "tokens: {} / {}",
                info.total_tokens, info.model_context_window
            );
        }
        NormalizedEntryType::UserAnsweredQuestions { answers } => {
            return answers
                .iter()
                .map(|item| format!("{}: {}", item.question, item.answer.join(", ")))
                .collect::<Vec<_>>()
                .join("\n");
        }
    };

    if entry.content.trim().is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {}", entry.content.trim())
    }
}

fn format_tool_use(
    tool_name: &str,
    action_type: &ActionType,
    status: &ToolStatus,
    content: &str,
) -> String {
    let action = match action_type {
        ActionType::CommandRun { command, .. } => format!("cmd `{command}`"),
        ActionType::FileRead { path } => format!("read {path}"),
        ActionType::FileEdit { path, .. } => format!("edit {path}"),
        ActionType::Search { query } => format!("search {query}"),
        ActionType::WebFetch { url } => format!("fetch {url}"),
        ActionType::Tool { tool_name, .. } => tool_name.clone(),
        ActionType::TaskCreate {
            description,
            subagent_type,
            ..
        } => {
            if let Some(subagent_type) = subagent_type {
                format!("spawn {subagent_type}: {description}")
            } else {
                description.clone()
            }
        }
        ActionType::PlanPresentation { .. } => "present plan".to_string(),
        ActionType::TodoManagement { operation, .. } => format!("todo {operation}"),
        ActionType::AskUserQuestion { .. } => "ask user".to_string(),
        ActionType::Other { description } => description.clone(),
    };
    let status = match status {
        ToolStatus::Created => "created",
        ToolStatus::Success => "success",
        ToolStatus::Failed => "failed",
        ToolStatus::Denied { .. } => "denied",
        ToolStatus::PendingApproval { .. } => "approval",
        ToolStatus::TimedOut => "timed out",
    };
    if content.trim().is_empty() {
        format!("tool {tool_name} [{status}] {action}")
    } else {
        format!("tool {tool_name} [{status}] {action}\n{}", content.trim())
    }
}

pub fn format_relative_time(ts: Option<DateTime<Utc>>) -> String {
    let Some(ts) = ts else {
        return "never".to_string();
    };
    let delta = Utc::now().signed_duration_since(ts);
    if delta.num_seconds() < 60 {
        format!("{}s ago", delta.num_seconds().max(0))
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

pub fn workspace_title(workspace: &Workspace) -> String {
    workspace
        .name
        .clone()
        .unwrap_or_else(|| workspace.branch.clone())
}

pub fn workspace_editor_path(bundle: &WorkspaceBundle) -> Option<PathBuf> {
    bundle
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.container_ref.as_ref())
        .map(PathBuf::from)
}

pub fn active_process(processes: &HashMap<Uuid, ExecutionProcess>) -> Option<ExecutionProcess> {
    processes
        .values()
        .filter(|process| process.run_reason != ExecutionProcessRunReason::DevServer)
        .max_by_key(|process| process.created_at)
        .cloned()
}

pub fn display_variant(variant: Option<&str>) -> &str {
    variant.unwrap_or("DEFAULT")
}

pub fn display_permission(policy: Option<&PermissionPolicy>) -> &str {
    match policy {
        Some(PermissionPolicy::Auto) => "AUTO",
        Some(PermissionPolicy::Supervised) => "SUPERVISED",
        Some(PermissionPolicy::Plan) => "PLAN",
        None => "DEFAULT",
    }
}
