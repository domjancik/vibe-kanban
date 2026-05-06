use std::{collections::HashMap, str::FromStr, time::Duration};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use db::models::{
    execution_process::ExecutionProcess, session::Session, workspace::WorkspaceWithStatus,
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
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::interval,
};
use uuid::Uuid;

use crate::{
    api::{Api, TerminalCommand, WorkspaceSubscriptions},
    model::{
        Focus, NetEvent, Pane, PatchType, TerminalState, WorkspaceBundle, WorkspaceSummary,
        active_process, diff_title, display_permission, display_variant, format_patch_entry,
        format_relative_time, workspace_title,
    },
};

enum WorkspaceRow<'a> {
    Header(&'static str),
    Workspace(&'a WorkspaceWithStatus),
}

struct AgentPickerState {
    query: String,
    selected: usize,
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
    notes_cursor: usize,
    agent_picker: Option<AgentPickerState>,
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
            notes_cursor: 0,
            agent_picker: None,
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
            NetEvent::Summaries { data, .. } => {
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
                    self.bundle.process_map = processes;
                    let previous_process_id = self.bundle.selected_process_id;
                    let next_process_id = previous_process_id
                        .filter(|selected| self.bundle.process_map.contains_key(selected))
                        .or_else(|| {
                            active_process(&self.bundle.process_map).map(|process| process.id)
                        });
                    let changed = next_process_id != previous_process_id;
                    self.bundle.selected_process_id = next_process_id;
                    if changed {
                        self.bundle.log_entries.clear();
                        self.rebind_logs_only();
                    }
                    self.sync_composer_executor_with_session();
                }
            }
            NetEvent::LogsUpdated {
                process_id,
                entries,
            } => {
                if Some(process_id) == self.bundle.selected_process_id {
                    self.bundle.log_entries = entries;
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
            NetEvent::NotesSaved(workspace_id) => {
                if Some(workspace_id) == self.selected_workspace_id {
                    self.bundle.notes_dirty = false;
                    self.bundle.last_notes_edit = None;
                    self.status = "Notes saved".to_string();
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
            NetEvent::ActionOk(message) => {
                self.status = message;
                self.error = None;
            }
            NetEvent::Error(message) => {
                self.error = Some(message.clone());
                self.status = message;
            }
            NetEvent::StreamClosed(_) => {}
        }
    }

    async fn handle_key(&mut self, key: KeyEvent, size: Rect) {
        if self.agent_picker.is_some() {
            self.handle_agent_picker_key(key);
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
                    if key.code == KeyCode::Esc {
                        self.focus = Focus::Main;
                    } else {
                        self.handle_text_input(key, false).await;
                    }
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

        match key {
            KeyEvent {
                code: KeyCode::Char('q'),
                ..
            } => self.should_quit = true,
            KeyEvent {
                code: KeyCode::Tab, ..
            } => self.focus = next_focus(&self.focus),
            KeyEvent {
                code: KeyCode::BackTab,
                ..
            } => self.focus = prev_focus(&self.focus),
            KeyEvent {
                code: KeyCode::Char('?'),
                ..
            } => {
                self.status = "Keys: Tab focus, j/k nav, 1-6 panes, i edit, Enter open/send, E executor, V variant, M model, R reasoning, A agent menu, P permission, p pin, x archive, n new session, s start dev, c cleanup, e editor, Esc/C-]/C-g leave terminal".to_string();
            }
            KeyEvent {
                code: KeyCode::Char('1'),
                ..
            } => self.selected_pane = Pane::Chat,
            KeyEvent {
                code: KeyCode::Char('2'),
                ..
            } => self.selected_pane = Pane::Changes,
            KeyEvent {
                code: KeyCode::Char('3'),
                ..
            } => self.selected_pane = Pane::Logs,
            KeyEvent {
                code: KeyCode::Char('4'),
                ..
            } => self.selected_pane = Pane::Git,
            KeyEvent {
                code: KeyCode::Char('5'),
                ..
            } => self.selected_pane = Pane::Terminal,
            KeyEvent {
                code: KeyCode::Char('6'),
                ..
            } => self.selected_pane = Pane::Notes,
            KeyEvent {
                code: KeyCode::Char('a'),
                ..
            } => self.show_archived = !self.show_archived,
            KeyEvent {
                code: KeyCode::Char('i'),
                ..
            } => {
                if matches!(self.selected_pane, Pane::Chat | Pane::Notes) {
                    self.focus = Focus::Composer;
                }
            }
            KeyEvent {
                code: KeyCode::Char('n'),
                ..
            } => {
                self.creating_new_session = true;
                self.selected_pane = Pane::Chat;
                self.focus = Focus::Composer;
                self.rebind_discovery_stream();
                self.status = "New session: type a prompt and press Enter".to_string();
            }
            KeyEvent {
                code: KeyCode::Char('p'),
                ..
            } => self.toggle_pinned().await,
            KeyEvent {
                code: KeyCode::Char('x'),
                ..
            } => self.toggle_archived().await,
            KeyEvent {
                code: KeyCode::Char('s'),
                ..
            } => self.start_dev_server().await,
            KeyEvent {
                code: KeyCode::Char('c'),
                ..
            } => self.run_cleanup().await,
            KeyEvent {
                code: KeyCode::Char('v'),
                ..
            } => self.stop_workspace().await,
            KeyEvent {
                code: KeyCode::Char('e'),
                ..
            } => self.open_editor().await,
            KeyEvent {
                code: KeyCode::Char('E'),
                ..
            } => self.cycle_executor().await,
            KeyEvent {
                code: KeyCode::Char('V'),
                ..
            } => self.cycle_variant().await,
            KeyEvent {
                code: KeyCode::Char('M'),
                ..
            } => self.cycle_model(),
            KeyEvent {
                code: KeyCode::Char('R'),
                ..
            } => self.cycle_reasoning(),
            KeyEvent {
                code: KeyCode::Char('A'),
                ..
            } => self.open_agent_picker(),
            KeyEvent {
                code: KeyCode::Char('P'),
                ..
            } => self.cycle_permission_mode(),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.handle_enter(size).await,
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => self.jump_to_boundary(false, size),
            KeyEvent {
                code: KeyCode::End, ..
            } => self.jump_to_boundary(true, size),
            KeyEvent {
                code: KeyCode::Left,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SUPER) => self.jump_to_boundary(false, size),
            KeyEvent {
                code: KeyCode::Right,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SUPER) => self.jump_to_boundary(true, size),
            KeyEvent {
                code: KeyCode::Up,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SUPER) => self.jump_to_boundary(false, size),
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SUPER) => self.jump_to_boundary(true, size),
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => self.move_selection(1, size),
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => self.move_selection(-1, size),
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => self.move_selection(self.page_step(size), size),
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => self.move_selection(-self.page_step(size), size),
            KeyEvent {
                code: KeyCode::Char('t'),
                ..
            } => {
                self.selected_pane = Pane::Terminal;
                self.bundle.terminal.input_mode = true;
                self.status = "Terminal input mode enabled".to_string();
            }
            _ => {}
        }
    }

    async fn handle_text_input(&mut self, key: KeyEvent, notes: bool) {
        let (buffer, cursor) = if notes {
            (&mut self.bundle.notes, &mut self.notes_cursor)
        } else {
            (&mut self.composer, &mut self.composer_cursor)
        };
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
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if *cursor > 0 {
                    buffer.remove(*cursor - 1);
                    *cursor -= 1;
                }
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                if *cursor < buffer.len() {
                    buffer.remove(*cursor);
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
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                buffer.insert(*cursor, ch);
                *cursor += 1;
            }
            _ => {}
        }
        if notes {
            self.bundle.notes_dirty = true;
            self.bundle.last_notes_edit = Some(std::time::Instant::now());
        }
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
        let Some(executor_config) = self.composer_config.clone() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let session = if self.creating_new_session {
            None
        } else {
            self.current_session().cloned()
        };
        match self
            .api
            .send_prompt(workspace_id, session, prompt, executor_config)
            .await
        {
            Ok(session_id) => {
                self.composer.clear();
                self.composer_cursor = 0;
                self.focus = Focus::Main;
                self.creating_new_session = false;
                self.bundle.selected_session_id = Some(session_id);
                self.api.load_workspace(workspace_id, self.tx.clone());
                self.status = "Prompt sent".to_string();
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    async fn flush_notes_if_needed(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        if !self.bundle.notes_dirty {
            return;
        }
        let Some(last_edit) = self.bundle.last_notes_edit else {
            return;
        };
        if last_edit.elapsed() < Duration::from_millis(900) {
            return;
        }
        match self
            .api
            .save_notes(workspace_id, self.bundle.notes.clone())
            .await
        {
            Ok(()) => {
                self.bundle.notes_dirty = false;
                self.bundle.last_notes_edit = None;
                self.status = "Notes saved".to_string();
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    fn handle_terminal_resize(&mut self, size: Rect) {
        let cols = size.width.saturating_sub(2).max(20);
        let rows = size.height.saturating_sub(8).max(8);
        self.bundle.terminal.size = (cols, rows);
        self.bundle.terminal.parser.set_size(rows, cols);
        if let Some(tx) = &self.subscriptions.terminal_tx {
            let _ = tx.send(TerminalCommand::Resize(cols, rows));
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
                    if self.bundle.sessions.is_empty() {
                        return;
                    }
                    let current = self
                        .bundle
                        .selected_session_id
                        .and_then(|id| {
                            self.bundle
                                .sessions
                                .iter()
                                .position(|session| session.id == id)
                        })
                        .unwrap_or(0) as i32;
                    let next = (current + delta)
                        .clamp(0, self.bundle.sessions.len().saturating_sub(1) as i32)
                        as usize;
                    self.bundle.selected_session_id = Some(self.bundle.sessions[next].id);
                    self.creating_new_session = false;
                    self.rebind_session_streams();
                    self.rebind_discovery_stream();
                }
                Pane::Git => {
                    let scroll = self.bundle.log_scroll as i32 + delta;
                    self.bundle.log_scroll = scroll.clamp(0, u16::MAX as i32) as u16;
                }
                _ => {}
            },
            Focus::Main | Focus::Composer => match self.selected_pane {
                Pane::Logs | Pane::Chat => {
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
                    if self.bundle.sessions.is_empty() {
                        return;
                    }
                    self.bundle.selected_session_id = Some(
                        if to_end {
                            self.bundle.sessions.last().map(|session| session.id)
                        } else {
                            self.bundle.sessions.first().map(|session| session.id)
                        }
                        .unwrap(),
                    );
                    self.rebind_session_streams();
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
                Pane::Chat | Pane::Logs | Pane::Git => {
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
            Pane::Chat => self
                .bundle
                .log_entries
                .iter()
                .map(render_chat_entry)
                .map(|lines| lines.len())
                .sum::<usize>(),
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

    fn render(&mut self, frame: &mut Frame) {
        self.handle_terminal_resize(frame.area());

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
    }

    fn render_main(&self, frame: &mut Frame, area: Rect) {
        let chunks = if self.selected_pane == Pane::Chat {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(3),
                    Constraint::Length(5),
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
        let composer_text = match self.selected_pane {
            Pane::Notes => self.bundle.notes.as_str(),
            _ => self.composer.as_str(),
        };
        if self.selected_pane == Pane::Chat {
            frame.render_widget(
                Paragraph::new(self.composer_selection_line())
                    .block(panel_block("Selection", false))
                    .wrap(Wrap { trim: false }),
                chunks[2],
            );
            frame.render_widget(
                Paragraph::new(composer_text)
                    .block(panel_block(composer_title, self.focus == Focus::Composer))
                    .wrap(Wrap { trim: false }),
                chunks[3],
            );
        } else {
            frame.render_widget(
                Paragraph::new(composer_text)
                    .block(panel_block(composer_title, self.focus == Focus::Composer))
                    .wrap(Wrap { trim: false }),
                chunks[2],
            );
        }
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(9),
                Constraint::Length(11),
                Constraint::Min(8),
            ])
            .split(area);

        let workspace_info = if let Some(workspace) = &self.bundle.workspace {
            vec![
                Line::raw(workspace_title(workspace)),
                Line::raw(format!("branch: {}", workspace.branch)),
                Line::raw(format!("archived: {}", workspace.archived)),
                Line::raw(format!("pinned: {}", workspace.pinned)),
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

        let sessions = self
            .bundle
            .sessions
            .iter()
            .map(|session| {
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
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if let Some(selected) = self.bundle.selected_session_id
            && let Some(index) = self
                .bundle
                .sessions
                .iter()
                .position(|session| session.id == selected)
        {
            state.select(Some(index));
        }
        frame.render_stateful_widget(
            List::new(sessions)
                .block(panel_block("Sessions", self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            chunks[1],
            &mut state,
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

    fn render_chat(&self, frame: &mut Frame, area: Rect) {
        let lines = self
            .bundle
            .log_entries
            .iter()
            .flat_map(render_chat_entry)
            .collect::<Vec<_>>();
        let title = if self.creating_new_session {
            "Conversation (new session)"
        } else {
            "Conversation"
        };
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block(title, self.focus == Focus::Main))
                .scroll((self.bundle.log_scroll, 0))
                .wrap(Wrap { trim: false }),
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
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block("Logs", self.focus == Focus::Main))
                .scroll((self.bundle.log_scroll, 0))
                .wrap(Wrap { trim: false }),
            area,
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
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(panel_block("Git", self.focus == Focus::Main))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn render_terminal(&self, frame: &mut Frame, area: Rect) {
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
        let title = if self.bundle.terminal.input_mode {
            "Terminal *"
        } else {
            "Terminal"
        };
        let block = panel_block(title, self.focus == Focus::Main);
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
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

    fn current_discovery_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
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
        self.bundle = WorkspaceBundle::default();
        self.bundle.terminal = TerminalState::default();
        self.api.load_workspace(workspace_id, self.tx.clone());
        self.api.replace_workspace_subscriptions(
            workspace_id,
            None,
            None,
            (size.width.saturating_sub(40), size.height.saturating_sub(6)),
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    fn rebind_session_streams(&mut self) {
        self.api.replace_process_stream(
            self.bundle.selected_session_id,
            self.tx.clone(),
            &mut self.subscriptions,
        );
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
        self.rebind_session_streams();
    }

    fn current_session(&self) -> Option<&Session> {
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

    fn filtered_workspace_ids(&self) -> Vec<Uuid> {
        self.filtered_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect()
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
                Ok(()) => self.status = "Stopped workspace execution".to_string(),
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

fn next_focus(current: &Focus) -> Focus {
    match current {
        Focus::WorkspaceList => Focus::Main,
        Focus::Main => Focus::Detail,
        Focus::Detail => Focus::Composer,
        Focus::Composer => Focus::WorkspaceList,
    }
}

fn prev_focus(current: &Focus) -> Focus {
    match current {
        Focus::WorkspaceList => Focus::Composer,
        Focus::Main => Focus::WorkspaceList,
        Focus::Detail => Focus::Main,
        Focus::Composer => Focus::Detail,
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

fn panel_block<'a>(title: &'a str, active: bool) -> Block<'a> {
    let style = if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Span::styled(title.to_string(), style))
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

fn render_chat_entry(entry: &PatchType) -> Vec<Line<'static>> {
    match entry {
        PatchType::NormalizedEntry(entry) => render_normalized_chat_entry(entry),
        PatchType::Stdout(output) => vec![
            Line::styled(
                "stdout",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                output.trim_end().to_string(),
                Style::default().fg(Color::Gray),
            ),
            Line::raw(""),
        ],
        PatchType::Stderr(output) => vec![
            Line::styled(
                "stderr",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                output.trim_end().to_string(),
                Style::default().fg(Color::LightRed),
            ),
            Line::raw(""),
        ],
        PatchType::Diff(diff) => vec![
            Line::styled(
                format!("diff {}", diff_title(diff)),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(format_patch_entry(entry), Style::default().fg(Color::Gray)),
            Line::raw(""),
        ],
    }
}

fn render_normalized_chat_entry(entry: &executors::logs::NormalizedEntry) -> Vec<Line<'static>> {
    use executors::logs::NormalizedEntryType;

    let content = entry.content.trim();
    match &entry.entry_type {
        NormalizedEntryType::ToolUse {
            tool_name,
            action_type,
            status,
        } => {
            let status_color = match status {
                executors::logs::ToolStatus::Success => Color::Green,
                executors::logs::ToolStatus::Failed
                | executors::logs::ToolStatus::Denied { .. } => Color::Red,
                executors::logs::ToolStatus::PendingApproval { .. } => Color::Yellow,
                executors::logs::ToolStatus::TimedOut => Color::LightRed,
                executors::logs::ToolStatus::Created => Color::Cyan,
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    "tool",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(tool_name.clone(), Style::default().fg(Color::LightCyan)),
                Span::raw(" "),
                Span::styled(
                    format!("[{status:?}]").to_lowercase(),
                    Style::default().fg(status_color),
                ),
                Span::raw(" "),
                Span::styled(
                    summarize_action(action_type),
                    Style::default().fg(Color::Gray),
                ),
            ])];
            if !content.is_empty() {
                lines.extend(
                    content
                        .lines()
                        .map(|line| {
                            Line::styled(format!("  {line}"), Style::default().fg(Color::DarkGray))
                        })
                        .collect::<Vec<_>>(),
                );
            }
            lines.push(Line::raw(""));
            lines
        }
        NormalizedEntryType::TokenUsageInfo(info) => vec![
            Line::from(vec![
                Span::styled(
                    "tokens",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{}/{}", info.total_tokens, info.model_context_window),
                    Style::default().fg(Color::LightYellow),
                ),
            ]),
            Line::raw(""),
        ],
        NormalizedEntryType::UserMessage => render_labeled_content("user", Color::Blue, content),
        NormalizedEntryType::AssistantMessage => {
            render_labeled_content("assistant", Color::Green, content)
        }
        NormalizedEntryType::SystemMessage => {
            render_labeled_content("system", Color::Magenta, content)
        }
        NormalizedEntryType::Thinking => render_labeled_content("thinking", Color::Gray, content),
        NormalizedEntryType::Loading => render_labeled_content("loading", Color::DarkGray, content),
        NormalizedEntryType::UserFeedback { .. } => {
            render_labeled_content("feedback", Color::LightBlue, content)
        }
        NormalizedEntryType::ErrorMessage { .. } => {
            render_labeled_content("error", Color::Red, content)
        }
        NormalizedEntryType::NextAction { failed, .. } => {
            let color = if *failed { Color::Red } else { Color::Cyan };
            render_labeled_content("next", color, content)
        }
        NormalizedEntryType::UserAnsweredQuestions { answers } => {
            let text = answers
                .iter()
                .map(|item| format!("{}: {}", item.question, item.answer.join(", ")))
                .collect::<Vec<_>>()
                .join("\n");
            render_labeled_content("answers", Color::LightBlue, &text)
        }
    }
}

fn render_labeled_content(label: &str, label_color: Color, content: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        label.to_string(),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if content.is_empty() {
        lines.push(Line::styled(
            "  (empty)".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        lines.extend(
            content
                .lines()
                .map(|line| Line::styled(format!("  {line}"), Style::default().fg(Color::White)))
                .collect::<Vec<_>>(),
        );
    }
    lines.push(Line::raw(""));
    lines
}

fn render_log_entry(index: usize, entry: &PatchType) -> Vec<Line<'static>> {
    let style = match entry {
        PatchType::NormalizedEntry(normalized) => match normalized.entry_type {
            executors::logs::NormalizedEntryType::ToolUse { .. } => {
                Style::default().fg(Color::Cyan)
            }
            executors::logs::NormalizedEntryType::TokenUsageInfo(_) => {
                Style::default().fg(Color::Yellow)
            }
            executors::logs::NormalizedEntryType::AssistantMessage => {
                Style::default().fg(Color::Green)
            }
            executors::logs::NormalizedEntryType::UserMessage => Style::default().fg(Color::Blue),
            executors::logs::NormalizedEntryType::ErrorMessage { .. } => {
                Style::default().fg(Color::Red)
            }
            _ => Style::default().fg(Color::Gray),
        },
        PatchType::Stdout(_) => Style::default().fg(Color::Gray),
        PatchType::Stderr(_) => Style::default().fg(Color::Red),
        PatchType::Diff(_) => Style::default().fg(Color::Magenta),
    };

    format_patch_entry(entry)
        .lines()
        .enumerate()
        .map(|(line_index, line)| {
            let prefix = if line_index == 0 {
                format!("{index:04} ")
            } else {
                "     ".to_string()
            };
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(line.to_string(), style),
            ])
        })
        .chain(std::iter::once(Line::raw("")))
        .collect()
}

fn summarize_action(action_type: &executors::logs::ActionType) -> String {
    use executors::logs::ActionType;

    match action_type {
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
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}

fn rect_from_size(size: Size) -> Rect {
    Rect::new(0, 0, size.width, size.height)
}
