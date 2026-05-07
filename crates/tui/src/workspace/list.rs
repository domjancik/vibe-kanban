use db::models::workspace::WorkspaceWithStatus;
use uuid::Uuid;

use crate::{
    app::{App, SearchTarget},
    model::workspace_title,
    workspace::WorkspaceRow,
};

impl App {
    fn workspace_filter_query(&self) -> String {
        self.active_search_query_for(SearchTarget::Workspaces)
            .unwrap_or("")
            .trim()
            .to_lowercase()
    }

    fn all_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let mut workspaces = self.active_workspaces.values().collect::<Vec<_>>();
        workspaces.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        workspaces
    }

    fn filtered_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let filter = self.workspace_filter_query();
        self.all_workspaces()
            .into_iter()
            .filter(|workspace| {
                if filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                title.contains(&filter) || branch.contains(&filter)
            })
            .collect()
    }

    fn filtered_archived_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let filter = self.workspace_filter_query();
        let mut workspaces = self
            .archived_workspaces
            .values()
            .filter(|workspace| {
                if filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                title.contains(&filter) || branch.contains(&filter)
            })
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        workspaces
    }

    pub(crate) fn workspace_rows(&self) -> Vec<WorkspaceRow<'_>> {
        let active = self.filtered_workspaces();
        let mut needs_attention = Vec::new();
        let mut running = Vec::new();
        let mut idle = Vec::new();

        for workspace in active {
            let summary = self.summaries.get(&workspace.id);
            let needs_attention_bucket = summary
                .is_some_and(|summary| summary.has_pending_approval || summary.has_unseen_turns);
            if needs_attention_bucket {
                needs_attention.push(workspace);
            } else if workspace.is_running
                || summary.is_some_and(|summary| summary.has_running_dev_server)
            {
                running.push(workspace);
            } else {
                idle.push(workspace);
            }
        }

        let mut rows = Vec::new();
        self.push_workspace_group(&mut rows, "Needs Attention", &needs_attention);
        self.push_workspace_group(&mut rows, "Running", &running);
        self.push_workspace_group(&mut rows, "Idle", &idle);

        if self.show_archived {
            let archived = self.filtered_archived_workspaces();
            self.push_workspace_group(&mut rows, "Archived", &archived);
        }

        rows
    }

    fn push_workspace_group<'a>(
        &self,
        rows: &mut Vec<WorkspaceRow<'a>>,
        title: &'static str,
        workspaces: &[&'a WorkspaceWithStatus],
    ) {
        if workspaces.is_empty() {
            return;
        }
        rows.push(WorkspaceRow::Header(title));
        rows.extend(workspaces.iter().copied().map(WorkspaceRow::Workspace));
    }

    pub(crate) fn selected_workspace_row_index(&self, rows: &[WorkspaceRow<'_>]) -> Option<usize> {
        let selected_id = self.selected_workspace_id?;
        rows.iter().position(|row| match row {
            WorkspaceRow::Header(_) => false,
            WorkspaceRow::Workspace(workspace) => workspace.id == selected_id,
        })
    }

    pub(crate) fn visible_workspace_ids(&self) -> Vec<Uuid> {
        self.workspace_rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspaceRow::Header(_) => None,
                WorkspaceRow::Workspace(workspace) => Some(workspace.id),
            })
            .collect()
    }

    pub(crate) fn find_workspace(&self, workspace_id: Uuid) -> Option<&WorkspaceWithStatus> {
        self.active_workspaces
            .get(&workspace_id)
            .or_else(|| self.archived_workspaces.get(&workspace_id))
    }

    pub(crate) fn sync_workspace_selection_to_filter(&mut self) {
        let visible = self.visible_workspace_ids();
        if visible.is_empty() {
            return;
        }
        if self
            .selected_workspace_id
            .is_some_and(|selected| visible.contains(&selected))
        {
            return;
        }
        self.selected_workspace_id = visible.first().copied();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::workspace::{Workspace, WorkspaceWithStatus};
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{App, SearchPromptState, SearchTarget},
        editor::ComposerEditorMode,
        model::{Focus, Pane, QueueStatus, WorkspaceBundle, WorkspaceSummary},
        workspace::WorkspaceRow,
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
            focus: Focus::WorkspaceList,
            maximized_panel: false,
            show_archived: false,
            filter: String::new(),
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

    fn workspace(name: &str, pinned: bool, archived: bool, second: i64) -> WorkspaceWithStatus {
        let created_at = Utc.timestamp_opt(second, 0).unwrap();
        WorkspaceWithStatus {
            workspace: Workspace {
                id: Uuid::new_v4(),
                task_id: None,
                container_ref: Some(format!("container-{name}")),
                branch: format!("{name}-branch"),
                setup_completed_at: None,
                created_at,
                updated_at: created_at,
                archived,
                pinned,
                name: Some(name.to_string()),
                worktree_deleted: false,
            },
            is_running: false,
            is_errored: false,
        }
    }

    fn summary(
        workspace_id: Uuid,
        pending: bool,
        dev_server: bool,
        unseen_turns: bool,
    ) -> WorkspaceSummary {
        WorkspaceSummary {
            workspace_id,
            latest_session_id: None,
            has_pending_approval: pending,
            files_changed: None,
            lines_added: None,
            lines_removed: None,
            latest_process_completed_at: None,
            latest_process_status: None,
            has_running_dev_server: dev_server,
            has_unseen_turns: unseen_turns,
            pr_status: None,
            pr_number: None,
            pr_url: None,
        }
    }

    #[test]
    fn workspace_rows_group_and_filter_visible_entries() {
        let mut app = test_app();
        let attention = workspace("attention", true, false, 1);
        let running = workspace("running", false, false, 2);
        let idle = workspace("idle", false, false, 3);
        let archived = workspace("archived", false, true, 4);

        let attention_id = attention.id;
        let running_id = running.id;
        let idle_id = idle.id;
        let archived_id = archived.id;

        app.active_workspaces
            .insert(attention_id, attention.clone());
        app.active_workspaces.insert(running_id, running.clone());
        app.active_workspaces.insert(idle_id, idle.clone());
        app.archived_workspaces
            .insert(archived_id, archived.clone());
        app.summaries
            .insert(attention_id, summary(attention_id, true, false, false));
        app.summaries
            .insert(running_id, summary(running_id, false, true, false));
        app.show_archived = true;

        let rows = app.workspace_rows();
        let headers = rows
            .iter()
            .filter_map(|row| match row {
                WorkspaceRow::Header(title) => Some(*title),
                WorkspaceRow::Workspace(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            headers,
            vec!["Needs Attention", "Running", "Idle", "Archived"]
        );
        assert_eq!(
            app.visible_workspace_ids(),
            vec![attention_id, running_id, idle_id, archived_id]
        );

        app.filter = "arch".to_string();
        let filtered = app.workspace_rows();
        assert_eq!(
            filtered
                .into_iter()
                .filter_map(|row| match row {
                    WorkspaceRow::Header(_) => None,
                    WorkspaceRow::Workspace(workspace) => Some(workspace.id),
                })
                .collect::<Vec<_>>(),
            vec![archived_id]
        );
    }

    #[test]
    fn pinned_workspaces_sort_ahead_within_their_group() {
        let mut app = test_app();
        let newer_unpinned = workspace("newer", false, false, 20);
        let older_pinned = workspace("older", true, false, 10);
        app.active_workspaces
            .insert(newer_unpinned.id, newer_unpinned.clone());
        app.active_workspaces
            .insert(older_pinned.id, older_pinned.clone());

        let visible = app
            .workspace_rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspaceRow::Header(_) => None,
                WorkspaceRow::Workspace(workspace) => Some(workspace.id),
            })
            .collect::<Vec<_>>();

        assert_eq!(visible, vec![older_pinned.id, newer_unpinned.id]);
    }

    #[test]
    fn workspace_rows_preview_inline_filter_without_changing_selection() {
        let mut app = test_app();
        let alpha = workspace("alpha", false, false, 1);
        let beta = workspace("beta", false, false, 2);
        app.active_workspaces.insert(alpha.id, alpha.clone());
        app.active_workspaces.insert(beta.id, beta.clone());
        app.selected_workspace_id = Some(alpha.id);
        app.search_prompt = Some(SearchPromptState {
            target: SearchTarget::Workspaces,
            query: "beta".to_string(),
            cursor: 4,
            original_query: String::new(),
            original_conversation_search: None,
            original_chat_end_offset: 0,
        });

        let rows = app.workspace_rows();

        assert!(app.filter.is_empty());
        assert_eq!(app.visible_workspace_ids(), vec![beta.id]);
        assert_eq!(app.selected_workspace_id, Some(alpha.id));
        assert!(app.selected_workspace_row_index(&rows).is_none());
    }
}
