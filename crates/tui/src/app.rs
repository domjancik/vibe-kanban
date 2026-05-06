use std::{collections::HashMap, str::FromStr, time::Duration};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
    scratch::DraftFollowUpData,
    session::Session,
    workspace::WorkspaceWithStatus,
};
use executors::{
    executor_discovery::ExecutorDiscoveredOptions,
    executors::BaseCodingAgent,
    model_selector::{AgentInfo, ModelInfo, PermissionPolicy},
    profile::{ExecutorConfig, ExecutorConfigs, ExecutorProfileId},
};
use futures_util::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::interval,
};
use uuid::Uuid;

use crate::{
    api::{Api, TerminalCommand, WorkspaceSubscriptions},
    conversation::{
        ChatRenderCache, ConversationScope, OptimisticConversationEntry, OptimisticState,
        chat_window_bounds, initial_conversation_process_ids, process_prompt, render_chat_entry,
        render_log_entry, render_optimistic_chat_entry, wrap_lines,
    },
    editor::{
        ComposerEditorMode, VimMode, VimOperator, line_end_index, line_start_index,
        move_cursor_vertical, next_word_start, prev_word_start, render_editor_buffer,
    },
    input::{AppIntent, map_app_key, next_focus, prev_focus},
    model::{
        Focus, NetEvent, Pane, PatchType, QueueStatus, TerminalState, WorkspaceBundle,
        WorkspaceSummary, active_process, diff_title, display_permission, display_variant,
        format_relative_time, workspace_title,
    },
    ui::{
        centered_rect, panel_block, rect_from_size, render_vertical_scrollbar,
        terminal_content_area,
    },
};

enum WorkspaceRow<'a> {
    Header(&'static str),
    Workspace(&'a WorkspaceWithStatus),
}

enum SessionRow<'a> {
    NewSession,
    Session(&'a Session),
}

#[derive(Clone, Copy)]
enum SessionTarget {
    NewSession,
    Existing(Uuid),
}

struct AgentPickerState {
    query: String,
    selected: usize,
}

struct SessionRenameState {
    session_id: Uuid,
    name: String,
    cursor: usize,
}

pub struct App {
    api: Api,
    rx: UnboundedReceiver<NetEvent>,
    tx: UnboundedSender<NetEvent>,
    workspace_streams: Vec<tokio::task::JoinHandle<()>>,
    summary_streams: Vec<tokio::task::JoinHandle<()>>,
    subscriptions: WorkspaceSubscriptions,
    active_workspaces: HashMap<Uuid, WorkspaceWithStatus>,
    archived_workspaces: HashMap<Uuid, WorkspaceWithStatus>,
    summaries: HashMap<Uuid, WorkspaceSummary>,
    selected_workspace_id: Option<Uuid>,
    selected_pane: Pane,
    focus: Focus,
    show_archived: bool,
    filter: String,
    status: String,
    error: Option<String>,
    bundle: WorkspaceBundle,
    executor_profiles: ExecutorConfigs,
    default_executor_profile: Option<ExecutorProfileId>,
    composer_config: Option<ExecutorConfig>,
    composer_options: Option<ExecutorDiscoveredOptions>,
    composer: String,
    composer_cursor: usize,
    composer_editor_mode: ComposerEditorMode,
    vim_pending_operator: Option<VimOperator>,
    composer_dirty: bool,
    composer_edit_revision: u64,
    draft_save_in_flight: bool,
    composer_queue_conflict: bool,
    composer_scratch_id: Option<Uuid>,
    composer_scratch_loaded: bool,
    queue_session_id: Option<Uuid>,
    queue_status: QueueStatus,
    queue_pending: bool,
    last_composer_edit: Option<std::time::Instant>,
    chat_end_offset: u16,
    chat_render_cache: Option<ChatRenderCache>,
    chat_render_cache_dirty: bool,
    last_chat_render_cache_build: Option<std::time::Instant>,
    conversation_loader: Option<tokio::task::JoinHandle<()>>,
    conversation_process_entries: HashMap<Uuid, Vec<PatchType>>,
    conversation_process_order: Vec<Uuid>,
    conversation_bootstrapping: bool,
    conversation_backfilling: bool,
    optimistic_entries: Vec<OptimisticConversationEntry>,
    notes_cursor: usize,
    notes_edit_revision: u64,
    notes_save_in_flight: bool,
    agent_picker: Option<AgentPickerState>,
    session_rename: Option<SessionRenameState>,
    creating_new_session: bool,
    should_quit: bool,
}

impl App {
    pub fn new(api: Api) -> Self {
        let (tx, rx) = unbounded_channel();
        let workspace_streams = api.spawn_workspace_streams(tx.clone());
        let summary_streams = api.spawn_summary_pollers(tx.clone());
        api.load_user_system_info(tx.clone());
        Self {
            api,
            rx,
            tx,
            workspace_streams,
            summary_streams,
            subscriptions: WorkspaceSubscriptions::default(),
            active_workspaces: HashMap::new(),
            archived_workspaces: HashMap::new(),
            summaries: HashMap::new(),
            selected_workspace_id: None,
            selected_pane: Pane::Chat,
            focus: Focus::WorkspaceList,
            show_archived: false,
            filter: String::new(),
            status: String::new(),
            error: None,
            bundle: WorkspaceBundle::default(),
            executor_profiles: ExecutorConfigs {
                executors: HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: None,
            composer_options: None,
            composer: String::new(),
            composer_cursor: 0,
            composer_editor_mode: ComposerEditorMode::Standard,
            vim_pending_operator: None,
            composer_dirty: false,
            composer_edit_revision: 0,
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
            optimistic_entries: Vec::new(),
            notes_cursor: 0,
            notes_edit_revision: 0,
            notes_save_in_flight: false,
            agent_picker: None,
            session_rename: None,
            creating_new_session: false,
            should_quit: false,
        }
    }

    pub async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut events = EventStream::new();
        let mut ticker = interval(Duration::from_millis(150));
        self.status = format!("Connected to {}", self.api.base_url);

        self.ensure_workspace_selected(rect_from_size(terminal.size()?));

        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            select! {
                _ = ticker.tick() => {
                    self.flush_notes_if_needed().await;
                    self.flush_draft_if_needed().await;
                }
                Some(event) = self.rx.recv() => {
                    self.handle_net_event(event, rect_from_size(terminal.size()?)).await;
                }
                Some(Ok(event)) = events.next() => {
                    if let CrosstermEvent::Key(key) = event {
                        self.handle_key(key, rect_from_size(terminal.size()?)).await;
                    }
                }
            }
        }

        self.subscriptions.abort();
        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        for handle in self.workspace_streams.drain(..) {
            handle.abort();
        }
        for handle in self.summary_streams.drain(..) {
            handle.abort();
        }
        Ok(())
    }

    async fn handle_net_event(&mut self, event: NetEvent, size: Rect) {
        match event {
            NetEvent::UserSystemLoaded(info) => {
                self.default_executor_profile = Some(info.config.executor_profile.clone());
                self.executor_profiles = info.profiles;
                if self.composer_config.is_none() {
                    self.composer_config = Some(ExecutorConfig::from(info.config.executor_profile));
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

    async fn handle_key(&mut self, key: KeyEvent, size: Rect) {
        if self.agent_picker.is_some() {
            self.handle_agent_picker_key(key);
            return;
        }
        if self.session_rename.is_some() {
            self.handle_session_rename_key(key).await;
            return;
        }

        if self.bundle.terminal.input_mode && self.selected_pane == Pane::Terminal {
            if is_terminal_exit_key(&key) {
                self.bundle.terminal.input_mode = false;
                self.focus = Focus::Main;
                self.status = "Left terminal input mode".to_string();
                return;
            }
            self.forward_terminal_key(key);
            return;
        }

        if self.focus == Focus::Composer {
            match self.selected_pane {
                Pane::Chat => {
                    self.handle_composer_key(key).await;
                    return;
                }
                Pane::Notes => {
                    if key.code == KeyCode::Esc {
                        self.focus = Focus::Main;
                    } else {
                        self.handle_text_input(key, true).await;
                    }
                    return;
                }
                _ => {}
            }
        }

        if let Some(intent) = map_app_key(key, self.creating_new_session, &self.selected_pane) {
            self.handle_app_intent(intent, size).await;
        }
    }

    async fn handle_app_intent(&mut self, intent: AppIntent, size: Rect) {
        match intent {
            AppIntent::CancelNewSession => self.cancel_new_session_flow(),
            AppIntent::ToggleComposerEditorMode => self.toggle_composer_editor_mode(),
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

    async fn handle_text_input(&mut self, key: KeyEvent, notes: bool) {
        let (buffer, cursor) = if notes {
            (&mut self.bundle.notes, &mut self.notes_cursor)
        } else {
            (&mut self.composer, &mut self.composer_cursor)
        };
        let mut changed = false;
        match key {
            KeyEvent {
                code: KeyCode::Enter,
                modifiers,
                ..
            } if !notes && !modifiers.contains(KeyModifiers::SHIFT) => {
                self.submit_prompt().await;
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                buffer.insert(*cursor, '\n');
                *cursor += 1;
                changed = true;
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if *cursor > 0 {
                    buffer.remove(*cursor - 1);
                    *cursor -= 1;
                    changed = true;
                }
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                if *cursor < buffer.len() {
                    buffer.remove(*cursor);
                    changed = true;
                }
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => *cursor = cursor.saturating_sub(1),
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => *cursor = (*cursor + 1).min(buffer.len()),
            KeyEvent {
                code: KeyCode::Up, ..
            } => *cursor = move_cursor_vertical(buffer, *cursor, -1),
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => *cursor = move_cursor_vertical(buffer, *cursor, 1),
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => *cursor = line_start_index(buffer, *cursor),
            KeyEvent {
                code: KeyCode::End, ..
            } => *cursor = line_end_index(buffer, *cursor),
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers,
                ..
            } if modifiers == KeyModifiers::CONTROL => {
                *cursor = line_start_index(buffer, *cursor);
            }
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers,
                ..
            } if modifiers == KeyModifiers::CONTROL => {
                *cursor = line_end_index(buffer, *cursor);
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                buffer.insert(*cursor, ch);
                *cursor += 1;
                changed = true;
            }
            _ => {}
        }
        if notes && changed {
            self.bundle.notes_dirty = true;
            self.bundle.last_notes_edit = Some(std::time::Instant::now());
            self.notes_edit_revision = self.notes_edit_revision.saturating_add(1);
        } else if changed {
            self.composer_dirty = true;
            self.last_composer_edit = Some(std::time::Instant::now());
            self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
        }
    }

    async fn handle_composer_key(&mut self, key: KeyEvent) {
        if let KeyEvent {
            code: KeyCode::F(2),
            ..
        } = key
        {
            self.toggle_composer_editor_mode();
            return;
        }

        match self.composer_editor_mode {
            ComposerEditorMode::Standard => {
                if key.code == KeyCode::Esc {
                    if self.creating_new_session && self.composer.trim().is_empty() {
                        self.cancel_new_session_flow();
                    } else {
                        self.focus = Focus::Main;
                    }
                } else {
                    self.handle_text_input(key, false).await;
                }
            }
            ComposerEditorMode::Vim(VimMode::Insert) => {
                if key.code == KeyCode::Esc {
                    if self.creating_new_session && self.composer.trim().is_empty() {
                        self.cancel_new_session_flow();
                    } else {
                        self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Normal);
                        self.status = "Composer mode: Vim Normal".to_string();
                    }
                } else {
                    self.handle_text_input(key, false).await;
                }
            }
            ComposerEditorMode::Vim(VimMode::Normal) => {
                if self.handle_vim_normal_key(key).await {
                    return;
                }
                if key.code == KeyCode::Esc {
                    if self.creating_new_session && self.composer.trim().is_empty() {
                        self.cancel_new_session_flow();
                    } else {
                        self.focus = Focus::Main;
                    }
                }
            }
        }
    }

    async fn handle_vim_normal_key(&mut self, key: KeyEvent) -> bool {
        if let Some(operator) = self.vim_pending_operator.take() {
            return self.execute_vim_operator(operator, key);
        }

        match key {
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.submit_prompt().await;
                true
            }
            KeyEvent {
                code: KeyCode::Char('i'),
                ..
            } => {
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('a'),
                ..
            } => {
                self.composer_cursor = (self.composer_cursor + 1).min(self.composer.len());
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('I'),
                ..
            } => {
                self.composer_cursor = line_start_index(&self.composer, self.composer_cursor);
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('A'),
                ..
            } => {
                self.composer_cursor = line_end_index(&self.composer, self.composer_cursor);
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('h') | KeyCode::Left,
                ..
            } => {
                self.composer_cursor = self.composer_cursor.saturating_sub(1);
                true
            }
            KeyEvent {
                code: KeyCode::Char('l') | KeyCode::Right,
                ..
            } => {
                self.composer_cursor = (self.composer_cursor + 1).min(self.composer.len());
                true
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => {
                self.composer_cursor =
                    move_cursor_vertical(&self.composer, self.composer_cursor, -1);
                true
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                self.composer_cursor =
                    move_cursor_vertical(&self.composer, self.composer_cursor, 1);
                true
            }
            KeyEvent {
                code: KeyCode::Char('w'),
                ..
            } => {
                self.composer_cursor = next_word_start(&self.composer, self.composer_cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('b'),
                ..
            } => {
                self.composer_cursor = prev_word_start(&self.composer, self.composer_cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('d'),
                ..
            } => {
                self.vim_pending_operator = Some(VimOperator::Delete);
                self.status = "d...".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('0') | KeyCode::Home,
                ..
            } => {
                self.composer_cursor = line_start_index(&self.composer, self.composer_cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('$') | KeyCode::End,
                ..
            } => {
                self.composer_cursor = line_end_index(&self.composer, self.composer_cursor);
                true
            }
            KeyEvent {
                code: KeyCode::Char('x') | KeyCode::Delete,
                ..
            } => {
                if self.composer_cursor < self.composer.len() {
                    self.composer.remove(self.composer_cursor);
                    self.composer_dirty = true;
                    self.last_composer_edit = Some(std::time::Instant::now());
                    self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
                }
                true
            }
            KeyEvent {
                code: KeyCode::Char('o'),
                ..
            } => {
                self.composer_cursor = line_end_index(&self.composer, self.composer_cursor);
                self.composer.insert(self.composer_cursor, '\n');
                self.composer_cursor += 1;
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            KeyEvent {
                code: KeyCode::Char('O'),
                ..
            } => {
                self.composer_cursor = line_start_index(&self.composer, self.composer_cursor);
                self.composer.insert(self.composer_cursor, '\n');
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.composer_edit_revision = self.composer_edit_revision.saturating_add(1);
                self.composer_editor_mode = ComposerEditorMode::Vim(VimMode::Insert);
                self.status = "Composer mode: Vim Insert".to_string();
                true
            }
            _ => false,
        }
    }

    fn execute_vim_operator(&mut self, operator: VimOperator, key: KeyEvent) -> bool {
        match operator {
            VimOperator::Delete => self.execute_vim_delete(key),
        }
    }

    fn execute_vim_delete(&mut self, key: KeyEvent) -> bool {
        let cursor = self.composer_cursor;
        let buffer = &self.composer;
        let range = match key {
            KeyEvent {
                code: KeyCode::Char('d'),
                ..
            } => {
                let line_start = line_start_index(buffer, cursor);
                let line_end = line_end_index(buffer, cursor);
                if line_end < buffer.len() {
                    Some((line_start, line_end + 1))
                } else if line_start > 0 {
                    Some((line_start - 1, line_end))
                } else {
                    Some((0, line_end))
                }
            }
            KeyEvent {
                code: KeyCode::Char('w'),
                ..
            } => Some((cursor, next_word_start(buffer, cursor))),
            KeyEvent {
                code: KeyCode::Char('b'),
                ..
            } => {
                let target = prev_word_start(buffer, cursor);
                Some((target, cursor))
            }
            KeyEvent {
                code: KeyCode::Char('$') | KeyCode::End,
                ..
            } => Some((cursor, line_end_index(buffer, cursor))),
            KeyEvent {
                code: KeyCode::Char('0') | KeyCode::Home,
                ..
            } => Some((line_start_index(buffer, cursor), cursor)),
            _ => None,
        };

        if let Some((start, end)) = range
            && start < end
            && end <= self.composer.len()
        {
            self.composer.drain(start..end);
            self.composer_cursor = start.min(self.composer.len());
            self.composer_dirty = true;
            self.last_composer_edit = Some(std::time::Instant::now());
        }
        self.status = "Composer mode: Vim Normal".to_string();
        true
    }

    async fn submit_prompt(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            self.status = "No workspace selected".to_string();
            return;
        };
        let prompt = self.composer.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if self.has_running_process() && !self.creating_new_session {
            self.queue_prompt().await;
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let restored_message = self.composer.clone();
        let scratch_id = self.current_composer_scratch_id();
        let optimistic_scope = self.current_conversation_scope();
        let session = if self.creating_new_session {
            None
        } else {
            self.current_session().cloned()
        };
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.last_composer_edit = None;
        self.composer_queue_conflict = false;
        self.focus = Focus::Main;
        let optimistic_id = optimistic_scope.clone().map(|scope| {
            self.push_optimistic_entry(scope, prompt.clone(), executor_config.clone())
        });
        match self
            .api
            .send_prompt(workspace_id, session, prompt, executor_config)
            .await
        {
            Ok(session_id) => {
                if let Some(workspace_scope) = self.selected_workspace_id {
                    self.rekey_new_session_optimistic_entries(workspace_scope, session_id);
                }
                self.creating_new_session = false;
                self.bundle.selected_session_id = Some(session_id);
                if let Some(scratch_id) = scratch_id {
                    let _ = self.api.delete_follow_up_draft(scratch_id).await;
                }
                self.api.load_workspace(workspace_id, self.tx.clone());
                self.status = "Prompt sent".to_string();
            }
            Err(error) => {
                if let Some(local_id) = optimistic_id {
                    self.mark_optimistic_failed(local_id);
                }
                self.composer = restored_message;
                self.composer_cursor = self.composer.len();
                self.composer_dirty = true;
                self.last_composer_edit = Some(std::time::Instant::now());
                self.focus = Focus::Composer;
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    async fn flush_notes_if_needed(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        if !self.bundle.notes_dirty || self.notes_save_in_flight {
            return;
        }
        let Some(last_edit) = self.bundle.last_notes_edit else {
            return;
        };
        if last_edit.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.notes_save_in_flight = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        let notes = self.bundle.notes.clone();
        let revision = self.notes_edit_revision;
        tokio::spawn(async move {
            match api.save_notes(workspace_id, notes).await {
                Ok(()) => {
                    let _ = tx.send(NetEvent::NotesSaved {
                        workspace_id,
                        revision,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::NotesSaveFailed {
                        workspace_id,
                        revision,
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    async fn flush_draft_if_needed(&mut self) {
        let Some(scratch_id) = self.current_composer_scratch_id() else {
            self.composer_dirty = false;
            self.draft_save_in_flight = false;
            self.last_composer_edit = None;
            self.composer_scratch_loaded = true;
            return;
        };
        if !self.composer_dirty || self.draft_save_in_flight {
            return;
        }
        let Some(last_edit) = self.last_composer_edit else {
            return;
        };
        if last_edit.elapsed() < Duration::from_millis(500) {
            return;
        }
        if self.is_queue_present() {
            self.composer_queue_conflict = true;
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            return;
        };
        self.draft_save_in_flight = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        let draft = DraftFollowUpData {
            message: self.composer.clone(),
            executor_config,
        };
        let revision = self.composer_edit_revision;
        tokio::spawn(async move {
            match api.save_follow_up_draft(scratch_id, draft).await {
                Ok(()) => {
                    let _ = tx.send(NetEvent::DraftSaved {
                        scratch_id,
                        revision,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::DraftSaveFailed {
                        scratch_id,
                        revision,
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    fn handle_terminal_resize(&mut self, size: Rect) {
        let cols = size.width.max(1);
        let rows = size.height.max(1);
        if self.bundle.terminal.size != (cols, rows) {
            self.bundle.terminal.size = (cols, rows);
            self.bundle.terminal.parser.set_size(rows, cols);
            if let Some(tx) = &self.subscriptions.terminal_tx {
                let _ = tx.send(TerminalCommand::Resize(cols, rows));
            }
        }
    }

    async fn handle_enter(&mut self, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                self.ensure_workspace_selected(size);
            }
            Focus::Main => {
                if self.selected_pane == Pane::Terminal {
                    self.bundle.terminal.input_mode = true;
                    self.status = "Terminal input mode enabled".to_string();
                }
            }
            Focus::Detail => self.switch_session_or_process(size),
            Focus::Composer => {}
        }
    }

    fn move_selection(&mut self, delta: i32, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                let ids = self.visible_workspace_ids();
                if ids.is_empty() {
                    return;
                }
                let current_index = self
                    .selected_workspace_id
                    .and_then(|id| ids.iter().position(|candidate| *candidate == id))
                    .unwrap_or(0) as i32;
                let next_index =
                    (current_index + delta).clamp(0, ids.len().saturating_sub(1) as i32) as usize;
                self.selected_workspace_id = Some(ids[next_index]);
                self.load_selected_workspace(size);
            }
            Focus::Detail => match self.selected_pane {
                Pane::Changes => {
                    if self.bundle.diffs.is_empty() {
                        return;
                    }
                    let next = (self.bundle.selected_diff_index as i32 + delta)
                        .clamp(0, self.bundle.diffs.len().saturating_sub(1) as i32)
                        as usize;
                    self.bundle.selected_diff_index = next;
                }
                Pane::Chat | Pane::Logs => {
                    let rows = self.session_rows();
                    if rows.is_empty() {
                        return;
                    }
                    let current = self.selected_session_row_index(&rows).unwrap_or(0) as i32;
                    let next =
                        (current + delta).clamp(0, rows.len().saturating_sub(1) as i32) as usize;
                    if let Some(target) = session_target(&rows[next]) {
                        self.select_session_target(target);
                    }
                }
                Pane::Git => {
                    let scroll = self.bundle.log_scroll as i32 + delta;
                    self.bundle.log_scroll = scroll.clamp(0, u16::MAX as i32) as u16;
                }
                _ => {}
            },
            Focus::Main | Focus::Composer => match self.selected_pane {
                Pane::Chat => self.adjust_chat_scroll(delta),
                Pane::Logs => {
                    let scroll = self.bundle.log_scroll as i32 + delta;
                    self.bundle.log_scroll = scroll.clamp(0, u16::MAX as i32) as u16;
                }
                _ => {}
            },
        }
    }

    fn page_step(&self, size: Rect) -> i32 {
        match self.focus {
            Focus::WorkspaceList => ((size.height.saturating_sub(4) / 3).max(1)) as i32,
            Focus::Detail => 5,
            Focus::Main | Focus::Composer => size.height.saturating_sub(6).max(1) as i32,
        }
    }

    fn jump_to_boundary(&mut self, to_end: bool, size: Rect) {
        match self.focus {
            Focus::WorkspaceList => {
                let ids = self.visible_workspace_ids();
                if ids.is_empty() {
                    return;
                }
                self.selected_workspace_id = Some(if to_end {
                    *ids.last().unwrap_or(&ids[0])
                } else {
                    ids[0]
                });
                self.load_selected_workspace(size);
            }
            Focus::Detail => match self.selected_pane {
                Pane::Changes => {
                    if self.bundle.diffs.is_empty() {
                        return;
                    }
                    self.bundle.selected_diff_index = if to_end {
                        self.bundle.diffs.len().saturating_sub(1)
                    } else {
                        0
                    };
                }
                Pane::Chat | Pane::Logs => {
                    let rows = self.session_rows();
                    if rows.is_empty() {
                        return;
                    }
                    let target = if to_end {
                        rows.last().unwrap_or(&rows[0])
                    } else {
                        &rows[0]
                    };
                    if let Some(target) = session_target(target) {
                        self.select_session_target(target);
                    }
                }
                Pane::Git => {
                    self.bundle.log_scroll = if to_end {
                        self.max_scroll_for_selected_pane()
                    } else {
                        0
                    };
                }
                _ => {}
            },
            Focus::Main | Focus::Composer => match self.selected_pane {
                Pane::Chat => self.chat_end_offset = if to_end { 0 } else { u16::MAX },
                Pane::Logs | Pane::Git => {
                    self.bundle.log_scroll = if to_end {
                        self.max_scroll_for_selected_pane()
                    } else {
                        0
                    };
                }
                _ => {}
            },
        }
    }

    fn max_scroll_for_selected_pane(&self) -> u16 {
        let lines = match self.selected_pane {
            Pane::Chat => self.chat_line_count(),
            Pane::Logs => self
                .bundle
                .log_entries
                .iter()
                .enumerate()
                .map(|(index, entry)| render_log_entry(index, entry).len())
                .sum::<usize>(),
            Pane::Git => self
                .bundle
                .git_status
                .iter()
                .map(|status| 3 + usize::from(status.status.is_rebase_in_progress))
                .sum::<usize>(),
            _ => 0,
        };
        lines.saturating_sub(1).min(u16::MAX as usize) as u16
    }

    fn terminal_stream_size(&self, size: Rect) -> (u16, u16) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(size);
        let body = if size.width >= 140 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(32),
                    Constraint::Min(50),
                    Constraint::Length(44),
                ])
                .split(outer[1])[1]
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(32), Constraint::Min(40)])
                .split(outer[1])[1]
        };
        let main = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(5),
            ])
            .split(body)[1];
        let content = terminal_content_area(main);
        (content.width.max(1), content.height.max(1))
    }

    fn adjust_chat_scroll(&mut self, delta: i32) {
        let offset = self.chat_end_offset as i32 - delta;
        self.chat_end_offset = offset.clamp(0, u16::MAX as i32) as u16;
    }

    fn render(&mut self, frame: &mut Frame) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(frame.area());

        frame.render_widget(self.header(), outer[0]);

        if frame.area().width >= 140 {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(32),
                    Constraint::Min(50),
                    Constraint::Length(44),
                ])
                .split(outer[1]);
            self.render_workspace_list(frame, body[0]);
            self.render_main(frame, body[1]);
            self.render_detail(frame, body[2]);
        } else {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(32), Constraint::Min(40)])
                .split(outer[1]);
            self.render_workspace_list(frame, body[0]);
            self.render_main(frame, body[1]);
        }

        frame.render_widget(self.footer(), outer[2]);

        if self.agent_picker.is_some() {
            self.render_agent_picker(frame, frame.area());
        }
        if self.session_rename.is_some() {
            self.render_session_rename(frame, frame.area());
        }
    }

    fn render_workspace_list(&self, frame: &mut Frame, area: Rect) {
        let rows = self.workspace_rows();
        let items = rows
            .iter()
            .map(|row| match row {
                WorkspaceRow::Header(title) => ListItem::new(Line::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                WorkspaceRow::Workspace(workspace) => {
                    let summary = self.summaries.get(&workspace.id);
                    let mut line = workspace_title(&workspace.workspace);
                    if workspace.workspace.pinned {
                        line.push_str("  [pin]");
                    }
                    if workspace.is_running {
                        line.push_str("  [run]");
                    }
                    if summary.is_some_and(|summary| summary.has_pending_approval) {
                        line.push_str("  [approval]");
                    }
                    if summary.is_some_and(|summary| summary.has_running_dev_server) {
                        line.push_str("  [dev]");
                    }
                    if summary.is_some_and(|summary| summary.has_unseen_turns) {
                        line.push_str("  [new]");
                    }
                    let meta = if let Some(summary) = summary {
                        format!(
                            "{}  +{} -{}  {}",
                            workspace.workspace.branch,
                            summary.lines_added.unwrap_or_default(),
                            summary.lines_removed.unwrap_or_default(),
                            format_relative_time(summary.latest_process_completed_at)
                        )
                    } else {
                        workspace.workspace.branch.clone()
                    };
                    let status_color = if summary.is_some_and(|summary| {
                        summary.has_pending_approval || summary.has_unseen_turns
                    }) {
                        Color::Yellow
                    } else if workspace.is_running
                        || summary.is_some_and(|summary| summary.has_running_dev_server)
                    {
                        Color::Green
                    } else {
                        Color::White
                    };
                    ListItem::new(Text::from(vec![
                        Line::styled(format!(" {line}"), Style::default().fg(status_color)),
                        Line::styled(format!(" {meta}"), Style::default().fg(Color::DarkGray)),
                        Line::raw(""),
                    ]))
                }
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default();
        if let Some(index) = self.selected_workspace_row_index(&rows) {
            state.select(Some(index));
        }

        let block = panel_block("Workspaces", self.focus == Focus::WorkspaceList);
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::Rgb(28, 38, 48))
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, area, &mut state);
        render_vertical_scrollbar(
            frame,
            area,
            rows.len(),
            viewport_capacity(area, 3),
            selected_list_offset(
                self.selected_workspace_row_index(&rows).unwrap_or(0),
                rows.len(),
                viewport_capacity(area, 3),
            ),
        );
    }

    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = if self.selected_pane == Pane::Chat {
            let composer_height = self.chat_composer_height(area.width);
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(8),
                    Constraint::Length(4),
                    Constraint::Length(composer_height),
                ])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(5),
                ])
                .split(area)
        };

        let tabs = Tabs::new(
            Pane::all()
                .iter()
                .map(|pane| Line::from(Span::raw(pane.title())))
                .collect::<Vec<_>>(),
        )
        .block(panel_block("Pane", self.focus == Focus::Main))
        .select(
            Pane::all()
                .iter()
                .position(|pane| pane == &self.selected_pane)
                .unwrap_or(0),
        )
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(tabs, chunks[0]);

        match self.selected_pane {
            Pane::Chat => self.render_chat(frame, chunks[1]),
            Pane::Changes => self.render_changes(frame, chunks[1]),
            Pane::Logs => self.render_logs(frame, chunks[1]),
            Pane::Git => self.render_git(frame, chunks[1]),
            Pane::Terminal => self.render_terminal(frame, chunks[1]),
            Pane::Notes => self.render_notes(frame, chunks[1]),
        }

        let composer_title = match self.selected_pane {
            Pane::Notes => "Notes Editor",
            _ => "Composer",
        };
        if self.selected_pane == Pane::Chat {
            frame.render_widget(
                Paragraph::new(Text::from(vec![
                    self.composer_selection_line(),
                    self.composer_status_line(),
                ]))
                .block(panel_block("Selection", false))
                .wrap(Wrap { trim: false }),
                chunks[2],
            );
            frame.render_widget(
                Paragraph::new(self.render_composer_text())
                    .block(panel_block(composer_title, self.focus == Focus::Composer))
                    .wrap(Wrap { trim: false }),
                chunks[3],
            );
        } else {
            frame.render_widget(
                Paragraph::new(self.bundle.notes.as_str())
                    .block(panel_block(composer_title, self.focus == Focus::Composer))
                    .wrap(Wrap { trim: false }),
                chunks[2],
            );
        }
    }

    fn chat_composer_height(&self, area_width: u16) -> u16 {
        let inner_width = area_width.saturating_sub(2).max(12) as usize;
        let wrapped_lines = if self.composer.is_empty() {
            1
        } else {
            self.composer
                .split('\n')
                .map(|line| line.chars().count().max(1).div_ceil(inner_width))
                .sum::<usize>()
                .max(1)
        };
        wrapped_lines.saturating_add(2).clamp(7, 16) as u16
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Min(7),
            ])
            .split(area);

        let workspace_info = if let Some(workspace) = &self.bundle.workspace {
            vec![
                Line::raw(workspace_title(workspace)),
                Line::raw(format!("branch: {}", workspace.branch)),
                Line::raw(format!("archived: {}", workspace.archived)),
                Line::raw(format!("pinned: {}", workspace.pinned)),
                Line::styled(
                    format!("draft: {}", self.draft_status_label()),
                    Style::default().fg(Color::LightBlue),
                ),
                Line::styled(
                    format!("queue: {}", self.queue_status_label()),
                    Style::default().fg(Color::Yellow),
                ),
                Line::styled(
                    "Enter send/queue  Q queue/replace  X cancel queue  D discard draft"
                        .to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Line::styled(
                    "v stop execution  s dev server  c cleanup  e editor  r rename session"
                        .to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Line::raw(format!(
                    "updated: {}",
                    format_relative_time(Some(workspace.updated_at))
                )),
            ]
        } else {
            vec![Line::raw("No workspace selected")]
        };
        frame.render_widget(
            Paragraph::new(Text::from(workspace_info)).block(panel_block("Workspace", false)),
            chunks[0],
        );

        let session_rows = self.session_rows();
        let sessions = session_rows
            .iter()
            .map(|row| match row {
                SessionRow::NewSession => ListItem::new(Text::from(vec![
                    Line::styled(
                        "+ New session",
                        Style::default()
                            .fg(Color::LightGreen)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        "Start a fresh thread in this workspace",
                        Style::default().fg(Color::DarkGray),
                    ),
                ])),
                SessionRow::Session(session) => {
                    let name = session
                        .name
                        .clone()
                        .unwrap_or_else(|| session.id.to_string());
                    let executor = session
                        .executor
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    ListItem::new(Text::from(vec![
                        Line::raw(name),
                        Line::styled(executor, Style::default().fg(Color::DarkGray)),
                    ]))
                }
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if let Some(index) = self.selected_session_row_index(&session_rows) {
            state.select(Some(index));
        }
        frame.render_stateful_widget(
            List::new(sessions)
                .block(panel_block("Sessions", self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            chunks[1],
            &mut state,
        );
        render_vertical_scrollbar(
            frame,
            chunks[1],
            session_rows.len(),
            viewport_capacity(chunks[1], 2),
            selected_list_offset(
                self.selected_session_row_index(&session_rows).unwrap_or(0),
                session_rows.len(),
                viewport_capacity(chunks[1], 2),
            ),
        );

        let mut processes = self
            .bundle
            .process_map
            .values()
            .cloned()
            .collect::<Vec<ExecutionProcess>>();
        processes.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        let process_lines = processes
            .iter()
            .map(|process| {
                let label = format!(
                    "{}  {:?}",
                    process.created_at.format("%H:%M:%S"),
                    process.status
                );
                ListItem::new(Text::from(vec![
                    Line::raw(label),
                    Line::styled(
                        format!("{:?}", process.run_reason),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(process_lines).block(panel_block("Processes", false)),
            chunks[2],
        );
    }

    fn render_chat(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.creating_new_session {
            "Conversation (new session)"
        } else {
            "Conversation"
        };
        let block = panel_block(title, self.focus == Focus::Main);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let (messages_area, status_area) = if inner.height > 1 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);
            (chunks[0], Some(chunks[1]))
        } else {
            (inner, None)
        };
        let padded_messages_area = messages_area.inner(ratatui::layout::Margin {
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
        let (total_lines, latest_token_usage, clamped_end_offset, start, end) = {
            let cache = self.chat_render_cache(content_area.width.max(1) as usize);
            let (clamped_end_offset, start, end, _) =
                chat_window_bounds(cache.lines.len(), visible_lines, requested_end_offset);
            (
                cache.lines.len(),
                cache.latest_token_usage,
                clamped_end_offset,
                start,
                end,
            )
        };
        if clamped_end_offset != self.chat_end_offset as usize {
            self.chat_end_offset = clamped_end_offset.min(u16::MAX as usize) as u16;
        }
        let lines = {
            let cache = self.chat_render_cache(content_area.width.max(1) as usize);
            cache.lines[start..end].to_vec()
        };
        frame.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            content_area,
        );
        let (_, _, _, top_offset) =
            chat_window_bounds(total_lines, visible_lines, clamped_end_offset);
        render_vertical_scrollbar(frame, area, total_lines, visible_lines, top_offset);

        if let Some(status_area) = status_area {
            self.render_chat_status(frame, status_area, latest_token_usage);
        }
    }

    fn render_chat_status(
        &self,
        frame: &mut Frame,
        area: Rect,
        latest_token_usage: Option<(u32, u32)>,
    ) {
        let Some((total_tokens, context_window)) = latest_token_usage else {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "latest context usage unavailable",
                    Style::default().fg(Color::DarkGray),
                )),
                area,
            );
            return;
        };

        let ratio = if context_window == 0 {
            0.0
        } else {
            (total_tokens as f64 / context_window as f64).clamp(0.0, 1.0)
        };
        let gauge_color = if ratio >= 0.85 {
            Color::Red
        } else if ratio >= 0.65 {
            Color::Yellow
        } else {
            Color::Green
        };

        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(format!("context {total_tokens}/{context_window}"))
                .gauge_style(Style::default().fg(gauge_color)),
            area,
        );
    }

    fn render_changes(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(area);

        let items = self
            .bundle
            .diffs
            .iter()
            .map(|diff| {
                let counts = format!(
                    "+{} -{}",
                    diff.additions.unwrap_or_default(),
                    diff.deletions.unwrap_or_default()
                );
                ListItem::new(Text::from(vec![
                    Line::raw(diff_title(diff)),
                    Line::styled(counts, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if !self.bundle.diffs.is_empty() {
            state.select(Some(self.bundle.selected_diff_index));
        }
        frame.render_stateful_widget(
            List::new(items)
                .block(panel_block("Files", self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            chunks[0],
            &mut state,
        );
        render_vertical_scrollbar(
            frame,
            chunks[0],
            self.bundle.diffs.len(),
            viewport_capacity(chunks[0], 2),
            selected_list_offset(
                self.bundle.selected_diff_index,
                self.bundle.diffs.len(),
                viewport_capacity(chunks[0], 2),
            ),
        );

        let diff_text = self
            .bundle
            .diffs
            .get(self.bundle.selected_diff_index)
            .map(render_diff_text)
            .unwrap_or_else(|| "No diff selected".to_string());
        frame.render_widget(
            Paragraph::new(diff_text)
                .block(panel_block("Diff", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            chunks[1],
        );
    }

    fn render_logs(&self, frame: &mut Frame, area: Rect) {
        let lines = self
            .bundle
            .log_entries
            .iter()
            .enumerate()
            .flat_map(|(index, entry)| render_log_entry(index, entry))
            .collect::<Vec<_>>();
        let total_lines = lines.len();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block("Logs", self.focus == Focus::Main))
                .scroll((self.bundle.log_scroll, 0))
                .wrap(Wrap { trim: false }),
            area,
        );
        render_vertical_scrollbar(
            frame,
            area,
            total_lines,
            area.height.saturating_sub(2) as usize,
            self.bundle.log_scroll as usize,
        );
    }

    fn render_git(&self, frame: &mut Frame, area: Rect) {
        let lines = if self.bundle.git_status.is_empty() {
            vec![Line::raw("No repository status available")]
        } else {
            self.bundle
                .git_status
                .iter()
                .flat_map(|status| {
                    let mut lines = vec![
                        Line::styled(
                            format!(
                                "{} -> {}",
                                status.repo_name, status.status.target_branch_name
                            ),
                            Style::default().fg(Color::Cyan),
                        ),
                        Line::raw(format!(
                            "ahead {}  behind {}  uncommitted {:?}  untracked {:?}",
                            status.status.commits_ahead.unwrap_or_default(),
                            status.status.commits_behind.unwrap_or_default(),
                            status.status.uncommitted_count,
                            status.status.untracked_count
                        )),
                    ];
                    if status.status.is_rebase_in_progress {
                        lines.push(Line::styled(
                            "rebase in progress",
                            Style::default().fg(Color::Yellow),
                        ));
                    }
                    if !status.status.conflicted_files.is_empty() {
                        lines.push(Line::styled(
                            format!("conflicts: {}", status.status.conflicted_files.join(", ")),
                            Style::default().fg(Color::Red),
                        ));
                    }
                    if let Some(pr) = status.status.merges.iter().find_map(|merge| match merge {
                        crate::model::Merge::Pr(pr) => Some(pr),
                        _ => None,
                    }) {
                        lines.push(Line::raw(format!(
                            "pr #{}  {}",
                            pr.pr_info.pr_number, pr.pr_info.pr_url
                        )));
                    }
                    lines.push(Line::raw(""));
                    lines
                })
                .collect()
        };
        let total_lines = lines.len();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block("Git", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            area,
        );
        render_vertical_scrollbar(
            frame,
            area,
            total_lines,
            area.height.saturating_sub(2) as usize,
            self.bundle.log_scroll as usize,
        );
    }

    fn render_terminal(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.bundle.terminal.input_mode {
            "Terminal *"
        } else {
            "Terminal"
        };
        let block = panel_block(title, self.focus == Focus::Main);
        let content_area = terminal_content_area(area);
        self.handle_terminal_resize(content_area);
        let screen = self.bundle.terminal.parser.screen();
        let mut lines = Vec::new();
        for row in 0..screen.size().0 {
            let mut text = String::new();
            for col in 0..screen.size().1 {
                if let Some(cell) = screen.cell(row + 1, col + 1) {
                    text.push(cell.contents().chars().next().unwrap_or(' '));
                }
            }
            lines.push(Line::raw(text.trim_end_matches(' ').to_string()));
        }
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(Text::from(lines)), content_area);
        if let Some(error) = &self.bundle.terminal.error {
            let popup = centered_rect(70, 20, area);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("Terminal error"),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
    }

    fn render_notes(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.bundle.notes.as_str())
                .block(panel_block("Notes", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn header(&self) -> Paragraph<'_> {
        let workspace = self
            .selected_workspace_id
            .and_then(|id| self.find_workspace(id))
            .map(|workspace| workspace_title(&workspace.workspace))
            .unwrap_or_else(|| "No workspace".to_string());
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Vibe Kanban TUI",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::raw(workspace),
            Span::raw("  "),
            Span::styled(self.selected_pane.title(), Style::default().fg(Color::Gray)),
        ]))
    }

    fn footer(&self) -> Paragraph<'_> {
        let status = self.error.as_deref().unwrap_or(&self.status);
        Paragraph::new(status.to_string()).block(Block::default().borders(Borders::TOP))
    }

    fn render_agent_picker(&self, frame: &mut Frame, area: Rect) {
        let Some(picker) = self.agent_picker.as_ref() else {
            return;
        };
        let popup = centered_rect(72, 55, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(6)])
            .split(popup);
        let options = self.filtered_agent_mode_options();
        let selected = picker.selected.min(options.len().saturating_sub(1));
        let items = options
            .iter()
            .map(|option| match option {
                None => ListItem::new(vec![
                    Line::styled(
                        "Default",
                        Style::default()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        "Use the executor default agent mode",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Some(agent) => {
                    let mut lines = vec![Line::from(vec![
                        Span::styled(
                            agent.label.clone(),
                            Style::default()
                                .fg(Color::LightBlue)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(agent.id.clone(), Style::default().fg(Color::Gray)),
                        if agent.is_default {
                            Span::styled("  default", Style::default().fg(Color::Yellow))
                        } else {
                            Span::raw("")
                        },
                    ])];
                    if let Some(description) = &agent.description {
                        lines.push(Line::styled(
                            description.clone(),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    ListItem::new(lines)
                }
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(selected));

        frame.render_widget(Clear, popup);
        frame.render_widget(panel_block("Agent Mode", true), popup);
        frame.render_widget(
            Paragraph::new(format!("Search: {}", picker.query))
                .block(panel_block("Filter", false))
                .wrap(Wrap { trim: false }),
            chunks[0],
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(panel_block("Options", false))
                .highlight_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> "),
            chunks[1],
            &mut state,
        );
    }

    fn render_session_rename(&self, frame: &mut Frame, area: Rect) {
        let Some(rename) = self.session_rename.as_ref() else {
            return;
        };
        let popup = centered_rect(64, 24, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(popup);

        frame.render_widget(Clear, popup);
        frame.render_widget(panel_block("Rename Session", true), popup);
        frame.render_widget(
            Paragraph::new(render_editor_buffer(
                &rename.name,
                rename.cursor,
                true,
                ComposerEditorMode::Standard,
            ))
            .block(panel_block("Name", false))
            .wrap(Wrap { trim: false }),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new("Enter save  Esc cancel")
                .block(panel_block("Hints", false))
                .wrap(Wrap { trim: false }),
            chunks[1],
        );
        frame.render_widget(
            Paragraph::new("Rename the selected session in this workspace."),
            chunks[2],
        );
    }

    fn composer_selection_line(&self) -> Line<'static> {
        let config = self.composer_config.as_ref();
        let executor = config
            .map(|config| config.executor.to_string())
            .unwrap_or_else(|| "loading".to_string());
        let variant = config
            .map(|config| display_variant(config.variant.as_deref()).to_string())
            .unwrap_or_else(|| "loading".to_string());
        let model = self
            .selected_model_label()
            .unwrap_or_else(|| "default".to_string());
        let reasoning = self
            .selected_reasoning_label()
            .unwrap_or_else(|| "default".to_string());
        let agent_mode = self.selected_agent_mode_label();
        let permission = config
            .map(|config| display_permission(config.permission_policy.as_ref()).to_string())
            .unwrap_or_else(|| "default".to_string());
        let shortcut_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);
        let label_style = Style::default().fg(Color::DarkGray);

        Line::from(vec![
            Span::styled("E", shortcut_style),
            Span::styled("xec ", label_style),
            Span::styled(executor, Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled("V", shortcut_style),
            Span::styled("ar ", label_style),
            Span::styled(variant, Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled("M", shortcut_style),
            Span::styled("odel ", label_style),
            Span::styled(model, Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled("R", shortcut_style),
            Span::styled("sn ", label_style),
            Span::styled(reasoning, Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::styled("A", shortcut_style),
            Span::styled("gt ", label_style),
            Span::styled(agent_mode, Style::default().fg(Color::LightBlue)),
            Span::raw("  "),
            Span::styled("P", shortcut_style),
            Span::styled("erm ", label_style),
            Span::styled(permission, Style::default().fg(Color::LightRed)),
        ])
    }

    fn composer_status_line(&self) -> Line<'static> {
        let draft = self.draft_status_label();
        let queue = self.queue_status_label();
        let mode = self.composer_editor_mode_label();
        let draft_style = if self.composer_queue_conflict {
            Style::default().fg(Color::Yellow)
        } else if self.composer_dirty {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let queue_style = if self.queue_pending {
            Style::default().fg(Color::Yellow)
        } else if self.is_queue_present() {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![
            Span::styled("Mode ", Style::default().fg(Color::Gray)),
            Span::styled(mode, Style::default().fg(Color::LightMagenta)),
            Span::raw("  "),
            Span::styled("Draft ", Style::default().fg(Color::Gray)),
            Span::styled(draft, draft_style),
            Span::raw("  "),
            Span::styled("Queue ", Style::default().fg(Color::Gray)),
            Span::styled(queue, queue_style),
            Span::raw("  "),
            Span::styled(
                "F2",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" editor mode ", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(
                "Q",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" queue ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "X",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "D",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" discard", Style::default().fg(Color::DarkGray)),
        ];
        if self.creating_new_session {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " create session ",
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " cancel",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }

    fn composer_editor_mode_label(&self) -> &'static str {
        match self.composer_editor_mode {
            ComposerEditorMode::Standard => "standard",
            ComposerEditorMode::Vim(VimMode::Insert) => "vim insert",
            ComposerEditorMode::Vim(VimMode::Normal) => "vim normal",
        }
    }

    fn toggle_composer_editor_mode(&mut self) {
        self.composer_editor_mode = match self.composer_editor_mode {
            ComposerEditorMode::Standard => ComposerEditorMode::Vim(VimMode::Insert),
            ComposerEditorMode::Vim(_) => ComposerEditorMode::Standard,
        };
        self.status = format!("Composer mode: {}", self.composer_editor_mode_label());
    }

    fn render_composer_text(&self) -> Text<'static> {
        let show_cursor = self.focus == Focus::Composer && self.selected_pane == Pane::Chat;
        render_editor_buffer(
            &self.composer,
            self.composer_cursor.min(self.composer.len()),
            show_cursor,
            self.composer_editor_mode,
        )
    }

    fn draft_status_label(&self) -> String {
        if self.composer_queue_conflict {
            "blocked by queued follow-up".to_string()
        } else if self.composer_dirty {
            "saving...".to_string()
        } else if self.composer_scratch_loaded {
            "synced".to_string()
        } else {
            "loading".to_string()
        }
    }

    fn queue_status_label(&self) -> String {
        if self.queue_pending {
            return "loading".to_string();
        }
        match &self.queue_status {
            QueueStatus::Empty => "not queued".to_string(),
            QueueStatus::Queued { message } => {
                format!("queued at {}", message.queued_at.format("%H:%M:%S"))
            }
        }
    }

    fn current_discovery_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    fn current_composer_scratch_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            self.selected_workspace_id
        } else {
            self.bundle.selected_session_id
        }
    }

    fn current_queue_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    fn sync_composer_context(&mut self) {
        let current_scope = self.current_conversation_scope();
        let optimistic_before = self.optimistic_entries.len();
        self.optimistic_entries
            .retain(|entry| Some(entry.scope.clone()) == current_scope);
        if self.optimistic_entries.len() != optimistic_before {
            self.mark_chat_render_cache_dirty();
        }

        let scratch_id = self.current_composer_scratch_id();
        if scratch_id != self.composer_scratch_id {
            self.composer_scratch_id = scratch_id;
            self.composer_scratch_loaded = scratch_id.is_none();
            self.composer.clear();
            self.composer_cursor = 0;
            self.composer_dirty = false;
            self.draft_save_in_flight = false;
            self.last_composer_edit = None;
            self.composer_queue_conflict = false;
            self.api
                .replace_draft_stream(scratch_id, self.tx.clone(), &mut self.subscriptions);
        }

        let queue_session_id = self.current_queue_session_id();
        if queue_session_id != self.queue_session_id {
            self.queue_session_id = queue_session_id;
            self.queue_status = QueueStatus::Empty;
            self.queue_pending = false;
        }
        self.refresh_queue_status();
    }

    fn refresh_queue_status(&mut self) {
        if let Some(session_id) = self.current_queue_session_id() {
            self.queue_pending = true;
            self.api.load_queue_status(session_id, self.tx.clone());
        } else {
            let had_queue = !matches!(self.queue_status, QueueStatus::Empty);
            self.queue_status = QueueStatus::Empty;
            self.queue_pending = false;
            if had_queue {
                self.mark_chat_render_cache_dirty();
            }
        }
    }

    fn has_running_process(&self) -> bool {
        self.bundle
            .process_map
            .values()
            .any(|process| process.status == ExecutionProcessStatus::Running)
    }

    fn is_queue_present(&self) -> bool {
        matches!(self.queue_status, QueueStatus::Queued { .. })
    }

    fn current_conversation_scope(&self) -> Option<ConversationScope> {
        if self.creating_new_session {
            self.selected_workspace_id
                .map(ConversationScope::NewSession)
        } else {
            self.bundle
                .selected_session_id
                .map(ConversationScope::Session)
        }
    }

    fn push_optimistic_entry(
        &mut self,
        scope: ConversationScope,
        message: String,
        executor_config: ExecutorConfig,
    ) -> Uuid {
        let local_id = Uuid::new_v4();
        self.optimistic_entries.push(OptimisticConversationEntry {
            local_id,
            scope,
            message,
            executor_config,
            state: OptimisticState::Pending,
        });
        self.mark_chat_render_cache_dirty();
        local_id
    }

    fn mark_optimistic_failed(&mut self, local_id: Uuid) {
        if let Some(entry) = self
            .optimistic_entries
            .iter_mut()
            .find(|entry| entry.local_id == local_id)
        {
            entry.state = OptimisticState::Failed;
            self.mark_chat_render_cache_dirty();
        }
    }

    fn rekey_new_session_optimistic_entries(&mut self, workspace_id: Uuid, session_id: Uuid) {
        for entry in &mut self.optimistic_entries {
            if entry.scope == ConversationScope::NewSession(workspace_id) {
                entry.scope = ConversationScope::Session(session_id);
            }
        }
        self.mark_chat_render_cache_dirty();
    }

    fn reset_conversation_state(&mut self) {
        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        self.conversation_process_entries.clear();
        self.conversation_process_order.clear();
        self.conversation_bootstrapping = false;
        self.conversation_backfilling = false;
        self.optimistic_entries.clear();
        self.reset_chat_render_cache();
    }

    fn refresh_conversation_history(&mut self) {
        let Some(session_id) = self.bundle.selected_session_id else {
            self.conversation_process_entries.clear();
            self.conversation_process_order.clear();
            self.conversation_bootstrapping = false;
            self.conversation_backfilling = false;
            return;
        };
        let mut process_order = self
            .bundle
            .process_map
            .values()
            .filter(|process| {
                !process.dropped && process.run_reason != ExecutionProcessRunReason::DevServer
            })
            .map(|process| process.id)
            .collect::<Vec<_>>();
        process_order.sort_by_key(|process_id| {
            self.bundle
                .process_map
                .get(process_id)
                .map(|process| process.created_at)
        });
        if process_order == self.conversation_process_order
            && process_order
                .iter()
                .all(|process_id| self.conversation_process_entries.contains_key(process_id))
        {
            return;
        }

        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        self.conversation_process_order = process_order.clone();
        self.conversation_process_entries
            .retain(|process_id, _| process_order.contains(process_id));
        self.conversation_bootstrapping = !process_order.is_empty();
        self.conversation_backfilling = false;

        if process_order.is_empty() {
            return;
        }

        let recent_ids = initial_conversation_process_ids(&process_order, &self.bundle.process_map);
        let remaining_ids = process_order
            .iter()
            .copied()
            .filter(|process_id| !recent_ids.contains(process_id))
            .collect::<Vec<_>>();
        let api = self.api.clone();
        let tx = self.tx.clone();
        self.conversation_loader = Some(tokio::spawn(async move {
            for process_id in recent_ids.iter().rev() {
                match api.fetch_process_log_snapshot(*process_id).await {
                    Ok(entries) => {
                        let _ = tx.send(NetEvent::ConversationHistoryLoaded {
                            session_id,
                            process_id: *process_id,
                            entries,
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(NetEvent::Error(error.to_string()));
                    }
                }
            }
            let _ = tx.send(NetEvent::ConversationBootstrapComplete { session_id });
            for process_id in remaining_ids.into_iter().rev() {
                match api.fetch_process_log_snapshot(process_id).await {
                    Ok(entries) => {
                        let _ = tx.send(NetEvent::ConversationHistoryLoaded {
                            session_id,
                            process_id,
                            entries,
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(NetEvent::Error(error.to_string()));
                    }
                }
            }
            let _ = tx.send(NetEvent::ConversationBackfillComplete { session_id });
        }));
    }

    fn reconcile_optimistic_entries(&mut self) {
        let Some(scope) = self.current_conversation_scope() else {
            self.optimistic_entries.clear();
            self.mark_chat_render_cache_dirty();
            return;
        };
        let before = self.optimistic_entries.len();
        let canonical_messages = self
            .canonical_chat_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                PatchType::NormalizedEntry(entry)
                    if matches!(
                        entry.entry_type,
                        executors::logs::NormalizedEntryType::UserMessage
                    ) =>
                {
                    Some(entry.content.trim().to_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.optimistic_entries.retain(|entry| {
            entry.scope != scope
                || entry.state == OptimisticState::Failed
                || !canonical_messages
                    .iter()
                    .any(|message| message == entry.message.trim())
        });
        if self.optimistic_entries.len() != before {
            self.mark_chat_render_cache_dirty();
        }
    }

    fn canonical_chat_entries(&self) -> Vec<PatchType> {
        self.conversation_process_order
            .iter()
            .flat_map(|process_id| self.process_chat_entries(*process_id))
            .collect()
    }

    fn process_chat_entries(&self, process_id: Uuid) -> Vec<PatchType> {
        let mut entries = Vec::new();
        let process_entries = self
            .conversation_process_entries
            .get(&process_id)
            .cloned()
            .unwrap_or_default();

        let has_user_message = process_entries.iter().any(|entry| {
            matches!(
                entry,
                PatchType::NormalizedEntry(entry)
                    if matches!(entry.entry_type, executors::logs::NormalizedEntryType::UserMessage)
            )
        });

        if !has_user_message
            && let Some(process) = self.bundle.process_map.get(&process_id)
            && let Some(prompt) = process_prompt(process)
        {
            entries.push(PatchType::NormalizedEntry(
                executors::logs::NormalizedEntry {
                    timestamp: Some(process.created_at.to_rfc3339()),
                    entry_type: executors::logs::NormalizedEntryType::UserMessage,
                    content: prompt,
                    metadata: None,
                },
            ));
        }

        entries.extend(process_entries);
        entries
    }

    fn chat_line_count(&self) -> usize {
        self.chat_lines().len()
    }

    fn chat_render_cache(&mut self, width: usize) -> &ChatRenderCache {
        let width = width.max(1);
        let should_rebuild = self
            .chat_render_cache
            .as_ref()
            .is_none_or(|cache| cache.width != width)
            || (self.chat_render_cache_dirty
                && self
                    .last_chat_render_cache_build
                    .is_none_or(|built| built.elapsed() >= Duration::from_millis(33)));
        if should_rebuild {
            self.chat_render_cache = Some(self.build_chat_render_cache(width));
            self.chat_render_cache_dirty = false;
            self.last_chat_render_cache_build = Some(std::time::Instant::now());
        }
        self.chat_render_cache
            .as_ref()
            .expect("chat cache populated")
    }

    fn build_chat_render_cache(&self, width: usize) -> ChatRenderCache {
        let lines = wrap_lines(self.chat_lines(), width);
        let latest_token_usage = self
            .canonical_chat_entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                PatchType::NormalizedEntry(entry) => match &entry.entry_type {
                    executors::logs::NormalizedEntryType::TokenUsageInfo(info) => {
                        Some((info.total_tokens, info.model_context_window))
                    }
                    _ => None,
                },
                _ => None,
            });
        ChatRenderCache {
            width,
            lines,
            latest_token_usage,
        }
    }

    fn mark_chat_render_cache_dirty(&mut self) {
        self.chat_render_cache_dirty = true;
    }

    fn reset_chat_render_cache(&mut self) {
        self.chat_render_cache = None;
        self.chat_render_cache_dirty = true;
        self.last_chat_render_cache_build = None;
    }

    fn chat_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.conversation_bootstrapping && self.conversation_process_entries.is_empty() {
            lines.push(Line::styled(
                "Loading recent conversation...",
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        } else if self.conversation_backfilling {
            lines.push(Line::styled(
                "Loading older messages...",
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        }
        if let QueueStatus::Queued { message } = &self.queue_status {
            lines.push(Line::styled(
                "queued follow-up",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            for line in message.data.message.lines() {
                lines.push(Line::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::LightYellow),
                ));
            }
            lines.push(Line::styled(
                format!("  executor {}", message.data.executor_config.executor),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        }
        lines.extend(
            self.canonical_chat_entries()
                .iter()
                .flat_map(render_chat_entry)
                .collect::<Vec<_>>(),
        );
        if let Some(scope) = self.current_conversation_scope() {
            for entry in self
                .optimistic_entries
                .iter()
                .filter(|entry| entry.scope == scope)
            {
                lines.extend(render_optimistic_chat_entry(entry));
            }
        }
        lines
    }

    fn rebind_discovery_stream(&mut self) {
        let Some(config) = self.composer_config.as_ref() else {
            return;
        };
        self.api.replace_discovery_stream(
            config.executor,
            self.selected_workspace_id,
            self.current_discovery_session_id(),
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    async fn refresh_preset_config(
        &mut self,
        executor: BaseCodingAgent,
        variant: Option<String>,
        message: &str,
    ) {
        let mut path = format!("/api/agents/preset-options?executor={executor}");
        if let Some(variant) = variant.as_deref() {
            path.push_str(&format!("&variant={variant}"));
        }
        match self.api.get::<ExecutorConfig>(&path).await {
            Ok(config) => {
                self.composer_config = Some(config);
                self.composer_options = None;
                self.rebind_discovery_stream();
                self.status = message.to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    fn executor_options(&self) -> Vec<BaseCodingAgent> {
        let mut options = self
            .executor_profiles
            .executors
            .keys()
            .copied()
            .collect::<Vec<_>>();
        options.sort_by_key(|executor| executor.to_string());
        options
    }

    fn variant_options(&self, executor: BaseCodingAgent) -> Vec<String> {
        let Some(profile) = self.executor_profiles.executors.get(&executor) else {
            return vec!["DEFAULT".to_string()];
        };
        let mut variants = profile
            .configurations
            .keys()
            .filter(|key| key.as_str() != "recently_used_models")
            .cloned()
            .collect::<Vec<_>>();
        variants.sort_by(|left, right| {
            if left == "DEFAULT" {
                std::cmp::Ordering::Less
            } else if right == "DEFAULT" {
                std::cmp::Ordering::Greater
            } else {
                left.cmp(right)
            }
        });
        if variants.is_empty() {
            variants.push("DEFAULT".to_string());
        }
        variants
    }

    fn model_options(&self) -> Vec<ModelInfo> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.models.clone())
            .unwrap_or_default()
    }

    fn selected_model_value(&self) -> Option<String> {
        self.composer_config
            .as_ref()
            .and_then(|config| config.model_id.clone())
            .or_else(|| {
                self.composer_options
                    .as_ref()
                    .and_then(|options| options.model_selector.default_model.clone())
            })
    }

    fn selected_model_label(&self) -> Option<String> {
        let selected = self.selected_model_value()?;
        self.model_options()
            .into_iter()
            .find(|model| model_key(model) == selected)
            .map(|model| {
                if let Some(provider_id) = model.provider_id {
                    format!("{provider_id}/{}", model.id)
                } else {
                    model.id
                }
            })
            .or(Some(selected))
    }

    fn reasoning_options(&self) -> Vec<String> {
        let Some(selected_model) = self.selected_model_value() else {
            return Vec::new();
        };
        self.model_options()
            .into_iter()
            .find(|model| model_key(model) == selected_model)
            .map(|model| {
                model
                    .reasoning_options
                    .into_iter()
                    .map(|option| option.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    fn selected_reasoning_label(&self) -> Option<String> {
        self.composer_config
            .as_ref()
            .and_then(|config| config.reasoning_id.clone())
            .or_else(|| {
                let selected_model = self.selected_model_value()?;
                self.model_options()
                    .into_iter()
                    .find(|model| model_key(model) == selected_model)
                    .and_then(|model| {
                        model
                            .reasoning_options
                            .into_iter()
                            .find(|option| option.is_default)
                            .map(|option| option.id)
                    })
            })
    }

    fn agent_mode_options(&self) -> Vec<AgentInfo> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.agents.clone())
            .unwrap_or_default()
    }

    fn filtered_agent_mode_options(&self) -> Vec<Option<AgentInfo>> {
        let query = self
            .agent_picker
            .as_ref()
            .map(|state| state.query.trim().to_lowercase())
            .unwrap_or_default();
        let mut options = vec![None];
        let mut agents = self
            .agent_mode_options()
            .into_iter()
            .filter(|agent| {
                query.is_empty()
                    || fuzzy_contains(&query, &agent.label)
                    || fuzzy_contains(&query, &agent.id)
                    || agent
                        .description
                        .as_ref()
                        .is_some_and(|description| fuzzy_contains(&query, description))
            })
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            right
                .is_default
                .cmp(&left.is_default)
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.id.cmp(&right.id))
        });
        options.extend(agents.into_iter().map(Some));
        options
    }

    fn selected_agent_mode_index(&self, options: &[Option<AgentInfo>]) -> usize {
        let current = self
            .composer_config
            .as_ref()
            .and_then(|config| config.agent_id.clone());
        options
            .iter()
            .position(|option| match (option, current.as_ref()) {
                (None, None) => true,
                (Some(agent), Some(current)) => &agent.id == current,
                _ => false,
            })
            .unwrap_or(0)
    }

    fn open_agent_picker(&mut self) {
        let options = self.agent_mode_options();
        if options.is_empty() {
            self.status = "No agent modes available".to_string();
            return;
        }
        let mut picker = AgentPickerState {
            query: String::new(),
            selected: 0,
        };
        picker.selected = self.selected_agent_mode_index(&self.filtered_agent_mode_options());
        self.agent_picker = Some(picker);
        self.status = "Select agent mode".to_string();
        self.error = None;
    }

    fn handle_agent_picker_key(&mut self, key: KeyEvent) {
        let options_len = self.filtered_agent_mode_options().len();
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.agent_picker = None;
                self.status = "Closed agent mode picker".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.apply_agent_picker_selection(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if !picker.query.is_empty() {
                    picker.query.pop();
                    picker.selected = 0;
                }
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                if options_len > 0 {
                    picker.selected = (picker.selected + 1).min(options_len.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => {
                picker.selected = picker.selected.saturating_sub(1);
            }
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if options_len > 0 {
                    picker.selected = (picker.selected + 8).min(options_len.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => {
                picker.selected = picker.selected.saturating_sub(8);
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => picker.selected = 0,
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if options_len > 0 {
                    picker.selected = options_len.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT)
                && !modifiers.contains(KeyModifiers::SUPER) =>
            {
                picker.query.push(ch);
                picker.selected = 0;
            }
            _ => {}
        }
    }

    fn apply_agent_picker_selection(&mut self) {
        let options = self.filtered_agent_mode_options();
        let selected = self
            .agent_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0)
            .min(options.len().saturating_sub(1));
        let Some(config) = self.composer_config.as_mut() else {
            self.agent_picker = None;
            self.status = "Composer config is still loading".to_string();
            return;
        };

        match options.get(selected).cloned().flatten() {
            Some(agent) => {
                config.agent_id = Some(agent.id.clone());
                self.status = format!("Updated agent mode to {}", agent.label);
            }
            None => {
                config.agent_id = None;
                self.status = "Updated agent mode to default".to_string();
            }
        }

        self.error = None;
        self.agent_picker = None;
    }

    fn open_session_rename(&mut self) {
        if self.focus != Focus::Detail || !matches!(self.selected_pane, Pane::Chat | Pane::Logs) {
            return;
        }
        let Some(session) = self.selected_session_for_rename() else {
            self.status = "Select a real session to rename".to_string();
            return;
        };
        let name = session
            .name
            .clone()
            .unwrap_or_else(|| session.id.to_string());
        self.session_rename = Some(SessionRenameState {
            session_id: session.id,
            cursor: name.len(),
            name,
        });
        self.status = "Rename session".to_string();
        self.error = None;
    }

    async fn handle_session_rename_key(&mut self, key: KeyEvent) {
        let Some(rename) = self.session_rename.as_mut() else {
            return;
        };

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.session_rename = None;
                self.status = "Cancelled session rename".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.submit_session_rename().await,
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if rename.cursor > 0 {
                    rename.name.remove(rename.cursor - 1);
                    rename.cursor -= 1;
                }
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                if rename.cursor < rename.name.len() {
                    rename.name.remove(rename.cursor);
                }
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => rename.cursor = rename.cursor.saturating_sub(1),
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => rename.cursor = (rename.cursor + 1).min(rename.name.len()),
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => rename.cursor = 0,
            KeyEvent {
                code: KeyCode::End, ..
            } => rename.cursor = rename.name.len(),
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers,
                ..
            } if modifiers == KeyModifiers::CONTROL => rename.cursor = 0,
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers,
                ..
            } if modifiers == KeyModifiers::CONTROL => rename.cursor = rename.name.len(),
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                rename.name.insert(rename.cursor, ch);
                rename.cursor += 1;
            }
            _ => {}
        }
    }

    async fn submit_session_rename(&mut self) {
        let Some(rename) = self.session_rename.take() else {
            return;
        };
        let trimmed = rename.name.trim().to_string();
        if trimmed.is_empty() {
            self.error = Some("Session name cannot be empty".to_string());
            self.status = "Session name cannot be empty".to_string();
            self.session_rename = Some(rename);
            return;
        }

        match self
            .api
            .rename_session(rename.session_id, trimmed.clone())
            .await
        {
            Ok(updated) => {
                if let Some(session) = self
                    .bundle
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == updated.id)
                {
                    *session = updated;
                }
                self.status = format!("Renamed session to {trimmed}");
                self.error = None;
            }
            Err(error) => {
                self.session_rename = Some(SessionRenameState {
                    session_id: rename.session_id,
                    cursor: rename.cursor.min(rename.name.len()),
                    name: rename.name,
                });
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    fn selected_agent_mode_label(&self) -> String {
        let Some(selected_id) = self
            .composer_config
            .as_ref()
            .and_then(|config| config.agent_id.clone())
        else {
            return "default".to_string();
        };

        self.agent_mode_options()
            .into_iter()
            .find(|agent| agent.id == selected_id)
            .map(|agent| agent.label)
            .unwrap_or(selected_id)
    }

    fn permission_options(&self) -> Vec<PermissionPolicy> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.permissions.clone())
            .unwrap_or_default()
    }

    async fn cycle_executor(&mut self) {
        let options = self.executor_options();
        if options.is_empty() {
            self.status = "Executor profiles are still loading".to_string();
            return;
        }
        let current = self
            .composer_config
            .as_ref()
            .map(|config| config.executor)
            .or_else(|| {
                self.default_executor_profile
                    .as_ref()
                    .map(|profile| profile.executor)
            })
            .unwrap_or(options[0]);
        let index = options
            .iter()
            .position(|executor| *executor == current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()];
        let variant = self
            .variant_options(next)
            .into_iter()
            .next()
            .and_then(default_variant_to_none);
        self.refresh_preset_config(next, variant, "Updated composer executor")
            .await;
    }

    async fn cycle_variant(&mut self) {
        let Some(config) = self.composer_config.as_ref() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let options = self.variant_options(config.executor);
        if options.is_empty() {
            self.status = "No variants available".to_string();
            return;
        }
        let current = display_variant(config.variant.as_deref());
        let index = options
            .iter()
            .position(|variant| variant == current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        self.refresh_preset_config(
            config.executor,
            default_variant_to_none(next),
            "Updated composer variant",
        )
        .await;
    }

    fn cycle_model(&mut self) {
        let options = self.model_options();
        if options.is_empty() {
            self.status = "No model options available".to_string();
            return;
        }
        let keys = options.iter().map(model_key).collect::<Vec<_>>();
        let current = self
            .selected_model_value()
            .unwrap_or_else(|| keys.first().cloned().unwrap_or_default());
        let index = keys.iter().position(|key| *key == current).unwrap_or(0);
        let next = keys[(index + 1) % keys.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.model_id = Some(next.clone());
            config.reasoning_id = None;
            self.status = format!("Updated model to {next}");
            self.error = None;
        }
    }

    fn cycle_reasoning(&mut self) {
        let options = self.reasoning_options();
        if options.is_empty() {
            self.status = "No reasoning options available".to_string();
            return;
        }
        let current = self
            .selected_reasoning_label()
            .unwrap_or_else(|| options[0].clone());
        let index = options
            .iter()
            .position(|option| option == &current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.reasoning_id = Some(next.clone());
            self.status = format!("Updated reasoning to {next}");
            self.error = None;
        }
    }

    fn cycle_permission_mode(&mut self) {
        let options = self.permission_options();
        if options.is_empty() {
            self.status = "No permission modes available".to_string();
            return;
        }
        let current = self
            .composer_config
            .as_ref()
            .and_then(|config| config.permission_policy.clone())
            .unwrap_or(options[0].clone());
        let index = options
            .iter()
            .position(|option| option == &current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.permission_policy = Some(next.clone());
            self.status = format!(
                "Updated permission mode to {}",
                display_permission(Some(&next))
            );
            self.error = None;
        }
    }

    fn sync_composer_executor_with_session(&mut self) {
        if self.creating_new_session {
            return;
        }
        if self.composer_dirty || !self.composer.is_empty() || self.is_queue_present() {
            return;
        }
        let Some(session) = self.current_session() else {
            return;
        };
        let Some(executor_name) = session.executor.as_deref() else {
            return;
        };
        let Ok(executor) = BaseCodingAgent::from_str(executor_name) else {
            return;
        };
        if self
            .composer_config
            .as_ref()
            .is_some_and(|config| config.executor == executor)
        {
            self.rebind_discovery_stream();
            return;
        }
        self.composer_config = Some(ExecutorConfig::new(executor));
        self.composer_options = None;
        self.rebind_discovery_stream();
    }

    fn ensure_workspace_selected(&mut self, size: Rect) {
        if self.selected_workspace_id.is_some() {
            return;
        }
        self.selected_workspace_id = self.visible_workspace_ids().first().copied();
        self.load_selected_workspace(size);
    }

    fn load_selected_workspace(&mut self, size: Rect) {
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

    fn rebind_session_streams(&mut self) {
        self.chat_end_offset = 0;
        self.api.replace_process_stream(
            self.bundle.selected_session_id,
            self.tx.clone(),
            &mut self.subscriptions,
        );
        self.sync_composer_context();
    }

    fn rebind_logs_only(&mut self) {
        self.api.replace_logs_stream(
            self.bundle.selected_process_id,
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    fn switch_session_or_process(&mut self, size: Rect) {
        let _ = size;
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows).unwrap_or(0);
        if let Some(row) = rows.get(index) {
            if let Some(target) = session_target(row) {
                self.select_session_target(target);
            }
        }
    }

    fn session_rows(&self) -> Vec<SessionRow<'_>> {
        let mut rows = Vec::with_capacity(self.bundle.sessions.len() + 1);
        rows.push(SessionRow::NewSession);
        rows.extend(self.bundle.sessions.iter().map(SessionRow::Session));
        rows
    }

    fn selected_session_row_index(&self, rows: &[SessionRow<'_>]) -> Option<usize> {
        if self.creating_new_session {
            return Some(0);
        }
        let selected = self.bundle.selected_session_id?;
        rows.iter().position(|row| match row {
            SessionRow::NewSession => false,
            SessionRow::Session(session) => session.id == selected,
        })
    }

    fn selected_session_for_rename(&self) -> Option<&Session> {
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows)?;
        match rows.get(index)? {
            SessionRow::Session(session) => Some(session),
            SessionRow::NewSession => None,
        }
    }

    fn select_session_target(&mut self, target: SessionTarget) {
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

    fn cancel_new_session_flow(&mut self) {
        if !self.creating_new_session {
            return;
        }
        self.creating_new_session = false;
        self.sync_composer_context();
        self.status = "Cancelled new session".to_string();
        self.error = None;
    }

    fn current_session(&self) -> Option<&Session> {
        self.bundle
            .selected_session_id
            .and_then(|id| self.bundle.sessions.iter().find(|session| session.id == id))
    }

    async fn queue_prompt(&mut self) {
        let Some(session_id) = self.current_queue_session_id() else {
            self.status = "Queueing is only available for an existing session".to_string();
            return;
        };
        let prompt = self.composer.trim().to_string();
        if prompt.is_empty() {
            self.status = "Composer is empty".to_string();
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let draft = DraftFollowUpData {
            message: prompt,
            executor_config,
        };
        if let Some(scratch_id) = self.current_composer_scratch_id() {
            let _ = self
                .api
                .save_follow_up_draft(scratch_id, draft.clone())
                .await;
        }
        match self.api.queue_follow_up(session_id, draft).await {
            Ok(status) => {
                self.queue_status = status;
                self.queue_pending = false;
                self.composer.clear();
                self.composer_cursor = 0;
                self.composer_dirty = false;
                self.last_composer_edit = None;
                self.focus = Focus::Main;
                self.status = "Queued follow-up".to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    async fn cancel_queued_prompt(&mut self) {
        let Some(session_id) = self.current_queue_session_id() else {
            self.status = "No session queue to cancel".to_string();
            return;
        };
        let queued = match &self.queue_status {
            QueueStatus::Queued { message } => Some(message.data.clone()),
            QueueStatus::Empty => None,
        };
        match self.api.cancel_queued_follow_up(session_id).await {
            Ok(status) => {
                self.queue_status = status;
                self.queue_pending = false;
                if let Some(queued) = queued {
                    let executor_changed = self
                        .composer_config
                        .as_ref()
                        .map(|config| config.executor != queued.executor_config.executor)
                        .unwrap_or(true);
                    self.composer = queued.message;
                    self.composer_cursor = self.composer.len();
                    self.composer_config = Some(queued.executor_config);
                    self.composer_dirty = true;
                    self.last_composer_edit = Some(std::time::Instant::now());
                    self.composer_queue_conflict = false;
                    if executor_changed {
                        self.rebind_discovery_stream();
                    }
                }
                self.status = "Cancelled queued follow-up".to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    async fn discard_draft(&mut self) {
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.last_composer_edit = None;
        self.composer_queue_conflict = false;
        if let Some(scratch_id) = self.current_composer_scratch_id() {
            match self.api.delete_follow_up_draft(scratch_id).await {
                Ok(()) => {
                    self.status = "Discarded follow-up draft".to_string();
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    self.status = error.to_string();
                }
            }
        }
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

    fn workspace_rows(&self) -> Vec<WorkspaceRow<'_>> {
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

    fn selected_workspace_row_index(&self, rows: &[WorkspaceRow<'_>]) -> Option<usize> {
        let selected_id = self.selected_workspace_id?;
        rows.iter().position(|row| match row {
            WorkspaceRow::Header(_) => false,
            WorkspaceRow::Workspace(workspace) => workspace.id == selected_id,
        })
    }

    fn visible_workspace_ids(&self) -> Vec<Uuid> {
        self.workspace_rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspaceRow::Header(_) => None,
                WorkspaceRow::Workspace(workspace) => Some(workspace.id),
            })
            .collect()
    }

    fn find_workspace(&self, workspace_id: Uuid) -> Option<&WorkspaceWithStatus> {
        self.active_workspaces
            .get(&workspace_id)
            .or_else(|| self.archived_workspaces.get(&workspace_id))
    }

    async fn toggle_pinned(&mut self) {
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

    async fn toggle_archived(&mut self) {
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

    async fn stop_workspace(&mut self) {
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

    async fn start_dev_server(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.start_dev_server(workspace_id).await {
                Ok(()) => self.status = "Started dev server".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    async fn run_cleanup(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.run_cleanup(workspace_id).await {
                Ok(()) => self.status = "Started cleanup script".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    async fn open_editor(&mut self) {
        if let Some(workspace_id) = self.selected_workspace_id {
            match self.api.open_editor(workspace_id).await {
                Ok(()) => self.status = "Requested editor open".to_string(),
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    fn forward_terminal_key(&mut self, key: KeyEvent) {
        let Some(tx) = &self.subscriptions.terminal_tx else {
            return;
        };
        let bytes = match key.code {
            KeyCode::Enter => vec![b'\r'],
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Tab => vec![b'\t'],
            KeyCode::Esc => vec![0x1b],
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                vec![(ch as u8) & 0x1f]
            }
            KeyCode::Char(ch) => ch.to_string().into_bytes(),
            _ => return,
        };
        let _ = tx.send(TerminalCommand::Input(bytes));
    }
}

fn render_diff_text(diff: &crate::model::LocalDiff) -> String {
    let old = diff.old_content.as_deref().unwrap_or("");
    let new = diff.new_content.as_deref().unwrap_or("");
    if diff.content_omitted {
        return format!(
            "{}\ncontent omitted  +{} -{}",
            diff_title(diff),
            diff.additions.unwrap_or_default(),
            diff.deletions.unwrap_or_default()
        );
    }
    let file = diff_title(diff);
    utils::diff::create_unified_diff(&file, old, new)
}

fn default_variant_to_none(variant: String) -> Option<String> {
    if variant == "DEFAULT" {
        None
    } else {
        Some(variant)
    }
}

fn is_terminal_exit_key(key: &KeyEvent) -> bool {
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => true,
        KeyEvent {
            code: KeyCode::Char(']'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyEvent {
            code: KeyCode::Char('g'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

fn model_key(model: &ModelInfo) -> String {
    if let Some(provider_id) = &model.provider_id {
        format!("{provider_id}/{}", model.id)
    } else {
        model.id.clone()
    }
}

fn fuzzy_contains(query: &str, candidate: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let candidate = candidate.to_lowercase();
    if candidate.contains(&query) {
        return true;
    }

    let mut query_chars = query.chars();
    let mut current = query_chars.next();
    for ch in candidate.chars() {
        if current.is_some_and(|needle| needle == ch) {
            current = query_chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

fn viewport_capacity(area: Rect, rows_per_item: u16) -> usize {
    area.height
        .saturating_sub(2)
        .max(1)
        .div_ceil(rows_per_item)
        .max(1) as usize
}

fn selected_list_offset(selected: usize, total: usize, viewport: usize) -> usize {
    if total <= viewport {
        0
    } else {
        selected
            .saturating_sub(viewport.saturating_sub(1))
            .min(total.saturating_sub(viewport))
    }
}

fn session_target(row: &SessionRow<'_>) -> Option<SessionTarget> {
    Some(match row {
        SessionRow::NewSession => SessionTarget::NewSession,
        SessionRow::Session(session) => SessionTarget::Existing(session.id),
    })
}
