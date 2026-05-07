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

use crate::editor::{
    clamp_char_boundary, line_end_index, line_start_index, move_cursor_vertical,
    next_char_boundary, prev_char_boundary,
};

pub fn apply_text_edit_action(buffer: &mut String, cursor: &mut usize, action: TextEditAction) {
    match action {
        TextEditAction::InsertChar(ch) => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            buffer.insert(*cursor, ch);
            *cursor += ch.len_utf8();
        }
        TextEditAction::InsertNewline => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            buffer.insert(*cursor, '\n');
            *cursor += 1;
        }
        TextEditAction::Backspace => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor > 0 {
                let start = prev_char_boundary(buffer, *cursor);
                buffer.drain(start..*cursor);
                *cursor = start;
            }
        }
        TextEditAction::Delete => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor < buffer.len() {
                let end = next_char_boundary(buffer, *cursor);
                buffer.drain(*cursor..end);
            }
        }
        TextEditAction::MoveLeft => *cursor = prev_char_boundary(buffer, *cursor),
        TextEditAction::MoveRight => *cursor = next_char_boundary(buffer, *cursor),
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

    #[test]
    fn applies_multibyte_character_edits_without_panicking() {
        let mut buffer = String::from("a§b");
        let mut cursor = 3;

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::Backspace);
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::InsertChar('§'));
        assert_eq!(buffer, "a§b");
        assert_eq!(cursor, 3);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::MoveLeft);
        assert_eq!(cursor, 1);

        apply_text_edit_action(&mut buffer, &mut cursor, TextEditAction::Delete);
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);
    }
}
