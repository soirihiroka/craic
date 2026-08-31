use crate::next_char_boundary;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorSearchMatch {
    pub start: usize,
    pub end: usize,
}

pub fn find_editor_search_matches(text: &str, query: &str) -> Vec<EditorSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let (haystack, needle) = if query.is_ascii() {
        (text.to_ascii_lowercase(), query.to_ascii_lowercase())
    } else {
        (text.to_string(), query.to_string())
    };
    let mut matches = Vec::new();
    let mut cursor = 0;
    while cursor <= haystack.len() {
        let Some(relative) = haystack[cursor..].find(&needle) else {
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            matches.push(EditorSearchMatch { start, end });
            cursor = if start == end {
                next_char_boundary(text, end.saturating_add(1))
            } else {
                end
            };
        } else {
            cursor = next_char_boundary(text, start.saturating_add(1));
        }
    }
    matches
}

pub fn editor_search_index_after(matches: &[EditorSearchMatch], offset: usize) -> Option<usize> {
    (!matches.is_empty()).then(|| {
        matches
            .iter()
            .position(|search_match| search_match.end > offset)
            .unwrap_or(0)
    })
}
