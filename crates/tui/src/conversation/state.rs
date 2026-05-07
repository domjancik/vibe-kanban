use std::time::Duration;

use executors::{
    logs::{NormalizedEntry, NormalizedEntryType},
    profile::ExecutorConfig,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};
use uuid::Uuid;

use crate::{
    app::App,
    conversation::{
        initial_conversation_process_ids, process_prompt, render_chat_entry,
        render_optimistic_chat_entry, wrap_lines,
    },
    model::{PatchType, QueueStatus},
};

#[derive(Clone, PartialEq, Eq)]
pub enum ConversationScope {
    Session(Uuid),
    NewSession(Uuid),
}

#[derive(Clone, PartialEq, Eq)]
pub enum OptimisticState {
    Pending,
    Failed,
}

#[derive(Clone)]
pub struct OptimisticConversationEntry {
    pub local_id: Uuid,
    pub scope: ConversationScope,
    pub message: String,
    pub executor_config: ExecutorConfig,
    pub state: OptimisticState,
}

pub struct ChatRenderCache {
    pub width: usize,
    pub lines: Vec<Line<'static>>,
    pub latest_token_usage: Option<(u32, u32)>,
}

impl App {
    pub(crate) fn current_conversation_scope(&self) -> Option<ConversationScope> {
        if self.creating_new_session {
            self.selected_workspace_id
                .map(ConversationScope::NewSession)
        } else {
            self.bundle
                .selected_session_id
                .map(ConversationScope::Session)
        }
    }

    pub(crate) fn push_optimistic_entry(
        &mut self,
        scope: ConversationScope,
        message: String,
        executor_config: ExecutorConfig,
    ) -> Uuid {
        let local_id = Uuid::new_v4();
        self.optimistic_entries.push(OptimisticConversationEntry {
            local_id,
            scope,
            message,
            executor_config,
            state: OptimisticState::Pending,
        });
        self.mark_chat_render_cache_dirty();
        local_id
    }

    pub(crate) fn mark_optimistic_failed(&mut self, local_id: Uuid) {
        if let Some(entry) = self
            .optimistic_entries
            .iter_mut()
            .find(|entry| entry.local_id == local_id)
        {
            entry.state = OptimisticState::Failed;
            self.mark_chat_render_cache_dirty();
        }
    }

    pub(crate) fn rekey_new_session_optimistic_entries(
        &mut self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) {
        for entry in &mut self.optimistic_entries {
            if entry.scope == ConversationScope::NewSession(workspace_id) {
                entry.scope = ConversationScope::Session(session_id);
            }
        }
        self.mark_chat_render_cache_dirty();
    }

    pub(crate) fn reset_conversation_state(&mut self) {
        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        self.conversation_process_entries.clear();
        self.conversation_process_order.clear();
        self.conversation_bootstrapping = false;
        self.conversation_backfilling = false;
        self.optimistic_entries.clear();
        self.reset_chat_render_cache();
    }

    pub(crate) fn refresh_conversation_history(&mut self) {
        let Some(session_id) = self.bundle.selected_session_id else {
            self.conversation_process_entries.clear();
            self.conversation_process_order.clear();
            self.conversation_bootstrapping = false;
            self.conversation_backfilling = false;
            return;
        };
        let mut process_order = self
            .bundle
            .process_map
            .values()
            .filter(|process| {
                !process.dropped
                    && process.run_reason
                        != db::models::execution_process::ExecutionProcessRunReason::DevServer
            })
            .map(|process| process.id)
            .collect::<Vec<_>>();
        process_order.sort_by_key(|process_id| {
            self.bundle
                .process_map
                .get(process_id)
                .map(|process| process.created_at)
        });
        if process_order == self.conversation_process_order
            && process_order
                .iter()
                .all(|process_id| self.conversation_process_entries.contains_key(process_id))
        {
            return;
        }

        if let Some(handle) = self.conversation_loader.take() {
            handle.abort();
        }
        self.conversation_process_order = process_order.clone();
        self.conversation_process_entries
            .retain(|process_id, _| process_order.contains(process_id));
        self.conversation_bootstrapping = !process_order.is_empty();
        self.conversation_backfilling = false;

        if process_order.is_empty() {
            return;
        }

        let recent_ids = initial_conversation_process_ids(&process_order, &self.bundle.process_map);
        let remaining_ids = process_order
            .iter()
            .copied()
            .filter(|process_id| !recent_ids.contains(process_id))
            .collect::<Vec<_>>();
        let api = self.api.clone();
        let tx = self.tx.clone();
        self.conversation_loader = Some(tokio::spawn(async move {
            for process_id in recent_ids.iter().rev() {
                match api.fetch_process_log_snapshot(*process_id).await {
                    Ok(entries) => {
                        let _ = tx.send(crate::model::NetEvent::ConversationHistoryLoaded {
                            session_id,
                            process_id: *process_id,
                            entries,
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(crate::model::NetEvent::Error(error.to_string()));
                    }
                }
            }
            let _ = tx.send(crate::model::NetEvent::ConversationBootstrapComplete { session_id });
            for process_id in remaining_ids.into_iter().rev() {
                match api.fetch_process_log_snapshot(process_id).await {
                    Ok(entries) => {
                        let _ = tx.send(crate::model::NetEvent::ConversationHistoryLoaded {
                            session_id,
                            process_id,
                            entries,
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(crate::model::NetEvent::Error(error.to_string()));
                    }
                }
            }
            let _ = tx.send(crate::model::NetEvent::ConversationBackfillComplete { session_id });
        }));
    }

    pub(crate) fn reconcile_optimistic_entries(&mut self) {
        let Some(scope) = self.current_conversation_scope() else {
            self.optimistic_entries.clear();
            self.mark_chat_render_cache_dirty();
            return;
        };
        let before = self.optimistic_entries.len();
        let canonical_messages = self
            .canonical_chat_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                PatchType::NormalizedEntry(entry)
                    if matches!(entry.entry_type, NormalizedEntryType::UserMessage) =>
                {
                    Some(entry.content.trim().to_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.optimistic_entries.retain(|entry| {
            entry.scope != scope
                || entry.state == OptimisticState::Failed
                || !canonical_messages
                    .iter()
                    .any(|message| message == entry.message.trim())
        });
        if self.optimistic_entries.len() != before {
            self.mark_chat_render_cache_dirty();
        }
    }

    fn canonical_chat_entries(&self) -> Vec<PatchType> {
        self.conversation_process_order
            .iter()
            .flat_map(|process_id| self.process_chat_entries(*process_id))
            .collect()
    }

    fn process_chat_entries(&self, process_id: Uuid) -> Vec<PatchType> {
        let mut entries = Vec::new();
        let process_entries = self
            .conversation_process_entries
            .get(&process_id)
            .cloned()
            .unwrap_or_default();

        let has_user_message = process_entries.iter().any(|entry| {
            matches!(
                entry,
                PatchType::NormalizedEntry(entry)
                    if matches!(entry.entry_type, NormalizedEntryType::UserMessage)
            )
        });

        if !has_user_message
            && let Some(process) = self.bundle.process_map.get(&process_id)
            && let Some(prompt) = process_prompt(process)
        {
            entries.push(PatchType::NormalizedEntry(NormalizedEntry {
                timestamp: Some(process.created_at.to_rfc3339()),
                entry_type: NormalizedEntryType::UserMessage,
                content: prompt,
                metadata: None,
            }));
        }

        entries.extend(process_entries);
        entries
    }

    pub(crate) fn chat_line_count(&self) -> usize {
        self.chat_lines().len()
    }

    pub(crate) fn chat_render_cache(&mut self, width: usize) -> &ChatRenderCache {
        let width = width.max(1);
        let should_rebuild = self
            .chat_render_cache
            .as_ref()
            .is_none_or(|cache| cache.width != width)
            || (self.chat_render_cache_dirty
                && self
                    .last_chat_render_cache_build
                    .is_none_or(|built| built.elapsed() >= Duration::from_millis(33)));
        if should_rebuild {
            self.chat_render_cache = Some(self.build_chat_render_cache(width));
            self.chat_render_cache_dirty = false;
            self.last_chat_render_cache_build = Some(std::time::Instant::now());
        }
        self.chat_render_cache
            .as_ref()
            .expect("chat cache populated")
    }

    fn build_chat_render_cache(&self, width: usize) -> ChatRenderCache {
        let lines = wrap_lines(self.chat_lines(), width);
        let latest_token_usage = self
            .canonical_chat_entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                PatchType::NormalizedEntry(entry) => match &entry.entry_type {
                    NormalizedEntryType::TokenUsageInfo(info) => {
                        Some((info.total_tokens, info.model_context_window))
                    }
                    _ => None,
                },
                _ => None,
            });
        ChatRenderCache {
            width,
            lines,
            latest_token_usage,
        }
    }

    pub(crate) fn mark_chat_render_cache_dirty(&mut self) {
        self.chat_render_cache_dirty = true;
    }

    fn reset_chat_render_cache(&mut self) {
        self.chat_render_cache = None;
        self.chat_render_cache_dirty = true;
        self.last_chat_render_cache_build = None;
    }

    fn chat_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.conversation_bootstrapping && self.conversation_process_entries.is_empty() {
            lines.push(Line::styled(
                "Loading recent conversation...",
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        } else if self.conversation_backfilling {
            lines.push(Line::styled(
                "Loading older messages...",
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        }
        if let QueueStatus::Queued { message } = &self.queue_status {
            lines.push(Line::styled(
                "queued follow-up",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            for line in message.data.message.lines() {
                lines.push(Line::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::LightYellow),
                ));
            }
            lines.push(Line::styled(
                format!("  executor {}", message.data.executor_config.executor),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::raw(""));
        }
        lines.extend(
            self.canonical_chat_entries()
                .iter()
                .flat_map(render_chat_entry)
                .collect::<Vec<_>>(),
        );
        if let Some(scope) = self.current_conversation_scope() {
            for entry in self
                .optimistic_entries
                .iter()
                .filter(|entry| entry.scope == scope)
            {
                lines.extend(render_optimistic_chat_entry(entry));
            }
        }
        lines
    }
}
