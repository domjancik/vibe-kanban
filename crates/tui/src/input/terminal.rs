use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    ExitInputMode,
    SendBytes(Vec<u8>),
}

pub fn map_terminal_key(key: KeyEvent) -> Option<TerminalInput> {
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => Some(TerminalInput::ExitInputMode),
        KeyEvent {
            code: KeyCode::Char(']'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(TerminalInput::ExitInputMode),
        KeyEvent {
            code: KeyCode::Char('g'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(TerminalInput::ExitInputMode),
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => Some(TerminalInput::SendBytes(vec![b'\r'])),
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => Some(TerminalInput::SendBytes(vec![0x7f])),
        KeyEvent {
            code: KeyCode::Tab, ..
        } => Some(TerminalInput::SendBytes(vec![b'\t'])),
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => Some(TerminalInput::SendBytes(b"\x1b[D".to_vec())),
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => Some(TerminalInput::SendBytes(b"\x1b[C".to_vec())),
        KeyEvent {
            code: KeyCode::Up, ..
        } => Some(TerminalInput::SendBytes(b"\x1b[A".to_vec())),
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => Some(TerminalInput::SendBytes(b"\x1b[B".to_vec())),
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TerminalInput::SendBytes(vec![(ch as u8) & 0x1f]))
        }
        KeyEvent {
            code: KeyCode::Char(ch),
            ..
        } => Some(TerminalInput::SendBytes(ch.to_string().into_bytes())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{TerminalInput, map_terminal_key};

    #[test]
    fn maps_terminal_exit_shortcuts() {
        assert_eq!(
            map_terminal_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(TerminalInput::ExitInputMode)
        );
        assert_eq!(
            map_terminal_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            Some(TerminalInput::ExitInputMode)
        );
    }

    #[test]
    fn maps_terminal_bytes_for_common_keys() {
        assert_eq!(
            map_terminal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(TerminalInput::SendBytes(vec![b'\r']))
        );
        assert_eq!(
            map_terminal_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(TerminalInput::SendBytes(vec![0x03]))
        );
        assert_eq!(
            map_terminal_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some(TerminalInput::SendBytes(b"\x1b[D".to_vec()))
        );
    }
}
