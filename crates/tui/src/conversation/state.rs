use executors::profile::ExecutorConfig;
use ratatui::text::Line;
use uuid::Uuid;

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
