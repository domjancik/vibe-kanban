use db::models::{execution_process::ExecutionProcess, session::Session};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{List, ListItem, ListState, Paragraph},
};

use crate::{
    app_state::App,
    editor::render_editor_buffer,
    model::{Focus, format_relative_time, workspace_title},
    ui::{panel_block, render_vertical_scrollbar},
    workspace::SessionRow,
};

impl App {
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
        frame.render_stateful_widget(
            List::new(sessions)
                .block(panel_block("Sessions", self.focus == Focus::Detail))
                .highlight_style(Style::default().fg(Color::Cyan).bg(Color::Rgb(28, 38, 48))),
            chunks[1],
            &mut state,
        );
        render_vertical_scrollbar(
            frame,
            chunks[1],
            session_rows.len(),
            crate::app::viewport_capacity(chunks[1], 2),
            crate::app::selected_list_offset(
                self.selected_session_row_index(&session_rows).unwrap_or(0),
                session_rows.len(),
                crate::app::viewport_capacity(chunks[1], 2),
            ),
        );

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
