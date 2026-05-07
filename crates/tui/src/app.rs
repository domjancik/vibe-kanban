use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent};
use db::models::execution_process::ExecutionProcessStatus;
use executors::model_selector::ModelInfo;
use futures_util::StreamExt;
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tokio::{select, time::interval};

pub use crate::app_state::App;
use crate::{
    api::TerminalCommand,
    conversation::render_log_entry,
    input::{TerminalInput, map_app_key, map_terminal_key},
    model::{Focus, NetEvent, Pane, QueueStatus, display_permission, display_variant},
    ui::{rect_from_size, terminal_content_area},
    workspace::session_target,
};

impl App {
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
            match map_terminal_key(key) {
                Some(TerminalInput::ExitInputMode) => {
                    self.bundle.terminal.input_mode = false;
                    self.focus = Focus::Main;
                    self.status = "Left terminal input mode".to_string();
                }
                Some(TerminalInput::SendBytes(bytes)) => self.send_terminal_input(bytes),
                None => {}
            }
            return;
        }

        if self.focus == Focus::Composer {
            match self.selected_pane {
                Pane::Chat => {
                    self.handle_editor_key(key, false).await;
                    return;
                }
                Pane::Notes => {
                    self.handle_editor_key(key, true).await;
                    return;
                }
                _ => {}
            }
        }

        if let Some(intent) = map_app_key(key, self.creating_new_session, &self.selected_pane) {
            self.handle_app_intent(intent, size).await;
        }
    }

    pub(crate) async fn submit_prompt(&mut self) {
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

    pub(crate) fn handle_terminal_resize(&mut self, size: Rect) {
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

    pub(crate) async fn handle_enter(&mut self, size: Rect) {
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

    pub(crate) fn move_selection(&mut self, delta: i32, size: Rect) {
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

    pub(crate) fn page_step(&self, size: Rect) -> i32 {
        match self.focus {
            Focus::WorkspaceList => ((size.height.saturating_sub(4) / 3).max(1)) as i32,
            Focus::Detail => 5,
            Focus::Main | Focus::Composer => size.height.saturating_sub(6).max(1) as i32,
        }
    }

    pub(crate) fn jump_to_boundary(&mut self, to_end: bool, size: Rect) {
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

    pub(crate) fn terminal_stream_size(&self, size: Rect) -> (u16, u16) {
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

    pub(crate) fn composer_selection_line(&self) -> Line<'static> {
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

    pub(crate) fn composer_status_line(&self) -> Line<'static> {
        let draft = self.draft_status_label();
        let queue = self.queue_status_label();
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

    pub(crate) fn draft_status_label(&self) -> String {
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

    pub(crate) fn queue_status_label(&self) -> String {
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

    pub(crate) fn has_running_process(&self) -> bool {
        self.bundle
            .process_map
            .values()
            .any(|process| process.status == ExecutionProcessStatus::Running)
    }

    fn send_terminal_input(&mut self, bytes: Vec<u8>) {
        let Some(tx) = &self.subscriptions.terminal_tx else {
            return;
        };
        let _ = tx.send(TerminalCommand::Input(bytes));
    }
}

pub(crate) fn default_variant_to_none(variant: String) -> Option<String> {
    if variant == "DEFAULT" {
        None
    } else {
        Some(variant)
    }
}

pub(crate) fn model_key(model: &ModelInfo) -> String {
    if let Some(provider_id) = &model.provider_id {
        format!("{provider_id}/{}", model.id)
    } else {
        model.id.clone()
    }
}

pub(crate) fn fuzzy_contains(query: &str, candidate: &str) -> bool {
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

pub(crate) fn viewport_capacity(area: Rect, rows_per_item: u16) -> usize {
    area.height
        .saturating_sub(2)
        .max(1)
        .div_ceil(rows_per_item)
        .max(1) as usize
}

pub(crate) fn selected_list_offset(selected: usize, total: usize, viewport: usize) -> usize {
    if total <= viewport {
        0
    } else {
        selected
            .saturating_sub(viewport.saturating_sub(1))
            .min(total.saturating_sub(viewport))
    }
}
