use ratatui::text::{Line, Span};

pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| wrap_line(line, width))
        .collect()
}

pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let line_style = line.style;
    let line_alignment = line.alignment;
    let mut wrapped = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0usize;

    for span in line.spans {
        let style = line_style.patch(span.style);
        let content = span.content.into_owned();
        if content.is_empty() {
            if current.is_empty() {
                current.push(Span::styled(String::new(), style));
            }
            continue;
        }

        for ch in content.chars() {
            if current_width >= width {
                let mut wrapped_line = Line::from(std::mem::take(&mut current));
                wrapped_line.style = line_style;
                wrapped_line.alignment = line_alignment;
                wrapped.push(wrapped_line);
                current_width = 0;
            }
            current.push(Span::styled(ch.to_string(), style));
            current_width += 1;
        }
    }

    if current.is_empty() {
        let mut wrapped_line = Line::raw(String::new());
        wrapped_line.style = line_style;
        wrapped_line.alignment = line_alignment;
        wrapped.push(wrapped_line);
    } else {
        let mut wrapped_line = Line::from(current);
        wrapped_line.style = line_style;
        wrapped_line.alignment = line_alignment;
        wrapped.push(wrapped_line);
    }

    wrapped
}

pub fn chat_window_bounds(
    total_lines: usize,
    visible_lines: usize,
    requested_end_offset: usize,
) -> (usize, usize, usize, usize) {
    let visible_lines = visible_lines.max(1);
    let max_end_offset = total_lines.saturating_sub(visible_lines);
    let clamped_end_offset = requested_end_offset.min(max_end_offset);
    let end = total_lines.saturating_sub(clamped_end_offset);
    let start = end.saturating_sub(visible_lines);
    let top_offset = total_lines.saturating_sub(visible_lines.saturating_add(clamped_end_offset));
    (clamped_end_offset, start, end, top_offset)
}

#[cfg(test)]
mod tests {
    use super::chat_window_bounds;

    #[test]
    fn chat_window_bounds_clamp_end_offset_to_last_full_page() {
        let (clamped, start, end, top_offset) = chat_window_bounds(100, 10, 99);
        assert_eq!(clamped, 90);
        assert_eq!(start, 0);
        assert_eq!(end, 10);
        assert_eq!(top_offset, 0);
    }

    #[test]
    fn chat_window_bounds_keep_short_transcript_visible_when_overscrolled() {
        let (clamped, start, end, top_offset) = chat_window_bounds(3, 10, usize::MAX);
        assert_eq!(clamped, 0);
        assert_eq!(start, 0);
        assert_eq!(end, 3);
        assert_eq!(top_offset, 0);
    }
}
