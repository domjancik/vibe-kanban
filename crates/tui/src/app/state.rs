use std::collections::HashMap;

use db::models::workspace::WorkspaceWithStatus;
use executors::{
    executor_discovery::ExecutorDiscoveredOptions,
    profile::{ExecutorConfig, ExecutorConfigs, ExecutorProfileId},
};
use ratatui::{text::Text, widgets::ListItem};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

use crate::{
    api::{Api, WorkspaceSubscriptions},
    conversation::{ChatRenderCache, OptimisticConversationEntry},
    editor::{ComposerEditorMode, VimOperator},
    model::{Focus, NetEvent, Pane, PatchType, QueueStatus, WorkspaceBundle, WorkspaceSummary},
};

pub(crate) struct AgentPickerState {
    pub(crate) query: String,
    pub(crate) selected: usize,
}

pub(crate) struct SessionRenameState {
    pub(crate) session_id: Uuid,
    pub(crate) name: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchTarget {
    Workspaces,
    Sessions,
    Conversation,
}

pub(crate) struct SearchPromptState {
    pub(crate) target: SearchTarget,
    pub(crate) query: String,
    pub(crate) cursor: usize,
    pub(crate) original_query: String,
    pub(crate) original_conversation_search: Option<ConversationSearchState>,
    pub(crate) original_chat_end_offset: u16,
}

#[derive(Clone)]
pub(crate) struct ConversationSearchState {
    pub(crate) query: String,
    pub(crate) matches: Vec<usize>,
    pub(crate) current_match: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ActionInFlightState {
    pub(crate) prompt_submit: bool,
    pub(crate) queue_mutation: bool,
    pub(crate) pin_toggle: bool,
    pub(crate) archive_toggle: bool,
    pub(crate) dev_server: bool,
    pub(crate) cleanup: bool,
    pub(crate) open_editor: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ComposerHeightCache {
    pub(crate) width: u16,
    pub(crate) revision: u64,
    pub(crate) height: u16,
}

#[derive(Clone)]
pub(crate) struct WorkspaceListRenderCache {
    pub(crate) revision: u64,
    pub(crate) query: String,
    pub(crate) items: Vec<ListItem<'static>>,
    pub(crate) row_ids: Vec<Option<Uuid>>,
}

#[derive(Clone)]
pub(crate) struct DetailPaneRenderCache {
    pub(crate) revision: u64,
    pub(crate) session_query: String,
    pub(crate) renaming_session_id: Option<Uuid>,
    pub(crate) workspace_info: Text<'static>,
    pub(crate) session_items: Vec<ListItem<'static>>,
    pub(crate) session_ids: Vec<Option<Uuid>>,
    pub(crate) process_items: Vec<ListItem<'static>>,
}

pub struct App {
    pub(crate) api: Api,
    pub(crate) rx: UnboundedReceiver<NetEvent>,
    pub(crate) tx: UnboundedSender<NetEvent>,
    pub(crate) workspace_streams: Vec<tokio::task::JoinHandle<()>>,
    pub(crate) summary_streams: Vec<tokio::task::JoinHandle<()>>,
    pub(crate) subscriptions: WorkspaceSubscriptions,
    pub(crate) active_workspaces: HashMap<Uuid, WorkspaceWithStatus>,
    pub(crate) archived_workspaces: HashMap<Uuid, WorkspaceWithStatus>,
    pub(crate) summaries: HashMap<Uuid, WorkspaceSummary>,
    pub(crate) selected_workspace_id: Option<Uuid>,
    pub(crate) selected_pane: Pane,
    pub(crate) focus: Focus,
    pub(crate) maximized_panel: bool,
    pub(crate) show_archived: bool,
    pub(crate) filter: String,
    pub(crate) session_filter: String,
    pub(crate) workspace_list_revision: u64,
    pub(crate) detail_revision: u64,
    pub(crate) workspace_list_cache: Option<WorkspaceListRenderCache>,
    pub(crate) detail_pane_cache: Option<DetailPaneRenderCache>,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) bundle: WorkspaceBundle,
    pub(crate) executor_profiles: ExecutorConfigs,
    pub(crate) default_executor_profile: Option<ExecutorProfileId>,
    pub(crate) composer_config: Option<ExecutorConfig>,
    pub(crate) composer_options: Option<ExecutorDiscoveredOptions>,
    pub(crate) composer: String,
    pub(crate) composer_cursor: usize,
    pub(crate) editor_mode: ComposerEditorMode,
    pub(crate) vim_pending_operator: Option<VimOperator>,
    pub(crate) composer_dirty: bool,
    pub(crate) composer_edit_revision: u64,
    pub(crate) composer_height_cache: Option<ComposerHeightCache>,
    pub(crate) draft_save_in_flight: bool,
    pub(crate) composer_queue_conflict: bool,
    pub(crate) composer_scratch_id: Option<Uuid>,
    pub(crate) composer_scratch_loaded: bool,
    pub(crate) queue_session_id: Option<Uuid>,
    pub(crate) queue_status: QueueStatus,
    pub(crate) queue_pending: bool,
    pub(crate) last_composer_edit: Option<std::time::Instant>,
    pub(crate) chat_end_offset: u16,
    pub(crate) chat_render_cache: Option<ChatRenderCache>,
    pub(crate) chat_render_cache_dirty: bool,
    pub(crate) last_chat_render_cache_build: Option<std::time::Instant>,
    pub(crate) conversation_loader: Option<tokio::task::JoinHandle<()>>,
    pub(crate) conversation_process_entries: HashMap<Uuid, Vec<PatchType>>,
    pub(crate) conversation_process_order: Vec<Uuid>,
    pub(crate) conversation_bootstrapping: bool,
    pub(crate) conversation_backfilling: bool,
    pub(crate) optimistic_entries: Vec<OptimisticConversationEntry>,
    pub(crate) notes_cursor: usize,
    pub(crate) notes_edit_revision: u64,
    pub(crate) notes_save_in_flight: bool,
    pub(crate) agent_picker: Option<AgentPickerState>,
    pub(crate) session_rename: Option<SessionRenameState>,
    pub(crate) search_prompt: Option<SearchPromptState>,
    pub(crate) conversation_search: Option<ConversationSearchState>,
    pub(crate) actions_in_flight: ActionInFlightState,
    pub(crate) creating_new_session: bool,
    pub(crate) should_quit: bool,
}

impl App {
    pub fn new(api: Api) -> Self {
        let (tx, rx) = unbounded_channel();
        let workspace_streams = api.spawn_workspace_streams(tx.clone());
        let summary_streams = api.spawn_summary_pollers(tx.clone());
        api.load_user_system_info(tx.clone());
        Self {
            api,
            rx,
            tx,
            workspace_streams,
            summary_streams,
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
            executor_profiles: ExecutorConfigs {
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
            actions_in_flight: ActionInFlightState::default(),
            creating_new_session: false,
            should_quit: false,
        }
    }

    pub(crate) fn mark_workspace_list_dirty(&mut self) {
        self.workspace_list_revision = self.workspace_list_revision.saturating_add(1);
        self.workspace_list_cache = None;
    }

    pub(crate) fn mark_detail_dirty(&mut self) {
        self.detail_revision = self.detail_revision.saturating_add(1);
        self.detail_pane_cache = None;
    }
}
