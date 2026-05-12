use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{app::App, model::workspace_title};

impl App {
    pub(crate) fn render(&mut self, frame: &mut Frame) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(frame.area());

        frame.render_widget(self.header(), outer[0]);

        if self.maximized_panel {
            match self.focused_region() {
                FocusedRegion::WorkspaceList => self.render_workspace_list(frame, outer[1]),
                FocusedRegion::Main => self.render_main(frame, outer[1]),
                FocusedRegion::Detail => self.render_detail(frame, outer[1]),
            }
        } else if frame.area().width >= 140 {
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
        if self.workspace_project_filter_picker.is_some() {
            self.render_workspace_project_filter_picker(frame, frame.area());
        }
        if self.workspace_create_repo_picker.is_some() {
            self.render_workspace_create_repo_picker(frame, frame.area());
        }
        if self.workspace_create_branch_picker.is_some() {
            self.render_workspace_create_branch_picker(frame, frame.area());
        }
        if self.pr_create.is_some() {
            self.render_pr_create_modal(frame, frame.area());
        }
        if self.snippet_preview.is_some() {
            self.render_snippet_preview(frame, frame.area());
        }
    }

    fn header(&self) -> Paragraph<'_> {
        let workspace = self
            .selected_workspace_id
            .and_then(|id| self.find_workspace(id))
            .map(|workspace| workspace_title(&workspace.workspace))
            .unwrap_or_else(|| {
                if self.creating_workspace {
                    "New Workspace".to_string()
                } else {
                    "No workspace".to_string()
                }
            });
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

    fn focused_region(&self) -> FocusedRegion {
        match self.focus {
            crate::model::Focus::WorkspaceList => FocusedRegion::WorkspaceList,
            crate::model::Focus::Main | crate::model::Focus::Composer => FocusedRegion::Main,
            crate::model::Focus::Detail => FocusedRegion::Detail,
        }
    }

    pub(crate) fn active_region_label(&self) -> &'static str {
        match self.focused_region() {
            FocusedRegion::WorkspaceList => "workspace list",
            FocusedRegion::Main => "main pane",
            FocusedRegion::Detail => "detail pane",
        }
    }
}

#[derive(Clone, Copy)]
enum FocusedRegion {
    WorkspaceList,
    Main,
    Detail,
}
