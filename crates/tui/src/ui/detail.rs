use db::models::{execution_process::ExecutionProcess, session::Session};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{App, DetailPaneRenderCache, SearchTarget, highlight_text_span},
    editor::render_editor_buffer,
    model::{Focus, format_relative_time, workspace_title},
    ui::{panel_block, render_vertical_scrollbar},
    workspace::SessionRow,
};

impl App {
    fn session_row_height_for_id(&self, session_id: Option<uuid::Uuid>) -> usize {
        if session_id.is_some_and(|session_id| {
            self.session_rename
                .as_ref()
                .is_some_and(|rename| rename.session_id == session_id)
        }) {
            3
        } else {
            2
        }
    }

    fn detail_pane_cache(&mut self) -> &DetailPaneRenderCache {
        let session_query = self
            .active_search_query_for(SearchTarget::Sessions)
            .unwrap_or("")
            .to_string();
        let renaming_session_id = self.session_rename.as_ref().map(|rename| rename.session_id);
        let needs_rebuild = self
            .detail_pane_cache
            .as_ref()
            .is_none_or(|cache| {
                cache.revision != self.detail_revision
                    || cache.session_query != session_query
                    || cache.renaming_session_id != renaming_session_id
            });
        if needs_rebuild {
            let workspace_info = if let Some(workspace) = &self.bundle.workspace {
                Text::from(vec![
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
                ])
            } else {
                Text::from(vec![Line::raw("No workspace selected")])
            };

            let session_rows = self.session_rows();
            let mut session_ids = Vec::with_capacity(session_rows.len());
            let session_items = session_rows
                .iter()
                .map(|row| match row {
                    SessionRow::NewSession => {
                        session_ids.push(None);
                        ListItem::new(Text::from(vec![
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
                        ]))
                    }
                    SessionRow::Session(session) => {
                        session_ids.push(Some(session.id));
                        self.render_session_row(session, &session_query)
                    }
                })
                .collect::<Vec<_>>();

            let mut processes = self
                .bundle
                .process_map
                .values()
                .cloned()
                .collect::<Vec<ExecutionProcess>>();
            processes.sort_by(|left, right| right.created_at.cmp(&left.created_at));
            let process_items = processes
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

            self.detail_pane_cache = Some(DetailPaneRenderCache {
                revision: self.detail_revision,
                session_query,
                renaming_session_id,
                workspace_info,
                session_items,
                session_ids,
                process_items,
            });
        }

        self.detail_pane_cache
            .as_ref()
            .expect("detail pane cache should be populated")
    }

    fn selected_session_row_index_from_ids(&self, session_ids: &[Option<uuid::Uuid>]) -> Option<usize> {
        if self.creating_new_session {
            return Some(0);
        }
        let selected = self.bundle.selected_session_id?;
        session_ids
            .iter()
            .position(|session_id| session_id.is_some_and(|session_id| session_id == selected))
    }

    fn session_scroll_metrics(&self, session_ids: &[Option<uuid::Uuid>], area: Rect) -> (usize, usize, usize) {
        let viewport_lines = area.height.saturating_sub(2).max(1) as usize;
        let total_lines = session_ids
            .iter()
            .map(|session_id| self.session_row_height_for_id(*session_id))
            .sum::<usize>();
        let selected_index = self.selected_session_row_index_from_ids(session_ids).unwrap_or(0);

        let mut top_index = selected_index.min(session_ids.len().saturating_sub(1));
        let mut used_lines = session_ids
            .get(top_index)
            .map(|session_id| self.session_row_height_for_id(*session_id))
            .unwrap_or(0);

        while top_index > 0 {
            let next_height = self.session_row_height_for_id(session_ids[top_index - 1]);
            if used_lines + next_height > viewport_lines {
                break;
            }
            top_index -= 1;
            used_lines += next_height;
        }

        let offset_lines = session_ids[..top_index]
            .iter()
            .map(|session_id| self.session_row_height_for_id(*session_id))
            .sum::<usize>();

        (total_lines, viewport_lines, offset_lines)
    }

    pub(crate) fn render_detail(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Min(7),
            ])
            .split(area);

        let cache = self.detail_pane_cache();
        frame.render_widget(
            Paragraph::new(cache.workspace_info.clone()).block(panel_block("Workspace", false)),
            chunks[0],
        );

        let session_sections = if self.inline_search_prompt(SearchTarget::Sessions).is_some() {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(3)])
                .split(chunks[1])
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(0), Constraint::Min(3)])
                .split(chunks[1])
        };
        if let Some(prompt) = self.inline_search_prompt(SearchTarget::Sessions) {
            frame.render_widget(
                Paragraph::new(prompt)
                    .block(panel_block("Session Filter", self.focus == Focus::Detail))
                    .wrap(Wrap { trim: false }),
                session_sections[0],
            );
        }

        let mut state = ListState::default();
        if let Some(index) = self.selected_session_row_index_from_ids(&cache.session_ids) {
            state.select(Some(index));
        }
        frame.render_stateful_widget(
            List::new(cache.session_items.clone())
                .block(panel_block("Sessions", self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            session_sections[1],
            &mut state,
        );
        let (total_lines, viewport_lines, offset_lines) =
            self.session_scroll_metrics(&cache.session_ids, session_sections[1]);
        render_vertical_scrollbar(
            frame,
            session_sections[1],
            total_lines,
            viewport_lines,
            offset_lines,
        );

        frame.render_widget(
            List::new(cache.process_items.clone()).block(panel_block("Processes", false)),
            chunks[2],
        );
    }

    fn render_session_row(&self, session: &Session, query: &str) -> ListItem<'static> {
        let is_renaming = self
            .session_rename
            .as_ref()
            .is_some_and(|rename| rename.session_id == session.id);

        if is_renaming && let Some(rename) = self.session_rename.as_ref() {
            let mut lines = vec![
                Line::styled(
                    "rename",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "Enter save  Esc cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            lines.extend(
                render_editor_buffer(&rename.name, rename.cursor, true, self.editor_mode).lines,
            );
            return ListItem::new(Text::from(lines));
        }

        let name = session
            .name
            .clone()
            .unwrap_or_else(|| session.id.to_string());
        let executor = session
            .executor
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ListItem::new(Text::from(vec![
            Line::from(highlight_text_span(
                &name,
                Style::default(),
                query,
                Style::default()
                    .bg(Color::Rgb(64, 56, 0))
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from({
                let mut spans = Vec::new();
                spans.extend(highlight_text_span(
                    &executor,
                    Style::default().fg(Color::DarkGray),
                    query,
                    Style::default()
                        .bg(Color::Rgb(64, 56, 0))
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!("  {}", session.id),
                    Style::default().fg(Color::DarkGray),
                ));
                spans
            }),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::session::Session;
    use ratatui::layout::Rect;
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{App, SessionRenameState},
        editor::ComposerEditorMode,
        model::{Focus, Pane, QueueStatus, WorkspaceBundle},
        workspace::SessionRow,
    };

    fn test_app() -> App {
        let api = Api::new("http://127.0.0.1:9".to_string()).unwrap();
        let (tx, rx) = unbounded_channel();
        App {
            api,
            rx,
            tx,
            workspace_streams: Vec::new(),
            summary_streams: Vec::new(),
            subscriptions: WorkspaceSubscriptions::default(),
            active_workspaces: HashMap::new(),
            archived_workspaces: HashMap::new(),
            summaries: HashMap::new(),
            selected_workspace_id: None,
            selected_pane: Pane::Chat,
            focus: Focus::Detail,
            maximized_panel: false,
            show_archived: false,
            filter: String::new(),
            session_filter: String::new(),
            status: String::new(),
            error: None,
            bundle: WorkspaceBundle::default(),
            executor_profiles: executors::profile::ExecutorConfigs {
                executors: HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: None,
            composer_options: None,
            composer: String::new(),
            composer_cursor: 0,
            editor_mode: ComposerEditorMode::Standard,
            vim_pending_operator: None,
            composer_dirty: false,
            composer_edit_revision: 0,
            composer_height_cache: None,
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
            search_prompt: None,
            conversation_search: None,
            actions_in_flight: Default::default(),
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn session(id: Uuid, name: &str) -> Session {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        Session {
            id,
            workspace_id: Uuid::new_v4(),
            name: Some(name.to_string()),
            executor: Some("CODEX".to_string()),
            agent_working_dir: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn session_row_height_only_expands_for_active_rename_row() {
        let first = session(Uuid::new_v4(), "first");
        let second = session(Uuid::new_v4(), "second");
        let mut app = test_app();
        app.session_rename = Some(SessionRenameState {
            session_id: second.id,
            name: "renaming".to_string(),
            cursor: 8,
        });

        assert_eq!(app.session_row_height(&SessionRow::NewSession), 2);
        assert_eq!(app.session_row_height(&SessionRow::Session(&first)), 2);
        assert_eq!(app.session_row_height(&SessionRow::Session(&second)), 3);
    }

    #[test]
    fn session_scroll_metrics_accounts_for_taller_rename_row() {
        let sessions = [
            session(Uuid::new_v4(), "one"),
            session(Uuid::new_v4(), "two"),
            session(Uuid::new_v4(), "three"),
        ];
        let mut app = test_app();
        app.bundle.selected_session_id = Some(sessions[1].id);
        app.session_rename = Some(SessionRenameState {
            session_id: sessions[1].id,
            name: "rename".to_string(),
            cursor: 6,
        });

        let rows = vec![
            SessionRow::NewSession,
            SessionRow::Session(&sessions[0]),
            SessionRow::Session(&sessions[1]),
            SessionRow::Session(&sessions[2]),
        ];

        let metrics = app.session_scroll_metrics(&rows, Rect::new(0, 0, 40, 6));

        assert_eq!(metrics, (9, 4, 4));
    }
}
