use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use db::models::{
    repo::Repo,
    scratch::{DraftWorkspaceData, DraftWorkspaceRepo},
};
use ratatui::layout::Rect;

use crate::{
    app::{
        App, fuzzy_contains,
        state::{
            WorkspaceCreateBranchPickerState, WorkspaceCreateRepoPickerState,
            WorkspaceCreateSelectedRepo, WorkspaceCreateState,
        },
    },
    model::{Focus, NetEvent, Pane},
};

impl App {
    pub(crate) fn enter_workspace_create_mode(&mut self) {
        if self.creating_workspace {
            return;
        }
        self.workspace_create_previous_selection = self.selected_workspace_id;
        self.creating_workspace = true;
        self.creating_new_session = false;
        self.selected_workspace_id = None;
        self.selected_pane = Pane::Chat;
        self.focus = Focus::Detail;
        self.session_rename = None;
        self.search_prompt = None;
        self.conversation_search = None;
        self.workspace_create_repo_picker = None;
        self.workspace_create_branch_picker = None;
        self.subscriptions.abort();
        self.bundle = crate::model::WorkspaceBundle::default();
        self.reset_conversation_state();
        self.composer.clear();
        self.invalidate_composer_layout_cache();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.draft_save_in_flight = false;
        self.last_composer_edit = None;
        self.composer_scratch_loaded = false;
        self.workspace_create = Some(WorkspaceCreateState {
            repos_loading: true,
            draft_loading: true,
            submitting: false,
            available_repos: Vec::new(),
            selected_repos: Vec::new(),
            selected_repo_index: 0,
        });
        if self.composer_config.is_none()
            && let Some(default_executor_profile) = self.default_executor_profile.clone()
        {
            self.composer_config = Some(default_executor_profile.into());
        }
        self.rebind_discovery_stream();
        self.api.load_workspace_create_bootstrap(self.tx.clone());
        self.mark_workspace_list_dirty();
        self.mark_detail_dirty();
        self.status = "New workspace: add repositories, then write the prompt".to_string();
        self.error = None;
    }

    pub(crate) fn cancel_workspace_create_mode(&mut self, size: Rect) {
        if !self.creating_workspace {
            return;
        }
        self.creating_workspace = false;
        self.workspace_create = None;
        self.workspace_create_repo_picker = None;
        self.workspace_create_branch_picker = None;
        self.composer.clear();
        self.invalidate_composer_layout_cache();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.draft_save_in_flight = false;
        self.last_composer_edit = None;
        self.selected_workspace_id = self
            .workspace_create_previous_selection
            .take()
            .or_else(|| self.visible_workspace_ids().first().copied());
        self.mark_workspace_list_dirty();
        if self.selected_workspace_id.is_some() {
            self.load_selected_workspace(size);
        }
        self.status = "Cancelled workspace creation".to_string();
        self.error = None;
    }

    pub(crate) fn workspace_create_rows_selected_index(&self) -> Option<usize> {
        self.workspace_create.as_ref().map(|state| {
            state
                .selected_repo_index
                .min(state.selected_repos.len().saturating_sub(1))
        })
    }

    pub(crate) fn workspace_create_selected_repo(&self) -> Option<&WorkspaceCreateSelectedRepo> {
        let state = self.workspace_create.as_ref()?;
        state.selected_repos.get(state.selected_repo_index)
    }

    pub(crate) fn workspace_create_filtered_repo_options(&self) -> Vec<Repo> {
        let Some(state) = self.workspace_create.as_ref() else {
            return Vec::new();
        };
        let query = self
            .workspace_create_repo_picker
            .as_ref()
            .map(|picker| picker.query.as_str())
            .unwrap_or("");
        state
            .available_repos
            .iter()
            .filter(|repo| {
                query.is_empty()
                    || fuzzy_contains(query, &repo.display_name)
                    || fuzzy_contains(query, &repo.name)
                    || fuzzy_contains(query, &repo.path.display().to_string())
            })
            .cloned()
            .collect()
    }

    pub(crate) fn open_workspace_create_repo_picker(&mut self) {
        let Some(state) = self.workspace_create.as_ref() else {
            return;
        };
        if state.repos_loading {
            self.status = "Repositories are still loading".to_string();
            return;
        }
        if state.available_repos.is_empty() {
            self.status = "No repositories available".to_string();
            return;
        }
        self.workspace_create_repo_picker = Some(WorkspaceCreateRepoPickerState {
            query: String::new(),
            selected: 0,
        });
        self.status = "Add repository".to_string();
    }

    pub(crate) fn handle_workspace_create_repo_picker_key(&mut self, key: KeyEvent) {
        let options_len = self.workspace_create_filtered_repo_options().len();
        let Some(mut_picker_selected) = self
            .workspace_create_repo_picker
            .as_ref()
            .map(|picker| picker.selected)
        else {
            return;
        };
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.workspace_create_repo_picker = None;
                self.status = "Closed repository picker".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                let options = self.workspace_create_filtered_repo_options();
                let Some(repo) = options.get(mut_picker_selected).cloned() else {
                    return;
                };
                self.workspace_create_repo_picker = None;
                self.open_workspace_create_branch_picker(repo);
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if let Some(picker) = self.workspace_create_repo_picker.as_mut()
                    && !picker.query.is_empty()
                {
                    picker.query.pop();
                    picker.selected = 0;
                }
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                if options_len > 0 {
                    if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                        picker.selected = (picker.selected + 1).min(options_len.saturating_sub(1));
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => {
                if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if options_len > 0 {
                    if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                        picker.selected = (picker.selected + 8).min(options_len.saturating_sub(1));
                    }
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => {
                if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(8);
                }
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                    picker.selected = 0;
                }
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if options_len > 0 {
                    if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                        picker.selected = options_len.saturating_sub(1);
                    }
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
                if let Some(picker) = self.workspace_create_repo_picker.as_mut() {
                    picker.query.push(ch);
                    picker.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn open_workspace_create_branch_picker(&mut self, repo: Repo) {
        if let Some(state) = self.workspace_create.as_ref()
            && let Some(index) = state
                .selected_repos
                .iter()
                .position(|entry| entry.repo.id == repo.id)
        {
            self.workspace_create_branch_picker = Some(WorkspaceCreateBranchPickerState {
                selected_repo_index: index,
                selected: 0,
                branches: Vec::new(),
            });
        } else if let Some(state) = self.workspace_create.as_mut() {
            state.selected_repos.push(WorkspaceCreateSelectedRepo {
                target_branch: repo.default_target_branch.clone().unwrap_or_default(),
                repo: repo.clone(),
            });
            state.selected_repo_index = state.selected_repos.len().saturating_sub(1);
            self.workspace_create_branch_picker = Some(WorkspaceCreateBranchPickerState {
                selected_repo_index: state.selected_repo_index,
                selected: 0,
                branches: Vec::new(),
            });
        }
        self.api
            .load_workspace_create_branches(repo.id, self.tx.clone());
        self.status = format!("Loading branches for {}", repo.display_name);
    }

    pub(crate) fn handle_workspace_create_branch_picker_key(&mut self, key: KeyEvent) {
        let branch_count = self
            .workspace_create_branch_picker
            .as_ref()
            .map(|picker| picker.branches.len())
            .unwrap_or(0);
        let Some(picker) = self.workspace_create_branch_picker.as_mut() else {
            return;
        };
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.workspace_create_branch_picker = None;
                self.status = "Closed branch picker".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                let Some(branch) = picker.branches.get(picker.selected).cloned() else {
                    return;
                };
                if let Some(state) = self.workspace_create.as_mut()
                    && let Some(selected_repo) =
                        state.selected_repos.get_mut(picker.selected_repo_index)
                {
                    selected_repo.target_branch = branch.name.clone();
                    state.selected_repo_index = picker.selected_repo_index;
                    self.mark_detail_dirty();
                }
                self.workspace_create_branch_picker = None;
                self.status = format!("Selected branch {}", branch.name);
            }
            KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                ..
            } => {
                if branch_count > 0 {
                    picker.selected = (picker.selected + 1).min(branch_count.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                ..
            } => picker.selected = picker.selected.saturating_sub(1),
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if branch_count > 0 {
                    picker.selected = (picker.selected + 8).min(branch_count.saturating_sub(1));
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => picker.selected = picker.selected.saturating_sub(8),
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => picker.selected = 0,
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                if branch_count > 0 {
                    picker.selected = branch_count.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn remove_selected_workspace_create_repo(&mut self) {
        let Some(state) = self.workspace_create.as_mut() else {
            return;
        };
        if state.selected_repos.is_empty() {
            return;
        }
        let removed = state.selected_repos.remove(state.selected_repo_index);
        state.selected_repo_index = state
            .selected_repo_index
            .min(state.selected_repos.len().saturating_sub(1));
        self.composer_dirty = true;
        self.last_composer_edit = Some(std::time::Instant::now());
        self.mark_detail_dirty();
        self.status = format!("Removed {}", removed.repo.display_name);
    }

    pub(crate) fn move_workspace_create_selection(&mut self, delta: i32) {
        let Some(state) = self.workspace_create.as_mut() else {
            return;
        };
        if state.selected_repos.is_empty() {
            return;
        }
        let current = state.selected_repo_index as i32;
        state.selected_repo_index = (current + delta)
            .clamp(0, state.selected_repos.len().saturating_sub(1) as i32)
            as usize;
    }

    pub(crate) fn apply_workspace_create_draft(&mut self, draft: DraftWorkspaceData) {
        let available_repos = self
            .workspace_create
            .as_ref()
            .map(|state| state.available_repos.clone())
            .unwrap_or_default();
        self.composer = draft.message;
        self.invalidate_composer_layout_cache();
        self.composer_cursor = self.composer.len();
        self.composer_dirty = false;
        self.draft_save_in_flight = false;
        self.last_composer_edit = None;
        self.composer_scratch_loaded = true;
        if let Some(executor_config) = draft.executor_config {
            let executor_changed = self
                .composer_config
                .as_ref()
                .map(|config| config.executor != executor_config.executor)
                .unwrap_or(true);
            self.composer_config = Some(executor_config);
            if executor_changed {
                self.rebind_discovery_stream();
            }
        }
        let available_by_id = available_repos
            .iter()
            .cloned()
            .map(|repo| (repo.id, repo))
            .collect::<std::collections::HashMap<_, _>>();
        let Some(state) = self.workspace_create.as_mut() else {
            return;
        };
        state.selected_repos = draft
            .repos
            .into_iter()
            .filter_map(|entry| {
                available_by_id.get(&entry.repo_id).cloned().map(|repo| {
                    WorkspaceCreateSelectedRepo {
                        repo,
                        target_branch: entry.target_branch,
                    }
                })
            })
            .collect();
        state.selected_repo_index = state
            .selected_repo_index
            .min(state.selected_repos.len().saturating_sub(1));
        state.draft_loading = false;
        self.mark_detail_dirty();
    }

    pub(crate) fn can_submit_workspace_create(&self) -> Result<(), &'static str> {
        let Some(state) = self.workspace_create.as_ref() else {
            return Err("Workspace creation is not active");
        };
        if state.selected_repos.is_empty() {
            return Err("Add at least one repository");
        }
        if state
            .selected_repos
            .iter()
            .any(|entry| entry.target_branch.trim().is_empty())
        {
            return Err("Select a branch for every repository");
        }
        if self.composer.trim().is_empty() {
            return Err("Enter a prompt");
        }
        if self.composer_config.is_none() {
            return Err("Choose an executor");
        }
        Ok(())
    }

    pub(crate) async fn submit_workspace_create(&mut self) {
        if !self.creating_workspace {
            return;
        }
        if self
            .workspace_create
            .as_ref()
            .is_some_and(|state| state.submitting)
        {
            self.status = "Workspace creation already in progress".to_string();
            return;
        }
        if let Err(message) = self.can_submit_workspace_create() {
            self.status = message.to_string();
            self.error = Some(message.to_string());
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            return;
        };
        let prompt = self.composer.trim().to_string();
        let name = prompt
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().chars().take(64).collect::<String>());
        let repos = self
            .workspace_create
            .as_ref()
            .map(|state| {
                state
                    .selected_repos
                    .iter()
                    .map(|entry| DraftWorkspaceRepo {
                        repo_id: entry.repo.id,
                        target_branch: entry.target_branch.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(state) = self.workspace_create.as_mut() {
            state.submitting = true;
        }
        self.status = "Creating workspace".to_string();
        self.error = None;
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api
                .create_and_start_workspace(name, prompt, repos, executor_config)
                .await
            {
                Ok(workspace) => {
                    let _ = api.clear_workspace_create_draft().await;
                    let _ = tx.send(NetEvent::WorkspaceCreateSubmitted { workspace });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::WorkspaceCreateSubmitFailed {
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    pub(crate) fn workspace_create_loading(&self) -> bool {
        self.workspace_create
            .as_ref()
            .is_some_and(|state| state.repos_loading || state.draft_loading)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::{repo::Repo, scratch::DraftWorkspaceData};
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{
            App, ToolCallDisplayMode,
            state::{WorkspaceCreateSelectedRepo, WorkspaceCreateState},
        },
        editor::ComposerEditorMode,
        model::{Focus, Pane, QueueStatus, WorkspaceBundle},
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
            composer_config: Some(executors::profile::ExecutorConfig::new(
                executors::executors::BaseCodingAgent::Codex,
            )),
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
            workspace_project_filter_picker: None,
            workspace_create: None,
            workspace_create_repo_picker: None,
            workspace_create_branch_picker: None,
            session_rename: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            creating_workspace: false,
            workspace_create_previous_selection: None,
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn repo(name: &str, branch: &str) -> Repo {
        let now = Utc.timestamp_opt(1, 0).unwrap();
        Repo {
            id: Uuid::new_v4(),
            path: std::path::PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            display_name: name.to_string(),
            setup_script: None,
            cleanup_script: None,
            archive_script: None,
            copy_files: None,
            parallel_setup_script: false,
            dev_server_script: None,
            default_target_branch: Some(branch.to_string()),
            default_working_dir: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn workspace_create_validation_requires_repo_prompt_and_executor() {
        let mut app = test_app();
        app.creating_workspace = true;
        app.workspace_create = Some(WorkspaceCreateState {
            repos_loading: false,
            draft_loading: false,
            submitting: false,
            available_repos: Vec::new(),
            selected_repos: Vec::new(),
            selected_repo_index: 0,
        });

        assert_eq!(
            app.can_submit_workspace_create(),
            Err("Add at least one repository")
        );

        app.workspace_create
            .as_mut()
            .unwrap()
            .selected_repos
            .push(WorkspaceCreateSelectedRepo {
                repo: repo("alpha", "main"),
                target_branch: "main".to_string(),
            });
        assert_eq!(app.can_submit_workspace_create(), Err("Enter a prompt"));

        app.composer = "Build the thing".to_string();
        app.composer_config = None;
        assert_eq!(app.can_submit_workspace_create(), Err("Choose an executor"));
    }

    #[test]
    fn apply_workspace_create_draft_restores_prompt_executor_and_known_repos() {
        let mut app = test_app();
        let alpha = repo("alpha", "main");
        let beta = repo("beta", "develop");
        app.creating_workspace = true;
        app.workspace_create = Some(WorkspaceCreateState {
            repos_loading: false,
            draft_loading: true,
            submitting: false,
            available_repos: vec![alpha.clone(), beta.clone()],
            selected_repos: Vec::new(),
            selected_repo_index: 0,
        });

        app.apply_workspace_create_draft(DraftWorkspaceData {
            message: "Create a workspace".to_string(),
            repos: vec![db::models::scratch::DraftWorkspaceRepo {
                repo_id: beta.id,
                target_branch: "feature/x".to_string(),
            }],
            executor_config: app.composer_config.clone(),
            linked_issue: None,
            attachments: Vec::new(),
        });

        assert_eq!(app.composer, "Create a workspace");
        let selected = &app.workspace_create.as_ref().unwrap().selected_repos;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].repo.id, beta.id);
        assert_eq!(selected[0].target_branch, "feature/x");
    }
}
