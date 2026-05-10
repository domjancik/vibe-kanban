use db::models::scratch::{TuiComposerDocument, TuiComposerSnippet};
use uuid::Uuid;

pub(crate) const PASTE_COLLAPSE_THRESHOLD_CHARS: usize = 1200;
pub(crate) const PASTE_HARD_CEILING_CHARS: usize = 250_000;
pub(crate) const SNIPPET_PLACEHOLDER_CHAR: char = '\u{FFFC}';

pub(crate) fn compose_document(
    text: String,
    snippets: &[TuiComposerSnippet],
) -> TuiComposerDocument {
    TuiComposerDocument {
        text,
        snippets: snippets.to_vec(),
    }
}

pub(crate) fn expand_document(text: &str, snippets: &[TuiComposerSnippet]) -> String {
    let mut snippet_iter = snippets.iter();
    let mut expanded = String::new();
    for ch in text.chars() {
        if ch == SNIPPET_PLACEHOLDER_CHAR {
            if let Some(snippet) = snippet_iter.next() {
                expanded.push_str(&snippet.full_text);
            }
        } else {
            expanded.push(ch);
        }
    }
    expanded
}

pub(crate) fn paste_char_count(text: &str) -> usize {
    text.chars().count()
}

pub(crate) fn paste_line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

pub(crate) fn should_collapse_paste(text: &str) -> bool {
    paste_char_count(text) > PASTE_COLLAPSE_THRESHOLD_CHARS
}

pub(crate) fn exceeds_paste_ceiling(text: &str) -> bool {
    paste_char_count(text) > PASTE_HARD_CEILING_CHARS
}

pub(crate) fn new_snippet(full_text: String) -> TuiComposerSnippet {
    TuiComposerSnippet {
        id: Uuid::new_v4(),
        char_count: paste_char_count(&full_text),
        line_count: paste_line_count(&full_text),
        full_text,
        collapsed: true,
    }
}

pub(crate) fn snippet_label(snippet: &TuiComposerSnippet) -> String {
    format!(
        "[Pasted Text] {} chars, {} lines",
        snippet.char_count, snippet.line_count
    )
}

pub(crate) fn placeholder_count_before(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .chars()
        .filter(|ch| *ch == SNIPPET_PLACEHOLDER_CHAR)
        .count()
}

#[cfg(test)]
mod tests {
    use super::{
        SNIPPET_PLACEHOLDER_CHAR, compose_document, expand_document, new_snippet,
        placeholder_count_before,
    };

    #[test]
    fn expand_document_restores_snippet_text_exactly() {
        let first = new_snippet("alpha\nbeta".to_string());
        let second = new_snippet("gamma".to_string());
        let text = format!("x{SNIPPET_PLACEHOLDER_CHAR}y{SNIPPET_PLACEHOLDER_CHAR}z");
        let expanded = expand_document(&text, &[first.clone(), second.clone()]);
        assert_eq!(expanded, "xalpha\nbetaygammaz");

        let document = compose_document(text, &[first, second]);
        assert_eq!(document.snippets.len(), 2);
    }

    #[test]
    fn counts_placeholder_positions_before_cursor() {
        let text = format!("a{SNIPPET_PLACEHOLDER_CHAR}b{SNIPPET_PLACEHOLDER_CHAR}");
        let first_end = "a".len() + SNIPPET_PLACEHOLDER_CHAR.len_utf8();
        assert_eq!(placeholder_count_before(&text, 0), 0);
        assert_eq!(placeholder_count_before(&text, first_end), 1);
        assert_eq!(placeholder_count_before(&text, text.len()), 2);
    }
}
