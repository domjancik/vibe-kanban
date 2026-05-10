use db::models::scratch::TuiComposerSnippet;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use crate::{
    editor::{ComposerEditorMode, VimMode, clamp_char_boundary},
    paste::{SNIPPET_PLACEHOLDER_CHAR, snippet_label},
};

pub fn render_editor_buffer(
    buffer: &str,
    cursor: usize,
    show_cursor: bool,
    mode: ComposerEditorMode,
) -> Text<'static> {
    render_editor_buffer_with_snippets(buffer, &[], cursor, show_cursor, mode)
}

pub fn render_editor_buffer_with_snippets(
    buffer: &str,
    snippets: &[TuiComposerSnippet],
    cursor: usize,
    show_cursor: bool,
    mode: ComposerEditorMode,
) -> Text<'static> {
    let cursor = clamp_char_boundary(buffer, cursor);
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
    let mut plain_start = 0usize;
    let mut snippet_iter = snippets.iter();

    while index < buffer.len() {
        let ch = buffer[index..].chars().next().unwrap_or(' ');
        if ch == SNIPPET_PLACEHOLDER_CHAR {
            if plain_start < index {
                current_spans.push(Span::raw(buffer[plain_start..index].to_string()));
            }
            let snippet = snippet_iter.next();
            let label = snippet
                .map(snippet_label)
                .unwrap_or_else(|| "[Pasted Text]".to_string());
            let snippet_style = if show_cursor && index == cursor {
                cursor_style
            } else {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            };
            current_spans.push(Span::styled(label, snippet_style));
            index += ch.len_utf8();
            plain_start = index;
            continue;
        }

        if show_cursor && index == cursor {
            if plain_start < index {
                current_spans.push(Span::raw(buffer[plain_start..index].to_string()));
            }
            if ch == '\n' {
                current_spans.push(Span::styled(cursor_glyph, cursor_style));
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                index += ch.len_utf8();
                plain_start = index;
                continue;
            }
            current_spans.push(Span::styled(ch.to_string(), cursor_style));
            index += ch.len_utf8();
            plain_start = index;
            continue;
        }

        if ch == '\n' {
            if plain_start < index {
                current_spans.push(Span::raw(buffer[plain_start..index].to_string()));
            }
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            index += ch.len_utf8();
            plain_start = index;
        } else {
            index += ch.len_utf8();
        }
    }

    if plain_start < buffer.len() {
        current_spans.push(Span::raw(buffer[plain_start..].to_string()));
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

    #[test]
    fn plain_text_is_batched_into_spans_around_cursor() {
        let text = render_editor_buffer("hello", 2, true, ComposerEditorMode::Standard);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].spans.len(), 3);
        assert_eq!(text.lines[0].spans[0].content.as_ref(), "he");
        assert_eq!(text.lines[0].spans[1].content.as_ref(), "l");
        assert_eq!(text.lines[0].spans[2].content.as_ref(), "lo");
    }

    #[test]
    fn plain_text_without_cursor_uses_single_span_per_line() {
        let text = render_editor_buffer("hello\nworld", 0, false, ComposerEditorMode::Standard);
        assert_eq!(text.lines.len(), 2);
        assert_eq!(text.lines[0].spans.len(), 1);
        assert_eq!(text.lines[0].spans[0].content.as_ref(), "hello");
        assert_eq!(text.lines[1].spans.len(), 1);
        assert_eq!(text.lines[1].spans[0].content.as_ref(), "world");
    }
}
