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
