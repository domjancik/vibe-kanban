use std::collections::HashMap;

use db::models::{
    execution_process::ExecutionProcess, scratch::DraftFollowUpData, session::Session,
    workspace::Workspace, workspace_repo::RepoWithTargetBranch,
};
use executors::{executor_discovery::ExecutorDiscoveredOptions, executors::BaseCodingAgent};
use uuid::Uuid;

use crate::model::{
    LocalDiff, PatchType, QueueStatus, RepoBranchStatus, UserSystemInfo, WorkspaceStreamState,
    WorkspaceSummary,
};

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
    TerminalConnected(Uuid),
    TerminalOutput(Uuid, Vec<u8>),
    TerminalError(Uuid, String),
    Error(String),
}
