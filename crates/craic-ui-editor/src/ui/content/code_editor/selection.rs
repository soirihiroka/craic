#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchoredSelection<T> {
    pub anchor: T,
    pub focus: T,
}

impl<T: Copy + Ord> AnchoredSelection<T> {
    pub fn ordered(self) -> Option<(T, T)> {
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
pub struct Selection {
    pub anchor: usize,
    pub focus: usize,
    pub visual_anchor: usize,
    pub visual_focus: usize,
}

impl Selection {
    pub fn visual_bounds(self) -> Option<(usize, usize)> {
        ordered_bounds(self.visual_anchor, self.visual_focus)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Character,
    Word,
    Line,
}

impl SelectionMode {
    pub fn for_press_count(press_count: i32) -> Self {
        match press_count {
            count if count >= 3 => Self::Line,
            2 => Self::Word,
            _ => Self::Character,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragSelection<T> {
    Character { anchor: T },
    Word { start: T, end: T },
    Line { start: T, end: T },
}

impl<T: Copy> DragSelection<T> {
    pub fn anchor(self) -> T {
        match self {
            Self::Character { anchor } => anchor,
            Self::Word { start, .. } | Self::Line { start, .. } => start,
        }
    }
}

pub fn ordered_bounds(anchor: usize, focus: usize) -> Option<(usize, usize)> {
    AnchoredSelection { anchor, focus }.ordered()
}

pub fn clipped_bounds(
    start: usize,
    end: usize,
    lower: usize,
    upper: usize,
) -> Option<(usize, usize)> {
    let start = start.max(lower).min(upper);
    let end = end.min(upper).max(lower);
    (start < end).then_some((start, end))
}

pub fn word_bounds_at(text: &str, offset: usize) -> Option<(usize, usize)> {
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

pub fn selection_for_mode<T, WordBounds, LineBounds>(
    point: T,
    mode: SelectionMode,
    word_bounds: WordBounds,
    line_bounds: LineBounds,
) -> AnchoredSelection<T>
where
    T: Copy,
    WordBounds: Fn(T) -> Option<(T, T)>,
    LineBounds: Fn(T) -> Option<(T, T)>,
{
    match mode {
        SelectionMode::Character => AnchoredSelection {
            anchor: point,
            focus: point,
        },
        SelectionMode::Word => word_bounds(point)
            .map(|(anchor, focus)| AnchoredSelection { anchor, focus })
            .unwrap_or(AnchoredSelection {
                anchor: point,
                focus: point,
            }),
        SelectionMode::Line => line_bounds(point)
            .map(|(anchor, focus)| AnchoredSelection { anchor, focus })
            .unwrap_or(AnchoredSelection {
                anchor: point,
                focus: point,
            }),
    }
}

pub fn drag_for_mode<T, WordBounds, LineBounds>(
    point: T,
    mode: SelectionMode,
    word_bounds: WordBounds,
    line_bounds: LineBounds,
) -> (DragSelection<T>, AnchoredSelection<T>)
where
    T: Copy,
    WordBounds: Fn(T) -> Option<(T, T)>,
    LineBounds: Fn(T) -> Option<(T, T)>,
{
    match mode {
        SelectionMode::Character => {
            let selection = AnchoredSelection {
                anchor: point,
                focus: point,
            };
            (DragSelection::Character { anchor: point }, selection)
        }
        SelectionMode::Word => {
            if let Some((start, end)) = word_bounds(point) {
                (
                    DragSelection::Word { start, end },
                    AnchoredSelection {
                        anchor: start,
                        focus: end,
                    },
                )
            } else {
                let selection = AnchoredSelection {
                    anchor: point,
                    focus: point,
                };
                (DragSelection::Character { anchor: point }, selection)
            }
        }
        SelectionMode::Line => {
            if let Some((start, end)) = line_bounds(point) {
                (
                    DragSelection::Line { start, end },
                    AnchoredSelection {
                        anchor: start,
                        focus: end,
                    },
                )
            } else {
                let selection = AnchoredSelection {
                    anchor: point,
                    focus: point,
                };
                (DragSelection::Character { anchor: point }, selection)
            }
        }
    }
}

pub fn selection_for_drag<T, WordBounds, LineBounds>(
    drag: DragSelection<T>,
    focus: T,
    word_bounds: WordBounds,
    line_bounds: LineBounds,
) -> AnchoredSelection<T>
where
    T: Copy + Ord,
    WordBounds: Fn(T) -> Option<(T, T)>,
    LineBounds: Fn(T) -> Option<(T, T)>,
{
    match drag {
        DragSelection::Character { anchor } => AnchoredSelection { anchor, focus },
        DragSelection::Word { start, end } => {
            let (focus_start, focus_end) = word_bounds(focus).unwrap_or((focus, focus));
            if focus < start {
                AnchoredSelection {
                    anchor: end,
                    focus: focus_start,
                }
            } else {
                AnchoredSelection {
                    anchor: start,
                    focus: focus_end,
                }
            }
        }
        DragSelection::Line { start, end } => {
            let (focus_start, focus_end) = line_bounds(focus).unwrap_or((focus, focus));
            if focus < start {
                AnchoredSelection {
                    anchor: end,
                    focus: focus_start,
                }
            } else {
                AnchoredSelection {
                    anchor: start,
                    focus: focus_end,
                }
            }
        }
    }
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
