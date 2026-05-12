use std::collections::HashMap;

use db::models::{
    execution_process::ExecutionProcess,
    repo::Repo,
    scratch::{DraftFollowUpData, DraftWorkspaceData},
    session::Session,
    workspace::Workspace,
    workspace_repo::RepoWithTargetBranch,
};
use executors::{executor_discovery::ExecutorDiscoveredOptions, executors::BaseCodingAgent};
use uuid::Uuid;

use crate::model::{
    AttachPrResponse, GitBranch, LocalDiff, PatchType, QueueStatus, RepoBranchStatus,
    UserSystemInfo, WorkspaceStreamState, WorkspaceSummary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceActionKind {
    TogglePinned,
    ToggleArchived,
    StopExecution,
    StartDevServer,
    RunCleanup,
    OpenEditor,
}

#[derive(Debug, Clone)]
pub enum NetEvent {
    UserSystemLoaded(UserSystemInfo),
    ActiveWorkspaces(WorkspaceStreamState),
    ArchivedWorkspaces(WorkspaceStreamState),
    Summaries(Vec<WorkspaceSummary>),
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
    WorkspaceCreateReposLoaded {
        repos: Vec<Repo>,
    },
    WorkspaceCreateDraftLoaded {
        draft: Option<DraftWorkspaceData>,
    },
    WorkspaceCreateDraftSaved {
        revision: u64,
    },
    WorkspaceCreateDraftSaveFailed {
        revision: u64,
        message: String,
    },
    WorkspaceCreateBranchesLoaded {
        repo_id: Uuid,
        branches: Vec<GitBranch>,
    },
    WorkspaceCreateSubmitted {
        workspace: Workspace,
    },
    WorkspaceCreateSubmitFailed {
        message: String,
    },
    DraftSaved {
        scratch_id: Uuid,
        revision: u64,
    },
    DraftSaveFailed {
        scratch_id: Uuid,
        revision: u64,
        message: String,
    },
    QueueLoaded {
        session_id: Uuid,
        status: QueueStatus,
    },
    NotesSaved {
        workspace_id: Uuid,
        revision: u64,
    },
    NotesSaveFailed {
        workspace_id: Uuid,
        revision: u64,
        message: String,
    },
    PromptSubmitted {
        workspace_id: Uuid,
        session_id: Uuid,
        workspace_scope: Option<Uuid>,
    },
    PromptSubmissionFailed {
        message: String,
        restored_draft: DraftFollowUpData,
        optimistic_id: Option<Uuid>,
    },
    QueuedPrompt {
        session_id: Uuid,
        status: QueueStatus,
    },
    QueuePromptFailed {
        message: String,
    },
    QueueCancelled {
        session_id: Uuid,
        status: QueueStatus,
        restored: Option<DraftFollowUpData>,
    },
    QueueCancelFailed {
        message: String,
    },
    DraftDiscarded {
        message: String,
    },
    DraftDiscardFailed {
        message: String,
    },
    WorkspaceActionFinished {
        kind: WorkspaceActionKind,
        success: bool,
        message: String,
    },
    PullRequestCreated {
        workspace_id: Uuid,
        repo_id: Uuid,
        pr_url: String,
    },
    PullRequestCreateFailed {
        message: String,
    },
    PullRequestAttached {
        workspace_id: Uuid,
        repo_id: Uuid,
        response: AttachPrResponse,
    },
    PullRequestAttachFailed {
        message: String,
    },
    PullRequestOpened {
        url: String,
    },
    PullRequestOpenFailed {
        url: String,
        message: String,
    },
    TerminalConnected(Uuid),
    TerminalOutput(Uuid, Vec<u8>),
    TerminalError(Uuid, String),
    Error(String),
}
