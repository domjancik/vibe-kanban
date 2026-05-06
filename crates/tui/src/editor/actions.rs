#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEditAction {
    InsertChar(char),
    InsertNewline,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
}

use crate::editor::{line_end_index, line_start_index, move_cursor_vertical};

pub fn apply_text_edit_action(buffer: &mut String, cursor: &mut usize, action: TextEditAction) {
    match action {
        TextEditAction::InsertChar(ch) => {
            buffer.insert(*cursor, ch);
            *cursor += 1;
        }
        TextEditAction::InsertNewline => {
            buffer.insert(*cursor, '\n');
            *cursor += 1;
        }
        TextEditAction::Backspace => {
            if *cursor > 0 {
                buffer.remove(*cursor - 1);
                *cursor -= 1;
            }
        }
        TextEditAction::Delete => {
            if *cursor < buffer.len() {
                buffer.remove(*cursor);
            }
        }
        TextEditAction::MoveLeft => *cursor = cursor.saturating_sub(1),
        TextEditAction::MoveRight => *cursor = (*cursor + 1).min(buffer.len()),
        TextEditAction::MoveUp => *cursor = move_cursor_vertical(buffer, *cursor, -1),
        TextEditAction::MoveDown => *cursor = move_cursor_vertical(buffer, *cursor, 1),
        TextEditAction::MoveLineStart => *cursor = line_start_index(buffer, *cursor),
        TextEditAction::MoveLineEnd => *cursor = line_end_index(buffer, *cursor),
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::{TextEditAction, apply_text_edit_action};

    #[test]
    fn applies_insert_delete_and_navigation_actions() {
        let mut buffer = String::from("ab");
        let mut cursor = 1;

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::InsertChar('X'));
        assert_eq!(buffer, "aXb");
        assert_eq!(cursor, 2);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::Backspace);
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::Delete);
        assert_eq!(buffer, "a");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn moves_to_line_boundaries() {
        let mut buffer = String::from("abc\ndef");
        let mut cursor = 5;

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::MoveLineStart);
        assert_eq!(cursor, 4);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::MoveLineEnd);
        assert_eq!(cursor, 7);
    }
}
