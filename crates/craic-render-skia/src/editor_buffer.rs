use std::ops::Deref;

pub struct EditorTextBuffer {
    before_gap: String,
    after_gap_reversed: String,
    text: String,
}

impl EditorTextBuffer {
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
            for character in moved.chars().rev() {
                self.after_gap_reversed.push(character);
            }
            return;
        }
        while self.before_gap.len() < offset {
            let Some(character) = self.after_gap_reversed.pop() else {
                break;
            };
            self.before_gap.push(character);
        }
    }

    fn delete_after_gap(&mut self, byte_len: usize) {
        let mut removed = 0;
        while removed < byte_len {
            let Some(character) = self.after_gap_reversed.pop() else {
                break;
            };
            removed += character.len_utf8();
        }
    }
}

impl Deref for EditorTextBuffer {
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

pub fn previous_word_boundary(text: &str, offset: usize) -> usize {
    let mut cursor = previous_char_boundary(text, offset);
    while let Some((previous, character)) = previous_character(text, cursor) {
        if !character.is_whitespace() {
            break;
        }
        cursor = previous;
    }
    let Some((_, character)) = previous_character(text, cursor) else {
        return 0;
    };
    let word = character == '_' || character.is_alphanumeric();
    while let Some((previous, character)) = previous_character(text, cursor) {
        if (character == '_' || character.is_alphanumeric()) != word || character.is_whitespace() {
            break;
        }
        cursor = previous;
    }
    cursor
}

pub fn next_word_boundary(text: &str, offset: usize) -> usize {
    let mut cursor = next_char_boundary(text, offset);
    while let Some((character, next)) = next_character(text, cursor) {
        if !character.is_whitespace() {
            break;
        }
        cursor = next;
    }
    let Some((character, _)) = next_character(text, cursor) else {
        return text.len();
    };
    let word = character == '_' || character.is_alphanumeric();
    while let Some((character, next)) = next_character(text, cursor) {
        if (character == '_' || character.is_alphanumeric()) != word || character.is_whitespace() {
            break;
        }
        cursor = next;
    }
    cursor
}

pub fn clamp_to_char_boundary(text: &str, offset: usize) -> usize {
    previous_char_boundary(text, offset)
}

pub fn byte_offset_for_line_column(text: &str, line: usize, column: usize) -> usize {
    let target_line = line.max(1);
    let target_column = column.max(1);
    let mut current_line = 1;
    let mut line_start = 0;
    for (offset, character) in text.char_indices() {
        if current_line == target_line {
            break;
        }
        if character == '\n' {
            current_line += 1;
            line_start = offset + character.len_utf8();
        }
    }
    if current_line != target_line {
        return text.len();
    }
    let mut current_column = 1;
    for (offset, character) in text[line_start..].char_indices() {
        if current_column >= target_column || character == '\n' {
            return line_start + offset;
        }
        current_column += 1;
    }
    text.len()
}

fn previous_character(text: &str, cursor: usize) -> Option<(usize, char)> {
    text[..cursor.min(text.len())].char_indices().next_back()
}

fn next_character(text: &str, cursor: usize) -> Option<(char, usize)> {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .chars()
        .next()
        .map(|character| (character, cursor + character.len_utf8()))
}
