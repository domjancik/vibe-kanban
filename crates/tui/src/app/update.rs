use ratatui::layout::Rect;

use crate::{
    app::App,
    input::{AppIntent, next_focus, prev_focus},
    model::{Focus, NetEvent, Pane, QueueStatus, active_process},
};

impl App {
    pub(crate) async fn handle_net_event(&mut self, event: NetEvent, size: Rect) {
        match event {
            NetEvent::UserSystemLoaded(info) => {
                self.default_executor_profile = Some(info.config.executor_profile.clone());
                self.executor_profiles = info.profiles;
                if self.composer_config.is_none() {
                    self.composer_config = Some(info.config.executor_profile.into());
                }
                self.rebind_discovery_stream();
            }
            NetEvent::ActiveWorkspaces(state) => {
                self.active_workspaces = state
                    .workspaces
                    .into_values()
                    .map(|workspace| (workspace.id, workspace))
                    .collect();
                self.ensure_workspace_selected(size);
            }
            NetEvent::ArchivedWorkspaces(state) => {
                self.archived_workspaces = state
                    .workspaces
                    .into_values()
                    .map(|workspace| (workspace.id, workspace))
                    .collect();
                self.ensure_workspace_selected(size);
            }
            NetEvent::Summaries(data) => {
                for summary in data {
                    self.summaries.insert(summary.workspace_id, summary);
                }
            }
            NetEvent::WorkspaceLoaded(workspace) => {
                self.bundle.workspace = Some(workspace);
            }
            NetEvent::SessionsLoaded {
                workspace_id,
                sessions,
            } => {
                if Some(workspace_id) == self.selected_workspace_id {
                    let previous = self.bundle.selected_session_id;
                    self.bundle.sessions = sessions;
                    let next_selected = previous
                        .filter(|selected| {
                            self.bundle
                                .sessions
                                .iter()
                                .any(|session| session.id == *selected)
                        })
                        .or_else(|| self.bundle.sessions.first().map(|session| session.id));
                    let changed = next_selected != self.bundle.selected_session_id;
                    self.bundle.selected_session_id = next_selected;
                    if changed {
                        self.bundle.process_map.clear();
                        self.bundle.log_entries.clear();
                        self.bundle.selected_process_id = None;
                        self.rebind_session_streams();
                    }
                    self.rebind_discovery_stream();
                    self.sync_composer_context();
                }
            }
            NetEvent::ReposLoaded {
                workspace_id,
                repos,
            } => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.repos = repos;
                }
            }
            NetEvent::GitStatusLoaded {
                workspace_id,
                statuses,
            } => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.git_status = statuses;
                }
            }
            NetEvent::NotesLoaded {
                workspace_id,
                notes,
            } => {
                if Some(workspace_id) == self.selected_workspace_id && !self.bundle.notes_dirty {
                    self.bundle.notes = notes;
                    self.notes_cursor = self.bundle.notes.len();
                }
            }
            NetEvent::NotesSaved {
                workspace_id,
                revision,
            } => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.notes_save_in_flight = false;
                    if revision == self.notes_edit_revision {
                        self.bundle.notes_dirty = false;
                        self.bundle.last_notes_edit = None;
                    }
                    self.status = "Notes saved".to_string();
                }
            }
            NetEvent::NotesSaveFailed {
                workspace_id,
                revision,
                message,
            } => {
                if Some(workspace_id) == self.selected_workspace_id
                    && revision == self.notes_edit_revision
                {
                    self.notes_save_in_flight = false;
                    self.error = Some(message.clone());
                    self.status = message;
                } else if Some(workspace_id) == self.selected_workspace_id {
                    self.notes_save_in_flight = false;
                }
            }
            NetEvent::DiffsUpdated {
                workspace_id,
                diffs,
            } => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.diffs = diffs;
                    if self.bundle.selected_diff_index >= self.bundle.diffs.len() {
                        self.bundle.selected_diff_index = self.bundle.diffs.len().saturating_sub(1);
                    }
                }
            }
            NetEvent::ProcessesUpdated {
                session_id,
                processes,
            } => {
                if Some(session_id) == self.bundle.selected_session_id {
                    let had_running = self.has_running_process();
                    let previous_count = self.bundle.process_map.len();
                    self.bundle.process_map = processes;
                    let previous_process_id = self.bundle.selected_process_id;
                    let next_process_id = active_process(&self.bundle.process_map)
                        .map(|process| process.id)
                        .or_else(|| {
                            previous_process_id
                                .filter(|selected| self.bundle.process_map.contains_key(selected))
                        });
                    let changed = next_process_id != previous_process_id;
                    self.bundle.selected_process_id = next_process_id;
                    if changed {
                        self.bundle.log_entries.clear();
                        self.rebind_logs_only();
                    }
                    self.sync_composer_executor_with_session();
                    if had_running != self.has_running_process()
                        || previous_count != self.bundle.process_map.len()
                    {
                        self.refresh_queue_status();
                    }
                    self.mark_chat_render_cache_dirty();
                    self.refresh_conversation_history();
                }
            }
            NetEvent::LogsUpdated {
                process_id,
                entries,
            } => {
                if Some(process_id) == self.bundle.selected_process_id {
                    self.bundle.log_entries = entries.clone();
                    self.chat_end_offset = 0;
                }
                if self.conversation_process_order.contains(&process_id) {
                    self.conversation_process_entries
                        .insert(process_id, entries);
                    self.reconcile_optimistic_entries();
                    self.mark_chat_render_cache_dirty();
                }
            }
            NetEvent::ExecutorOptionsUpdated { executor, options } => {
                if self
                    .composer_config
                    .as_ref()
                    .is_some_and(|config| config.executor == executor)
                {
                    self.composer_options = Some(options);
                }
            }
            NetEvent::DraftLoaded { scratch_id, draft } => {
                if Some(scratch_id) == self.current_composer_scratch_id()
                    && (!self.composer_scratch_loaded || !self.composer_dirty)
                {
                    self.composer = draft
                        .as_ref()
                        .map(|draft| draft.message.trim_end_matches('\n').to_string())
                        .unwrap_or_default();
                    self.composer_cursor = self.composer.len();
                    self.composer_scratch_loaded = true;
                    self.composer_dirty = false;
                    self.last_composer_edit = None;
                    self.composer_queue_conflict = false;
                    if let Some(draft) = draft {
                        let executor_changed = self
                            .composer_config
                            .as_ref()
                            .map(|config| config.executor != draft.executor_config.executor)
                            .unwrap_or(true);
                        self.composer_config = Some(draft.executor_config);
                        if executor_changed {
                            self.rebind_discovery_stream();
                        }
                    }
                }
            }
            NetEvent::DraftSaved {
                scratch_id,
                revision,
            } => {
                if Some(scratch_id) == self.current_composer_scratch_id() {
                    self.draft_save_in_flight = false;
                    if revision == self.composer_edit_revision {
                        self.composer_dirty = false;
                        self.last_composer_edit = None;
                        self.composer_queue_conflict = false;
                    }
                }
            }
            NetEvent::DraftSaveFailed {
                scratch_id,
                revision,
                message,
            } => {
                if Some(scratch_id) == self.current_composer_scratch_id()
                    && revision == self.composer_edit_revision
                {
                    self.draft_save_in_flight = false;
                    if message.contains("queued") {
                        self.composer_queue_conflict = true;
                    }
                    self.error = Some(message.clone());
                    self.status = message;
                } else if Some(scratch_id) == self.current_composer_scratch_id() {
                    self.draft_save_in_flight = false;
                }
            }
            NetEvent::ConversationHistoryLoaded {
                session_id,
                process_id,
                entries,
            } => {
                if Some(session_id) == self.bundle.selected_session_id {
                    self.conversation_process_entries
                        .insert(process_id, entries);
                    self.reconcile_optimistic_entries();
                    self.mark_chat_render_cache_dirty();
                    self.chat_end_offset = 0;
                }
            }
            NetEvent::ConversationBootstrapComplete { session_id } => {
                if Some(session_id) == self.bundle.selected_session_id {
                    self.conversation_bootstrapping = false;
                    self.conversation_backfilling = self.conversation_process_entries.len()
                        < self.conversation_process_order.len();
                    self.mark_chat_render_cache_dirty();
                }
            }
            NetEvent::ConversationBackfillComplete { session_id } => {
                if Some(session_id) == self.bundle.selected_session_id {
                    self.conversation_bootstrapping = false;
                    self.conversation_backfilling = false;
                    self.mark_chat_render_cache_dirty();
                }
            }
            NetEvent::QueueLoaded { session_id, status } => {
                if Some(session_id) == self.current_queue_session_id() {
                    self.queue_session_id = Some(session_id);
                    self.queue_status = status;
                    self.queue_pending = false;
                    if matches!(self.queue_status, QueueStatus::Empty) {
                        self.composer_queue_conflict = false;
                    }
                    self.mark_chat_render_cache_dirty();
                }
            }
            NetEvent::TerminalConnected(workspace_id) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.connected = true;
                    self.bundle.terminal.error = None;
                }
            }
            NetEvent::TerminalOutput(workspace_id, bytes) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.parser.process(&bytes);
                }
            }
            NetEvent::TerminalError(workspace_id, error) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.error = Some(error);
                    self.bundle.terminal.connected = false;
                }
            }
            NetEvent::Error(message) => {
                self.error = Some(message.clone());
                self.status = message;
            }
        }
    }

    pub(crate) async fn handle_app_intent(&mut self, intent: AppIntent, size: Rect) {
        match intent {
            AppIntent::CancelNewSession => self.cancel_new_session_flow(),
            AppIntent::ToggleComposerEditorMode => self.toggle_editor_mode(),
            AppIntent::Quit => self.should_quit = true,
            AppIntent::FocusNext => self.focus = next_focus(&self.focus),
            AppIntent::FocusPrev => self.focus = prev_focus(&self.focus),
            AppIntent::ShowHelp => {
                self.status = "Keys: Tab focus, j/k nav, 1-6 panes, i edit, Enter open/send, r rename session, E executor, V variant, M model, R reasoning, A agent menu, P permission, p pin, x archive, n new session, s start dev, c cleanup, e editor, Esc/C-]/C-g leave terminal".to_string();
            }
            AppIntent::SelectPane(pane) => self.selected_pane = pane,
            AppIntent::ToggleShowArchived => self.show_archived = !self.show_archived,
            AppIntent::EnterEditMode => {
                if matches!(self.selected_pane, Pane::Chat | Pane::Notes) {
                    self.focus = Focus::Composer;
                }
            }
            AppIntent::StartNewSession => {
                self.creating_new_session = true;
                self.selected_pane = Pane::Chat;
                self.focus = Focus::Composer;
                self.rebind_discovery_stream();
                self.sync_composer_context();
                self.status = "New session: type a prompt and press Enter".to_string();
            }
            AppIntent::TogglePinned => self.toggle_pinned().await,
            AppIntent::ToggleArchived => self.toggle_archived().await,
            AppIntent::StartDevServer => self.start_dev_server().await,
            AppIntent::RunCleanup => self.run_cleanup().await,
            AppIntent::StopWorkspace => self.stop_workspace().await,
            AppIntent::OpenEditor => self.open_editor().await,
            AppIntent::OpenSessionRename => self.open_session_rename(),
            AppIntent::CycleExecutor => self.cycle_executor().await,
            AppIntent::CycleVariant => self.cycle_variant().await,
            AppIntent::CycleModel => self.cycle_model(),
            AppIntent::CycleReasoning => self.cycle_reasoning(),
            AppIntent::OpenAgentPicker => self.open_agent_picker(),
            AppIntent::CyclePermissionMode => self.cycle_permission_mode(),
            AppIntent::QueuePrompt => self.queue_prompt().await,
            AppIntent::CancelQueuedPrompt => self.cancel_queued_prompt().await,
            AppIntent::DiscardDraft => self.discard_draft().await,
            AppIntent::Enter => self.handle_enter(size).await,
            AppIntent::JumpToStart => self.jump_to_boundary(false, size),
            AppIntent::JumpToEnd => self.jump_to_boundary(true, size),
            AppIntent::MoveSelection(delta) => self.move_selection(delta, size),
            AppIntent::PageSelection(direction) => {
                self.move_selection(direction * self.page_step(size), size)
            }
            AppIntent::EnterTerminalInputMode => {
                self.selected_pane = Pane::Terminal;
                self.bundle.terminal.input_mode = true;
                self.status = "Terminal input mode enabled".to_string();
            }
        }
    }
}
