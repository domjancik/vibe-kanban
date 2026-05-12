use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::TextEditAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputEvent {
    Submit,
    Edit(TextEditAction),
}

#[derive(Debug, Clone, Copy)]
pub struct TextInputOptions {
    pub submit_on_enter: bool,
    pub enter_inserts_newline: bool,
    pub shift_enter_inserts_newline: bool,
}

pub fn map_text_input_key(key: KeyEvent, options: TextInputOptions) -> Option<TextInputEvent> {
    match key {
        KeyEvent {
            code: KeyCode::Enter,
            modifiers,
            ..
        } if options.submit_on_enter && !modifiers.contains(KeyModifiers::SHIFT) => {
            Some(TextInputEvent::Submit)
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers,
            ..
        } if options.enter_inserts_newline
            || (options.shift_enter_inserts_newline && modifiers.contains(KeyModifiers::SHIFT)) =>
        {
            Some(TextInputEvent::Edit(TextEditAction::InsertNewline))
        }
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TextInputEvent::Edit(TextEditAction::BackspaceWord))
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::Backspace)),
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TextInputEvent::Edit(TextEditAction::DeleteWord))
        }
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::Delete)),
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TextInputEvent::Edit(TextEditAction::MoveWordLeft))
        }
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveLeft)),
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TextInputEvent::Edit(TextEditAction::MoveWordRight))
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveRight)),
        KeyEvent {
            code: KeyCode::Up, ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveUp)),
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveDown)),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveLineStart)),
        KeyEvent {
            code: KeyCode::End, ..
        } => Some(TextInputEvent::Edit(TextEditAction::MoveLineEnd)),
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers,
            ..
        } if modifiers == KeyModifiers::CONTROL => {
            Some(TextInputEvent::Edit(TextEditAction::MoveLineStart))
        }
        KeyEvent {
            code: KeyCode::Char('w'),
            modifiers,
            ..
        } if modifiers == KeyModifiers::CONTROL => {
            Some(TextInputEvent::Edit(TextEditAction::BackspaceWord))
        }
        KeyEvent {
            code: KeyCode::Char('e'),
            modifiers,
            ..
        } if modifiers == KeyModifiers::CONTROL => {
            Some(TextInputEvent::Edit(TextEditAction::MoveLineEnd))
        }
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            Some(TextInputEvent::Edit(TextEditAction::InsertChar(ch)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::{
        editor::TextEditAction,
        input::{TextInputEvent, TextInputOptions, map_text_input_key},
    };

    #[test]
    fn composer_enter_submits_but_shift_enter_inserts_newline() {
        let options = TextInputOptions {
            submit_on_enter: true,
            enter_inserts_newline: false,
            shift_enter_inserts_newline: true,
        };

        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), options),
            Some(TextInputEvent::Submit)
        );
        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), options),
            Some(TextInputEvent::Edit(TextEditAction::InsertNewline))
        );
    }

    #[test]
    fn notes_enter_inserts_newline() {
        let options = TextInputOptions {
            submit_on_enter: false,
            enter_inserts_newline: true,
            shift_enter_inserts_newline: false,
        };

        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), options),
            Some(TextInputEvent::Edit(TextEditAction::InsertNewline))
        );
    }

    #[test]
    fn control_a_and_control_e_map_to_line_navigation() {
        let options = TextInputOptions {
            submit_on_enter: false,
            enter_inserts_newline: true,
            shift_enter_inserts_newline: false,
        };

        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::MoveLineStart))
        );
        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::MoveLineEnd))
        );
    }

    #[test]
    fn alt_and_control_arrow_shortcuts_map_to_word_navigation() {
        let options = TextInputOptions {
            submit_on_enter: false,
            enter_inserts_newline: true,
            shift_enter_inserts_newline: false,
        };

        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT), options),
            Some(TextInputEvent::Edit(TextEditAction::MoveWordLeft))
        );
        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT), options),
            Some(TextInputEvent::Edit(TextEditAction::MoveWordRight))
        );
        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), options),
            Some(TextInputEvent::Edit(TextEditAction::MoveWordLeft))
        );
        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::MoveWordRight))
        );
    }

    #[test]
    fn alt_and_control_delete_shortcuts_map_to_word_deletion() {
        let options = TextInputOptions {
            submit_on_enter: false,
            enter_inserts_newline: true,
            shift_enter_inserts_newline: false,
        };

        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::BackspaceWord))
        );
        assert_eq!(
            map_text_input_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT), options),
            Some(TextInputEvent::Edit(TextEditAction::DeleteWord))
        );
        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::BackspaceWord))
        );
        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::BackspaceWord))
        );
        assert_eq!(
            map_text_input_key(
                KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL),
                options
            ),
            Some(TextInputEvent::Edit(TextEditAction::DeleteWord))
        );
    }
}
