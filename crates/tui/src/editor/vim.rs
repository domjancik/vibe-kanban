#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposerEditorMode {
    Standard,
    Vim(VimMode),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VimOperator {
    Delete,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn next_word_start(buffer: &str, cursor: usize) -> usize {
    let cursor = cursor.min(buffer.len());
    let chars: Vec<(usize, char)> = buffer[cursor..].char_indices().collect();
    if chars.is_empty() {
        return cursor;
    }

    let mut i = 0;
    let first = chars[0].1;

    if is_word_char(first) {
        while i < chars.len() && is_word_char(chars[i].1) {
            i += 1;
        }
    } else if !first.is_whitespace() {
        while i < chars.len() && !is_word_char(chars[i].1) && !chars[i].1.is_whitespace() {
            i += 1;
        }
    }

    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1;
    }

    if i >= chars.len() {
        buffer.len()
    } else {
        cursor + chars[i].0
    }
}

pub fn prev_word_start(buffer: &str, cursor: usize) -> usize {
    let cursor = cursor.min(buffer.len());
    if cursor == 0 {
        return 0;
    }

    let before: Vec<(usize, char)> = buffer[..cursor].char_indices().collect();
    if before.is_empty() {
        return 0;
    }

    let mut i = before.len();

    while i > 0 && before[i - 1].1.is_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }

    let class_char = before[i - 1].1;
    if is_word_char(class_char) {
        while i > 0 && is_word_char(before[i - 1].1) {
            i -= 1;
        }
    } else {
        while i > 0 && !is_word_char(before[i - 1].1) && !before[i - 1].1.is_whitespace() {
            i -= 1;
        }
    }

    before[i].0
}

#[cfg(test)]
mod tests {
    use crate::editor::{next_word_start, prev_word_start};

    #[test]
    fn next_word_skips_to_next_word_boundary() {
        assert_eq!(next_word_start("hello world", 0), 6);
        assert_eq!(next_word_start("hello world", 5), 6);
        assert_eq!(next_word_start("hello  world", 0), 7);
        assert_eq!(next_word_start("foo.bar", 0), 3);
        assert_eq!(next_word_start("foo.bar", 3), 4);
        assert_eq!(next_word_start("hello", 0), 5);
        assert_eq!(next_word_start("", 0), 0);
    }

    #[test]
    fn next_word_crosses_newlines() {
        assert_eq!(next_word_start("hello\nworld", 0), 6);
    }

    #[test]
    fn prev_word_moves_to_previous_word_start() {
        assert_eq!(prev_word_start("hello world", 11), 6);
        assert_eq!(prev_word_start("hello world", 6), 0);
        assert_eq!(prev_word_start("foo.bar", 4), 3);
        assert_eq!(prev_word_start("foo.bar", 3), 0);
        assert_eq!(prev_word_start("hello", 5), 0);
        assert_eq!(prev_word_start("", 0), 0);
    }

    #[test]
    fn prev_word_skips_whitespace_before_word() {
        assert_eq!(prev_word_start("hello   world", 8), 0);
    }
}
