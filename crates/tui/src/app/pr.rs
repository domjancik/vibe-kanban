use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    app::{App, PrCreateField, state::PrCreateState},
    editor::apply_text_edit_action,
    input::{TextInputOptions, map_text_input_key},
    model::{AttachExistingPrRequest, CreatePrApiRequest, Merge, MergeStatus, NetEvent},
};

impl App {
    pub(crate) fn current_git_repo_status(&self) -> Option<&crate::model::RepoBranchStatus> {
        self.bundle.git_status.get(
            self.bundle
                .selected_git_repo_index
                .min(self.bundle.git_status.len().saturating_sub(1)),
        )
    }

    pub(crate) fn current_git_repo_id(&self) -> Option<uuid::Uuid> {
        self.current_git_repo_status()
            .map(|status| status.repo_id)
            .or_else(|| {
                self.bundle
                    .repos
                    .get(
                        self.bundle
                            .selected_git_repo_index
                            .min(self.bundle.repos.len().saturating_sub(1)),
                    )
                    .map(|repo| repo.repo.id)
            })
    }

    pub(crate) fn open_pr_create(&mut self) {
        let Some(repo_id) = self.current_git_repo_id() else {
            self.status = "No repository available for pull request".to_string();
            return;
        };
        let repo_name = self
            .current_git_repo_status()
            .map(|status| status.repo_name.clone())
            .or_else(|| {
                self.bundle
                    .repos
                    .iter()
                    .find(|repo| repo.repo.id == repo_id)
                    .map(|repo| repo.repo.display_name.clone())
            })
            .unwrap_or_else(|| "Repository".to_string());
        let target_branch = self
            .current_git_repo_status()
            .map(|status| status.status.target_branch_name.clone())
            .or_else(|| {
                self.bundle
                    .repos
                    .iter()
                    .find(|repo| repo.repo.id == repo_id)
                    .map(|repo| repo.target_branch.clone())
            })
            .unwrap_or_default();
        let mut title = self
            .bundle
            .workspace
            .as_ref()
            .map(|workspace| {
                workspace
                    .name
                    .clone()
                    .unwrap_or_else(|| workspace.branch.clone())
            })
            .unwrap_or_else(|| repo_name.clone());
        if self.bundle.repos.len() > 1 && !title.contains(&repo_name) {
            title = format!("{title} ({repo_name})");
        }
        let title_cursor = title.len();
        let target_branch_cursor = target_branch.len();
        self.pr_create = Some(PrCreateState {
            repo_id,
            repo_name,
            title,
            title_cursor,
            body: String::new(),
            body_cursor: 0,
            target_branch,
            target_branch_cursor,
            draft: false,
            auto_generate_description: false,
            selected_field: PrCreateField::Title,
            submitting: false,
        });
        self.status = "Create pull request".to_string();
        self.error = None;
    }

    pub(crate) async fn primary_pr_action(&mut self) {
        if self.focus != crate::model::Focus::Main || self.selected_pane != crate::model::Pane::Git
        {
            return;
        }
        if self.selected_repo_open_pr().is_some() {
            self.open_selected_pull_request().await;
        } else {
            self.open_pr_create();
        }
    }

    pub(crate) async fn open_selected_pull_request(&mut self) {
        let Some(pr) = self
            .selected_repo_open_pr()
            .or_else(|| self.selected_repo_any_pr())
        else {
            self.status = "No linked pull request".to_string();
            return;
        };
        let url = pr.pr_url.clone();
        self.status = format!("Opening PR #{}", pr.pr_number);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match utils::browser::open_browser(&url).await {
                Ok(()) => NetEvent::PullRequestOpened { url },
                Err(error) => NetEvent::PullRequestOpenFailed {
                    url,
                    message: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub(crate) async fn attach_selected_pull_request(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        let Some(repo_id) = self.current_git_repo_id() else {
            self.status = "No repository available for PR attach".to_string();
            return;
        };
        self.status = "Attaching existing pull request".to_string();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match api
                .attach_existing_pull_request(workspace_id, &AttachExistingPrRequest { repo_id })
                .await
            {
                Ok(response) => NetEvent::PullRequestAttached {
                    workspace_id,
                    repo_id,
                    response,
                },
                Err(error) => NetEvent::PullRequestAttachFailed {
                    message: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub(crate) fn handle_pr_create_paste(&mut self, pasted: String) {
        let Some(state) = self.pr_create.as_mut() else {
            return;
        };
        match state.selected_field {
            PrCreateField::Title => insert_text(&mut state.title, &mut state.title_cursor, &pasted),
            PrCreateField::Body => insert_text(&mut state.body, &mut state.body_cursor, &pasted),
            PrCreateField::TargetBranch => insert_text(
                &mut state.target_branch,
                &mut state.target_branch_cursor,
                &pasted,
            ),
            PrCreateField::Draft | PrCreateField::AutoGenerate => {}
        }
    }

    pub(crate) async fn handle_pr_create_key(&mut self, key: KeyEvent) {
        let Some(submitting) = self.pr_create.as_ref().map(|state| state.submitting) else {
            return;
        };
        if submitting {
            if key.code == KeyCode::Esc {
                self.status = "Pull request creation in progress".to_string();
            }
            return;
        }
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.pr_create = None;
                self.status = "Cancelled pull request creation".to_string();
            }
            KeyEvent {
                code: KeyCode::Tab, ..
            } => {
                if let Some(state) = self.pr_create.as_mut() {
                    cycle_pr_field(state, true);
                }
            }
            KeyEvent {
                code: KeyCode::BackTab,
                ..
            } => {
                if let Some(state) = self.pr_create.as_mut() {
                    cycle_pr_field(state, false);
                }
            }
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers,
                ..
            } if modifiers == KeyModifiers::CONTROL => {
                self.submit_pr_create().await;
            }
            KeyEvent {
                code: KeyCode::Char(' '),
                ..
            } => {
                if let Some(state) = self.pr_create.as_mut() {
                    match state.selected_field {
                        PrCreateField::Draft => state.draft = !state.draft,
                        PrCreateField::AutoGenerate => {
                            state.auto_generate_description = !state.auto_generate_description;
                        }
                        _ => {}
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                let selected_field = self
                    .pr_create
                    .as_ref()
                    .map(|state| state.selected_field)
                    .unwrap_or(PrCreateField::Title);
                match selected_field {
                    PrCreateField::Draft => {
                        if let Some(state) = self.pr_create.as_mut() {
                            state.draft = !state.draft;
                        }
                    }
                    PrCreateField::AutoGenerate => {
                        if let Some(state) = self.pr_create.as_mut() {
                            state.auto_generate_description = !state.auto_generate_description;
                        }
                    }
                    _ => self.submit_pr_create().await,
                }
            }
            _ => {
                let selected_field = self
                    .pr_create
                    .as_ref()
                    .map(|state| state.selected_field)
                    .unwrap_or(PrCreateField::Title);
                let options = TextInputOptions {
                    submit_on_enter: false,
                    enter_inserts_newline: false,
                    shift_enter_inserts_newline: false,
                };
                if let Some(event) = map_text_input_key(key, options) {
                    match selected_field {
                        PrCreateField::Title => {
                            if let Some(state) = self.pr_create.as_mut()
                                && let crate::input::TextInputEvent::Edit(action) = event
                            {
                                apply_text_edit_action(
                                    &mut state.title,
                                    &mut state.title_cursor,
                                    action,
                                );
                            }
                        }
                        PrCreateField::Body => {
                            if let Some(state) = self.pr_create.as_mut()
                                && let crate::input::TextInputEvent::Edit(action) = event
                            {
                                apply_text_edit_action(
                                    &mut state.body,
                                    &mut state.body_cursor,
                                    action,
                                );
                            }
                        }
                        PrCreateField::TargetBranch => {
                            if let Some(state) = self.pr_create.as_mut()
                                && let crate::input::TextInputEvent::Edit(action) = event
                            {
                                apply_text_edit_action(
                                    &mut state.target_branch,
                                    &mut state.target_branch_cursor,
                                    action,
                                );
                            }
                        }
                        PrCreateField::Draft | PrCreateField::AutoGenerate => {}
                    }
                }
            }
        }
    }

    pub(crate) async fn submit_pr_create(&mut self) {
        let Some(workspace_id) = self.selected_workspace_id else {
            return;
        };
        let Some(state) = self.pr_create.as_mut() else {
            return;
        };
        if state.title.trim().is_empty() {
            self.error = Some("Pull request title cannot be empty".to_string());
            self.status = "Pull request title cannot be empty".to_string();
            return;
        }
        if state.target_branch.trim().is_empty() {
            self.error = Some("Pull request target branch cannot be empty".to_string());
            self.status = "Pull request target branch cannot be empty".to_string();
            return;
        }
        state.submitting = true;
        self.status = format!("Creating PR for {}", state.repo_name);
        let request = CreatePrApiRequest {
            title: state.title.trim().to_string(),
            body: (!state.body.trim().is_empty()).then(|| state.body.trim().to_string()),
            target_branch: Some(state.target_branch.trim().to_string()),
            draft: Some(state.draft),
            repo_id: state.repo_id,
            auto_generate_description: state.auto_generate_description,
        };
        let repo_id = state.repo_id;
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match api.create_pull_request(workspace_id, &request).await {
                Ok(pr_url) => NetEvent::PullRequestCreated {
                    workspace_id,
                    repo_id,
                    pr_url,
                },
                Err(error) => NetEvent::PullRequestCreateFailed {
                    message: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub(crate) fn selected_repo_open_pr(&self) -> Option<&crate::model::PullRequestInfo> {
        self.current_git_repo_status()?
            .status
            .merges
            .iter()
            .find_map(|merge| match merge {
                Merge::Pr(pr) if matches!(pr.pr_info.status, MergeStatus::Open) => {
                    Some(&pr.pr_info)
                }
                _ => None,
            })
    }

    pub(crate) fn selected_repo_any_pr(&self) -> Option<&crate::model::PullRequestInfo> {
        self.current_git_repo_status()?
            .status
            .merges
            .iter()
            .find_map(|merge| match merge {
                Merge::Pr(pr) => Some(&pr.pr_info),
                _ => None,
            })
    }

    pub(crate) fn upsert_local_pr_state(
        &mut self,
        repo_id: uuid::Uuid,
        pr_url: String,
        pr_number: i64,
        pr_status: MergeStatus,
    ) {
        if let Some(status) = self
            .bundle
            .git_status
            .iter_mut()
            .find(|status| status.repo_id == repo_id)
        {
            status
                .status
                .merges
                .retain(|merge| !matches!(merge, Merge::Pr(_)));
            status.status.merges.push(Merge::Pr(crate::model::PrMerge {
                pr_info: crate::model::PullRequestInfo {
                    status: pr_status.clone(),
                    pr_number,
                    pr_url: pr_url.clone(),
                },
            }));
        }
        if let Some(workspace_id) = self.selected_workspace_id
            && let Some(summary) = self.summaries.get_mut(&workspace_id)
        {
            summary.pr_status = Some(pr_status);
            summary.pr_number = Some(pr_number);
            summary.pr_url = Some(pr_url);
        }
        self.mark_git_dirty();
        self.mark_workspace_list_dirty();
    }
}

fn cycle_pr_field(state: &mut PrCreateState, forward: bool) {
    state.selected_field = match (state.selected_field, forward) {
        (PrCreateField::Title, true) => PrCreateField::Body,
        (PrCreateField::Body, true) => PrCreateField::TargetBranch,
        (PrCreateField::TargetBranch, true) => PrCreateField::Draft,
        (PrCreateField::Draft, true) => PrCreateField::AutoGenerate,
        (PrCreateField::AutoGenerate, true) => PrCreateField::Title,
        (PrCreateField::Title, false) => PrCreateField::AutoGenerate,
        (PrCreateField::Body, false) => PrCreateField::Title,
        (PrCreateField::TargetBranch, false) => PrCreateField::Body,
        (PrCreateField::Draft, false) => PrCreateField::TargetBranch,
        (PrCreateField::AutoGenerate, false) => PrCreateField::Draft,
    };
}

fn insert_text(buffer: &mut String, cursor: &mut usize, text: &str) {
    let cursor_value = (*cursor).min(buffer.len());
    buffer.insert_str(cursor_value, text);
    *cursor = cursor_value + text.len();
}
