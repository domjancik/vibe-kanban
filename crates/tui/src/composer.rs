use std::str::FromStr;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use db::models::scratch::DraftFollowUpData;
use executors::{
    executors::BaseCodingAgent,
    model_selector::{AgentInfo, ModelInfo, PermissionPolicy},
    profile::ExecutorConfig,
};
use uuid::Uuid;

use crate::{
    app_state::{AgentPickerState, App},
    model::{Focus, NetEvent, QueueStatus},
};

impl App {
    pub(crate) async fn flush_draft_if_needed(&mut self) {
        let Some(scratch_id) = self.current_composer_scratch_id() else {
            self.composer_dirty = false;
            self.draft_save_in_flight = false;
            self.last_composer_edit = None;
            self.composer_scratch_loaded = true;
            return;
        };
        if !self.composer_dirty || self.draft_save_in_flight {
            return;
        }
        let Some(last_edit) = self.last_composer_edit else {
            return;
        };
        if last_edit.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        if self.is_queue_present() {
            self.composer_queue_conflict = true;
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            return;
        };
        self.draft_save_in_flight = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        let draft = DraftFollowUpData {
            message: self.composer.clone(),
            executor_config,
        };
        let revision = self.composer_edit_revision;
        tokio::spawn(async move {
            match api.save_follow_up_draft(scratch_id, draft).await {
                Ok(()) => {
                    let _ = tx.send(NetEvent::DraftSaved {
                        scratch_id,
                        revision,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::DraftSaveFailed {
                        scratch_id,
                        revision,
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    fn current_discovery_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn current_composer_scratch_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            self.selected_workspace_id
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn current_queue_session_id(&self) -> Option<Uuid> {
        if self.creating_new_session {
            None
        } else {
            self.bundle.selected_session_id
        }
    }

    pub(crate) fn sync_composer_context(&mut self) {
        let current_scope = self.current_conversation_scope();
        let optimistic_before = self.optimistic_entries.len();
        self.optimistic_entries
            .retain(|entry| Some(entry.scope.clone()) == current_scope);
        if self.optimistic_entries.len() != optimistic_before {
            self.mark_chat_render_cache_dirty();
        }

        let scratch_id = self.current_composer_scratch_id();
        if scratch_id != self.composer_scratch_id {
            self.composer_scratch_id = scratch_id;
            self.composer_scratch_loaded = scratch_id.is_none();
            self.composer.clear();
            self.composer_cursor = 0;
            self.composer_dirty = false;
            self.draft_save_in_flight = false;
            self.last_composer_edit = None;
            self.composer_queue_conflict = false;
            self.api
                .replace_draft_stream(scratch_id, self.tx.clone(), &mut self.subscriptions);
        }

        let queue_session_id = self.current_queue_session_id();
        if queue_session_id != self.queue_session_id {
            self.queue_session_id = queue_session_id;
            self.queue_status = QueueStatus::Empty;
            self.queue_pending = false;
        }
        self.refresh_queue_status();
    }

    pub(crate) fn refresh_queue_status(&mut self) {
        if let Some(session_id) = self.current_queue_session_id() {
            self.queue_pending = true;
            self.api.load_queue_status(session_id, self.tx.clone());
        } else {
            let had_queue = !matches!(self.queue_status, QueueStatus::Empty);
            self.queue_status = QueueStatus::Empty;
            self.queue_pending = false;
            if had_queue {
                self.mark_chat_render_cache_dirty();
            }
        }
    }

    pub(crate) fn is_queue_present(&self) -> bool {
        matches!(self.queue_status, QueueStatus::Queued { .. })
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

    pub(crate) fn sync_composer_executor_with_session(&mut self) {
        if self.creating_new_session {
            return;
        }
        if self.composer_dirty || !self.composer.is_empty() || self.is_queue_present() {
            return;
        }
        let Some(session) = self.current_session() else {
            return;
        };
        let Some(executor_name) = session.executor.as_deref() else {
            return;
        };
        let Ok(executor) = BaseCodingAgent::from_str(executor_name) else {
            return;
        };
        if self
            .composer_config
            .as_ref()
            .is_some_and(|config| config.executor == executor)
        {
            self.rebind_discovery_stream();
            return;
        }
        self.composer_config = Some(ExecutorConfig::new(executor));
        self.composer_options = None;
        self.rebind_discovery_stream();
    }

    pub(crate) async fn queue_prompt(&mut self) {
        let Some(session_id) = self.current_queue_session_id() else {
            self.status = "Queueing is only available for an existing session".to_string();
            return;
        };
        let prompt = self.composer.trim().to_string();
        if prompt.is_empty() {
            self.status = "Composer is empty".to_string();
            return;
        }
        let Some(executor_config) = self.composer_config.clone() else {
            self.status = "Composer config is still loading".to_string();
            return;
        };
        let draft = DraftFollowUpData {
            message: prompt,
            executor_config,
        };
        if let Some(scratch_id) = self.current_composer_scratch_id() {
            let _ = self
                .api
                .save_follow_up_draft(scratch_id, draft.clone())
                .await;
        }
        match self.api.queue_follow_up(session_id, draft).await {
            Ok(status) => {
                self.queue_status = status;
                self.queue_pending = false;
                self.composer.clear();
                self.composer_cursor = 0;
                self.composer_dirty = false;
                self.last_composer_edit = None;
                self.focus = Focus::Main;
                self.status = "Queued follow-up".to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    pub(crate) async fn cancel_queued_prompt(&mut self) {
        let Some(session_id) = self.current_queue_session_id() else {
            self.status = "No session queue to cancel".to_string();
            return;
        };
        let queued = match &self.queue_status {
            QueueStatus::Queued { message } => Some(message.data.clone()),
            QueueStatus::Empty => None,
        };
        match self.api.cancel_queued_follow_up(session_id).await {
            Ok(status) => {
                self.queue_status = status;
                self.queue_pending = false;
                if let Some(queued) = queued {
                    let executor_changed = self
                        .composer_config
                        .as_ref()
                        .map(|config| config.executor != queued.executor_config.executor)
                        .unwrap_or(true);
                    self.composer = queued.message;
                    self.composer_cursor = self.composer.len();
                    self.composer_config = Some(queued.executor_config);
                    self.composer_dirty = true;
                    self.last_composer_edit = Some(std::time::Instant::now());
                    self.composer_queue_conflict = false;
                    if executor_changed {
                        self.rebind_discovery_stream();
                    }
                }
                self.status = "Cancelled queued follow-up".to_string();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }

    pub(crate) async fn discard_draft(&mut self) {
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.last_composer_edit = None;
        self.composer_queue_conflict = false;
        if let Some(scratch_id) = self.current_composer_scratch_id() {
            match self.api.delete_follow_up_draft(scratch_id).await {
                Ok(()) => {
                    self.status = "Discarded follow-up draft".to_string();
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    self.status = error.to_string();
                }
            }
        }
    }
}
