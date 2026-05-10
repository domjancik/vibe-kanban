use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use executors::{
    executors::BaseCodingAgent,
    model_selector::{AgentInfo, ModelInfo, PermissionPolicy},
    profile::ExecutorConfig,
};
use uuid::Uuid;

use crate::app::{AgentPickerState, App};

impl App {
    fn executor_change_locked(&self) -> bool {
        !self.creating_new_session && self.current_session().is_some()
    }

    fn current_discovery_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn rebind_discovery_stream(&mut self) {
        let Some(config) = self.composer_config.as_ref() else {
            return;
        };
        self.api.replace_discovery_stream(
            config.executor,
            self.selected_workspace_id,
            self.current_discovery_session_id(),
            self.tx.clone(),
            &mut self.subscriptions,
        );
    }

    async fn refresh_preset_config(
        &mut self,
        executor: BaseCodingAgent,
        variant: Option<String>,
        message: &str,
    ) {
        let mut path = format!("/api/agents/preset-options?executor={executor}");
        if let Some(variant) = variant.as_deref() {
            path.push_str(&format!("&variant={variant}"));
        }
        match self.api.get::<ExecutorConfig>(&path).await {
            Ok(config) => {
                self.composer_config = Some(config);
                self.composer_options = None;
                self.rebind_discovery_stream();
                self.status = message.to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    fn executor_options(&self) -> Vec<BaseCodingAgent> {
        let mut options = self
            .executor_profiles
            .executors
            .keys()
            .copied()
            .collect::<Vec<_>>();
        options.sort_by_key(|executor| executor.to_string());
        options
    }

    fn variant_options(&self, executor: BaseCodingAgent) -> Vec<String> {
        let Some(profile) = self.executor_profiles.executors.get(&executor) else {
            return vec!["DEFAULT".to_string()];
        };
        let mut variants = profile
            .configurations
            .keys()
            .filter(|key| key.as_str() != "recently_used_models")
            .cloned()
            .collect::<Vec<_>>();
        variants.sort_by(|left, right| {
            if left == "DEFAULT" {
                std::cmp::Ordering::Less
            } else if right == "DEFAULT" {
                std::cmp::Ordering::Greater
            } else {
                left.cmp(right)
            }
        });
        if variants.is_empty() {
            variants.push("DEFAULT".to_string());
        }
        variants
    }

    fn model_options(&self) -> Vec<ModelInfo> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.models.clone())
            .unwrap_or_default()
    }

    fn selected_model_value(&self) -> Option<String> {
        self.composer_config
            .as_ref()
            .and_then(|config| config.model_id.clone())
            .or_else(|| {
                self.composer_options
                    .as_ref()
                    .and_then(|options| options.model_selector.default_model.clone())
            })
    }

    pub(crate) fn selected_model_label(&self) -> Option<String> {
        let selected = self.selected_model_value()?;
        self.model_options()
            .into_iter()
            .find(|model| crate::app::model_key(model) == selected)
            .map(|model| {
                if let Some(provider_id) = model.provider_id {
                    format!("{provider_id}/{}", model.id)
                } else {
                    model.id
                }
            })
            .or(Some(selected))
    }

    fn reasoning_options(&self) -> Vec<String> {
        let Some(selected_model) = self.selected_model_value() else {
            return Vec::new();
        };
        self.model_options()
            .into_iter()
            .find(|model| crate::app::model_key(model) == selected_model)
            .map(|model| {
                model
                    .reasoning_options
                    .into_iter()
                    .map(|option| option.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    pub(crate) fn selected_reasoning_label(&self) -> Option<String> {
        self.composer_config
            .as_ref()
            .and_then(|config| config.reasoning_id.clone())
            .or_else(|| {
                let selected_model = self.selected_model_value()?;
                self.model_options()
                    .into_iter()
                    .find(|model| crate::app::model_key(model) == selected_model)
                    .and_then(|model| {
                        model
                            .reasoning_options
                            .into_iter()
                            .find(|option| option.is_default)
                            .map(|option| option.id)
                    })
            })
    }

    fn agent_mode_options(&self) -> Vec<AgentInfo> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.agents.clone())
            .unwrap_or_default()
    }

    pub(crate) fn filtered_agent_mode_options(&self) -> Vec<Option<AgentInfo>> {
        let query = self
            .agent_picker
            .as_ref()
            .map(|state| state.query.trim().to_lowercase())
            .unwrap_or_default();
        let mut options = vec![None];
        let mut agents =
            self.agent_mode_options()
                .into_iter()
                .filter(|agent| {
                    query.is_empty()
                        || crate::app::fuzzy_contains(&query, &agent.label)
                        || crate::app::fuzzy_contains(&query, &agent.id)
                        || agent.description.as_ref().is_some_and(|description| {
                            crate::app::fuzzy_contains(&query, description)
                        })
                })
                .collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            right
                .is_default
                .cmp(&left.is_default)
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.id.cmp(&right.id))
        });
        options.extend(agents.into_iter().map(Some));
        options
    }

    fn selected_agent_mode_index(&self, options: &[Option<AgentInfo>]) -> usize {
        let current = self
            .composer_config
            .as_ref()
            .and_then(|config| config.agent_id.clone());
        options
            .iter()
            .position(|option| match (option, current.as_ref()) {
                (None, None) => true,
                (Some(agent), Some(current)) => &agent.id == current,
                _ => false,
            })
            .unwrap_or(0)
    }

    pub(crate) fn open_agent_picker(&mut self) {
        let options = self.agent_mode_options();
        if options.is_empty() {
            self.status = "No agent modes available".to_string();
            return;
        }
        let mut picker = AgentPickerState {
            query: String::new(),
            selected: 0,
        };
        picker.selected = self.selected_agent_mode_index(&self.filtered_agent_mode_options());
        self.agent_picker = Some(picker);
        self.status = "Select agent mode".to_string();
        self.error = None;
    }

    pub(crate) fn handle_agent_picker_key(&mut self, key: KeyEvent) {
        let options_len = self.filtered_agent_mode_options().len();
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.agent_picker = None;
                self.status = "Closed agent mode picker".to_string();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.apply_agent_picker_selection(),
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

    fn apply_agent_picker_selection(&mut self) {
        let options = self.filtered_agent_mode_options();
        let selected = self
            .agent_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0)
            .min(options.len().saturating_sub(1));
        let Some(config) = self.composer_config.as_mut() else {
            self.agent_picker = None;
            self.status = "Composer config is still loading".to_string();
            return;
        };

        match options.get(selected).cloned().flatten() {
            Some(agent) => {
                config.agent_id = Some(agent.id.clone());
                self.status = format!("Updated agent mode to {}", agent.label);
            }
            None => {
                config.agent_id = None;
                self.status = "Updated agent mode to default".to_string();
            }
        }

        self.error = None;
        self.agent_picker = None;
    }

    pub(crate) fn selected_agent_mode_label(&self) -> String {
        let Some(selected_id) = self
            .composer_config
            .as_ref()
            .and_then(|config| config.agent_id.clone())
        else {
            return "default".to_string();
        };

        self.agent_mode_options()
            .into_iter()
            .find(|agent| agent.id == selected_id)
            .map(|agent| agent.label)
            .unwrap_or(selected_id)
    }

    fn permission_options(&self) -> Vec<PermissionPolicy> {
        self.composer_options
            .as_ref()
            .map(|options| options.model_selector.permissions.clone())
            .unwrap_or_default()
    }

    pub(crate) async fn cycle_executor(&mut self) {
        if self.executor_change_locked() {
            self.status = "Executor cannot be changed for an existing session".to_string();
            self.error = None;
            return;
        }
        let options = self.executor_options();
        if options.is_empty() {
            self.status = "Executor profiles are still loading".to_string();
            return;
        }
        let current = self
            .composer_config
            .as_ref()
            .map(|config| config.executor)
            .or_else(|| {
                self.default_executor_profile
                    .as_ref()
                    .map(|profile| profile.executor)
            })
            .unwrap_or(options[0]);
        let index = options
            .iter()
            .position(|executor| *executor == current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()];
        let variant = self
            .variant_options(next)
            .into_iter()
            .next()
            .and_then(crate::app::default_variant_to_none);
        self.refresh_preset_config(next, variant, "Updated composer executor")
            .await;
    }

    pub(crate) async fn cycle_variant(&mut self) {
        let Some(config) = self.composer_config.as_ref() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let options = self.variant_options(config.executor);
        if options.is_empty() {
            self.status = "No variants available".to_string();
            return;
        }
        let current = crate::model::display_variant(config.variant.as_deref());
        let index = options
            .iter()
            .position(|variant| variant == current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        self.refresh_preset_config(
            config.executor,
            crate::app::default_variant_to_none(next),
            "Updated composer variant",
        )
        .await;
    }

    pub(crate) fn cycle_model(&mut self) {
        let options = self.model_options();
        if options.is_empty() {
            self.status = "No model options available".to_string();
            return;
        }
        let keys = options
            .iter()
            .map(crate::app::model_key)
            .collect::<Vec<_>>();
        let current = self
            .selected_model_value()
            .unwrap_or_else(|| keys.first().cloned().unwrap_or_default());
        let index = keys.iter().position(|key| *key == current).unwrap_or(0);
        let next = keys[(index + 1) % keys.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.model_id = Some(next.clone());
            config.reasoning_id = None;
            self.status = format!("Updated model to {next}");
            self.error = None;
        }
    }

    pub(crate) fn cycle_reasoning(&mut self) {
        let options = self.reasoning_options();
        if options.is_empty() {
            self.status = "No reasoning options available".to_string();
            return;
        }
        let current = self
            .selected_reasoning_label()
            .unwrap_or_else(|| options[0].clone());
        let index = options
            .iter()
            .position(|option| option == &current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.reasoning_id = Some(next.clone());
            self.status = format!("Updated reasoning to {next}");
            self.error = None;
        }
    }

    pub(crate) fn cycle_permission_mode(&mut self) {
        let options = self.permission_options();
        if options.is_empty() {
            self.status = "No permission modes available".to_string();
            return;
        }
        let current = self
            .composer_config
            .as_ref()
            .and_then(|config| config.permission_policy.clone())
            .unwrap_or(options[0].clone());
        let index = options
            .iter()
            .position(|option| option == &current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()].clone();
        if let Some(config) = self.composer_config.as_mut() {
            config.permission_policy = Some(next.clone());
            self.status = format!(
                "Updated permission mode to {}",
                crate::model::display_permission(Some(&next))
            );
            self.error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::session::Session;
    use executors::{
        executor_discovery::ExecutorDiscoveredOptions,
        executors::BaseCodingAgent,
        model_selector::{AgentInfo, ModelInfo, ModelSelectorConfig, ReasoningOption},
        profile::{ExecutorConfig, ExecutorConfigs, ExecutorProfile},
    };
    use tokio::sync::mpsc::unbounded_channel;

    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::{AgentPickerState, App},
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
            executor_profiles: ExecutorConfigs {
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

    fn sample_agent(executor: BaseCodingAgent) -> executors::executors::CodingAgent {
        ExecutorConfigs::get_cached()
            .executors
            .get(&executor)
            .and_then(|profile| profile.configurations.values().next())
            .cloned()
            .expect("executor profile available for tests")
    }

    #[test]
    fn executor_and_variant_options_are_sorted_as_expected() {
        let mut app = test_app();
        let codex_agent = sample_agent(BaseCodingAgent::Codex);
        let claude_agent = sample_agent(BaseCodingAgent::ClaudeCode);

        app.executor_profiles.executors = HashMap::from([
            (
                BaseCodingAgent::Codex,
                ExecutorProfile {
                    recently_used_models: None,
                    configurations: HashMap::from([
                        ("PLAN".to_string(), codex_agent.clone()),
                        ("DEFAULT".to_string(), codex_agent.clone()),
                        ("ROUTER".to_string(), codex_agent),
                    ]),
                },
            ),
            (
                BaseCodingAgent::ClaudeCode,
                ExecutorProfile {
                    recently_used_models: None,
                    configurations: HashMap::from([("DEFAULT".to_string(), claude_agent)]),
                },
            ),
        ]);

        assert_eq!(
            app.executor_options(),
            vec![BaseCodingAgent::ClaudeCode, BaseCodingAgent::Codex]
        );
        assert_eq!(
            app.variant_options(BaseCodingAgent::Codex),
            vec![
                "DEFAULT".to_string(),
                "PLAN".to_string(),
                "ROUTER".to_string()
            ]
        );
    }

    #[test]
    fn model_reasoning_and_agent_picker_helpers_follow_config_fallbacks() {
        let mut app = test_app();
        app.composer_config = Some(ExecutorConfig {
            executor: BaseCodingAgent::Codex,
            variant: None,
            model_id: None,
            agent_id: Some("review".to_string()),
            reasoning_id: None,
            permission_policy: None,
        });
        app.composer_options = Some(ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                providers: Vec::new(),
                models: vec![ModelInfo {
                    id: "gpt-5".to_string(),
                    name: "GPT-5".to_string(),
                    provider_id: Some("openai".to_string()),
                    reasoning_options: vec![
                        ReasoningOption {
                            id: "low".to_string(),
                            label: "Low".to_string(),
                            is_default: false,
                        },
                        ReasoningOption {
                            id: "high".to_string(),
                            label: "High".to_string(),
                            is_default: true,
                        },
                    ],
                }],
                default_model: Some("openai/gpt-5".to_string()),
                agents: vec![
                    AgentInfo {
                        id: "review".to_string(),
                        label: "Review".to_string(),
                        description: Some("Review code".to_string()),
                        is_default: false,
                    },
                    AgentInfo {
                        id: "build".to_string(),
                        label: "Builder".to_string(),
                        description: Some("Build features".to_string()),
                        is_default: true,
                    },
                ],
                permissions: Vec::new(),
            },
            slash_commands: Vec::new(),
            loading_models: false,
            loading_agents: false,
            loading_slash_commands: false,
            error: None,
        });

        assert_eq!(app.selected_model_value().as_deref(), Some("openai/gpt-5"));
        assert_eq!(app.selected_reasoning_label().as_deref(), Some("high"));

        app.agent_picker = Some(AgentPickerState {
            query: "rvw".to_string(),
            selected: 0,
        });
        let filtered = app.filtered_agent_mode_options();
        assert_eq!(filtered.len(), 2);
        assert!(matches!(
            filtered[1].as_ref().map(|agent| agent.id.as_str()),
            Some("review")
        ));

        let selected = app.selected_agent_mode_index(&[
            None,
            Some(AgentInfo {
                id: "build".to_string(),
                label: "Builder".to_string(),
                description: None,
                is_default: true,
            }),
            Some(AgentInfo {
                id: "review".to_string(),
                label: "Review".to_string(),
                description: None,
                is_default: false,
            }),
        ]);
        assert_eq!(selected, 2);
    }

    #[tokio::test]
    async fn cycle_executor_is_blocked_for_existing_session() {
        let mut app = test_app();
        app.executor_profiles.executors = HashMap::from([
            (
                BaseCodingAgent::Codex,
                ExecutorProfile {
                    recently_used_models: None,
                    configurations: HashMap::from([(
                        "DEFAULT".to_string(),
                        sample_agent(BaseCodingAgent::Codex),
                    )]),
                },
            ),
            (
                BaseCodingAgent::ClaudeCode,
                ExecutorProfile {
                    recently_used_models: None,
                    configurations: HashMap::from([(
                        "DEFAULT".to_string(),
                        sample_agent(BaseCodingAgent::ClaudeCode),
                    )]),
                },
            ),
        ]);
        let session_id = uuid::Uuid::new_v4();
        app.bundle.selected_session_id = Some(session_id);
        app.bundle.sessions.push(Session {
            id: session_id,
            workspace_id: uuid::Uuid::new_v4(),
            name: Some("Existing".to_string()),
            executor: Some(BaseCodingAgent::Codex.to_string()),
            agent_working_dir: None,
            created_at: Utc.timestamp_opt(1, 0).unwrap(),
            updated_at: Utc.timestamp_opt(1, 0).unwrap(),
        });
        app.composer_config = Some(ExecutorConfig::new(BaseCodingAgent::Codex));

        app.cycle_executor().await;

        assert_eq!(
            app.composer_config.as_ref().map(|config| config.executor),
            Some(BaseCodingAgent::Codex)
        );
        assert_eq!(
            app.status,
            "Executor cannot be changed for an existing session"
        );
    }
}
