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
            match self.compact_secondary_region() {
                FocusedRegion::Main => self.render_main(frame, body[1]),
                FocusedRegion::Detail => self.render_detail(frame, body[1]),
                FocusedRegion::WorkspaceList => self.render_main(frame, body[1]),
            }
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

    fn compact_secondary_region(&self) -> FocusedRegion {
        match self.focused_region() {
            FocusedRegion::Detail => FocusedRegion::Detail,
            FocusedRegion::WorkspaceList | FocusedRegion::Main => FocusedRegion::Main,
        }
    }
}

#[derive(Clone, Copy)]
enum FocusedRegion {
    WorkspaceList,
    Main,
    Detail,
}

#[cfg(test)]
mod tests {
    use super::FocusedRegion;
    use crate::{app::App, model::Focus};

    fn test_app() -> App {
        let api = crate::api::Api::new("http://127.0.0.1:9".to_string()).unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        App {
            api,
            rx,
            tx,
            workspace_streams: Vec::new(),
            summary_streams: Vec::new(),
            subscriptions: crate::api::WorkspaceSubscriptions::default(),
            active_workspaces: std::collections::HashMap::new(),
            archived_workspaces: std::collections::HashMap::new(),
            summaries: std::collections::HashMap::new(),
            selected_workspace_id: None,
            selected_pane: crate::model::Pane::Chat,
            focus: Focus::Main,
            detail_section: crate::app::DetailSection::Sessions,
            maximized_panel: false,
            show_archived: false,
            filter: String::new(),
            workspace_project_filters: Vec::new(),
            session_filter: String::new(),
            workspace_list_revision: 0,
            detail_revision: 0,
            workspace_list_cache: None,
            detail_pane_cache: None,
            status: String::new(),
            error: None,
            bundle: crate::model::WorkspaceBundle::default(),
            executor_profiles: executors::profile::ExecutorConfigs {
                executors: std::collections::HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: None,
            composer_options: None,
            composer: String::new(),
            composer_snippets: Vec::new(),
            composer_cursor: 0,
            editor_mode: crate::editor::ComposerEditorMode::Standard,
            vim_pending_operator: None,
            composer_dirty: false,
            composer_edit_revision: 0,
            composer_height_cache: None,
            draft_save_in_flight: false,
            composer_queue_conflict: false,
            composer_scratch_id: None,
            composer_scratch_loaded: false,
            queue_session_id: None,
            queue_status: crate::model::QueueStatus::Empty,
            queue_pending: false,
            last_composer_edit: None,
            chat_end_offset: 0,
            chat_render_cache: None,
            chat_render_cache_dirty: true,
            last_chat_render_cache_build: None,
            conversation_loader: None,
            conversation_process_entries: std::collections::HashMap::new(),
            conversation_process_order: Vec::new(),
            conversation_bootstrapping: false,
            conversation_backfilling: false,
            current_todos: None,
            selected_todo_index: 0,
            optimistic_entries: Vec::new(),
            notes_cursor: 0,
            notes_edit_revision: 0,
            notes_save_in_flight: false,
            agent_picker: None,
            workspace_project_filter_picker: None,
            workspace_create: None,
            workspace_create_repo_picker: None,
            workspace_create_branch_picker: None,
            session_rename: None,
            pr_create: None,
            snippet_preview: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: crate::app::ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            creating_workspace: false,
            workspace_create_previous_selection: None,
            creating_new_session: false,
            should_quit: false,
        }
    }

    #[test]
    fn compact_layout_shows_detail_when_detail_is_focused() {
        let mut app = test_app();
        app.focus = Focus::Detail;
        assert!(matches!(
            app.compact_secondary_region(),
            FocusedRegion::Detail
        ));
    }

    #[test]
    fn compact_layout_shows_main_for_workspace_and_composer_focus() {
        let mut app = test_app();
        app.focus = Focus::WorkspaceList;
        assert!(matches!(
            app.compact_secondary_region(),
            FocusedRegion::Main
        ));
        app.focus = Focus::Composer;
        assert!(matches!(
            app.compact_secondary_region(),
            FocusedRegion::Main
        ));
    }
}
