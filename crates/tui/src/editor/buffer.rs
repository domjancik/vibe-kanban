pub fn line_start_index(buffer: &str, cursor: usize) -> usize {
    let cursor = cursor.min(buffer.len());
    buffer[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

pub fn line_end_index(buffer: &str, cursor: usize) -> usize {
    let cursor = cursor.min(buffer.len());
    buffer[cursor..]
        .find('\n')
        .map_or(buffer.len(), |offset| cursor + offset)
}

pub fn cursor_column(buffer: &str, cursor: usize) -> usize {
    let start = line_start_index(buffer, cursor);
    buffer[start..cursor.min(buffer.len())].chars().count()
}

pub fn byte_index_for_column(line: &str, column: usize) -> usize {
    line.char_indices()
        .nth(column)
        .map(|(index, _)| index)
        .unwrap_or(line.len())
}

pub fn move_cursor_vertical(buffer: &str, cursor: usize, direction: i32) -> usize {
    let cursor = cursor.min(buffer.len());
    let current_line_start = line_start_index(buffer, cursor);
    let current_column = cursor_column(buffer, cursor);

    if direction < 0 {
        if current_line_start == 0 {
            return cursor;
        }
        let previous_line_end = current_line_start.saturating_sub(1);
        let previous_line_start = line_start_index(buffer, previous_line_end);
        let previous_line = &buffer[previous_line_start..previous_line_end];
        return previous_line_start + byte_index_for_column(previous_line, current_column);
    }

    let current_line_end = line_end_index(buffer, cursor);
    if current_line_end >= buffer.len() {
        return cursor;
    }
    let next_line_start = current_line_end + 1;
    let next_line_end = line_end_index(buffer, next_line_start);
    let next_line = &buffer[next_line_start..next_line_end];
    next_line_start + byte_index_for_column(next_line, current_column)
}

#[cfg(test)]
mod tests {
    use super::move_cursor_vertical;

    #[test]
    fn vertical_cursor_movement_clamps_to_shorter_line_end() {
        let buffer = "abcd\nxy\nwxyz";
        let down = move_cursor_vertical(buffer, 3, 1);
        assert_eq!(down, 7);
        let up = move_cursor_vertical(buffer, 7, -1);
        assert_eq!(up, 2);
    }
}
