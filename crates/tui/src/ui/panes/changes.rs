use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::App,
    model::{ChangesPaneRenderCache, DiffViewMode, Focus, LocalDiff, diff_title},
    ui::{panel_block, render_vertical_scrollbar},
};

impl App {
    fn changes_pane_cache(&mut self) -> &ChangesPaneRenderCache {
        let needs_rebuild = self.bundle.changes_cache.as_ref().is_none_or(|cache| {
            cache.revision != self.bundle.changes_revision
                || cache.diff_index != self.bundle.selected_diff_index
                || cache.diff_view_mode != self.bundle.diff_view_mode
        });
        if needs_rebuild {
            let file_items = self
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
            let selected = self.bundle.diffs.get(self.bundle.selected_diff_index);
            let unified_text = selected
                .map(render_unified_diff_text)
                .unwrap_or_else(|| Text::from("No diff selected"));
            let (side_by_side_left, side_by_side_right) =
                selected.map(render_side_by_side_text).unwrap_or_else(|| {
                    (
                        Text::from("No diff selected"),
                        Text::from("No diff selected"),
                    )
                });
            self.bundle.changes_cache = Some(ChangesPaneRenderCache {
                revision: self.bundle.changes_revision,
                diff_index: self.bundle.selected_diff_index,
                diff_view_mode: self.bundle.diff_view_mode,
                file_items,
                unified_text,
                side_by_side_left,
                side_by_side_right,
            });
        }
        self.bundle
            .changes_cache
            .as_ref()
            .expect("changes pane cache should be populated")
    }

    pub(crate) fn render_changes(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(area);

        let cache = self.changes_pane_cache().clone();
        let mut state = ListState::default();
        if !self.bundle.diffs.is_empty() {
            state.select(Some(self.bundle.selected_diff_index));
        }
        frame.render_stateful_widget(
            List::new(cache.file_items.clone())
                .block(panel_block("Files", self.focus == Focus::Main))
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

        match self.bundle.diff_view_mode {
            DiffViewMode::Unified => {
                self.render_unified_diff(frame, chunks[1], &cache.unified_text)
            }
            DiffViewMode::SideBySide => self.render_side_by_side_diff(
                frame,
                chunks[1],
                &cache.side_by_side_left,
                &cache.side_by_side_right,
            ),
        }
    }

    fn render_unified_diff(&self, frame: &mut Frame, area: Rect, diff_text: &Text<'static>) {
        let title = "Diff [unified | b side-by-side]";
        frame.render_widget(
            Paragraph::new(diff_text.clone())
                .block(panel_block(title, self.focus == Focus::Detail))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn render_side_by_side_diff(
        &self,
        frame: &mut Frame,
        area: Rect,
        left_text: &Text<'static>,
        right_text: &Text<'static>,
    ) {
        let outer = panel_block(
            "Diff [side-by-side | b unified]",
            self.focus == Focus::Detail,
        );
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        frame.render_widget(
            Paragraph::new(left_text.clone())
                .block(Block::bordered().title("Old"))
                .wrap(Wrap { trim: false }),
            columns[0],
        );
        frame.render_widget(
            Paragraph::new(right_text.clone())
                .block(Block::bordered().title("New"))
                .wrap(Wrap { trim: false }),
            columns[1],
        );
    }
}

fn render_unified_diff_text(diff: &LocalDiff) -> Text<'static> {
    if diff.content_omitted {
        return Text::from(vec![
            Line::styled(diff_title(diff), Style::default().fg(Color::Cyan)),
            Line::styled(
                format!(
                    "content omitted  +{} -{}",
                    diff.additions.unwrap_or_default(),
                    diff.deletions.unwrap_or_default()
                ),
                Style::default().fg(Color::Yellow),
            ),
        ]);
    }

    let old = diff.old_content.as_deref().unwrap_or("");
    let new = diff.new_content.as_deref().unwrap_or("");
    let file = diff_title(diff);
    let diff_text = utils::diff::create_unified_diff(&file, old, new);
    Text::from(
        diff_text
            .lines()
            .map(|line| {
                let style = if line.starts_with("diff --git")
                    || line.starts_with("--- ")
                    || line.starts_with("+++ ")
                {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if line.starts_with("@@") {
                    Style::default().fg(Color::Yellow)
                } else if line.starts_with('+') && !line.starts_with("+++") {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Line::styled(line.to_string(), style)
            })
            .collect::<Vec<_>>(),
    )
}

fn render_side_by_side_text(diff: &LocalDiff) -> (Text<'static>, Text<'static>) {
    if diff.content_omitted {
        let summary = Text::from(vec![
            Line::styled(diff_title(diff), Style::default().fg(Color::Cyan)),
            Line::styled(
                format!(
                    "content omitted  +{} -{}",
                    diff.additions.unwrap_or_default(),
                    diff.deletions.unwrap_or_default()
                ),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        return (summary.clone(), summary);
    }

    let old_lines = split_diff_lines(diff.old_content.as_deref().unwrap_or(""));
    let new_lines = split_diff_lines(diff.new_content.as_deref().unwrap_or(""));
    let row_count = old_lines.len().max(new_lines.len()).max(1);
    let mut left = Vec::with_capacity(row_count);
    let mut right = Vec::with_capacity(row_count);
    let mut old_number = 1usize;
    let mut new_number = 1usize;

    for index in 0..row_count {
        let old_line = old_lines.get(index).copied();
        let new_line = new_lines.get(index).copied();
        let changed = old_line != new_line;
        left.push(render_side_column_line(
            old_line,
            old_line.is_some().then_some(old_number),
            changed,
            true,
        ));
        right.push(render_side_column_line(
            new_line,
            new_line.is_some().then_some(new_number),
            changed,
            false,
        ));
        if old_line.is_some() {
            old_number += 1;
        }
        if new_line.is_some() {
            new_number += 1;
        }
    }

    (Text::from(left), Text::from(right))
}

fn split_diff_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.lines().collect()
    }
}

fn render_side_column_line(
    content: Option<&str>,
    line_number: Option<usize>,
    changed: bool,
    is_old: bool,
) -> Line<'static> {
    let base = if is_old {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };
    let muted = Style::default().fg(Color::DarkGray);

    match (content, line_number) {
        (Some(content), Some(line_number)) => {
            let style = if changed {
                base
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(vec![
                Span::styled(format!("{line_number:>4} "), muted),
                Span::styled(content.to_string(), style),
            ])
        }
        _ => Line::from(vec![Span::styled("     ".to_string(), muted)]),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{render_side_by_side_text, render_unified_diff_text};
    use crate::model::{DiffChangeKind, LocalDiff};

    fn diff(old: &str, new: &str) -> LocalDiff {
        LocalDiff {
            change: DiffChangeKind::Modified,
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            old_content: Some(old.to_string()),
            new_content: Some(new.to_string()),
            content_omitted: false,
            additions: Some(1),
            deletions: Some(1),
            repo_id: None,
        }
    }

    #[test]
    fn unified_diff_text_colors_headers_and_changes() {
        let text = render_unified_diff_text(&diff("old\n", "new\n"));

        assert_eq!(text.lines[0].style.fg, Some(Color::Cyan));
        assert_eq!(text.lines[3].style.fg, Some(Color::Yellow));
        assert_eq!(text.lines[4].style.fg, Some(Color::Red));
        assert_eq!(text.lines[5].style.fg, Some(Color::Green));
    }

    #[test]
    fn side_by_side_text_colors_changed_columns() {
        let (left, right) = render_side_by_side_text(&diff("one\ntwo\n", "one\nTHREE\n"));

        assert_eq!(left.lines[0].spans[1].style.fg, Some(Color::Gray));
        assert_eq!(right.lines[0].spans[1].style.fg, Some(Color::Gray));
        assert_eq!(left.lines[1].spans[1].style.fg, Some(Color::Red));
        assert_eq!(right.lines[1].spans[1].style.fg, Some(Color::Green));
    }
}
