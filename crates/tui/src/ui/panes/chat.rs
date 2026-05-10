use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Gauge, Paragraph, Wrap},
};

use crate::{
    app::{App, SearchTarget, highlight_line_matches},
    conversation::chat_window_bounds,
    model::Focus,
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    pub(crate) fn render_chat(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.creating_workspace {
            "Create Workspace"
        } else if self.creating_new_session {
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

        if self.creating_workspace {
            let selected_repos = self
                .workspace_create
                .as_ref()
                .map(|state| {
                    state
                        .selected_repos
                        .iter()
                        .map(|entry| {
                            format!("{} -> {}", entry.repo.display_name, entry.target_branch)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let summary = if selected_repos.is_empty() {
                vec![
                    Line::styled(
                        "1. Press a to add repositories",
                        Style::default().fg(Color::LightBlue),
                    ),
                    Line::styled(
                        "2. Choose a branch for each repo",
                        Style::default().fg(Color::LightBlue),
                    ),
                    Line::styled(
                        "3. Write the prompt below and press Enter",
                        Style::default().fg(Color::LightBlue),
                    ),
                ]
            } else {
                let mut lines = vec![Line::styled(
                    format!("Selected repos: {}", selected_repos.len()),
                    Style::default().fg(Color::Green),
                )];
                lines.extend(selected_repos.into_iter().map(Line::raw));
                lines
            };
            frame.render_widget(
                Paragraph::new(Text::from(summary)).wrap(Wrap { trim: false }),
                inner.inner(Margin {
                    vertical: 1,
                    horizontal: 1,
                }),
            );
            return;
        }

        let search_active = self
            .inline_search_prompt(SearchTarget::Conversation)
            .is_some();
        let vertical_constraints = if search_active && inner.height > 3 {
            vec![
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
        } else if inner.height > 1 {
            vec![Constraint::Min(1), Constraint::Length(1)]
        } else {
            vec![Constraint::Min(1)]
        };
        let chat_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vertical_constraints)
            .split(inner);
        let (messages_area, status_area) = if search_active && chat_chunks.len() >= 3 {
            if let Some(prompt) = self.inline_search_prompt(SearchTarget::Conversation) {
                frame.render_widget(
                    Paragraph::new(prompt).wrap(Wrap { trim: false }),
                    chat_chunks[0],
                );
            }
            (chat_chunks[1], Some(chat_chunks[2]))
        } else if inner.height > 1 {
            (chat_chunks[0], chat_chunks.get(1).copied())
        } else {
            (inner, None)
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
            let query = self
                .active_search_query_for(SearchTarget::Conversation)
                .unwrap_or("")
                .to_string();
            let cache = self.chat_render_cache(content_area.width.max(1) as usize);
            cache.lines[start..end]
                .iter()
                .map(|line| highlight_line_matches(line, &query))
                .collect::<Vec<_>>()
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
        let todo_chunks = if self.current_todo_state().is_some() && area.width >= 28 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(16), Constraint::Min(10)])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(0), Constraint::Min(1)])
                .split(area)
        };

        if let Some(todo_state) = self.current_todo_state()
            && todo_chunks[0].width > 0
        {
            let todo_style = if todo_state.total > 0 && todo_state.completed == todo_state.total {
                Style::default().fg(Color::Green)
            } else if todo_state.in_progress_index.is_some() {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("Todos {}/{}", todo_state.completed, todo_state.total),
                    todo_style,
                )),
                todo_chunks[0],
            );
        }

        let context_area = todo_chunks[1];
        let Some((total_tokens, context_window)) = latest_token_usage else {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "latest context usage unavailable",
                    Style::default().fg(Color::DarkGray),
                )),
                context_area,
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
            context_area,
        );
    }
}
