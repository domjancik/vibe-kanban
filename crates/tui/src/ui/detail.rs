use db::models::{execution_process::ExecutionProcess, session::Session};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{List, ListItem, ListState, Paragraph},
};

use crate::{
    app::App,
    editor::render_editor_buffer,
    model::{Focus, format_relative_time, workspace_title},
    ui::{panel_block, render_vertical_scrollbar},
    workspace::SessionRow,
};

impl App {
    fn session_row_height(&self, row: &SessionRow<'_>) -> usize {
        match row {
            SessionRow::NewSession => 2,
            SessionRow::Session(session)
                if self
                    .session_rename
                    .as_ref()
                    .is_some_and(|rename| rename.session_id == session.id) =>
            {
                3
            }
            SessionRow::Session(_) => 2,
        }
    }

    fn session_scroll_metrics(&self, rows: &[SessionRow<'_>], area: Rect) -> (usize, usize, usize) {
        let viewport_lines = area.height.saturating_sub(2).max(1) as usize;
        let total_lines = rows
            .iter()
            .map(|row| self.session_row_height(row))
            .sum::<usize>();
        let selected_index = self.selected_session_row_index(rows).unwrap_or(0);

        let mut top_index = selected_index.min(rows.len().saturating_sub(1));
        let mut used_lines = rows
            .get(top_index)
            .map(|row| self.session_row_height(row))
            .unwrap_or(0);

        while top_index > 0 {
            let next_height = self.session_row_height(&rows[top_index - 1]);
            if used_lines + next_height > viewport_lines {
                break;
            }
            top_index -= 1;
            used_lines += next_height;
        }

        let offset_lines = rows[..top_index]
            .iter()
            .map(|row| self.session_row_height(row))
            .sum::<usize>();

        (total_lines, viewport_lines, offset_lines)
    }

    pub(crate) fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Min(7),
            ])
            .split(area);

        let workspace_info = if let Some(workspace) = &self.bundle.workspace {
            vec![
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
            ]
        } else {
            vec![Line::raw("No workspace selected")]
        };
        frame.render_widget(
            Paragraph::new(Text::from(workspace_info)).block(panel_block("Workspace", false)),
            chunks[0],
        );

        let session_rows = self.session_rows();
        let sessions = session_rows
            .iter()
            .map(|row| match row {
                SessionRow::NewSession => ListItem::new(Text::from(vec![
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
                ])),
                SessionRow::Session(session) => self.render_session_row(session),
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if let Some(index) = self.selected_session_row_index(&session_rows) {
            state.select(Some(index));
        }
        let sessions_title = if self.session_filter.is_empty() {
            "Sessions".to_string()
        } else {
            format!("Sessions / {}", self.session_filter)
        };
        frame.render_stateful_widget(
            List::new(sessions)
                .block(panel_block(&sessions_title, self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            chunks[1],
            &mut state,
        );
        let (total_lines, viewport_lines, offset_lines) =
            self.session_scroll_metrics(&session_rows, chunks[1]);
        render_vertical_scrollbar(frame, chunks[1], total_lines, viewport_lines, offset_lines);

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

    fn render_session_row(&self, session: &Session) -> ListItem<'static> {
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
            Line::raw(name),
            Line::styled(executor, Style::default().fg(Color::DarkGray)),
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
