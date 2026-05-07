use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Tabs, Wrap},
};

use crate::{
    app::{App, state::ComposerHeightCache},
    editor::render_editor_buffer,
    model::{Focus, Pane},
    ui::panel_block,
};

impl App {
    pub(crate) fn render_main(&mut self, frame: &mut Frame, area: Rect) {
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
        } else if self.selected_pane == Pane::Notes {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(10)])
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

        if self.selected_pane == Pane::Chat {
            let composer_title = self.editor_panel_title();
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
        } else if self.selected_pane != Pane::Notes {
            let composer_title = self.editor_panel_title();
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

    pub(crate) fn chat_composer_height(&mut self, area_width: u16) -> u16 {
        if let Some(cache) = self.composer_height_cache
            && cache.width == area_width
            && cache.revision == self.composer_edit_revision
        {
            return cache.height;
        }
        let height = self.compute_chat_composer_height(area_width);
        self.composer_height_cache = Some(ComposerHeightCache {
            width: area_width,
            revision: self.composer_edit_revision,
            height,
        });
        height
    }

    fn compute_chat_composer_height(&self, area_width: u16) -> u16 {
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
}
