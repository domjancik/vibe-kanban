use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub fn parse_inline_markdown(content: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut code = false;
    let chars: Vec<char> = content.chars().collect();
    let mut index = 0usize;

    let flush = |spans: &mut Vec<Span<'static>>,
                 buffer: &mut String,
                 bold: bool,
                 italic: bool,
                 strike: bool,
                 code: bool| {
        if buffer.is_empty() {
            return;
        }
        let mut style = base_style;
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if strike {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if code {
            style = style
                .bg(Color::Rgb(32, 32, 32))
                .add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(std::mem::take(buffer), style));
    };

    while index < chars.len() {
        if chars[index] == '['
            && let Some(close_bracket) = chars[index + 1..].iter().position(|ch| *ch == ']')
        {
            let close_bracket = index + 1 + close_bracket;
            if chars.get(close_bracket + 1) == Some(&'(')
                && let Some(close_paren) =
                    chars[close_bracket + 2..].iter().position(|ch| *ch == ')')
            {
                flush(&mut spans, &mut buffer, bold, italic, strike, code);
                let close_paren = close_bracket + 2 + close_paren;
                let text = chars[index + 1..close_bracket].iter().collect::<String>();
                spans.push(Span::styled(
                    text,
                    base_style.add_modifier(Modifier::UNDERLINED),
                ));
                index = close_paren + 1;
                continue;
            }
        }

        if index + 1 < chars.len() && chars[index] == '*' && chars[index + 1] == '*' {
            flush(&mut spans, &mut buffer, bold, italic, strike, code);
            bold = !bold;
            index += 2;
            continue;
        }
        if index + 1 < chars.len() && chars[index] == '~' && chars[index + 1] == '~' {
            flush(&mut spans, &mut buffer, bold, italic, strike, code);
            strike = !strike;
            index += 2;
            continue;
        }
        if chars[index] == '`' {
            flush(&mut spans, &mut buffer, bold, italic, strike, code);
            code = !code;
            index += 1;
            continue;
        }
        if (chars[index] == '*' || chars[index] == '_') && is_italic_delimiter(&chars, index) {
            flush(&mut spans, &mut buffer, bold, italic, strike, code);
            italic = !italic;
            index += 1;
            continue;
        }

        buffer.push(chars[index]);
        index += 1;
    }

    flush(&mut spans, &mut buffer, bold, italic, strike, code);
    spans
}

pub fn render_markdown_lines(content: &str, base_style: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            lines.push(Line::styled(
                format!("  {raw_line}"),
                base_style
                    .bg(Color::Rgb(32, 32, 32))
                    .add_modifier(Modifier::DIM),
            ));
            continue;
        }

        if raw_line.trim().is_empty() {
            lines.push(Line::raw(""));
            continue;
        }

        if let Some((level, text)) = markdown_heading(raw_line) {
            let style = base_style
                .add_modifier(Modifier::BOLD)
                .add_modifier(if level <= 2 {
                    Modifier::UNDERLINED
                } else {
                    Modifier::empty()
                });
            lines.push(Line::from(parse_inline_markdown(
                &format!("  {text}"),
                style,
            )));
            continue;
        }

        if let Some(text) = raw_line.trim_start().strip_prefix("> ") {
            let mut spans = vec![Span::styled(
                "> ",
                base_style.fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )];
            spans.extend(parse_inline_markdown(
                text,
                base_style
                    .add_modifier(Modifier::ITALIC)
                    .add_modifier(Modifier::DIM),
            ));
            lines.push(Line::from(spans));
            continue;
        }

        if let Some((prefix, text)) = markdown_list_prefix(raw_line) {
            let mut spans = vec![Span::styled(
                format!("  {prefix} "),
                base_style.fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )];
            spans.extend(parse_inline_markdown(text, base_style));
            lines.push(Line::from(spans));
            continue;
        }

        let mut spans = vec![Span::styled("  ", base_style)];
        spans.extend(parse_inline_markdown(raw_line, base_style));
        lines.push(Line::from(spans));
    }

    lines
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let text = trimmed[hashes..].trim_start();
    if text.is_empty() {
        None
    } else {
        Some((hashes, text))
    }
}

fn markdown_list_prefix(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(bullet) {
            return Some(("•".to_string(), text));
        }
    }

    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits > 0 && trimmed[digits..].starts_with(". ") {
        let prefix = trimmed[..digits].to_string();
        let text = &trimmed[(digits + 2)..];
        return Some((format!("{prefix}."), text));
    }

    None
}

fn is_italic_delimiter(chars: &[char], index: usize) -> bool {
    let marker = chars[index];
    let prev = index.checked_sub(1).and_then(|idx| chars.get(idx));
    let next = chars.get(index + 1);

    if prev.is_some_and(|ch| ch.is_alphanumeric()) && next.is_some_and(|ch| ch.is_alphanumeric()) {
        return false;
    }

    chars[index + 1..].contains(&marker)
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};

    use crate::conversation::parse_inline_markdown;

    #[test]
    fn underscores_inside_identifiers_do_not_trigger_italics() {
        let spans = parse_inline_markdown("render_chat after", Style::default());
        let rendered = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(rendered, "render_chat after");
        assert!(
            spans
                .iter()
                .all(|span| !span.style.add_modifier.contains(Modifier::ITALIC))
        );
    }
}
