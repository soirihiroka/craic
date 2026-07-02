#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) struct AnchoredSelection<T> {
    pub(in crate::ui) anchor: T,
    pub(in crate::ui) focus: T,
}

impl<T: Copy + Ord> AnchoredSelection<T> {
    pub(in crate::ui) fn ordered(self) -> Option<(T, T)> {
        if self.anchor == self.focus {
            return None;
        }
        Some(if self.anchor < self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::ui::content::code_editor) struct Selection {
    pub(in crate::ui::content::code_editor) anchor: usize,
    pub(in crate::ui::content::code_editor) focus: usize,
    pub(in crate::ui::content::code_editor) visual_anchor: usize,
    pub(in crate::ui::content::code_editor) visual_focus: usize,
}

impl Selection {
    pub(in crate::ui::content::code_editor) fn visual_bounds(self) -> Option<(usize, usize)> {
        ordered_bounds(self.visual_anchor, self.visual_focus)
    }
}

pub(in crate::ui) fn ordered_bounds(anchor: usize, focus: usize) -> Option<(usize, usize)> {
    AnchoredSelection { anchor, focus }.ordered()
}

pub(in crate::ui) fn clipped_bounds(
    start: usize,
    end: usize,
    lower: usize,
    upper: usize,
) -> Option<(usize, usize)> {
    let start = start.max(lower).min(upper);
    let end = end.min(upper).max(lower);
    (start < end).then_some((start, end))
}

pub(in crate::ui) fn word_bounds_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let offset = offset.min(text.len());
    let (_, ch) = next_char(text, offset)?;
    let group = selectable_group(ch)?;

    let mut start = offset;
    while let Some((previous, ch)) = previous_char(text, start) {
        if selectable_group(ch) != Some(group) {
            break;
        }
        start = previous;
    }

    let mut end = offset;
    while let Some((current, ch)) = next_char(text, end) {
        if selectable_group(ch) != Some(group) {
            break;
        }
        end = current + ch.len_utf8();
    }

    Some((start, end))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextGroup {
    Word,
    Punctuation,
}

fn selectable_group(ch: char) -> Option<TextGroup> {
    (!ch.is_whitespace()).then(|| text_group(ch))
}

fn text_group(ch: char) -> TextGroup {
    if ch == '_' || ch.is_alphanumeric() {
        TextGroup::Word
    } else {
        TextGroup::Punctuation
    }
}

fn previous_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    text[..cursor.min(text.len())].char_indices().last()
}

fn next_char(text: &str, cursor: usize) -> Option<(usize, char)> {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .char_indices()
        .next()
        .map(|(offset, ch)| (cursor + offset, ch))
}
