use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app::{App, ConversationSearchState, SearchPromptState, SearchTarget},
    editor::apply_text_edit_action,
    input::{TextInputEvent, TextInputOptions, map_text_input_key},
    model::{Focus, Pane},
};

impl App {
    pub(crate) fn open_search(&mut self, size: Rect) {
        let Some(target) = self.search_target() else {
            self.status =
                "Search is available in workspaces, sessions, and conversation".to_string();
            return;
        };

        let query = match target {
            SearchTarget::Workspaces => self.filter.clone(),
            SearchTarget::Sessions => self.session_filter.clone(),
            SearchTarget::Conversation => self
                .conversation_search
                .as_ref()
                .map(|search| search.query.clone())
                .unwrap_or_default(),
        };

        self.search_prompt = Some(SearchPromptState {
            target,
            cursor: query.len(),
            original_query: query.clone(),
            original_conversation_search: (target == SearchTarget::Conversation)
                .then(|| self.conversation_search.clone())
                .flatten(),
            original_chat_end_offset: self.chat_end_offset,
            query,
        });
        self.error = None;
        self.status = match target {
            SearchTarget::Workspaces => "Filter workspaces".to_string(),
            SearchTarget::Sessions => "Filter sessions".to_string(),
            SearchTarget::Conversation => "Search conversation".to_string(),
        };

        if target == SearchTarget::Conversation {
            self.update_conversation_search(size, true);
        }
    }

    pub(crate) async fn handle_search_prompt_key(&mut self, key: KeyEvent, size: Rect) {
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.cancel_search_prompt(size);
            }
            _ => {
                let options = TextInputOptions {
                    submit_on_enter: true,
                    enter_inserts_newline: false,
                    shift_enter_inserts_newline: false,
                };
                match map_text_input_key(key, options) {
                    Some(TextInputEvent::Submit) => self.submit_search_prompt(size),
                    Some(TextInputEvent::Edit(action)) => {
                        if let Some(prompt) = self.search_prompt.as_mut() {
                            apply_text_edit_action(&mut prompt.query, &mut prompt.cursor, action);
                        }
                        self.preview_search_prompt(size);
                    }
                    None => {}
                }
            }
        }
    }

    pub(crate) fn advance_conversation_search(&mut self, forward: bool, size: Rect) {
        if self.selected_pane != Pane::Chat {
            return;
        }
        let query = self
            .conversation_search
            .as_ref()
            .map(|state| state.query.clone())
            .unwrap_or_default();
        if query.is_empty() {
            self.status = "No active conversation search".to_string();
            return;
        }

        let Some((matches, total_lines, visible_lines)) =
            self.conversation_search_snapshot(&query, size)
        else {
            self.status = "Conversation search unavailable".to_string();
            return;
        };
        if matches.is_empty() {
            self.conversation_search = Some(ConversationSearchState {
                query,
                matches,
                current_match: 0,
            });
            self.status = "No conversation matches".to_string();
            return;
        }

        let current_match = self
            .conversation_search
            .as_ref()
            .map(|state| state.current_match.min(matches.len().saturating_sub(1)))
            .unwrap_or(0);
        let next_match = if forward {
            (current_match + 1) % matches.len()
        } else {
            current_match
                .checked_sub(1)
                .unwrap_or(matches.len().saturating_sub(1))
        };
        self.conversation_search = Some(ConversationSearchState {
            query: query.clone(),
            matches: matches.clone(),
            current_match: next_match,
        });
        self.jump_to_conversation_match(matches[next_match], total_lines, visible_lines);
        self.status = format!(
            "Conversation match {}/{} for {}",
            next_match + 1,
            matches.len(),
            query
        );
    }

    fn submit_search_prompt(&mut self, size: Rect) {
        self.apply_search_prompt(size);
        self.search_prompt = None;
    }

    fn preview_search_prompt(&mut self, size: Rect) {
        let Some((target, query)) = self
            .search_prompt
            .as_ref()
            .map(|prompt| (prompt.target, prompt.query.clone()))
        else {
            return;
        };

        match target {
            SearchTarget::Workspaces => {
                self.status = if query.is_empty() {
                    "Workspace filter preview cleared".to_string()
                } else {
                    format!("Workspace filter preview: {query}")
                };
            }
            SearchTarget::Sessions => {
                self.status = if query.is_empty() {
                    "Session filter preview cleared".to_string()
                } else {
                    format!("Session filter preview: {query}")
                };
            }
            SearchTarget::Conversation => self.update_conversation_search(size, true),
        }
    }

    fn cancel_search_prompt(&mut self, size: Rect) {
        let Some(prompt) = self.search_prompt.take() else {
            return;
        };

        match prompt.target {
            SearchTarget::Workspaces => {
                let previous = self.selected_workspace_id;
                self.filter = prompt.original_query;
                self.mark_workspace_list_dirty();
                self.sync_workspace_selection_to_filter();
                if self.selected_workspace_id != previous {
                    self.load_selected_workspace(size);
                }
            }
            SearchTarget::Sessions => {
                self.session_filter = prompt.original_query;
                self.mark_detail_dirty();
                self.sync_session_selection_to_filter();
            }
            SearchTarget::Conversation => {
                self.conversation_search = prompt.original_conversation_search;
                self.chat_end_offset = prompt.original_chat_end_offset;
            }
        }

        self.status = "Closed search".to_string();
    }

    fn apply_search_prompt(&mut self, size: Rect) {
        let Some((target, query)) = self
            .search_prompt
            .as_ref()
            .map(|prompt| (prompt.target, prompt.query.clone()))
        else {
            return;
        };

        match target {
            SearchTarget::Workspaces => {
                let previous = self.selected_workspace_id;
                self.filter = query;
                self.mark_workspace_list_dirty();
                self.sync_workspace_selection_to_filter();
                if self.selected_workspace_id != previous {
                    self.load_selected_workspace(size);
                }
                self.status = if self.filter.is_empty() {
                    "Workspace filter cleared".to_string()
                } else {
                    format!("Workspace filter: {}", self.filter)
                };
            }
            SearchTarget::Sessions => {
                self.session_filter = query;
                self.mark_detail_dirty();
                self.sync_session_selection_to_filter();
                self.status = if self.session_filter.is_empty() {
                    "Session filter cleared".to_string()
                } else {
                    format!("Session filter: {}", self.session_filter)
                };
            }
            SearchTarget::Conversation => self.update_conversation_search(size, true),
        }
    }

    fn update_conversation_search(&mut self, size: Rect, jump_to_first: bool) {
        let query = self
            .search_prompt
            .as_ref()
            .filter(|prompt| prompt.target == SearchTarget::Conversation)
            .map(|prompt| prompt.query.clone())
            .or_else(|| {
                self.conversation_search
                    .as_ref()
                    .map(|state| state.query.clone())
            })
            .unwrap_or_default();

        if query.is_empty() {
            self.conversation_search = None;
            self.status = "Conversation search cleared".to_string();
            return;
        }

        let Some((matches, total_lines, visible_lines)) =
            self.conversation_search_snapshot(&query, size)
        else {
            return;
        };

        let current_match = self
            .conversation_search
            .as_ref()
            .filter(|state| state.query == query && !state.matches.is_empty())
            .map(|state| state.current_match.min(matches.len().saturating_sub(1)))
            .unwrap_or(0);
        let current_match = if matches.is_empty() || jump_to_first {
            0
        } else {
            current_match
        };

        self.conversation_search = Some(ConversationSearchState {
            query: query.clone(),
            matches: matches.clone(),
            current_match,
        });

        if let Some(line_index) = matches.get(current_match).copied() {
            self.jump_to_conversation_match(line_index, total_lines, visible_lines);
            self.status = format!(
                "Conversation match {}/{} for {}",
                current_match + 1,
                matches.len(),
                query
            );
        } else {
            self.status = format!("No conversation matches for {}", query);
        }
    }

    fn conversation_search_snapshot(
        &mut self,
        query: &str,
        size: Rect,
    ) -> Option<(Vec<usize>, usize, usize)> {
        let (width, visible_lines) = self.chat_search_metrics(size)?;
        let cache = self.chat_render_cache(width);
        let matches = matching_line_indexes(&cache.lines, query);
        Some((matches, cache.lines.len(), visible_lines))
    }

    fn jump_to_conversation_match(
        &mut self,
        line_index: usize,
        total_lines: usize,
        visible_lines: usize,
    ) {
        let end = (line_index + visible_lines).min(total_lines);
        let offset = total_lines.saturating_sub(end);
        self.chat_end_offset = offset.min(u16::MAX as usize) as u16;
    }

    fn search_target(&self) -> Option<SearchTarget> {
        match self.focus {
            Focus::WorkspaceList => Some(SearchTarget::Workspaces),
            Focus::Detail => Some(SearchTarget::Sessions),
            Focus::Main if self.selected_pane == Pane::Chat => Some(SearchTarget::Conversation),
            _ => None,
        }
    }

    pub(crate) fn active_search_query_for(&self, target: SearchTarget) -> Option<&str> {
        if self
            .search_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.target == target)
        {
            return self
                .search_prompt
                .as_ref()
                .map(|prompt| prompt.query.as_str());
        }
        match target {
            SearchTarget::Workspaces => (!self.filter.is_empty()).then_some(self.filter.as_str()),
            SearchTarget::Sessions => {
                (!self.session_filter.is_empty()).then_some(self.session_filter.as_str())
            }
            SearchTarget::Conversation => self
                .conversation_search
                .as_ref()
                .filter(|search| !search.query.is_empty())
                .map(|search| search.query.as_str()),
        }
    }

    pub(crate) fn inline_search_prompt(&self, target: SearchTarget) -> Option<Line<'static>> {
        let active = self
            .search_prompt
            .as_ref()
            .filter(|prompt| prompt.target == target);
        let query = active
            .map(|prompt| prompt.query.as_str())
            .or_else(|| self.active_search_query_for(target))?;
        let label = match target {
            SearchTarget::Workspaces => "workspace filter",
            SearchTarget::Sessions => "session filter",
            SearchTarget::Conversation => "conversation search",
        };
        let mut spans = vec![
            Span::styled(
                "/ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
        ];
        if let Some(prompt) = active {
            spans.extend(
                crate::editor::render_editor_buffer(
                    &prompt.query,
                    prompt.cursor,
                    true,
                    self.editor_mode,
                )
                .lines
                .into_iter()
                .next()
                .map(|line| line.spans)
                .unwrap_or_default(),
            );
        } else {
            spans.push(Span::styled(
                query.to_string(),
                Style::default().fg(Color::White),
            ));
        }
        Some(Line::from(spans))
    }

    fn chat_search_metrics(&mut self, size: Rect) -> Option<(usize, usize)> {
        if self.selected_pane != Pane::Chat {
            return None;
        }

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(size);
        let main_area = if self.maximized_panel {
            outer[1]
        } else if size.width >= 140 {
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

        let composer_height = self.chat_composer_height(main_area.width);
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(4),
                Constraint::Length(composer_height),
            ])
            .split(main_area);
        let chat_area = main_chunks[1];
        let panel_inner = chat_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let inner = if self
            .inline_search_prompt(SearchTarget::Conversation)
            .is_some()
            && panel_inner.height > 3
        {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(panel_inner)[1]
        } else {
            panel_inner
        };
        if inner.height == 0 || inner.width == 0 {
            return None;
        }
        let visible_lines = inner.height.saturating_sub(1).max(1) as usize;
        let width = inner.width.saturating_sub(2).max(1) as usize;
        Some((width, visible_lines))
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn matching_line_indexes(lines: &[Line<'_>], query: &str) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            (!find_case_insensitive_match_ranges(&line_text(line), query).is_empty())
                .then_some(index)
        })
        .collect()
}

pub(crate) fn highlight_line_matches(line: &Line<'_>, query: &str) -> Line<'static> {
    let matches = find_case_insensitive_match_ranges(&line_text(line), query);
    if matches.is_empty() {
        return Line::from(
            line.spans
                .iter()
                .map(|span| Span::styled(span.content.to_string(), span.style))
                .collect::<Vec<_>>(),
        );
    }

    let highlight_style = Style::default()
        .bg(Color::Rgb(64, 56, 0))
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut line_offset = 0;
    let mut match_index = 0;
    for span in &line.spans {
        let text = span.content.as_ref();
        let span_start = line_offset;
        let span_end = span_start + text.len();
        let mut local_cursor = 0;

        while let Some(&(match_start, match_end)) = matches.get(match_index) {
            if match_end <= span_start {
                match_index += 1;
                continue;
            }
            if match_start >= span_end {
                break;
            }

            let overlap_start = match_start.max(span_start) - span_start;
            let overlap_end = match_end.min(span_end) - span_start;
            if local_cursor < overlap_start {
                spans.push(Span::styled(
                    text[local_cursor..overlap_start].to_string(),
                    span.style,
                ));
            }
            spans.push(Span::styled(
                text[overlap_start..overlap_end].to_string(),
                span.style.patch(highlight_style),
            ));
            local_cursor = overlap_end;

            if match_end <= span_end {
                match_index += 1;
            } else {
                break;
            }
        }

        if local_cursor < text.len() {
            spans.push(Span::styled(text[local_cursor..].to_string(), span.style));
        }
        line_offset = span_end;
    }
    Line::from(spans)
}

pub(crate) fn highlight_text_span(
    text: &str,
    base_style: Style,
    query: &str,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    let matches = find_case_insensitive_match_ranges(text, query);
    if matches.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, end) in matches {
        if cursor < start {
            spans.push(Span::styled(text[cursor..start].to_string(), base_style));
        }
        spans.push(Span::styled(
            text[start..end].to_string(),
            base_style.patch(highlight_style),
        ));
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::styled(text[cursor..].to_string(), base_style));
    }
    spans
}

fn find_case_insensitive_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let lowered_query = query.to_lowercase();
    if lowered_query.is_empty() {
        return Vec::new();
    }

    let text_chars = text.char_indices().collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut index = 0;
    while index < text_chars.len() {
        let mut lowered_window = String::new();
        let mut matched_end = None;

        for end_index in index..text_chars.len() {
            lowered_window.push_str(&text_chars[end_index].1.to_lowercase().to_string());
            if lowered_window == lowered_query {
                matched_end = Some(end_index + 1);
                break;
            }
            if !lowered_query.starts_with(&lowered_window) {
                break;
            }
        }

        if let Some(end_index) = matched_end {
            let start = text_chars[index].0;
            let end = if end_index < text_chars.len() {
                text_chars[end_index].0
            } else {
                text.len()
            };
            matches.push((start, end));
            index = end_index;
        } else {
            index += 1;
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use ratatui::{
        layout::Rect,
        style::{Color, Style},
        text::{Line, Span},
    };

    use super::{
        find_case_insensitive_match_ranges, highlight_line_matches, highlight_text_span, line_text,
        matching_line_indexes,
    };
    use crate::{api::Api, app::App, model::Focus};

    #[test]
    fn line_text_concatenates_spans() {
        let line = Line::from(vec!["hel".into(), "lo".into()]);
        assert_eq!(line_text(&line), "hello");
    }

    #[test]
    fn matching_line_indexes_is_case_insensitive() {
        let lines = vec![
            Line::from("first"),
            Line::from("Needle here"),
            Line::from("another needle"),
        ];
        assert_eq!(matching_line_indexes(&lines, "needle"), vec![1, 2]);
        assert!(matching_line_indexes(&lines, "").is_empty());
    }

    #[test]
    fn find_case_insensitive_match_ranges_finds_multiple_ranges() {
        assert_eq!(
            find_case_insensitive_match_ranges("Needle needle", "needle"),
            vec![(0, 6), (7, 13)]
        );
    }

    #[test]
    fn find_case_insensitive_match_ranges_handles_unicode_case_folding() {
        assert_eq!(
            find_case_insensitive_match_ranges("Über Café", "über"),
            vec![(0, "Über".len())]
        );
    }

    #[test]
    fn highlight_text_span_preserves_unmatched_segments() {
        let spans = highlight_text_span("alpha beta", Style::default(), "be", Style::default());
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content.as_ref(), "alpha ");
        assert_eq!(spans[1].content.as_ref(), "be");
        assert_eq!(spans[2].content.as_ref(), "ta");
    }

    #[test]
    fn highlight_line_matches_across_span_boundaries() {
        let line = Line::from(vec![
            Span::styled("foo", Style::default().fg(Color::Blue)),
            Span::styled(" bar", Style::default().fg(Color::Green)),
        ]);

        let highlighted = highlight_line_matches(&line, "foo bar");

        assert_eq!(line_text(&highlighted), "foo bar");
        assert_eq!(highlighted.spans.len(), 2);
        assert_eq!(highlighted.spans[0].style.bg, Some(Color::Rgb(64, 56, 0)));
        assert_eq!(highlighted.spans[1].style.bg, Some(Color::Rgb(64, 56, 0)));
    }

    #[tokio::test]
    async fn canceling_workspace_search_restores_original_filter() {
        let mut app = App::new(Api::new("http://127.0.0.1:9".to_string()).unwrap());
        app.focus = Focus::WorkspaceList;
        app.filter = "orig".to_string();

        app.open_search(Rect::new(0, 0, 120, 40));
        if let Some(prompt) = app.search_prompt.as_mut() {
            prompt.query = "changed".to_string();
            prompt.cursor = prompt.query.len();
        }
        app.apply_search_prompt(Rect::new(0, 0, 120, 40));
        assert_eq!(app.filter, "changed");

        app.cancel_search_prompt(Rect::new(0, 0, 120, 40));

        assert_eq!(app.filter, "orig");
        assert!(app.search_prompt.is_none());
    }
}
