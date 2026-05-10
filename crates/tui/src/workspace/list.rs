use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use db::models::workspace::WorkspaceWithStatus;
use ratatui::layout::Rect;
use uuid::Uuid;

use crate::{
    app::{App, SearchTarget, WorkspaceProjectFilter, WorkspaceProjectFilterPickerState},
    model::workspace_title,
    workspace::WorkspaceRow,
};

#[derive(Clone)]
pub(crate) struct WorkspaceProjectFilterOption {
    pub(crate) value: WorkspaceProjectFilter,
    pub(crate) label: String,
}

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
                if !self.workspace_matches_project_filters(workspace) {
                    return false;
                }
                if filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                let project = self
                    .summaries
                    .get(&workspace.id)
                    .and_then(|summary| summary.project_name.as_deref())
                    .unwrap_or("");
                title.contains(&filter)
                    || branch.contains(&filter)
                    || project.to_lowercase().contains(&filter)
            })
            .collect()
    }

    fn filtered_archived_workspaces(&self) -> Vec<&WorkspaceWithStatus> {
        let filter = self.workspace_filter_query();
        let mut workspaces = self
            .archived_workspaces
            .values()
            .filter(|workspace| {
                if !self.workspace_matches_project_filters(workspace) {
                    return false;
                }
                if filter.is_empty() {
                    return true;
                }
                let title = workspace_title(&workspace.workspace).to_lowercase();
                let branch = workspace.branch.to_lowercase();
                let project = self
                    .summaries
                    .get(&workspace.id)
                    .and_then(|summary| summary.project_name.as_deref())
                    .unwrap_or("");
                title.contains(&filter)
                    || branch.contains(&filter)
                    || project.to_lowercase().contains(&filter)
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

    fn workspace_matches_project_filters(&self, workspace: &WorkspaceWithStatus) -> bool {
        if self.workspace_project_filters.is_empty() {
            return true;
        }

        let workspace_project = self
            .summaries
            .get(&workspace.id)
            .and_then(|summary| summary.project_id)
            .map(WorkspaceProjectFilter::Project)
            .or_else(|| {
                workspace
                    .workspace
                    .task_id
                    .is_none()
                    .then_some(WorkspaceProjectFilter::NoProject)
            });

        workspace_project
            .as_ref()
            .is_some_and(|project| self.workspace_project_filters.contains(project))
    }

    pub(crate) fn workspace_project_filter_options(&self) -> Vec<WorkspaceProjectFilterOption> {
        let mut options = self
            .active_workspaces
            .values()
            .chain(self.archived_workspaces.values())
            .filter_map(|workspace| {
                self.summaries
                    .get(&workspace.id)
                    .and_then(|summary| summary.project_id.zip(summary.project_name.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .map(|(project_id, project_name)| WorkspaceProjectFilterOption {
                value: WorkspaceProjectFilter::Project(project_id),
                label: project_name,
            })
            .collect::<Vec<_>>();
        options.sort_by(|left, right| left.label.cmp(&right.label));

        let has_no_project = self
            .active_workspaces
            .values()
            .chain(self.archived_workspaces.values())
            .any(|workspace| workspace.workspace.task_id.is_none());
        if has_no_project {
            options.insert(
                0,
                WorkspaceProjectFilterOption {
                    value: WorkspaceProjectFilter::NoProject,
                    label: "No project".to_string(),
                },
            );
        }
        options
    }

    pub(crate) fn filtered_workspace_project_filter_options(
        &self,
    ) -> Vec<WorkspaceProjectFilterOption> {
        let query = self
            .workspace_project_filter_picker
            .as_ref()
            .map(|picker| picker.query.as_str())
            .unwrap_or("");
        self.workspace_project_filter_options()
            .into_iter()
            .filter(|option| crate::app::fuzzy_contains(query, &option.label))
            .collect()
    }

    pub(crate) fn open_workspace_project_filter_picker(&mut self) {
        let options = self.workspace_project_filter_options();
        if options.is_empty() {
            self.status = "No projects linked to workspaces".to_string();
            return;
        }
        let mut picker = WorkspaceProjectFilterPickerState {
            query: String::new(),
            selected: 0,
            staged_filters: self.workspace_project_filters.clone(),
        };
        if let Some(current) = picker.staged_filters.first()
            && let Some(index) = options.iter().position(|option| &option.value == current)
        {
            picker.selected = index;
        }
        self.workspace_project_filter_picker = Some(picker);
        self.status = "Filter workspaces by project".to_string();
        self.error = None;
    }

    pub(crate) fn handle_workspace_project_filter_picker_key(&mut self, key: KeyEvent, size: Rect) {
        let options_len = self.filtered_workspace_project_filter_options().len();
        let Some(picker) = self.workspace_project_filter_picker.as_mut() else {
            return;
        };

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.workspace_project_filter_picker = None;
                self.status = "Closed project filter".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.apply_workspace_project_filter(size);
            }
            KeyEvent {
                code: KeyCode::Char(' '),
                ..
            } => {
                self.toggle_workspace_project_filter_selection();
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if !picker.query.is_empty() {
                    picker.query.pop();
                    picker.selected = 0;
                }
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                if options_len > 0 {
                    picker.selected = (picker.selected + 1).min(options_len.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => {
                picker.selected = picker.selected.saturating_sub(1);
            }
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if options_len > 0 {
                    picker.selected = (picker.selected + 8).min(options_len.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => {
                picker.selected = picker.selected.saturating_sub(8);
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => picker.selected = 0,
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if options_len > 0 {
                    picker.selected = options_len.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT)
                && !modifiers.contains(KeyModifiers::SUPER) =>
            {
                picker.query.push(ch);
                picker.selected = 0;
            }
            _ => {}
        }
    }

    fn toggle_workspace_project_filter_selection(&mut self) {
        let options = self.filtered_workspace_project_filter_options();
        let Some(picker) = self.workspace_project_filter_picker.as_mut() else {
            return;
        };
        let Some(option) = options.get(picker.selected) else {
            return;
        };
        if let Some(index) = picker
            .staged_filters
            .iter()
            .position(|value| value == &option.value)
        {
            picker.staged_filters.remove(index);
        } else {
            picker.staged_filters.push(option.value.clone());
        }
    }

    fn apply_workspace_project_filter(&mut self, size: Rect) {
        let Some(picker) = self.workspace_project_filter_picker.take() else {
            return;
        };
        let previous = self.selected_workspace_id;
        self.workspace_project_filters = picker.staged_filters;
        self.mark_workspace_list_dirty();
        self.sync_workspace_selection_to_filter();
        if self.selected_workspace_id != previous {
            self.load_selected_workspace(size);
        }
        self.status = if self.workspace_project_filters.is_empty() {
            "Workspace project filter cleared".to_string()
        } else {
            format!(
                "Workspace project filters: {}",
                self.workspace_project_filters.len()
            )
        };
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
        rows.push(WorkspaceRow::NewWorkspace);
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

    #[cfg(test)]
    fn selected_workspace_row_index(&self, rows: &[WorkspaceRow<'_>]) -> Option<usize> {
        if self.creating_workspace {
            return rows
                .iter()
                .position(|row| matches!(row, WorkspaceRow::NewWorkspace));
        }
        let selected_id = self.selected_workspace_id?;
        rows.iter().position(|row| match row {
            WorkspaceRow::NewWorkspace => false,
            WorkspaceRow::Header(_) => false,
            WorkspaceRow::Workspace(workspace) => workspace.id == selected_id,
        })
    }

    pub(crate) fn visible_workspace_ids(&self) -> Vec<Uuid> {
        self.workspace_rows()
            .into_iter()
            .filter_map(|row| match row {
                WorkspaceRow::NewWorkspace => None,
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
        if self.creating_workspace {
            return;
        }
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
        app::{App, SearchPromptState, SearchTarget, WorkspaceProjectFilter},
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
            project_id: None,
            project_name: None,
            remote_project_id: None,
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
                WorkspaceRow::NewWorkspace => None,
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
                    WorkspaceRow::NewWorkspace => None,
                    WorkspaceRow::Header(_) => None,
                    WorkspaceRow::Workspace(workspace) => Some(workspace.id),
                })
                .collect::<Vec<_>>(),
            vec![archived_id]
        );
    }

    #[test]
    fn workspace_rows_respect_project_filters() {
        let mut app = test_app();
        let alpha = workspace("alpha", false, false, 1);
        let beta = workspace("beta", false, false, 2);
        let orphan = workspace("orphan", false, false, 3);
        let alpha_id = alpha.id;
        let beta_id = beta.id;
        let orphan_id = orphan.id;

        app.active_workspaces.insert(alpha_id, alpha);
        app.active_workspaces.insert(beta_id, beta);
        let mut orphan_workspace = orphan;
        orphan_workspace.workspace.task_id = None;
        app.active_workspaces.insert(orphan_id, orphan_workspace);

        let alpha_project = Uuid::new_v4();
        let beta_project = Uuid::new_v4();
        app.summaries.insert(
            alpha_id,
            WorkspaceSummary {
                workspace_id: alpha_id,
                project_id: Some(alpha_project),
                project_name: Some("Alpha".to_string()),
                remote_project_id: None,
                latest_session_id: None,
                has_pending_approval: false,
                files_changed: None,
                lines_added: None,
                lines_removed: None,
                latest_process_completed_at: None,
                latest_process_status: None,
                has_running_dev_server: false,
                has_unseen_turns: false,
                pr_status: None,
                pr_number: None,
                pr_url: None,
            },
        );
        app.summaries.insert(
            beta_id,
            WorkspaceSummary {
                workspace_id: beta_id,
                project_id: Some(beta_project),
                project_name: Some("Beta".to_string()),
                remote_project_id: None,
                latest_session_id: None,
                has_pending_approval: false,
                files_changed: None,
                lines_added: None,
                lines_removed: None,
                latest_process_completed_at: None,
                latest_process_status: None,
                has_running_dev_server: false,
                has_unseen_turns: false,
                pr_status: None,
                pr_number: None,
                pr_url: None,
            },
        );

        app.workspace_project_filters = vec![WorkspaceProjectFilter::Project(alpha_project)];
        let visible = app.visible_workspace_ids();
        assert_eq!(visible, vec![alpha_id]);

        app.workspace_project_filters = vec![WorkspaceProjectFilter::NoProject];
        let visible = app.visible_workspace_ids();
        assert_eq!(visible, vec![orphan_id]);
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
                WorkspaceRow::NewWorkspace => None,
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
