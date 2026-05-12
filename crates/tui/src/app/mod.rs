pub mod editor;
pub mod pr;
pub mod runtime;
pub mod search;
pub mod shell;
pub mod state;
pub mod status;
pub mod update;
pub mod util;

pub use self::state::App;
pub(crate) use self::{
    search::{highlight_line_matches, highlight_text_span},
    state::{
        AgentPickerState, ConversationSearchState, DetailSection, PrCreateField, SearchPromptState,
        SearchTarget, SessionRenameState, ToolCallDisplayMode, WorkspaceProjectFilter,
        WorkspaceProjectFilterPickerState,
    },
    util::{
        default_variant_to_none, fuzzy_contains, model_key, selected_list_offset, viewport_capacity,
    },
};
