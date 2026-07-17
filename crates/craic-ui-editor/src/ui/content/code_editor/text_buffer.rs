use std::ops::Deref;

pub struct TextBuffer {
    before_gap: String,
    after_gap_reversed: String,
    text: String,
}

impl TextBuffer {
    pub fn new(text: &str) -> Self {
        Self {
            before_gap: text.to_string(),
            after_gap_reversed: String::new(),
            text: text.to_string(),
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.before_gap.clear();
        self.before_gap.push_str(text);
        self.after_gap_reversed.clear();
        self.text.clear();
        self.text.push_str(text);
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn replace_range(&mut self, start: usize, old_end: usize, replacement: &str) {
        let start = previous_char_boundary(&self.text, start.min(self.text.len()));
        let old_end = previous_char_boundary(&self.text, old_end.min(self.text.len()).max(start));
        self.move_gap_to(start);
        self.delete_after_gap(old_end - start);
        self.before_gap.push_str(replacement);
        self.text.replace_range(start..old_end, replacement);
    }

    fn move_gap_to(&mut self, offset: usize) {
        if offset < self.before_gap.len() {
            let moved = self.before_gap.split_off(offset);
            self.after_gap_reversed.reserve(moved.len());
            for ch in moved.chars().rev() {
                self.after_gap_reversed.push(ch);
            }
            return;
        }

        while self.before_gap.len() < offset {
            let Some(ch) = self.after_gap_reversed.pop() else {
                break;
            };
            self.before_gap.push(ch);
        }
    }

    fn delete_after_gap(&mut self, byte_len: usize) {
        let mut removed = 0usize;
        while removed < byte_len {
            let Some(ch) = self.after_gap_reversed.pop() else {
                break;
            };
            removed += ch.len_utf8();
        }
    }
}

impl Deref for TextBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

pub fn previous_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn next_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

pub fn clamp_to_char_boundary(text: &str, offset: usize) -> usize {
    previous_char_boundary(text, offset)
}

pub fn byte_offset_for_line_column(text: &str, line: usize, column: usize) -> usize {
    let target_line = line.max(1);
    let target_column = column.max(1);
    let mut current_line = 1usize;
    let mut line_start = 0usize;

    for (offset, ch) in text.char_indices() {
        if current_line == target_line {
            break;
        }
        if ch == '\n' {
            current_line += 1;
            line_start = offset + ch.len_utf8();
        }
    }

    if current_line != target_line {
        return text.len();
    }

    let mut current_column = 1usize;
    for (offset, ch) in text[line_start..].char_indices() {
        if current_column >= target_column || ch == '\n' {
            return line_start + offset;
        }
        current_column += 1;
    }

    text.len()
}
