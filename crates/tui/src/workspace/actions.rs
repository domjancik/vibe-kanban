use ratatui::layout::Rect;

use crate::{
    app::App,
    model::{NetEvent, Pane, QueueStatus, TerminalState, WorkspaceActionKind, WorkspaceBundle},
};

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
        self.session_filter.clear();
        self.search_prompt = None;
        self.conversation_search = None;
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

    pub(crate) async fn toggle_pinned(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        if self.actions_in_flight.pin_toggle {
            self.status = "Pin update already in progress".to_string();
            return;
        }
        let Some(next_pinned) = self
            .find_workspace(workspace_id)
            .map(|workspace| !workspace.pinned)
        else {
            return;
        };
        self.actions_in_flight.pin_toggle = true;
        self.status = "Updating pin state".to_string();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match api.toggle_pinned(workspace_id, next_pinned).await {
                Ok(()) => NetEvent::WorkspaceActionFinished {
                    kind: WorkspaceActionKind::TogglePinned,
                    workspace_id: Some(workspace_id),
                    success: true,
                    message: "Updated pin state".to_string(),
                },
                Err(error) => NetEvent::WorkspaceActionFinished {
                    kind: WorkspaceActionKind::TogglePinned,
                    workspace_id: Some(workspace_id),
                    success: false,
                    message: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub(crate) async fn toggle_archived(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        if self.actions_in_flight.archive_toggle {
            self.status = "Archive update already in progress".to_string();
            return;
        }
        let Some(next_archived) = self
            .find_workspace(workspace_id)
            .map(|workspace| !workspace.archived)
        else {
            return;
        };
        self.actions_in_flight.archive_toggle = true;
        self.status = "Updating archive state".to_string();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match api.toggle_archived(workspace_id, next_archived).await {
                Ok(()) => NetEvent::WorkspaceActionFinished {
                    kind: WorkspaceActionKind::ToggleArchived,
                    workspace_id: Some(workspace_id),
                    success: true,
                    message: "Updated archive state".to_string(),
                },
                Err(error) => NetEvent::WorkspaceActionFinished {
                    kind: WorkspaceActionKind::ToggleArchived,
                    workspace_id: Some(workspace_id),
                    success: false,
                    message: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub(crate) async fn stop_workspace(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            if self.actions_in_flight.dev_server {
                self.status = "Workspace stop already in progress".to_string();
                return;
            }
            self.actions_in_flight.dev_server = true;
            self.status = "Stopping workspace execution".to_string();
            let api = self.api.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let event = match api.stop_workspace(workspace_id).await {
                    Ok(()) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::StopWorkspace,
                        workspace_id: Some(workspace_id),
                        success: true,
                        message: "Stopped workspace execution".to_string(),
                    },
                    Err(error) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::StopWorkspace,
                        workspace_id: Some(workspace_id),
                        success: false,
                        message: error.to_string(),
                    },
                };
                let _ = tx.send(event);
            });
        }
    }

    pub(crate) async fn start_dev_server(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            if self.actions_in_flight.dev_server {
                self.status = "Dev server action already in progress".to_string();
                return;
            }
            self.actions_in_flight.dev_server = true;
            self.status = "Starting dev server".to_string();
            let api = self.api.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let event = match api.start_dev_server(workspace_id).await {
                    Ok(()) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::StartDevServer,
                        workspace_id: Some(workspace_id),
                        success: true,
                        message: "Started dev server".to_string(),
                    },
                    Err(error) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::StartDevServer,
                        workspace_id: Some(workspace_id),
                        success: false,
                        message: error.to_string(),
                    },
                };
                let _ = tx.send(event);
            });
        }
    }

    pub(crate) async fn run_cleanup(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            if self.actions_in_flight.cleanup {
                self.status = "Cleanup already in progress".to_string();
                return;
            }
            self.actions_in_flight.cleanup = true;
            self.status = "Starting cleanup script".to_string();
            let api = self.api.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let event = match api.run_cleanup(workspace_id).await {
                    Ok(()) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::RunCleanup,
                        workspace_id: Some(workspace_id),
                        success: true,
                        message: "Started cleanup script".to_string(),
                    },
                    Err(error) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::RunCleanup,
                        workspace_id: Some(workspace_id),
                        success: false,
                        message: error.to_string(),
                    },
                };
                let _ = tx.send(event);
            });
        }
    }

    pub(crate) async fn open_editor(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            if self.actions_in_flight.open_editor {
                self.status = "Open editor already in progress".to_string();
                return;
            }
            self.actions_in_flight.open_editor = true;
            self.status = "Requesting editor open".to_string();
            let api = self.api.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let event = match api.open_editor(workspace_id).await {
                    Ok(()) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::OpenEditor,
                        workspace_id: Some(workspace_id),
                        success: true,
                        message: "Requested editor open".to_string(),
                    },
                    Err(error) => NetEvent::WorkspaceActionFinished {
                        kind: WorkspaceActionKind::OpenEditor,
                        workspace_id: Some(workspace_id),
                        success: false,
                        message: error.to_string(),
                    },
                };
                let _ = tx.send(event);
            });
        }
    }

    pub(crate) fn select_session_target(&mut self, target: crate::workspace::SessionTarget) {
        match target {
            crate::workspace::SessionTarget::NewSession => {
                self.session_rename = None;
                self.conversation_search = None;
                self.creating_new_session = true;
                self.selected_pane = Pane::Chat;
                self.rebind_discovery_stream();
                self.sync_composer_context();
                self.status = "New session: type a prompt and press Enter".to_string();
            }
            crate::workspace::SessionTarget::Existing(session_id) => {
                if self.bundle.selected_session_id != Some(session_id) || self.creating_new_session
                {
                    self.session_rename = None;
                    self.conversation_search = None;
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
}
