use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use uuid::Uuid;

pub(crate) const PASTE_COLLAPSE_THRESHOLD_CHARS: usize = 1200;
pub(crate) const PASTE_HARD_CEILING_CHARS: usize = 250_000;
pub(crate) const SNIPPET_PLACEHOLDER_CHAR: char = '\u{FFFC}';

const SNIPPET_MARKER_PREFIX: &str = "<|VK_PASTED_TEXT:";
const SNIPPET_MARKER_SUFFIX: &str = "|>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiComposerSnippet {
    pub(crate) id: Uuid,
    pub(crate) full_text: String,
    pub(crate) char_count: usize,
    pub(crate) line_count: usize,
    pub(crate) collapsed: bool,
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

pub(crate) fn serialize_document_for_draft(text: &str, snippets: &[TuiComposerSnippet]) -> String {
    let mut snippet_iter = snippets.iter();
    let mut serialized = String::new();
    for ch in text.chars() {
        if ch == SNIPPET_PLACEHOLDER_CHAR {
            if let Some(snippet) = snippet_iter.next() {
                serialized.push_str(&snippet_marker(snippet));
            }
        } else {
            serialized.push(ch);
        }
    }
    serialized
}

pub(crate) fn restore_document_from_draft(message: &str) -> (String, Vec<TuiComposerSnippet>) {
    let mut text = String::new();
    let mut snippets = Vec::new();
    let mut cursor = 0usize;

    while let Some(relative_start) = message[cursor..].find(SNIPPET_MARKER_PREFIX) {
        let start = cursor + relative_start;
        text.push_str(&message[cursor..start]);

        let marker_start = start + SNIPPET_MARKER_PREFIX.len();
        let Some(relative_end) = message[marker_start..].find(SNIPPET_MARKER_SUFFIX) else {
            text.push_str(&message[start..]);
            return (text, snippets);
        };
        let marker_end = marker_start + relative_end;
        let marker = &message[marker_start..marker_end];
        let Some(snippet) = parse_snippet_marker(marker) else {
            text.push_str(&message[start..marker_end + SNIPPET_MARKER_SUFFIX.len()]);
            cursor = marker_end + SNIPPET_MARKER_SUFFIX.len();
            continue;
        };

        text.push(SNIPPET_PLACEHOLDER_CHAR);
        snippets.push(snippet);
        cursor = marker_end + SNIPPET_MARKER_SUFFIX.len();
    }

    text.push_str(&message[cursor..]);
    (text, snippets)
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

fn snippet_marker(snippet: &TuiComposerSnippet) -> String {
    let payload = BASE64.encode(snippet.full_text.as_bytes());
    format!(
        "{SNIPPET_MARKER_PREFIX}{}:{payload}{SNIPPET_MARKER_SUFFIX}",
        snippet.id
    )
}

fn parse_snippet_marker(marker: &str) -> Option<TuiComposerSnippet> {
    let (id, payload) = marker.split_once(':')?;
    let id = Uuid::parse_str(id).ok()?;
    let decoded = BASE64.decode(payload).ok()?;
    let full_text = String::from_utf8(decoded).ok()?;
    Some(TuiComposerSnippet {
        id,
        char_count: paste_char_count(&full_text),
        line_count: paste_line_count(&full_text),
        full_text,
        collapsed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        SNIPPET_PLACEHOLDER_CHAR, expand_document, new_snippet, placeholder_count_before,
        restore_document_from_draft, serialize_document_for_draft,
    };

    #[test]
    fn expand_document_restores_snippet_text_exactly() {
        let first = new_snippet("alpha\nbeta".to_string());
        let second = new_snippet("gamma".to_string());
        let text = format!("x{SNIPPET_PLACEHOLDER_CHAR}y{SNIPPET_PLACEHOLDER_CHAR}z");
        let expanded = expand_document(&text, &[first, second]);
        assert_eq!(expanded, "xalpha\nbetaygammaz");
    }

    #[test]
    fn draft_roundtrip_reconstructs_collapsed_snippets() {
        let first = new_snippet("alpha\nbeta".to_string());
        let second = new_snippet("gamma".to_string());
        let text = format!("x{SNIPPET_PLACEHOLDER_CHAR}y{SNIPPET_PLACEHOLDER_CHAR}z");

        let serialized = serialize_document_for_draft(&text, &[first.clone(), second.clone()]);
        let (restored_text, restored_snippets) = restore_document_from_draft(&serialized);

        assert_eq!(restored_text, text);
        assert_eq!(restored_snippets, vec![first, second]);
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
