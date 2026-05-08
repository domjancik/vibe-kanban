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
    api::net_error,
    app::{App, ToolCallDisplayMode},
    conversation::{
        initial_conversation_process_ids, process_prompt, render_chat_entry,
        render_collapsed_tool_run, render_optimistic_chat_entry, wrap_lines,
    },
    model::{PatchType, QueueStatus},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationScope {
    Session(Uuid),
    NewSession(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub user_message_offsets: Vec<usize>,
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
                        let _ = tx.send(net_error(
                            format!("conversation bootstrap snapshot for process {process_id}"),
                            error,
                        ));
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
                        let _ = tx.send(net_error(
                            format!("conversation backfill snapshot for process {process_id}"),
                            error,
                        ));
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
        let (lines, user_message_offsets) = self.rendered_chat_lines(width);
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
            user_message_offsets,
            latest_token_usage,
        }
    }

    pub(crate) fn jump_to_user_message(&mut self, forward: bool, visible_lines: usize) {
        let width = self
            .chat_render_cache
            .as_ref()
            .map(|cache| cache.width)
            .unwrap_or(80);
        let visible_lines = visible_lines.max(1);
        let requested_end_offset = self.chat_end_offset as usize;
        let (total_lines, top_offset, user_message_offsets) = {
            let cache = self.chat_render_cache(width);
            let (_, _, _, top_offset) = crate::conversation::chat_window_bounds(
                cache.lines.len(),
                visible_lines,
                requested_end_offset,
            );
            (
                cache.lines.len(),
                top_offset,
                cache.user_message_offsets.clone(),
            )
        };
        if user_message_offsets.is_empty() {
            return;
        }

        let target_top = if forward {
            user_message_offsets
                .iter()
                .copied()
                .find(|offset| *offset > top_offset)
                .or_else(|| {
                    (requested_end_offset != 0).then_some(total_lines.saturating_sub(visible_lines))
                })
        } else {
            user_message_offsets
                .iter()
                .copied()
                .rev()
                .find(|offset| *offset < top_offset)
                .or_else(|| user_message_offsets.first().copied())
        };

        if let Some(target_top) = target_top {
            self.chat_end_offset = total_lines
                .saturating_sub(visible_lines.saturating_add(target_top))
                .min(u16::MAX as usize) as u16;
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
        self.rendered_chat_lines(usize::MAX).0
    }

    fn rendered_chat_lines(&self, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines = Vec::new();
        let mut user_message_offsets = Vec::new();
        if self.conversation_bootstrapping && self.conversation_process_entries.is_empty() {
            lines.extend(wrap_lines(
                vec![Line::styled(
                    "Loading recent conversation...",
                    Style::default().fg(Color::DarkGray),
                )],
                width,
            ));
            lines.extend(wrap_lines(vec![Line::raw("")], width));
            return (lines, user_message_offsets);
        } else if self.conversation_backfilling {
            lines.extend(wrap_lines(
                vec![Line::styled(
                    "Loading older messages...",
                    Style::default().fg(Color::DarkGray),
                )],
                width,
            ));
            lines.extend(wrap_lines(vec![Line::raw("")], width));
        }
        if let QueueStatus::Queued { message } = &self.queue_status {
            let mut queue_lines = vec![Line::styled(
                "queued follow-up",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )];
            for line in message.data.message.lines() {
                queue_lines.push(Line::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::LightYellow),
                ));
            }
            queue_lines.push(Line::styled(
                format!("  executor {}", message.data.executor_config.executor),
                Style::default().fg(Color::DarkGray),
            ));
            queue_lines.push(Line::raw(""));
            lines.extend(wrap_lines(queue_lines, width));
        }
        let canonical_entries = self.canonical_chat_entries();
        let mut entry_index = 0usize;
        while entry_index < canonical_entries.len() {
            if self.tool_call_display_mode == ToolCallDisplayMode::Collapsed {
                let tool_run_len = canonical_entries[entry_index..]
                    .iter()
                    .take_while(|entry| {
                        matches!(
                            entry,
                            PatchType::NormalizedEntry(normalized)
                                if matches!(normalized.entry_type, NormalizedEntryType::ToolUse { .. })
                        )
                    })
                    .count();
                if tool_run_len > 1 {
                    let tool_run = canonical_entries[entry_index..entry_index + tool_run_len]
                        .iter()
                        .filter_map(|entry| match entry {
                            PatchType::NormalizedEntry(normalized) => Some(normalized.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    lines.extend(wrap_lines(render_collapsed_tool_run(&tool_run), width));
                    entry_index += tool_run_len;
                    continue;
                }
            }
            let entry = &canonical_entries[entry_index];
            let is_user_message = matches!(
                &entry,
                PatchType::NormalizedEntry(entry)
                    if matches!(entry.entry_type, NormalizedEntryType::UserMessage)
            );
            let rendered = render_chat_entry(&entry);
            if is_user_message {
                user_message_offsets.push(lines.len());
            }
            lines.extend(wrap_lines(rendered, width));
            entry_index += 1;
        }
        if let Some(scope) = self.current_conversation_scope() {
            for entry in self
                .optimistic_entries
                .iter()
                .filter(|entry| entry.scope == scope)
            {
                user_message_offsets.push(lines.len());
                lines.extend(wrap_lines(render_optimistic_chat_entry(entry), width));
            }
        }
        (lines, user_message_offsets)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use db::models::execution_process::{
        ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus, ExecutorActionField,
    };
    use executors::{
        actions::{
            ExecutorAction, ExecutorActionType, coding_agent_initial::CodingAgentInitialRequest,
        },
        executor_discovery::ExecutorDiscoveredOptions,
        logs::{NormalizedEntry, NormalizedEntryType, TokenUsageInfo},
        profile::{ExecutorConfig, ExecutorConfigs},
    };
    use sqlx::types::Json;
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    use super::{OptimisticConversationEntry, OptimisticState};
    use crate::{
        api::{Api, WorkspaceSubscriptions},
        app::App,
        conversation::ConversationScope,
        editor::ComposerEditorMode,
        model::{Focus, Pane, PatchType, QueueStatus, WorkspaceBundle},
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
            composer_config: Some(ExecutorConfig::new(
                executors::executors::BaseCodingAgent::Codex,
            )),
            composer_options: Some(ExecutorDiscoveredOptions::default()),
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
            chat_render_cache_dirty: false,
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
            session_rename: None,
            search_prompt: None,
            conversation_search: None,
            tool_call_display_mode: crate::app::ToolCallDisplayMode::Expanded,
            actions_in_flight: Default::default(),
            creating_new_session: false,
            should_quit: false,
        }
    }

    fn process(id: Uuid, prompt: &str, created_at_second: i64) -> ExecutionProcess {
        let created_at = Utc.timestamp_opt(created_at_second, 0).unwrap();
        ExecutionProcess {
            id,
            session_id: Uuid::new_v4(),
            run_reason: ExecutionProcessRunReason::CodingAgent,
            executor_action: Json(ExecutorActionField::ExecutorAction(ExecutorAction::new(
                ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                    prompt: prompt.to_string(),
                    executor_config: ExecutorConfig::new(
                        executors::executors::BaseCodingAgent::Codex,
                    ),
                    working_dir: None,
                }),
                None,
            ))),
            status: ExecutionProcessStatus::Completed,
            exit_code: Some(0),
            dropped: false,
            started_at: created_at,
            completed_at: Some(created_at),
            created_at,
            updated_at: created_at,
        }
    }

    fn user_message(content: &str) -> PatchType {
        PatchType::NormalizedEntry(NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::UserMessage,
            content: content.to_string(),
            metadata: None,
        })
    }

    #[test]
    fn chat_render_cache_tracks_user_message_offsets() {
        let mut app = test_app();
        let session_id = Uuid::new_v4();
        let process_id = Uuid::new_v4();
        app.bundle.selected_session_id = Some(session_id);
        app.conversation_process_order = vec![process_id];
        app.conversation_process_entries.insert(
            process_id,
            vec![
                user_message("first"),
                PatchType::NormalizedEntry(NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::AssistantMessage,
                    content: "reply".to_string(),
                    metadata: None,
                }),
                user_message("second"),
            ],
        );

        let cache = app.chat_render_cache(40);
        assert_eq!(cache.user_message_offsets.len(), 2);
        assert_eq!(cache.user_message_offsets[0], 0);
        assert!(cache.user_message_offsets[1] > cache.user_message_offsets[0]);

        let first_label = cache.lines[cache.user_message_offsets[0]]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let second_label = cache.lines[cache.user_message_offsets[1]]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(first_label.contains("user"));
        assert!(second_label.contains("user"));
    }

    #[test]
    fn jump_to_user_message_moves_between_user_turns() {
        let mut app = test_app();
        let session_id = Uuid::new_v4();
        let process_id = Uuid::new_v4();
        app.bundle.selected_session_id = Some(session_id);
        app.conversation_process_order = vec![process_id];
        app.conversation_process_entries.insert(
            process_id,
            vec![
                user_message("first"),
                PatchType::NormalizedEntry(NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::AssistantMessage,
                    content: "reply".to_string(),
                    metadata: None,
                }),
                user_message("second"),
            ],
        );
        let _ = app.chat_render_cache(40);

        app.chat_end_offset = 0;
        app.jump_to_user_message(false, 2);
        let (line_count, second_offset, end_offset) = {
            let cache = app.chat_render_cache(40);
            (
                cache.lines.len(),
                cache.user_message_offsets[1],
                app.chat_end_offset as usize,
            )
        };
        let (_, _, _, top_offset) =
            crate::conversation::chat_window_bounds(line_count, 2, end_offset);
        assert_eq!(top_offset, second_offset);

        app.jump_to_user_message(true, 2);
        assert_eq!(app.chat_end_offset, 0);
    }

    #[test]
    fn current_conversation_scope_covers_new_existing_and_none() {
        let mut app = test_app();
        assert_eq!(app.current_conversation_scope(), None);

        let workspace_id = Uuid::new_v4();
        app.selected_workspace_id = Some(workspace_id);
        app.creating_new_session = true;
        assert_eq!(
            app.current_conversation_scope(),
            Some(ConversationScope::NewSession(workspace_id))
        );

        let session_id = Uuid::new_v4();
        app.creating_new_session = false;
        app.bundle.selected_session_id = Some(session_id);
        assert_eq!(
            app.current_conversation_scope(),
            Some(ConversationScope::Session(session_id))
        );
    }

    #[test]
    fn optimistic_entry_mutations_mark_cache_and_scope_correctly() {
        let mut app = test_app();
        app.chat_render_cache_dirty = false;
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let local_id = app.push_optimistic_entry(
            ConversationScope::NewSession(workspace_id),
            "draft".to_string(),
            ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex),
        );
        assert!(app.chat_render_cache_dirty);
        assert_eq!(app.optimistic_entries.len(), 1);

        app.chat_render_cache_dirty = false;
        app.mark_optimistic_failed(local_id);
        assert_eq!(app.optimistic_entries[0].state, OptimisticState::Failed);
        assert!(app.chat_render_cache_dirty);

        app.chat_render_cache_dirty = false;
        app.rekey_new_session_optimistic_entries(workspace_id, session_id);
        assert_eq!(
            app.optimistic_entries[0].scope,
            ConversationScope::Session(session_id)
        );
        assert!(app.chat_render_cache_dirty);
    }

    #[tokio::test]
    async fn reset_conversation_state_clears_cached_inputs() {
        let mut app = test_app();
        let process_id = Uuid::new_v4();
        app.conversation_loader = Some(tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }));
        app.conversation_process_entries
            .insert(process_id, vec![user_message("hello")]);
        app.conversation_process_order.push(process_id);
        app.conversation_bootstrapping = true;
        app.conversation_backfilling = true;
        app.optimistic_entries.push(OptimisticConversationEntry {
            local_id: Uuid::new_v4(),
            scope: ConversationScope::Session(Uuid::new_v4()),
            message: "pending".to_string(),
            executor_config: ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex),
            state: OptimisticState::Pending,
        });
        app.chat_render_cache = Some(super::ChatRenderCache {
            width: 80,
            lines: Vec::new(),
            user_message_offsets: Vec::new(),
            latest_token_usage: None,
        });
        app.last_chat_render_cache_build = Some(std::time::Instant::now());

        app.reset_conversation_state();

        assert!(app.conversation_loader.is_none());
        assert!(app.conversation_process_entries.is_empty());
        assert!(app.conversation_process_order.is_empty());
        assert!(!app.conversation_bootstrapping);
        assert!(!app.conversation_backfilling);
        assert!(app.optimistic_entries.is_empty());
        assert!(app.chat_render_cache.is_none());
        assert!(app.chat_render_cache_dirty);
        assert!(app.last_chat_render_cache_build.is_none());
    }

    #[test]
    fn process_chat_entries_synthesizes_missing_user_prompt_and_reconcile_drops_matches() {
        let mut app = test_app();
        let session_id = Uuid::new_v4();
        let process_id = Uuid::new_v4();
        app.bundle.selected_session_id = Some(session_id);
        app.bundle
            .process_map
            .insert(process_id, process(process_id, "hello there", 10));
        app.conversation_process_order.push(process_id);
        app.conversation_process_entries.insert(
            process_id,
            vec![PatchType::NormalizedEntry(NormalizedEntry {
                timestamp: None,
                entry_type: NormalizedEntryType::AssistantMessage,
                content: "response".to_string(),
                metadata: None,
            })],
        );

        let entries = app.process_chat_entries(process_id);
        assert!(matches!(
            entries.first(),
            Some(PatchType::NormalizedEntry(entry))
                if matches!(entry.entry_type, NormalizedEntryType::UserMessage)
                    && entry.content == "hello there"
        ));

        app.optimistic_entries = vec![
            OptimisticConversationEntry {
                local_id: Uuid::new_v4(),
                scope: ConversationScope::Session(session_id),
                message: "hello there".to_string(),
                executor_config: ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex),
                state: OptimisticState::Pending,
            },
            OptimisticConversationEntry {
                local_id: Uuid::new_v4(),
                scope: ConversationScope::Session(session_id),
                message: "hello there".to_string(),
                executor_config: ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex),
                state: OptimisticState::Failed,
            },
        ];

        app.reconcile_optimistic_entries();

        assert_eq!(app.optimistic_entries.len(), 1);
        assert_eq!(app.optimistic_entries[0].state, OptimisticState::Failed);
    }

    #[test]
    fn chat_cache_and_loading_banners_reflect_canonical_state() {
        let mut app = test_app();
        let process_id = Uuid::new_v4();
        app.bundle
            .process_map
            .insert(process_id, process(process_id, "hi", 10));
        app.conversation_process_order.push(process_id);
        app.conversation_process_entries.insert(
            process_id,
            vec![PatchType::NormalizedEntry(NormalizedEntry {
                timestamp: None,
                entry_type: NormalizedEntryType::TokenUsageInfo(TokenUsageInfo {
                    total_tokens: 42,
                    model_context_window: 128_000,
                }),
                content: String::new(),
                metadata: None,
            })],
        );

        let cache = app.build_chat_render_cache(40);
        assert_eq!(cache.latest_token_usage, Some((42, 128_000)));

        app.conversation_bootstrapping = true;
        app.conversation_process_entries.clear();
        let bootstrap_lines = app.chat_lines();
        let bootstrap_text = bootstrap_lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(bootstrap_text.contains("Loading recent conversation"));

        app.conversation_bootstrapping = false;
        app.conversation_backfilling = true;
        let backfill_lines = app.chat_lines();
        let backfill_text = backfill_lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(backfill_text.contains("Loading older messages"));
    }
}
