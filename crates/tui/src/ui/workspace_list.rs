use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{App, SearchTarget, highlight_text_span, state::WorkspaceListRenderCache},
    model::{Focus, format_relative_time, workspace_title},
    ui::{panel_block, render_vertical_scrollbar},
    workspace::WorkspaceRow,
};

impl App {
    fn workspace_list_cache(&mut self) -> &WorkspaceListRenderCache {
        let query = self
            .active_search_query_for(SearchTarget::Workspaces)
            .unwrap_or("")
            .to_string();
        let needs_rebuild = self.workspace_list_cache.as_ref().is_none_or(|cache| {
            cache.revision != self.workspace_list_revision || cache.query != query
        });
        if needs_rebuild {
            let rows = self.workspace_rows();
            let mut row_ids = Vec::with_capacity(rows.len());
            let mut new_workspace_row_index = None;
            let items = rows
                .iter()
                .enumerate()
                .map(|(index, row)| match row {
                    WorkspaceRow::NewWorkspace => {
                        new_workspace_row_index = Some(index);
                        row_ids.push(None);
                        ListItem::new(Text::from(vec![
                            Line::styled(
                                " + New Workspace",
                                Style::default()
                                    .fg(Color::LightGreen)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Line::styled(
                                " Start a fresh workspace creation flow",
                                Style::default().fg(Color::DarkGray),
                            ),
                            Line::raw(""),
                        ]))
                    }
                    WorkspaceRow::Header(title) => {
                        row_ids.push(None);
                        ListItem::new(Line::styled(
                            format!(" {title} "),
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ))
                    }
                    WorkspaceRow::Workspace(workspace) => {
                        row_ids.push(Some(workspace.id));
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
                            Line::from({
                                let mut spans = vec![ratatui::text::Span::styled(
                                    " ".to_string(),
                                    Style::default().fg(status_color),
                                )];
                                spans.extend(highlight_text_span(
                                    &line,
                                    Style::default().fg(status_color),
                                    &query,
                                    Style::default()
                                        .bg(Color::Rgb(64, 56, 0))
                                        .fg(Color::Yellow)
                                        .add_modifier(Modifier::BOLD),
                                ));
                                spans
                            }),
                            Line::from({
                                let mut spans = vec![ratatui::text::Span::styled(
                                    " ".to_string(),
                                    Style::default().fg(Color::DarkGray),
                                )];
                                spans.extend(highlight_text_span(
                                    &meta,
                                    Style::default().fg(Color::DarkGray),
                                    &query,
                                    Style::default()
                                        .bg(Color::Rgb(64, 56, 0))
                                        .fg(Color::Yellow)
                                        .add_modifier(Modifier::BOLD),
                                ));
                                spans
                            }),
                            Line::raw(""),
                        ]))
                    }
                })
                .collect::<Vec<_>>();
            self.workspace_list_cache = Some(WorkspaceListRenderCache {
                revision: self.workspace_list_revision,
                query,
                items,
                row_ids,
                new_workspace_row_index,
            });
        }

        self.workspace_list_cache
            .as_ref()
            .expect("workspace list cache should be populated")
    }

    fn selected_workspace_row_index_from_cache(
        &self,
        cache: &WorkspaceListRenderCache,
    ) -> Option<usize> {
        if self.creating_workspace {
            return cache.new_workspace_row_index;
        }
        let selected_id = self.selected_workspace_id?;
        cache
            .row_ids
            .iter()
            .position(|row_id| row_id.is_some_and(|row_id| row_id == selected_id))
    }

    pub(crate) fn render_workspace_list(&mut self, frame: &mut Frame, area: Rect) {
        let sections = if self
            .inline_search_prompt(SearchTarget::Workspaces)
            .is_some()
        {
            ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    ratatui::layout::Constraint::Length(3),
                    ratatui::layout::Constraint::Min(3),
                ])
                .split(area)
        } else {
            ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    ratatui::layout::Constraint::Length(0),
                    ratatui::layout::Constraint::Min(3),
                ])
                .split(area)
        };
        if let Some(prompt) = self.inline_search_prompt(SearchTarget::Workspaces) {
            frame.render_widget(
                Paragraph::new(prompt)
                    .block(panel_block(
                        "Workspace Filter",
                        self.focus == Focus::WorkspaceList,
                    ))
                    .wrap(Wrap { trim: false }),
                sections[0],
            );
        }

        let list_area = sections[1];
        let cache = self.workspace_list_cache().clone();

        let mut state = ListState::default();
        if let Some(index) = self.selected_workspace_row_index_from_cache(&cache) {
            state.select(Some(index));
        }

        let title = if self.workspace_project_filters.is_empty() {
            "Workspaces".to_string()
        } else {
            format!(
                "Workspaces [{} projects]",
                self.workspace_project_filters.len()
            )
        };
        let block = panel_block(&title, self.focus == Focus::WorkspaceList);
        let list = List::new(cache.items.clone()).block(block).highlight_style(
            Style::default()
                .bg(Color::Rgb(28, 38, 48))
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, list_area, &mut state);
        render_vertical_scrollbar(
            frame,
            list_area,
            cache.row_ids.len(),
            crate::app::viewport_capacity(list_area, 3),
            crate::app::selected_list_offset(
                self.selected_workspace_row_index_from_cache(&cache)
                    .unwrap_or(0),
                cache.row_ids.len(),
                crate::app::viewport_capacity(list_area, 3),
            ),
        );
    }
}
