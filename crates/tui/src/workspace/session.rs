use db::models::session::Session;
use ratatui::layout::Rect;

use crate::{
    app::{App, DetailSection, SearchTarget},
    workspace::{SessionRow, session_target},
};

impl App {
    fn filtered_sessions(&self) -> Vec<&Session> {
        let filter = self
            .active_search_query_for(SearchTarget::Sessions)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        self.bundle
            .sessions
            .iter()
            .filter(|session| {
                if filter.is_empty() {
                    return true;
                }
                let name = session.name.as_deref().unwrap_or_default().to_lowercase();
                let executor = session
                    .executor
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase();
                let id = session.id.to_string();
                name.contains(&filter) || executor.contains(&filter) || id.contains(&filter)
            })
            .collect()
    }

    pub(crate) fn switch_session_or_process(&mut self, size: Rect) {
        let _ = size;
        if self.detail_section == DetailSection::Todos {
            return;
        }
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows).unwrap_or(0);
        if let Some(row) = rows.get(index)
            && let Some(target) = session_target(row)
        {
            self.select_session_target(target);
        }
    }

    pub(crate) fn session_rows(&self) -> Vec<SessionRow<'_>> {
        let filtered_sessions = self.filtered_sessions();
        let mut rows = Vec::with_capacity(filtered_sessions.len() + 1);
        rows.push(SessionRow::NewSession);
        rows.extend(filtered_sessions.into_iter().map(SessionRow::Session));
        rows
    }

    pub(crate) fn selected_session_row_index(&self, rows: &[SessionRow<'_>]) -> Option<usize> {
        if self.creating_new_session {
            return Some(0);
        }
        let selected = self.bundle.selected_session_id?;
        rows.iter().position(|row| match row {
            SessionRow::NewSession => false,
            SessionRow::Session(session) => session.id == selected,
        })
    }

    pub(crate) fn selected_session_for_rename(&self) -> Option<&Session> {
        let rows = self.session_rows();
        let index = self.selected_session_row_index(&rows)?;
        match rows.get(index)? {
            SessionRow::Session(session) => Some(session),
            SessionRow::NewSession => None,
        }
    }

    pub(crate) fn current_session(&self) -> Option<&Session> {
        self.bundle
            .selected_session_id
            .and_then(|id| self.bundle.sessions.iter().find(|session| session.id == id))
    }

    pub(crate) fn sync_session_selection_to_filter(&mut self) {
        if self.creating_new_session {
            return;
        }
        let rows = self.session_rows();
        if self.selected_session_row_index(&rows).is_some() {
            return;
        }
        if let Some(session_id) = rows.into_iter().find_map(|row| match row {
            SessionRow::Session(session) => Some(session.id),
            SessionRow::NewSession => None,
        }) {
            self.bundle.selected_session_id = Some(session_id);
            self.rebind_session_streams();
            self.rebind_discovery_stream();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{App, SearchPromptState, SearchTarget},
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
            bundle: WorkspaceBundle::default(),
            executor_profiles: executors::profile::ExecutorConfigs {
                executors: HashMap::new(),
            },
            default_executor_profile: None,
            composer_config: None,
            composer_options: None,
            composer: String::new(),
            composer_snippets: Vec::new(),
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
            current_todos: None,
            selected_todo_index: 0,
            optimistic_entries: Vec::new(),
            notes_cursor: 0,
            notes_edit_revision: 0,
            notes_save_in_flight: false,
            agent_picker: None,
            workspace_project_filter_picker: None,
            session_rename: None,
            pr_create: None,
            snippet_preview: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: crate::app::ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            workspace_create: None,
            workspace_create_repo_picker: None,
            workspace_create_branch_picker: None,
            creating_workspace: false,
            workspace_create_previous_selection: None,
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn session(id: Uuid, name: &str) -> db::models::session::Session {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        db::models::session::Session {
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
    fn selected_session_row_index_tracks_new_session_and_existing_rows() {
        let first = session(Uuid::new_v4(), "first");
        let second = session(Uuid::new_v4(), "second");
        let rows = vec![
            SessionRow::NewSession,
            SessionRow::Session(&first),
            SessionRow::Session(&second),
        ];

        let mut app = test_app();
        app.creating_new_session = true;
        assert_eq!(app.selected_session_row_index(&rows), Some(0));

        app.creating_new_session = false;
        app.bundle.selected_session_id = Some(second.id);
        assert_eq!(app.selected_session_row_index(&rows), Some(2));
    }

    #[test]
    fn session_rows_preview_inline_filter_without_switching_active_session() {
        let first = session(Uuid::new_v4(), "alpha");
        let second = session(Uuid::new_v4(), "beta");

        let mut app = test_app();
        app.bundle.sessions = vec![first.clone(), second.clone()];
        app.bundle.selected_session_id = Some(first.id);
        app.search_prompt = Some(SearchPromptState {
            target: SearchTarget::Sessions,
            query: "beta".to_string(),
            cursor: 4,
            original_query: String::new(),
            original_conversation_search: None,
            original_chat_end_offset: 0,
        });

        let rows = app.session_rows();

        assert!(app.session_filter.is_empty());
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], SessionRow::NewSession));
        assert!(matches!(rows[1], SessionRow::Session(session) if session.id == second.id));
        assert_eq!(app.bundle.selected_session_id, Some(first.id));
        assert!(app.selected_session_row_index(&rows).is_none());
    }
}
