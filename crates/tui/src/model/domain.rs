use std::collections::HashMap;

use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessRunReason},
    session::Session,
    workspace::Workspace,
    workspace_repo::RepoWithTargetBranch,
};
use uuid::Uuid;

use crate::model::{LocalDiff, PatchType, RepoBranchStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffViewMode {
    #[default]
    Unified,
    SideBySide,
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
    pub diff_view_mode: DiffViewMode,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::execution_process::{
        ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus, ExecutorActionField,
    };
    use executors::actions::{
        ExecutorAction, ExecutorActionType,
        script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
    };
    use sqlx::types::Json;
    use uuid::Uuid;

    use super::{Pane, active_process};

    fn process(
        id: Uuid,
        created_at_second: i64,
        run_reason: ExecutionProcessRunReason,
    ) -> ExecutionProcess {
        let created_at = Utc.timestamp_opt(created_at_second, 0).unwrap();
        ExecutionProcess {
            id,
            session_id: Uuid::new_v4(),
            run_reason,
            executor_action: Json(ExecutorActionField::ExecutorAction(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: "echo hi".to_string(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::DevServer,
                    working_dir: None,
                }),
                None,
            ))),
            status: ExecutionProcessStatus::Completed,
            exit_code: Some(0),
            dropped: false,
            started_at: created_at,
            completed_at: Some(created_at),
            created_at,
            updated_at: created_at,
        }
    }

    #[test]
    fn pane_all_and_titles_stay_stable() {
        let panes = Pane::all();
        assert_eq!(panes.len(), 6);
        assert_eq!(panes[0].title(), "Chat");
        assert_eq!(panes[5].title(), "Notes");
    }

    #[test]
    fn active_process_ignores_dev_servers_and_prefers_latest_process() {
        let latest = process(Uuid::new_v4(), 30, ExecutionProcessRunReason::CodingAgent);
        let ignored = process(Uuid::new_v4(), 40, ExecutionProcessRunReason::DevServer);
        let older = process(Uuid::new_v4(), 10, ExecutionProcessRunReason::CodingAgent);
        let processes = HashMap::from([
            (ignored.id, ignored),
            (older.id, older.clone()),
            (latest.id, latest.clone()),
        ]);

        assert_eq!(
            active_process(&processes).map(|process| process.id),
            Some(latest.id)
        );
    }
}
