use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::Pane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppIntent {
    CancelNewSession,
    ToggleComposerEditorMode,
    ToggleMaximizedPanel,
    Quit,
    FocusNext,
    FocusPrev,
    ShowHelp,
    OpenSearch,
    SelectPane(Pane),
    ToggleShowArchived,
    EnterEditMode,
    StartNewSession,
    TogglePinned,
    ToggleArchived,
    StartDevServer,
    RunCleanup,
    StopWorkspace,
    OpenEditor,
    OpenSessionRename,
    CycleExecutor,
    CycleVariant,
    CycleModel,
    CycleReasoning,
    OpenAgentPicker,
    CyclePermissionMode,
    QueuePrompt,
    CancelQueuedPrompt,
    DiscardDraft,
    ToggleDiffViewMode,
    ToggleToolRunCollapse,
    PrevUserMessage,
    NextUserMessage,
    Enter,
    JumpToStart,
    JumpToEnd,
    MoveSelection(i32),
    PageSelection(i32),
    EnterTerminalInputMode,
}

pub fn map_app_key(
    key: KeyEvent,
    creating_new_session: bool,
    selected_pane: &Pane,
) -> Option<AppIntent> {
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } if creating_new_session && *selected_pane == Pane::Chat => {
            Some(AppIntent::CancelNewSession)
        }
        KeyEvent {
            code: KeyCode::F(2),
            ..
        } => Some(AppIntent::ToggleComposerEditorMode),
        KeyEvent {
            code: KeyCode::Char('w'),
            modifiers,
            ..
        } if modifiers == KeyModifiers::CONTROL => Some(AppIntent::ToggleMaximizedPanel),
        KeyEvent {
            code: KeyCode::Char('q'),
            ..
        } => Some(AppIntent::Quit),
        KeyEvent {
            code: KeyCode::Tab, ..
        } => Some(AppIntent::FocusNext),
        KeyEvent {
            code: KeyCode::BackTab,
            ..
        } => Some(AppIntent::FocusPrev),
        KeyEvent {
            code: KeyCode::Char('?'),
            ..
        } => Some(AppIntent::ShowHelp),
        KeyEvent {
            code: KeyCode::Char('/'),
            ..
        } => Some(AppIntent::OpenSearch),
        KeyEvent {
            code: KeyCode::Char('1'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Chat)),
        KeyEvent {
            code: KeyCode::Char('2'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Changes)),
        KeyEvent {
            code: KeyCode::Char('3'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Logs)),
        KeyEvent {
            code: KeyCode::Char('4'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Git)),
        KeyEvent {
            code: KeyCode::Char('5'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Terminal)),
        KeyEvent {
            code: KeyCode::Char('6'),
            ..
        } => Some(AppIntent::SelectPane(Pane::Notes)),
        KeyEvent {
            code: KeyCode::Char('a'),
            ..
        } => Some(AppIntent::ToggleShowArchived),
        KeyEvent {
            code: KeyCode::Char('i'),
            ..
        } => Some(AppIntent::EnterEditMode),
        KeyEvent {
            code: KeyCode::Char('n'),
            ..
        } => Some(AppIntent::StartNewSession),
        KeyEvent {
            code: KeyCode::Char('p'),
            ..
        } => Some(AppIntent::TogglePinned),
        KeyEvent {
            code: KeyCode::Char('x'),
            ..
        } => Some(AppIntent::ToggleArchived),
        KeyEvent {
            code: KeyCode::Char('s'),
            ..
        } => Some(AppIntent::StartDevServer),
        KeyEvent {
            code: KeyCode::Char('c'),
            ..
        } => Some(AppIntent::RunCleanup),
        KeyEvent {
            code: KeyCode::Char('v'),
            ..
        } => Some(AppIntent::StopWorkspace),
        KeyEvent {
            code: KeyCode::Char('e'),
            ..
        } => Some(AppIntent::OpenEditor),
        KeyEvent {
            code: KeyCode::Char('r'),
            ..
        } => Some(AppIntent::OpenSessionRename),
        KeyEvent {
            code: KeyCode::Char('E'),
            ..
        } => Some(AppIntent::CycleExecutor),
        KeyEvent {
            code: KeyCode::Char('V'),
            ..
        } => Some(AppIntent::CycleVariant),
        KeyEvent {
            code: KeyCode::Char('M'),
            ..
        } => Some(AppIntent::CycleModel),
        KeyEvent {
            code: KeyCode::Char('R'),
            ..
        } => Some(AppIntent::CycleReasoning),
        KeyEvent {
            code: KeyCode::Char('A'),
            ..
        } => Some(AppIntent::OpenAgentPicker),
        KeyEvent {
            code: KeyCode::Char('P'),
            ..
        } => Some(AppIntent::CyclePermissionMode),
        KeyEvent {
            code: KeyCode::Char('Q'),
            ..
        } => Some(AppIntent::QueuePrompt),
        KeyEvent {
            code: KeyCode::Char('X'),
            ..
        } => Some(AppIntent::CancelQueuedPrompt),
        KeyEvent {
            code: KeyCode::Char('D'),
            ..
        } => Some(AppIntent::DiscardDraft),
        KeyEvent {
            code: KeyCode::Char('b'),
            ..
        } => Some(AppIntent::ToggleDiffViewMode),
        KeyEvent {
            code: KeyCode::Char('T'),
            ..
        } => Some(AppIntent::ToggleToolRunCollapse),
        KeyEvent {
            code: KeyCode::Char('['),
            ..
        } => Some(AppIntent::PrevUserMessage),
        KeyEvent {
            code: KeyCode::Char(']'),
            ..
        } => Some(AppIntent::NextUserMessage),
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => Some(AppIntent::Enter),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => Some(AppIntent::JumpToStart),
        KeyEvent {
            code: KeyCode::End, ..
        } => Some(AppIntent::JumpToEnd),
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER) => Some(AppIntent::JumpToStart),
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER) => Some(AppIntent::JumpToEnd),
        KeyEvent {
            code: KeyCode::Up,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER) => Some(AppIntent::JumpToStart),
        KeyEvent {
            code: KeyCode::Down,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER) => Some(AppIntent::JumpToEnd),
        KeyEvent {
            code: KeyCode::Char('j') | KeyCode::Down,
            ..
        } => Some(AppIntent::MoveSelection(1)),
        KeyEvent {
            code: KeyCode::Char('k') | KeyCode::Up,
            ..
        } => Some(AppIntent::MoveSelection(-1)),
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => Some(AppIntent::PageSelection(1)),
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => Some(AppIntent::PageSelection(-1)),
        KeyEvent {
            code: KeyCode::Char('t'),
            ..
        } => Some(AppIntent::EnterTerminalInputMode),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::{
        input::{AppIntent, map_app_key},
        model::Pane,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn maps_navigation_keys_to_intents() {
        assert_eq!(
            map_app_key(key(KeyCode::Char('/')), false, &Pane::Chat),
            Some(AppIntent::OpenSearch)
        );
        assert_eq!(
            map_app_key(key(KeyCode::Tab), false, &Pane::Chat),
            Some(AppIntent::FocusNext)
        );
        assert_eq!(
            map_app_key(
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                false,
                &Pane::Chat
            ),
            Some(AppIntent::ToggleMaximizedPanel)
        );
        assert_eq!(
            map_app_key(key(KeyCode::PageDown), false, &Pane::Chat),
            Some(AppIntent::PageSelection(1))
        );
        assert_eq!(
            map_app_key(key(KeyCode::Char('j')), false, &Pane::Chat),
            Some(AppIntent::MoveSelection(1))
        );
    }

    #[test]
    fn maps_pane_shortcuts_to_specific_panes() {
        assert_eq!(
            map_app_key(key(KeyCode::Char('1')), false, &Pane::Git),
            Some(AppIntent::SelectPane(Pane::Chat))
        );
        assert_eq!(
            map_app_key(key(KeyCode::Char('6')), false, &Pane::Git),
            Some(AppIntent::SelectPane(Pane::Notes))
        );
    }

    #[test]
    fn esc_only_cancels_new_session_for_chat() {
        assert_eq!(
            map_app_key(key(KeyCode::Esc), true, &Pane::Chat),
            Some(AppIntent::CancelNewSession)
        );
        assert_eq!(map_app_key(key(KeyCode::Esc), false, &Pane::Chat), None);
        assert_eq!(map_app_key(key(KeyCode::Esc), true, &Pane::Notes), None);
    }

    #[test]
    fn super_arrows_map_to_boundary_jumps() {
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER);

        assert_eq!(
            map_app_key(right, false, &Pane::Chat),
            Some(AppIntent::JumpToEnd)
        );
        assert_eq!(
            map_app_key(up, false, &Pane::Chat),
            Some(AppIntent::JumpToStart)
        );
    }
}
