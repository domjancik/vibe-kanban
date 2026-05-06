use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use crate::editor::{ComposerEditorMode, VimMode};

pub fn render_editor_buffer(
    buffer: &str,
    cursor: usize,
    show_cursor: bool,
    mode: ComposerEditorMode,
) -> Text<'static> {
    let cursor = cursor.min(buffer.len());
    let cursor_style = match mode {
        ComposerEditorMode::Standard | ComposerEditorMode::Vim(VimMode::Insert) => {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        }
        ComposerEditorMode::Vim(VimMode::Normal) => Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    };
    let cursor_glyph = match mode {
        ComposerEditorMode::Standard | ComposerEditorMode::Vim(VimMode::Insert) => "▏",
        ComposerEditorMode::Vim(VimMode::Normal) => "█",
    };

    let mut lines = Vec::new();
    let mut current_spans = Vec::new();
    let mut index = 0usize;

    while index < buffer.len() {
        if show_cursor && index == cursor {
            let ch = buffer[index..].chars().next().unwrap_or(' ');
            if ch == '\n' {
                current_spans.push(Span::styled(cursor_glyph, cursor_style));
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                index += ch.len_utf8();
                continue;
            }
            current_spans.push(Span::styled(ch.to_string(), cursor_style));
            index += ch.len_utf8();
            continue;
        }

        let ch = buffer[index..].chars().next().unwrap_or(' ');
        index += ch.len_utf8();
        if ch == '\n' {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
        } else {
            current_spans.push(Span::raw(ch.to_string()));
        }
    }

    if show_cursor && cursor == buffer.len() {
        current_spans.push(Span::styled(cursor_glyph, cursor_style));
    }

    lines.push(Line::from(current_spans));
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use crate::editor::{ComposerEditorMode, VimMode, render_editor_buffer};

    #[test]
    fn composer_renderer_shows_cursor_span() {
        let text = render_editor_buffer("hello", 1, true, ComposerEditorMode::Vim(VimMode::Normal));
        assert_eq!(text.lines[0].spans[0].content.as_ref(), "h");
        assert_eq!(text.lines[0].spans[1].content.as_ref(), "e");
        assert_eq!(text.lines[0].spans[1].style.bg, Some(Color::Yellow));
    }

    #[test]
    fn empty_buffer_renders_cursor_on_first_line() {
        let text = render_editor_buffer("", 0, true, ComposerEditorMode::Standard);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].spans.len(), 1);
        assert_eq!(text.lines[0].spans[0].content.as_ref(), "▏");
        assert_eq!(text.lines[0].spans[0].style.bg, Some(Color::Cyan));
    }
}
