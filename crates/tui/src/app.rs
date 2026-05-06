use std::{str::FromStr, time::Duration};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use db::models::{
    execution_process::{ExecutionProcessRunReason, ExecutionProcessStatus},
    scratch::DraftFollowUpData,
};
use executors::{
    executors::BaseCodingAgent,
    model_selector::{AgentInfo, ModelInfo, PermissionPolicy},
    profile::ExecutorConfig,
};
use futures_util::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use tokio::{select, time::interval};
use uuid::Uuid;

pub use crate::app_state::App;
use crate::{
    api::TerminalCommand,
    app_state::AgentPickerState,
    conversation::{
        ChatRenderCache, ConversationScope, OptimisticConversationEntry, OptimisticState,
        chat_window_bounds, initial_conversation_process_ids, process_prompt, render_chat_entry,
        render_log_entry, render_optimistic_chat_entry, wrap_lines,
    },
    editor::render_editor_buffer,
    input::{TerminalInput, map_app_key, map_terminal_key},
    model::{
        Focus, NetEvent, Pane, PatchType, QueueStatus, diff_title, display_permission,
        display_variant, workspace_title,
    },
    ui::{
        centered_rect, panel_block, rect_from_size, render_vertical_scrollbar,
        terminal_content_area,
    },
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

        let composer_title = self.editor_panel_title();
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
                    .block(panel_block(&composer_title, self.focus == Focus::Composer))
                    .wrap(Wrap { trim: false }),
                chunks[3],
            );
        } else {
            frame.render_widget(
                Paragraph::new(render_editor_buffer(
                    &self.bundle.notes,
                    self.notes_cursor.min(self.bundle.notes.len()),
                    self.focus == Focus::Composer && self.selected_pane == Pane::Notes,
                    self.editor_mode,
                ))
                .block(panel_block(&composer_title, self.focus == Focus::Composer))
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

    fn current_discovery_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn current_composer_scratch_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            self.selected_workspace_id
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn current_queue_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn sync_composer_context(&mut self) {
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

    pub(crate) fn refresh_queue_status(&mut self) {
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

    pub(crate) fn has_running_process(&self) -> bool {
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

    pub(crate) fn reset_conversation_state(&mut self) {
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

    pub(crate) fn refresh_conversation_history(&mut self) {
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

    pub(crate) fn reconcile_optimistic_entries(&mut self) {
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

    pub(crate) fn mark_chat_render_cache_dirty(&mut self) {
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

    pub(crate) fn rebind_discovery_stream(&mut self) {
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

    pub(crate) fn filtered_agent_mode_options(&self) -> Vec<Option<AgentInfo>> {
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

    pub(crate) fn open_agent_picker(&mut self) {
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

    pub(crate) async fn cycle_executor(&mut self) {
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

    pub(crate) async fn cycle_variant(&mut self) {
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

    pub(crate) fn cycle_model(&mut self) {
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

    pub(crate) fn cycle_reasoning(&mut self) {
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

    pub(crate) fn cycle_permission_mode(&mut self) {
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

    pub(crate) fn sync_composer_executor_with_session(&mut self) {
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

    pub(crate) async fn queue_prompt(&mut self) {
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

    pub(crate) async fn cancel_queued_prompt(&mut self) {
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

    pub(crate) async fn discard_draft(&mut self) {
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

    fn send_terminal_input(&mut self, bytes: Vec<u8>) {
        let Some(tx) = &self.subscriptions.terminal_tx else {
            return;
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
