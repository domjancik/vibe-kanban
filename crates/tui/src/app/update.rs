use db::models::scratch::DraftFollowUpData;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    text::Line,
};

use crate::{
    api::transport::log_tui,
    app::{App, ToolCallDisplayMode},
    conversation::chat_window_bounds,
    input::{AppIntent, next_focus, prev_focus},
    model::{Focus, NetEvent, Pane, QueueStatus, WorkspaceActionKind, active_process},
};

#[derive(Clone)]
struct ChatViewportAnchor {
    top_offset: usize,
    visible_line_texts: Vec<String>,
}

struct ChatViewportMetrics {
    content_width: usize,
    total_lines: usize,
    visible_lines: usize,
    clamped_end_offset: usize,
    start: usize,
    end: usize,
    top_offset: usize,
}

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
                self.mark_workspace_list_dirty();
                self.ensure_workspace_selected(size);
            }
            NetEvent::ArchivedWorkspaces(state) => {
                self.archived_workspaces = state
                    .workspaces
                    .into_values()
                    .map(|workspace| (workspace.id, workspace))
                    .collect();
                self.mark_workspace_list_dirty();
                self.ensure_workspace_selected(size);
            }
            NetEvent::Summaries(data) => {
                let mut changed = false;
                for summary in data {
                    let workspace_id = summary.workspace_id;
                    if self.summaries.get(&workspace_id) != Some(&summary) {
                        self.summaries.insert(workspace_id, summary);
                        changed = true;
                    }
                }
                if changed {
                    self.mark_workspace_list_dirty();
                }
            }
            NetEvent::WorkspaceLoaded(workspace) => {
                self.bundle.workspace = Some(workspace);
                self.mark_detail_dirty();
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
                    self.mark_detail_dirty();
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
                    self.mark_git_dirty();
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
                    self.mark_changes_dirty();
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
                    self.mark_detail_dirty();
                    self.mark_chat_render_cache_dirty();
                    self.refresh_conversation_history();
                }
            }
            NetEvent::LogsUpdated {
                process_id,
                entries,
            } => {
                let chat_anchor = self.capture_chat_viewport_anchor(size);
                if Some(process_id) == self.bundle.selected_process_id {
                    self.bundle.log_entries = entries.clone();
                    self.mark_logs_dirty();
                }
                if self.conversation_process_order.contains(&process_id) {
                    self.conversation_process_entries
                        .insert(process_id, entries);
                    self.recompute_current_todo_state();
                    self.reconcile_optimistic_entries();
                    self.mark_chat_render_cache_dirty();
                    self.restore_chat_viewport_anchor(size, chat_anchor);
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
                    if self.is_queue_present() && draft.is_some() {
                        return;
                    }
                    if let Some(draft) = draft {
                        let message = draft.message.trim_end_matches('\n').to_string();
                        let executor_config = draft.executor_config.clone();
                        self.apply_follow_up_draft(DraftFollowUpData {
                            message,
                            executor_config,
                        });
                    } else {
                        self.restore_composer_document(String::new());
                    }
                    self.composer_scratch_loaded = true;
                    self.composer_dirty = false;
                    self.last_composer_edit = None;
                    self.composer_queue_conflict = false;
                }
            }
            NetEvent::WorkspaceCreateReposLoaded { repos } => {
                if let Some(state) = self.workspace_create.as_mut() {
                    state.available_repos = repos;
                    state.repos_loading = false;
                    self.mark_detail_dirty();
                }
            }
            NetEvent::WorkspaceCreateDraftLoaded { draft } => {
                if self.creating_workspace {
                    if let Some(draft) = draft {
                        self.apply_workspace_create_draft(draft);
                    } else if let Some(state) = self.workspace_create.as_mut() {
                        state.draft_loading = false;
                    }
                    self.composer_scratch_loaded = true;
                    self.mark_detail_dirty();
                }
            }
            NetEvent::WorkspaceCreateDraftSaved { revision } => {
                if self.creating_workspace {
                    self.draft_save_in_flight = false;
                    if revision == self.composer_edit_revision {
                        self.composer_dirty = false;
                        self.last_composer_edit = None;
                    }
                }
            }
            NetEvent::WorkspaceCreateDraftSaveFailed { revision, message } => {
                if self.creating_workspace {
                    self.draft_save_in_flight = false;
                    if revision == self.composer_edit_revision {
                        self.error = Some(message.clone());
                        self.status = message;
                    }
                }
            }
            NetEvent::WorkspaceCreateBranchesLoaded { repo_id, branches } => {
                if let Some(picker) = self.workspace_create_branch_picker.as_mut()
                    && let Some(state) = self.workspace_create.as_mut()
                    && state
                        .selected_repos
                        .get(picker.selected_repo_index)
                        .is_some_and(|entry| entry.repo.id == repo_id)
                {
                    let default_branch = state.selected_repos[picker.selected_repo_index]
                        .target_branch
                        .clone();
                    let selected = branches
                        .iter()
                        .position(|branch| branch.name == default_branch)
                        .or_else(|| branches.iter().position(|branch| branch.is_current))
                        .unwrap_or(0);
                    picker.branches = branches;
                    picker.selected = selected;
                    self.status = "Select target branch".to_string();
                }
            }
            NetEvent::WorkspaceCreateSubmitted { workspace } => {
                self.creating_workspace = false;
                self.workspace_create = None;
                self.workspace_create_repo_picker = None;
                self.workspace_create_branch_picker = None;
                self.selected_workspace_id = Some(workspace.id);
                self.mark_workspace_list_dirty();
                self.load_selected_workspace(size);
                self.status = "Workspace created".to_string();
                self.error = None;
            }
            NetEvent::WorkspaceCreateSubmitFailed { message } => {
                if let Some(state) = self.workspace_create.as_mut() {
                    state.submitting = false;
                }
                self.error = Some(message.clone());
                self.status = message;
                self.focus = Focus::Composer;
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
                    let chat_anchor = self.capture_chat_viewport_anchor(size);
                    self.conversation_process_entries
                        .insert(process_id, entries);
                    self.recompute_current_todo_state();
                    self.reconcile_optimistic_entries();
                    self.mark_chat_render_cache_dirty();
                    self.restore_chat_viewport_anchor(size, chat_anchor);
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
                    self.mark_detail_dirty();
                    self.mark_chat_render_cache_dirty();
                }
            }
            NetEvent::PromptSubmitted {
                workspace_id,
                session_id,
                workspace_scope,
            } => {
                self.actions_in_flight.prompt_submit = false;
                if let Some(workspace_scope) = workspace_scope {
                    self.rekey_new_session_optimistic_entries(workspace_scope, session_id);
                }
                self.creating_new_session = false;
                self.bundle.selected_session_id = Some(session_id);
                self.mark_detail_dirty();
                if Some(workspace_id) == self.selected_workspace_id {
                    self.api.load_workspace(workspace_id, self.tx.clone());
                }
                self.status = "Prompt sent".to_string();
                self.error = None;
            }
            NetEvent::PromptSubmissionFailed {
                message,
                restored_draft,
                optimistic_id,
            } => {
                self.actions_in_flight.prompt_submit = false;
                if let Some(local_id) = optimistic_id {
                    self.mark_optimistic_failed(local_id);
                }
                self.apply_follow_up_draft(restored_draft);
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.focus = Focus::Composer;
                self.error = Some(message.clone());
                self.status = message;
            }
            NetEvent::QueuedPrompt { session_id, status } => {
                self.actions_in_flight.queue_mutation = false;
                if Some(session_id) == self.current_queue_session_id() {
                    self.queue_status = status;
                    self.queue_pending = false;
                    self.mark_detail_dirty();
                    self.composer.clear();
                    self.composer_snippets.clear();
                    self.invalidate_composer_layout_cache();
                    self.composer_cursor = 0;
                    self.composer_dirty = false;
                    self.last_composer_edit = None;
                    self.focus = Focus::Main;
                    self.status = "Queued follow-up".to_string();
                    self.error = None;
                }
            }
            NetEvent::QueuePromptFailed { message } => {
                self.actions_in_flight.queue_mutation = false;
                self.error = Some(message.clone());
                self.status = message;
            }
            NetEvent::QueueCancelled {
                session_id,
                status,
                restored,
            } => {
                self.actions_in_flight.queue_mutation = false;
                if Some(session_id) == self.current_queue_session_id() {
                    self.queue_status = status;
                    self.queue_pending = false;
                    self.mark_detail_dirty();
                    if let Some(queued) = restored {
                        self.apply_follow_up_draft(queued);
                        self.composer_dirty = true;
                        self.last_composer_edit = Some(std::time::Instant::now());
                        self.composer_queue_conflict = false;
                    }
                    self.status = "Cancelled queued follow-up".to_string();
                    self.error = None;
                }
            }
            NetEvent::QueueCancelFailed { message } => {
                self.actions_in_flight.queue_mutation = false;
                self.error = Some(message.clone());
                self.status = message;
            }
            NetEvent::DraftDiscarded { message } => {
                self.actions_in_flight.queue_mutation = false;
                self.status = message;
                self.error = None;
            }
            NetEvent::DraftDiscardFailed { message } => {
                self.actions_in_flight.queue_mutation = false;
                self.error = Some(message.clone());
                self.status = message;
            }
            NetEvent::WorkspaceActionFinished {
                kind,
                success,
                message,
            } => {
                match kind {
                    WorkspaceActionKind::TogglePinned => {
                        self.actions_in_flight.pin_toggle = false;
                    }
                    WorkspaceActionKind::ToggleArchived => {
                        self.actions_in_flight.archive_toggle = false;
                    }
                    WorkspaceActionKind::StopExecution => {
                        self.actions_in_flight.stop_execution = false;
                    }
                    WorkspaceActionKind::StartDevServer => {
                        self.actions_in_flight.dev_server = false;
                    }
                    WorkspaceActionKind::RunCleanup => {
                        self.actions_in_flight.cleanup = false;
                    }
                    WorkspaceActionKind::OpenEditor => {
                        self.actions_in_flight.open_editor = false;
                    }
                }
                if success {
                    self.status = message;
                    self.error = None;
                } else {
                    self.error = Some(message.clone());
                    self.status = message;
                }
            }
            NetEvent::TerminalConnected(workspace_id) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.connected = true;
                    self.bundle.terminal.error = None;
                    self.mark_terminal_dirty();
                }
            }
            NetEvent::TerminalOutput(workspace_id, bytes) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.parser.process(&bytes);
                    self.mark_terminal_dirty();
                }
            }
            NetEvent::TerminalError(workspace_id, error) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.terminal.error = Some(error);
                    self.bundle.terminal.connected = false;
                    self.mark_terminal_dirty();
                }
            }
            NetEvent::Error(message) => {
                log_tui(format!("net event error: {message}"));
                self.error = Some(message.clone());
                self.status = message;
            }
        }
    }

    pub(crate) async fn handle_app_intent(&mut self, intent: AppIntent, size: Rect) {
        match intent {
            AppIntent::CancelNewSession => self.cancel_new_session_flow(),
            AppIntent::ToggleComposerEditorMode => self.toggle_editor_mode(),
            AppIntent::ToggleMaximizedPanel => {
                self.maximized_panel = !self.maximized_panel;
                self.status = if self.maximized_panel {
                    format!("Maximized {}", self.active_region_label())
                } else {
                    "Restored multi-panel layout".to_string()
                };
            }
            AppIntent::Quit => self.should_quit = true,
            AppIntent::FocusNext => self.focus = next_focus(&self.focus),
            AppIntent::FocusPrev => self.focus = prev_focus(&self.focus),
            AppIntent::ShowHelp => {
                self.status = "Keys: Tab focus, Ctrl+W maximize active panel, / search or filter, F workspace project filter, t focus todos, n/N next/prev chat match, [/ ] user turns, T compact tool runs, j/k nav, 1-6 panes, i edit, Enter open/send, r rename session, E executor (new session only), V variant, M model, R reasoning, A agent menu, P permission, p pin, x archive, v stop execution, n new session, s start dev, c cleanup, e editor, create mode: a add repo, Enter branch, d remove, Esc cancel, Esc/C-]/C-g leave terminal".to_string();
            }
            AppIntent::OpenSearch => self.open_search(size),
            AppIntent::FocusTodos => self.focus_todos(),
            AppIntent::SelectPane(pane) => {
                if self.creating_workspace {
                    self.selected_pane = Pane::Chat;
                } else {
                    self.selected_pane = pane;
                }
            }
            AppIntent::ToggleShowArchived => {
                self.show_archived = !self.show_archived;
                self.mark_workspace_list_dirty();
            }
            AppIntent::OpenWorkspaceProjectFilter => self.open_workspace_project_filter_picker(),
            AppIntent::EnterEditMode => {
                if matches!(self.selected_pane, Pane::Chat | Pane::Notes) {
                    self.focus = Focus::Composer;
                }
            }
            AppIntent::StartNewSession => {
                if self.creating_workspace {
                    self.status = "Workspace creation is active".to_string();
                    return;
                }
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
            AppIntent::StopExecution => self.stop_current_execution().await,
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
            AppIntent::ToggleDiffViewMode => {
                if self.selected_pane == Pane::Changes {
                    self.bundle.diff_view_mode = match self.bundle.diff_view_mode {
                        crate::model::DiffViewMode::Unified => {
                            crate::model::DiffViewMode::SideBySide
                        }
                        crate::model::DiffViewMode::SideBySide => {
                            crate::model::DiffViewMode::Unified
                        }
                    };
                    self.mark_changes_dirty();
                    self.status = match self.bundle.diff_view_mode {
                        crate::model::DiffViewMode::Unified => "Diff view: unified".to_string(),
                        crate::model::DiffViewMode::SideBySide => {
                            "Diff view: side by side".to_string()
                        }
                    };
                }
            }
            AppIntent::ToggleToolRunCollapse => {
                if self.selected_pane == Pane::Chat {
                    self.tool_call_display_mode = match self.tool_call_display_mode {
                        ToolCallDisplayMode::Expanded => ToolCallDisplayMode::Collapsed,
                        ToolCallDisplayMode::Collapsed => ToolCallDisplayMode::Expanded,
                    };
                    self.mark_chat_render_cache_dirty();
                    self.status = match self.tool_call_display_mode {
                        ToolCallDisplayMode::Expanded => "Tool calls: expanded".to_string(),
                        ToolCallDisplayMode::Collapsed => "Tool calls: collapsed runs".to_string(),
                    };
                }
            }
            AppIntent::PrevUserMessage | AppIntent::NextUserMessage => {}
            AppIntent::Enter => self.handle_enter(size).await,
            AppIntent::JumpToStart => self.jump_to_boundary(false, size),
            AppIntent::JumpToEnd => self.jump_to_boundary(true, size),
            AppIntent::MoveSelection(delta) => self.move_selection(delta, size),
            AppIntent::PageSelection(direction) => {
                self.move_selection(direction * self.page_step(size), size)
            }
        }
    }

    fn capture_chat_viewport_anchor(&mut self, size: Rect) -> Option<ChatViewportAnchor> {
        let metrics = self.chat_viewport_metrics(size)?;
        if metrics.clamped_end_offset == 0 {
            return None;
        }
        let cache = self.chat_render_cache(metrics.content_width);
        Some(ChatViewportAnchor {
            top_offset: metrics.top_offset,
            visible_line_texts: cache.lines[metrics.start..metrics.end]
                .iter()
                .take(3)
                .map(chat_line_text)
                .collect(),
        })
    }

    fn restore_chat_viewport_anchor(&mut self, size: Rect, anchor: Option<ChatViewportAnchor>) {
        let Some(anchor) = anchor else {
            return;
        };
        let Some(metrics) = self.chat_viewport_metrics(size) else {
            return;
        };
        let cache = self.chat_render_cache(metrics.content_width);
        let restored_top_offset =
            find_chat_anchor_top_offset(&cache.lines, &anchor).unwrap_or(anchor.top_offset);
        self.chat_end_offset = metrics
            .total_lines
            .saturating_sub(metrics.visible_lines.saturating_add(restored_top_offset))
            .min(u16::MAX as usize) as u16;
    }

    fn chat_viewport_metrics(&mut self, size: Rect) -> Option<ChatViewportMetrics> {
        if self.selected_pane != Pane::Chat {
            return None;
        }

        let body_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(size)[1];

        let main_area = if self.maximized_panel {
            if !matches!(self.focus, Focus::Main | Focus::Composer) {
                return None;
            }
            body_area
        } else if size.width >= 140 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(32),
                    Constraint::Min(50),
                    Constraint::Length(44),
                ])
                .split(body_area)[1]
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(32), Constraint::Min(40)])
                .split(body_area)[1]
        };

        let composer_height = self.chat_composer_height(main_area.width);
        let messages_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(4),
                Constraint::Length(composer_height),
            ])
            .split(main_area)[1];

        let content_width = messages_area.width.saturating_sub(4).max(1) as usize;
        let has_status_row = self
            .chat_render_cache(content_width)
            .latest_token_usage
            .is_some();
        let messages_area = if has_status_row {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(messages_area)[0]
        } else {
            messages_area
        };
        let padded_messages_area = messages_area.inner(Margin {
            vertical: 0,
            horizontal: 1,
        });
        let content_area = if padded_messages_area.width > 0 {
            padded_messages_area
        } else {
            messages_area
        };
        let visible_lines = content_area.height.max(1) as usize;
        let requested_end_offset = self.chat_end_offset as usize;
        let cache = self.chat_render_cache(content_width);
        let (clamped_end_offset, start, end, top_offset) =
            chat_window_bounds(cache.lines.len(), visible_lines, requested_end_offset);

        Some(ChatViewportMetrics {
            content_width,
            total_lines: cache.lines.len(),
            visible_lines,
            clamped_end_offset,
            start,
            end,
            top_offset,
        })
    }
}

fn chat_line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn find_chat_anchor_top_offset(lines: &[Line<'_>], anchor: &ChatViewportAnchor) -> Option<usize> {
    if anchor.visible_line_texts.is_empty() {
        return Some(anchor.top_offset);
    }
    let line_texts = lines.iter().map(chat_line_text).collect::<Vec<_>>();
    line_texts
        .windows(anchor.visible_line_texts.len())
        .position(|window| window == anchor.visible_line_texts.as_slice())
        .or_else(|| {
            line_texts
                .iter()
                .position(|line| line == &anchor.visible_line_texts[0])
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::{
        execution_process::{
            ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus,
            ExecutorActionField,
        },
        scratch::DraftFollowUpData,
        session::Session,
    };
    use executors::{
        actions::{
            ExecutorAction, ExecutorActionType,
            script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
        },
        logs::{NormalizedEntry, NormalizedEntryType},
        profile::{ExecutorConfig, ExecutorConfigs},
    };
    use ratatui::layout::Rect;
    use sqlx::types::Json;
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::App,
        editor::ComposerEditorMode,
        model::{Focus, NetEvent, Pane, PatchType, QueueStatus, WorkspaceBundle},
    };

    fn visible_chat_lines(app: &mut App, size: Rect) -> Vec<String> {
        let metrics = app.chat_viewport_metrics(size).expect("chat viewport");
        let cache = app.chat_render_cache(metrics.content_width);
        cache.lines[metrics.start..metrics.end]
            .iter()
            .map(super::chat_line_text)
            .collect()
    }

    fn test_app() -> App {
        let api = Api::new("http://127.0.0.1:9".to_string()).unwrap();
        let (tx, rx) = unbounded_channel();
        App {
            api,
            rx,
            tx,
            workspace_streams: Vec::new(),
            summary_streams: Vec::new(),
            subscriptions: WorkspaceSubscriptions::default(),
            active_workspaces: HashMap::new(),
            archived_workspaces: HashMap::new(),
            summaries: HashMap::new(),
            selected_workspace_id: None,
            selected_pane: Pane::Chat,
            focus: Focus::Main,
            detail_section: crate::app::DetailSection::Sessions,
            maximized_panel: false,
            show_archived: false,
            filter: String::new(),
            workspace_project_filters: Vec::new(),
            session_filter: String::new(),
            workspace_list_revision: 0,
            detail_revision: 0,
            workspace_list_cache: None,
            detail_pane_cache: None,
            status: String::new(),
            error: None,
            bundle: WorkspaceBundle::default(),
            executor_profiles: ExecutorConfigs {
                executors: HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: Some(ExecutorConfig::new(
                executors::executors::BaseCodingAgent::Codex,
            )),
            composer_options: None,
            composer: String::new(),
            composer_snippets: Vec::new(),
            composer_cursor: 0,
            editor_mode: ComposerEditorMode::Standard,
            vim_pending_operator: None,
            composer_dirty: false,
            composer_edit_revision: 0,
            composer_height_cache: None,
            draft_save_in_flight: false,
            composer_queue_conflict: false,
            composer_scratch_id: None,
            composer_scratch_loaded: false,
            queue_session_id: None,
            queue_status: QueueStatus::Empty,
            queue_pending: false,
            last_composer_edit: None,
            chat_end_offset: 0,
            chat_render_cache: None,
            chat_render_cache_dirty: true,
            last_chat_render_cache_build: None,
            conversation_loader: None,
            conversation_process_entries: HashMap::new(),
            conversation_process_order: Vec::new(),
            conversation_bootstrapping: false,
            conversation_backfilling: false,
            current_todos: None,
            selected_todo_index: 0,
            optimistic_entries: Vec::new(),
            notes_cursor: 0,
            notes_edit_revision: 0,
            notes_save_in_flight: false,
            agent_picker: None,
            workspace_project_filter_picker: None,
            session_rename: None,
            snippet_preview: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: crate::app::ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            workspace_create: None,
            workspace_create_repo_picker: None,
            workspace_create_branch_picker: None,
            creating_workspace: false,
            workspace_create_previous_selection: None,
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn session(id: Uuid, name: &str) -> Session {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        Session {
            id,
            workspace_id: Uuid::new_v4(),
            name: Some(name.to_string()),
            executor: Some("CODEX".to_string()),
            agent_working_dir: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn process(id: Uuid, second: i64) -> ExecutionProcess {
        let created_at = Utc.timestamp_opt(second, 0).unwrap();
        ExecutionProcess {
            id,
            session_id: Uuid::new_v4(),
            run_reason: ExecutionProcessRunReason::CodingAgent,
            executor_action: Json(ExecutorActionField::ExecutorAction(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: "echo hi".to_string(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
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

    #[tokio::test]
    async fn sessions_loaded_preserves_existing_selection_when_still_present() {
        let workspace_id = Uuid::new_v4();
        let first = session(Uuid::new_v4(), "first");
        let second = session(Uuid::new_v4(), "second");
        let process_id = Uuid::new_v4();
        let mut app = test_app();
        app.selected_workspace_id = Some(workspace_id);
        app.bundle.selected_session_id = Some(second.id);
        app.bundle
            .process_map
            .insert(process_id, process(process_id, 1));
        app.bundle.log_entries = vec![PatchType::Stdout("keep".to_string())];

        app.handle_net_event(
            NetEvent::SessionsLoaded {
                workspace_id,
                sessions: vec![first, second.clone()],
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert_eq!(app.bundle.selected_session_id, Some(second.id));
        assert_eq!(app.bundle.log_entries.len(), 1);
        assert_eq!(app.bundle.process_map.len(), 1);
    }

    #[tokio::test]
    async fn sessions_loaded_falls_back_and_clears_process_state_when_selection_disappears() {
        let workspace_id = Uuid::new_v4();
        let missing = session(Uuid::new_v4(), "missing");
        let first = session(Uuid::new_v4(), "first");
        let process_id = Uuid::new_v4();
        let mut app = test_app();
        app.selected_workspace_id = Some(workspace_id);
        app.bundle.selected_session_id = Some(missing.id);
        app.bundle.selected_process_id = Some(process_id);
        app.bundle
            .process_map
            .insert(process_id, process(process_id, 1));
        app.bundle.log_entries = vec![PatchType::Stdout("clear".to_string())];

        app.handle_net_event(
            NetEvent::SessionsLoaded {
                workspace_id,
                sessions: vec![first.clone()],
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert_eq!(app.bundle.selected_session_id, Some(first.id));
        assert!(app.bundle.process_map.is_empty());
        assert!(app.bundle.log_entries.is_empty());
        assert!(app.bundle.selected_process_id.is_none());
    }

    #[tokio::test]
    async fn draft_loaded_does_not_overwrite_local_dirty_edits() {
        let scratch_id = Uuid::new_v4();
        let mut app = test_app();
        app.bundle.selected_session_id = Some(scratch_id);
        app.composer = "local".to_string();
        app.composer_dirty = true;
        app.composer_scratch_loaded = true;

        app.handle_net_event(
            NetEvent::DraftLoaded {
                scratch_id,
                draft: Some(DraftFollowUpData {
                    message: "remote".to_string(),
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                }),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert_eq!(app.composer, "local");
        assert!(app.composer_dirty);
    }

    #[tokio::test]
    async fn queued_session_ignores_non_empty_draft_reload() {
        let session_id = Uuid::new_v4();
        let mut app = test_app();
        app.bundle.selected_session_id = Some(session_id);
        app.queue_status = QueueStatus::Queued {
            message: crate::model::QueuedMessage {
                session_id,
                data: DraftFollowUpData {
                    message: "queued".to_string(),
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                },
                queued_at: chrono::Utc::now(),
            },
        };
        app.composer_scratch_loaded = false;

        app.handle_net_event(
            NetEvent::DraftLoaded {
                scratch_id: session_id,
                draft: Some(DraftFollowUpData {
                    message: "stale".to_string(),
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                }),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert!(app.composer.is_empty());
        assert!(!app.composer_scratch_loaded);
    }

    #[tokio::test]
    async fn prompt_submission_failed_restores_composer_and_clears_in_flight_flag() {
        let local_id = Uuid::new_v4();
        let mut app = test_app();
        app.actions_in_flight.prompt_submit = true;
        app.focus = Focus::Main;

        app.handle_net_event(
            NetEvent::PromptSubmissionFailed {
                message: "backend slow".to_string(),
                restored_draft: DraftFollowUpData {
                    message: "retry me".to_string(),
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                },
                optimistic_id: Some(local_id),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert!(!app.actions_in_flight.prompt_submit);
        assert_eq!(app.composer, "retry me");
        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.status, "backend slow");
    }

    #[tokio::test]
    async fn queued_prompt_event_clears_composer_and_in_flight_flag() {
        let session_id = Uuid::new_v4();
        let mut app = test_app();
        app.bundle.selected_session_id = Some(session_id);
        app.composer = "queued".to_string();
        app.composer_cursor = app.composer.len();
        app.composer_dirty = true;
        app.actions_in_flight.queue_mutation = true;
        app.focus = Focus::Composer;

        app.handle_net_event(
            NetEvent::QueuedPrompt {
                session_id,
                status: QueueStatus::Empty,
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert!(!app.actions_in_flight.queue_mutation);
        assert!(app.composer.is_empty());
        assert!(!app.composer_dirty);
        assert_eq!(app.focus, Focus::Main);
        assert_eq!(app.status, "Queued follow-up");
    }

    #[tokio::test]
    async fn processes_updated_selects_latest_active_process_and_clears_logs_on_change() {
        let session_id = Uuid::new_v4();
        let old_process = process(Uuid::new_v4(), 1);
        let newer_process = process(Uuid::new_v4(), 2);
        let old_process_id = old_process.id;
        let mut app = test_app();
        app.bundle.selected_session_id = Some(session_id);
        app.bundle.selected_process_id = Some(old_process.id);
        app.bundle
            .process_map
            .insert(old_process.id, old_process.clone());
        app.bundle.log_entries = vec![PatchType::Stdout("stale".to_string())];

        app.handle_net_event(
            NetEvent::ProcessesUpdated {
                session_id,
                processes: HashMap::from([
                    (old_process.id, old_process),
                    (newer_process.id, newer_process.clone()),
                ]),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert_eq!(app.bundle.selected_process_id, Some(newer_process.id));
        assert!(app.bundle.log_entries.is_empty());
        assert!(app.queue_pending);
        assert!(app.chat_render_cache_dirty);
        assert_eq!(
            app.conversation_process_order,
            vec![old_process_id, newer_process.id]
        );
    }

    #[tokio::test]
    async fn processes_updated_keeps_previous_selection_when_only_dev_server_remains_active() {
        let session_id = Uuid::new_v4();
        let selected = process(Uuid::new_v4(), 1);
        let mut dev_server = process(Uuid::new_v4(), 2);
        dev_server.run_reason = ExecutionProcessRunReason::DevServer;
        let mut app = test_app();
        app.bundle.selected_session_id = Some(session_id);
        app.bundle.selected_process_id = Some(selected.id);

        app.handle_net_event(
            NetEvent::ProcessesUpdated {
                session_id,
                processes: HashMap::from([
                    (selected.id, selected.clone()),
                    (dev_server.id, dev_server),
                ]),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;

        assert_eq!(app.bundle.selected_process_id, Some(selected.id));
    }

    #[tokio::test]
    async fn logs_updated_only_replaces_selected_process_log_and_preserves_chat_offset() {
        let selected_process = Uuid::new_v4();
        let other_process = Uuid::new_v4();
        let mut entries = (0..24)
            .map(|index| PatchType::Stdout(format!("visible {index}")))
            .collect::<Vec<_>>();
        let mut app = test_app();
        let size = Rect::new(0, 0, 80, 24);
        app.selected_pane = Pane::Chat;
        app.focus = Focus::Main;
        app.bundle.selected_process_id = Some(selected_process);
        app.conversation_process_order = vec![selected_process];
        app.conversation_process_entries
            .insert(selected_process, entries.clone());
        app.mark_chat_render_cache_dirty();
        app.chat_end_offset = 7;
        let previous_lines = visible_chat_lines(&mut app, size);

        app.handle_net_event(
            NetEvent::LogsUpdated {
                process_id: selected_process,
                entries: entries.clone(),
            },
            size,
        )
        .await;

        assert_eq!(app.bundle.log_entries.len(), entries.len());
        assert_eq!(visible_chat_lines(&mut app, size), previous_lines);
        assert_eq!(
            app.conversation_process_entries
                .get(&selected_process)
                .map(Vec::len)
                .unwrap_or_default(),
            entries.len()
        );

        app.bundle.log_entries.clear();
        app.chat_end_offset = 5;
        let previous_offset = app.chat_end_offset;
        entries.push(PatchType::Stdout("visible 24".to_string()));
        app.handle_net_event(
            NetEvent::LogsUpdated {
                process_id: other_process,
                entries: vec![PatchType::Stdout("other".to_string())],
            },
            size,
        )
        .await;
        assert!(app.bundle.log_entries.is_empty());
        assert_eq!(app.chat_end_offset, previous_offset);
    }

    #[tokio::test]
    async fn logs_updated_keeps_following_tail_when_chat_is_at_bottom() {
        let selected_process = Uuid::new_v4();
        let mut entries = (0..24)
            .map(|index| PatchType::Stdout(format!("visible {index}")))
            .collect::<Vec<_>>();
        let mut app = test_app();
        let size = Rect::new(0, 0, 80, 24);
        app.selected_pane = Pane::Chat;
        app.focus = Focus::Main;
        app.bundle.selected_process_id = Some(selected_process);
        app.conversation_process_order = vec![selected_process];
        app.conversation_process_entries
            .insert(selected_process, entries.clone());
        app.mark_chat_render_cache_dirty();
        app.chat_end_offset = 0;

        entries.push(PatchType::Stdout("visible 24".to_string()));
        app.handle_net_event(
            NetEvent::LogsUpdated {
                process_id: selected_process,
                entries,
            },
            size,
        )
        .await;

        assert_eq!(app.chat_end_offset, 0);
        assert_eq!(
            app.chat_viewport_metrics(size)
                .expect("chat viewport")
                .clamped_end_offset,
            0
        );
    }

    #[tokio::test]
    async fn conversation_history_loaded_updates_process_entries_and_preserves_scroll() {
        let session_id = Uuid::new_v4();
        let process_id = Uuid::new_v4();
        let size = Rect::new(0, 0, 80, 24);
        let entries = (0..24)
            .map(|index| {
                PatchType::NormalizedEntry(NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::AssistantMessage,
                    content: format!("hello {index}"),
                    metadata: None,
                })
            })
            .collect::<Vec<_>>();
        let mut app = test_app();
        app.selected_pane = Pane::Chat;
        app.focus = Focus::Main;
        app.bundle.selected_session_id = Some(session_id);
        app.conversation_process_order = vec![process_id];
        app.conversation_process_entries
            .insert(process_id, entries[..20].to_vec());
        app.mark_chat_render_cache_dirty();
        app.chat_end_offset = 7;
        let previous_lines = visible_chat_lines(&mut app, size);

        app.handle_net_event(
            NetEvent::ConversationHistoryLoaded {
                session_id,
                process_id,
                entries: entries.clone(),
            },
            size,
        )
        .await;

        assert_eq!(
            app.conversation_process_entries
                .get(&process_id)
                .map(Vec::len),
            Some(entries.len())
        );
        assert_eq!(visible_chat_lines(&mut app, size), previous_lines);
    }

    #[tokio::test]
    async fn draft_save_failed_sets_queue_conflict_only_for_matching_revision() {
        let scratch_id = Uuid::new_v4();
        let mut app = test_app();
        app.bundle.selected_session_id = Some(scratch_id);
        app.composer_edit_revision = 4;
        app.draft_save_in_flight = true;

        app.handle_net_event(
            NetEvent::DraftSaveFailed {
                scratch_id,
                revision: 4,
                message: "queued message exists".to_string(),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;
        assert!(app.composer_queue_conflict);
        assert!(!app.draft_save_in_flight);

        app.composer_queue_conflict = false;
        app.draft_save_in_flight = true;
        app.handle_net_event(
            NetEvent::DraftSaveFailed {
                scratch_id,
                revision: 3,
                message: "queued message exists".to_string(),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;
        assert!(!app.composer_queue_conflict);
        assert!(!app.draft_save_in_flight);
    }

    #[tokio::test]
    async fn notes_save_events_only_apply_for_matching_workspace_and_revision() {
        let selected_workspace = Uuid::new_v4();
        let other_workspace = Uuid::new_v4();
        let mut app = test_app();
        app.selected_workspace_id = Some(selected_workspace);
        app.notes_edit_revision = 8;
        app.notes_save_in_flight = true;
        app.bundle.notes_dirty = true;
        app.bundle.last_notes_edit = Some(std::time::Instant::now());

        app.handle_net_event(
            NetEvent::NotesSaved {
                workspace_id: selected_workspace,
                revision: 8,
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;
        assert!(!app.bundle.notes_dirty);
        assert!(app.bundle.last_notes_edit.is_none());
        assert!(!app.notes_save_in_flight);

        app.bundle.notes_dirty = true;
        app.notes_save_in_flight = true;
        app.error = None;
        app.handle_net_event(
            NetEvent::NotesSaveFailed {
                workspace_id: other_workspace,
                revision: 8,
                message: "ignore me".to_string(),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;
        assert!(app.error.is_none());
        assert!(app.notes_save_in_flight);

        app.handle_net_event(
            NetEvent::NotesSaveFailed {
                workspace_id: selected_workspace,
                revision: 7,
                message: "stale".to_string(),
            },
            Rect::new(0, 0, 80, 24),
        )
        .await;
        assert!(app.error.is_none());
        assert!(!app.notes_save_in_flight);
    }

    #[tokio::test]
    async fn identical_summaries_do_not_invalidate_workspace_list() {
        use crate::model::WorkspaceSummary;

        let workspace_id = Uuid::new_v4();
        let summary = WorkspaceSummary {
            workspace_id,
            project_id: None,
            project_name: None,
            remote_project_id: None,
            latest_session_id: Some(Uuid::new_v4()),
            has_pending_approval: true,
            files_changed: Some(3),
            lines_added: Some(10),
            lines_removed: Some(4),
            latest_process_completed_at: Some(Utc.timestamp_opt(5, 0).unwrap()),
            latest_process_status: Some(ExecutionProcessStatus::Completed),
            has_running_dev_server: false,
            has_unseen_turns: true,
            pr_status: None,
            pr_number: None,
            pr_url: None,
        };
        let mut app = test_app();
        app.summaries.insert(workspace_id, summary.clone());
        app.workspace_list_revision = 7;

        app.handle_net_event(NetEvent::Summaries(vec![summary]), Rect::new(0, 0, 80, 24))
            .await;

        assert_eq!(app.workspace_list_revision, 7);
    }

    #[tokio::test]
    async fn changed_summaries_invalidate_workspace_list() {
        use crate::model::WorkspaceSummary;

        let workspace_id = Uuid::new_v4();
        let existing = WorkspaceSummary {
            workspace_id,
            project_id: None,
            project_name: None,
            remote_project_id: None,
            latest_session_id: Some(Uuid::new_v4()),
            has_pending_approval: false,
            files_changed: Some(1),
            lines_added: Some(2),
            lines_removed: Some(1),
            latest_process_completed_at: Some(Utc.timestamp_opt(5, 0).unwrap()),
            latest_process_status: Some(ExecutionProcessStatus::Completed),
            has_running_dev_server: false,
            has_unseen_turns: false,
            pr_status: None,
            pr_number: None,
            pr_url: None,
        };
        let updated = WorkspaceSummary {
            has_pending_approval: true,
            ..existing.clone()
        };
        let mut app = test_app();
        app.summaries.insert(workspace_id, existing);
        app.workspace_list_revision = 7;

        app.handle_net_event(NetEvent::Summaries(vec![updated]), Rect::new(0, 0, 80, 24))
            .await;

        assert_eq!(app.workspace_list_revision, 8);
    }
}
