use std::collections::HashMap;

use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessRunReason},
    session::Session,
    workspace::Workspace,
    workspace_repo::RepoWithTargetBranch,
};
use uuid::Uuid;

use crate::model::{LocalDiff, PatchType, RepoBranchStatus};

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

pub fn active_process(processes: &HashMap<Uuid, ExecutionProcess>) -> Option<ExecutionProcess> {
    processes
        .values()
        .filter(|process| process.run_reason != ExecutionProcessRunReason::DevServer)
        .max_by_key(|process| process.created_at)
        .cloned()
}
