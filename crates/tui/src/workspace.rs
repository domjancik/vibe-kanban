use db::models::{session::Session, workspace::WorkspaceWithStatus};
use ratatui::layout::Rect;
use uuid::Uuid;

use crate::{
    app_state::App,
    model::{Pane, QueueStatus, TerminalState, WorkspaceBundle, workspace_title},
};

pub(crate) enum WorkspaceRow<'a> {
    Header(&'static str),
    Workspace(&'a WorkspaceWithStatus),
}

pub(crate) enum SessionRow<'a> {
    NewSession,
    Session(&'a Session),
}

#[derive(Clone, Copy)]
pub(crate) enum SessionTarget {
    NewSession,
    Existing(Uuid),
}

impl App {
    pub(crate) fn ensure_workspace_selected(&mut self, size: Rect) {
        if self.selected_workspace_id.is_some() {
            return;
        }
        self.selected_workspace_id = self.visible_workspace_ids().first().copied();
        self.load_selected_workspace(size);
    }

    pub(crate) fn load_selected_workspace(&mut self, size: Rect) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        self.creating_new_session = false;
        self.chat_end_offset = 0;
        let terminal_size = self.terminal_stream_size(size);
        self.bundle = WorkspaceBundle::default();
        self.bundle.terminal = TerminalState::default();
        self.bundle.terminal.size = terminal_size;
        self.bundle
            .terminal
            .parser
            .set_size(terminal_size.1, terminal_size.0);
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.draft_save_in_flight = false;
        self.last_composer_edit = None;
        self.composer_scratch_loaded = false;
        self.composer_scratch_id = None;
        self.notes_save_in_flight = false;
        self.queue_status = QueueStatus::Empty;
        self.queue_session_id = None;
        self.queue_pending = false;
        self.session_rename = None;
        self.reset_conversation_state();
        self.api.load_workspace(workspace_id, self.tx.clone());
        self.api.replace_workspace_subscriptions(
            workspace_id,
            None,
            None,
            terminal_size,
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    pub(crate) fn rebind_session_streams(&mut self) {
        self.chat_end_offset = 0;
        self.api.replace_process_stream(
            self.bundle.selected_session_id,
            self.tx.clone(),
            &mut self.subscriptions,
        );
        self.sync_composer_context();
    }

    pub(crate) fn rebind_logs_only(&mut self) {
        self.api.replace_logs_stream(
            self.bundle.selected_process_id,
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    pub(crate) fn switch_session_or_process(&mut self, size: Rect) {
        let _ = size;
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows).unwrap_or(0);
        if let Some(row) = rows.get(index)
            && let Some(target) = session_target(row)
        {
            self.select_session_target(target);
        }
    }

    pub(crate) fn session_rows(&self) -> Vec<SessionRow<'_>> {
        let mut rows = Vec::with_capacity(self.bundle.sessions.len() + 1);
        rows.push(SessionRow::NewSession);
        rows.extend(self.bundle.sessions.iter().map(SessionRow::Session));
        rows
    }

    pub(crate) fn selected_session_row_index(&self, rows: &[SessionRow<'_>]) -> Option<usize> {
        if self.creating_new_session {
            return Some(0);
        }
        let selected = self.bundle.selected_session_id?;
        rows.iter().position(|row| match row {
            SessionRow::NewSession => false,
            SessionRow::Session(session) => session.id == selected,
        })
    }

    pub(crate) fn selected_session_for_rename(&self) -> Option<&Session> {
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows)?;
        match rows.get(index)? {
            SessionRow::Session(session) => Some(session),
            SessionRow::NewSession => None,
        }
    }

    pub(crate) fn select_session_target(&mut self, target: SessionTarget) {
        match target {
            SessionTarget::NewSession => {
                self.session_rename = None;
                self.creating_new_session = true;
                self.selected_pane = Pane::Chat;
                self.rebind_discovery_stream();
                self.sync_composer_context();
                self.status = "New session: type a prompt and press Enter".to_string();
            }
            SessionTarget::Existing(session_id) => {
                if self.bundle.selected_session_id != Some(session_id) || self.creating_new_session
                {
                    self.session_rename = None;
                    self.bundle.selected_session_id = Some(session_id);
                    self.creating_new_session = false;
                    self.rebind_session_streams();
                    self.rebind_discovery_stream();
                }
            }
        }
    }

    pub(crate) fn cancel_new_session_flow(&mut self) {
        if !self.creating_new_session {
            return;
        }
        self.creating_new_session = false;
        self.sync_composer_context();
        self.status = "Cancelled new session".to_string();
        self.error = None;
    }

    pub(crate) fn current_session(&self) -> Option<&Session> {
        self.bundle
            .selected_session_id
            .and_then(|id| self.bundle.sessions.iter().find(|session| session.id == id))
    }

    fn all_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let mut workspaces = self.active_workspaces.values().collect::<Vec<_>>();
        workspaces.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        workspaces
    }

    fn filtered_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        self.all_workspaces()
            .into_iter()
            .filter(|workspace| {
                if self.filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                let filter = self.filter.to_lowercase();
                title.contains(&filter) || branch.contains(&filter)
            })
            .collect()
    }

    fn filtered_archived_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let mut workspaces = self
            .archived_workspaces
            .values()
            .filter(|workspace| {
                if self.filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                let filter = self.filter.to_lowercase();
                title.contains(&filter) || branch.contains(&filter)
            })
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        workspaces
    }

    pub(crate) fn workspace_rows(&self) -> Vec<WorkspaceRow<'_>> {
        let active = self.filtered_workspaces();
        let mut needs_attention = Vec::new();
        let mut running = Vec::new();
        let mut idle = Vec::new();

        for workspace in active {
            let summary = self.summaries.get(&workspace.id);
            let needs_attention_bucket = summary
                .is_some_and(|summary| summary.has_pending_approval || summary.has_unseen_turns);
            if needs_attention_bucket {
                needs_attention.push(workspace);
            } else if workspace.is_running
                || summary.is_some_and(|summary| summary.has_running_dev_server)
            {
                running.push(workspace);
            } else {
                idle.push(workspace);
            }
        }

        let mut rows = Vec::new();
        self.push_workspace_group(&mut rows, "Needs Attention", &needs_attention);
        self.push_workspace_group(&mut rows, "Running", &running);
        self.push_workspace_group(&mut rows, "Idle", &idle);

        if self.show_archived {
            let archived = self.filtered_archived_workspaces();
            self.push_workspace_group(&mut rows, "Archived", &archived);
        }

        rows
    }

    fn push_workspace_group<'a>(
        &self,
        rows: &mut Vec<WorkspaceRow<'a>>,
        title: &'static str,
        workspaces: &[&'a WorkspaceWithStatus],
    ) {
        if workspaces.is_empty() {
            return;
        }
        rows.push(WorkspaceRow::Header(title));
        rows.extend(workspaces.iter().copied().map(WorkspaceRow::Workspace));
    }

    pub(crate) fn selected_workspace_row_index(&self, rows: &[WorkspaceRow<'_>]) -> Option<usize> {
        let selected_id = self.selected_workspace_id?;
        rows.iter().position(|row| match row {
            WorkspaceRow::Header(_) => false,
            WorkspaceRow::Workspace(workspace) => workspace.id == selected_id,
        })
    }

    pub(crate) fn visible_workspace_ids(&self) -> Vec<Uuid> {
        self.workspace_rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspaceRow::Header(_) => None,
                WorkspaceRow::Workspace(workspace) => Some(workspace.id),
            })
            .collect()
    }

    pub(crate) fn find_workspace(&self, workspace_id: Uuid) -> Option<&WorkspaceWithStatus> {
        self.active_workspaces
            .get(&workspace_id)
            .or_else(|| self.archived_workspaces.get(&workspace_id))
    }

    pub(crate) async fn toggle_pinned(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        let Some(workspace) = self.find_workspace(workspace_id) else {
            return;
        };
        match self
            .api
            .toggle_pinned(workspace_id, !workspace.pinned)
            .await
        {
            Ok(()) => self.status = "Updated pin state".to_string(),
            Err(error) => self.status = error.to_string(),
        }
    }

    pub(crate) async fn toggle_archived(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        let Some(workspace) = self.find_workspace(workspace_id) else {
            return;
        };
        match self
            .api
            .toggle_archived(workspace_id, !workspace.archived)
            .await
        {
            Ok(()) => self.status = "Updated archive state".to_string(),
            Err(error) => self.status = error.to_string(),
        }
    }

    pub(crate) async fn stop_workspace(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.stop_workspace(workspace_id).await {
                Ok(()) => {
                    self.status = "Stopped workspace execution".to_string();
                    self.refresh_queue_status();
                }
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    pub(crate) async fn start_dev_server(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.start_dev_server(workspace_id).await {
                Ok(()) => self.status = "Started dev server".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    pub(crate) async fn run_cleanup(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.run_cleanup(workspace_id).await {
                Ok(()) => self.status = "Started cleanup script".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    pub(crate) async fn open_editor(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.open_editor(workspace_id).await {
                Ok(()) => self.status = "Requested editor open".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }
}

pub(crate) fn session_target(row: &SessionRow<'_>) -> Option<SessionTarget> {
    match row {
        SessionRow::NewSession => Some(SessionTarget::NewSession),
        SessionRow::Session(session) => Some(SessionTarget::Existing(session.id)),
    }
}
