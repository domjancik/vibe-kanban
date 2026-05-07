use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app_state::App,
    model::{Focus, diff_title},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    pub(crate) fn render_changes(&self, frame: &mut Frame, area: Rect) {
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
            crate::app::viewport_capacity(chunks[0], 2),
            crate::app::selected_list_offset(
                self.bundle.selected_diff_index,
                self.bundle.diffs.len(),
                crate::app::viewport_capacity(chunks[0], 2),
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
