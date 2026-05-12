#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEditAction {
    InsertChar(char),
    InsertNewline,
    Backspace,
    Delete,
    BackspaceWord,
    DeleteWord,
    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
}

use crate::editor::{
    clamp_char_boundary, line_end_index, line_start_index, move_cursor_vertical,
    next_char_boundary, next_word_start, prev_char_boundary, prev_word_start,
};

pub fn apply_text_edit_action(
    buffer: &mut String,
    cursor: &mut usize,
    action: TextEditAction,
) -> bool {
    match action {
        TextEditAction::InsertChar(ch) => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            buffer.insert(*cursor, ch);
            *cursor += ch.len_utf8();
            true
        }
        TextEditAction::InsertNewline => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            buffer.insert(*cursor, '\n');
            *cursor += 1;
            true
        }
        TextEditAction::Backspace => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor > 0 {
                let start = prev_char_boundary(buffer, *cursor);
                buffer.drain(start..*cursor);
                *cursor = start;
                true
            } else {
                false
            }
        }
        TextEditAction::BackspaceWord => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor > 0 {
                let start = prev_word_start(buffer, *cursor);
                if start < *cursor {
                    buffer.drain(start..*cursor);
                    *cursor = start;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        TextEditAction::Delete => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor < buffer.len() {
                let end = next_char_boundary(buffer, *cursor);
                buffer.drain(*cursor..end);
                true
            } else {
                false
            }
        }
        TextEditAction::DeleteWord => {
            *cursor = clamp_char_boundary(buffer, *cursor);
            if *cursor < buffer.len() {
                let end = next_word_start(buffer, *cursor);
                if *cursor < end {
                    buffer.drain(*cursor..end);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        TextEditAction::MoveLeft => move_cursor_with(buffer, cursor, |buffer, cursor| {
            prev_char_boundary(buffer, cursor)
        }),
        TextEditAction::MoveRight => move_cursor_with(buffer, cursor, |buffer, cursor| {
            next_char_boundary(buffer, cursor)
        }),
        TextEditAction::MoveWordLeft => move_cursor_with(buffer, cursor, |buffer, cursor| {
            prev_word_start(buffer, cursor)
        }),
        TextEditAction::MoveWordRight => move_cursor_with(buffer, cursor, |buffer, cursor| {
            next_word_start(buffer, cursor)
        }),
        TextEditAction::MoveUp => move_cursor_with(buffer, cursor, |buffer, cursor| {
            move_cursor_vertical(buffer, cursor, -1)
        }),
        TextEditAction::MoveDown => move_cursor_with(buffer, cursor, |buffer, cursor| {
            move_cursor_vertical(buffer, cursor, 1)
        }),
        TextEditAction::MoveLineStart => move_cursor_with(buffer, cursor, |buffer, cursor| {
            line_start_index(buffer, cursor)
        }),
        TextEditAction::MoveLineEnd => move_cursor_with(buffer, cursor, |buffer, cursor| {
            line_end_index(buffer, cursor)
        }),
    }
}

fn move_cursor_with<F>(buffer: &str, cursor: &mut usize, next: F) -> bool
where
    F: FnOnce(&str, usize) -> usize,
{
    let previous = *cursor;
    *cursor = next(buffer, *cursor);
    *cursor != previous
}

#[cfg(test)]
mod tests {
    use crate::editor::{TextEditAction, apply_text_edit_action};

    #[test]
    fn applies_insert_delete_and_navigation_actions() {
        let mut buffer = String::from("ab");
        let mut cursor = 1;

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::InsertChar('X')
        ));
        assert_eq!(buffer, "aXb");
        assert_eq!(cursor, 2);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::Backspace
        ));
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::Delete
        ));
        assert_eq!(buffer, "a");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn applies_word_navigation_and_deletion_actions() {
        let mut buffer = String::from("hello world again");
        let mut cursor = 6;

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveWordRight
        ));
        assert_eq!(cursor, 12);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveWordLeft
        ));
        assert_eq!(cursor, 6);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::BackspaceWord
        ));
        assert_eq!(buffer, "world again");
        assert_eq!(cursor, 0);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::DeleteWord
        ));
        assert_eq!(buffer, "again");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn moves_to_line_boundaries() {
        let mut buffer = String::from("abc\ndef");
        let mut cursor = 5;

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveLineStart
        ));
        assert_eq!(cursor, 4);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveLineEnd
        ));
        assert_eq!(cursor, 7);
    }

    #[test]
    fn applies_multibyte_character_edits_without_panicking() {
        let mut buffer = String::from("a§b");
        let mut cursor = 3;

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::Backspace
        ));
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::InsertChar('§')
        ));
        assert_eq!(buffer, "a§b");
        assert_eq!(cursor, 3);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveLeft
        ));
        assert_eq!(cursor, 1);

        assert!(apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::Delete
        ));
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn reports_no_change_for_boundary_navigation_and_empty_deletes() {
        let mut buffer = String::from("ab");
        let mut cursor = 0;

        assert!(!apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::MoveLeft
        ));
        assert!(!apply_text_edit_action(
            &mut buffer,
            &mut cursor,
            TextEditAction::Backspace
        ));
        assert_eq!(buffer, "ab");
        assert_eq!(cursor, 0);
    }
}
