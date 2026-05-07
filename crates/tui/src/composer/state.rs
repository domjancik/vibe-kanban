use std::str::FromStr;

use db::models::scratch::DraftFollowUpData;
use executors::{executors::BaseCodingAgent, profile::ExecutorConfig};
use uuid::Uuid;

use crate::{
    app::App,
    conversation::ConversationScope,
    model::{NetEvent, QueueStatus},
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
        let current_scope: Option<ConversationScope> = self.current_conversation_scope();
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
            self.invalidate_composer_layout_cache();
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
        if self.actions_in_flight.queue_mutation {
            self.status = "Queue action already in progress".to_string();
            return;
        }
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
        self.actions_in_flight.queue_mutation = true;
        self.status = "Queueing follow-up".to_string();
        let scratch_id = self.current_composer_scratch_id();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Some(scratch_id) = scratch_id {
                let _ = api.save_follow_up_draft(scratch_id, draft.clone()).await;
            }
            match api.queue_follow_up(session_id, draft).await {
                Ok(status) => {
                    let _ = tx.send(NetEvent::QueuedPrompt { session_id, status });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::QueuePromptFailed {
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    pub(crate) async fn cancel_queued_prompt(&mut self) {
        if self.actions_in_flight.queue_mutation {
            self.status = "Queue action already in progress".to_string();
            return;
        }
        let Some(session_id) = self.current_queue_session_id() else {
            self.status = "No session queue to cancel".to_string();
            return;
        };
        let queued = match &self.queue_status {
            QueueStatus::Queued { message } => Some(message.data.clone()),
            QueueStatus::Empty => None,
        };
        self.actions_in_flight.queue_mutation = true;
        self.status = "Cancelling queued follow-up".to_string();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.cancel_queued_follow_up(session_id).await {
                Ok(status) => {
                    let _ = tx.send(NetEvent::QueueCancelled {
                        session_id,
                        status,
                        restored: queued,
                    });
                }
                Err(error) => {
                    let _ = tx.send(NetEvent::QueueCancelFailed {
                        message: error.to_string(),
                    });
                }
            }
        });
    }

    pub(crate) async fn discard_draft(&mut self) {
        if self.actions_in_flight.queue_mutation {
            self.status = "Queue action already in progress".to_string();
            return;
        }
        self.composer.clear();
        self.invalidate_composer_layout_cache();
        self.composer_cursor = 0;
        self.composer_dirty = false;
        self.last_composer_edit = None;
        self.composer_queue_conflict = false;
        if let Some(scratch_id) = self.current_composer_scratch_id() {
            self.actions_in_flight.queue_mutation = true;
            self.status = "Discarding follow-up draft".to_string();
            let api = self.api.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                match api.delete_follow_up_draft(scratch_id).await {
                    Ok(()) => {
                        let _ = tx.send(NetEvent::DraftDiscarded {
                            message: "Discarded follow-up draft".to_string(),
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(NetEvent::DraftDiscardFailed {
                            message: error.to_string(),
                        });
                    }
                }
            });
        }
    }
}
