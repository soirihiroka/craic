use objc2_foundation::NSRange;

pub fn byte_offset(text: &str, target: usize) -> Option<usize> {
    let (byte, exact) = byte_offset_position(text, target);
    exact.then_some(byte)
}

pub fn byte_offset_clamped(text: &str, target: usize) -> usize {
    byte_offset_position(text, target).0
}

fn byte_offset_position(text: &str, target: usize) -> (usize, bool) {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == target {
            return (byte, true);
        }
        units += character.len_utf16();
        if units > target {
            return (byte, false);
        }
    }
    (text.len(), units == target)
}

pub fn offset_for_byte(text: &str, byte: usize) -> usize {
    text.get(..byte.min(text.len()))
        .unwrap_or_default()
        .encode_utf16()
        .count()
}

pub fn exact_range_for_bytes(text: &str, start: usize, end: usize) -> Option<NSRange> {
    if start >= end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return None;
    }
    let utf16_start = offset_for_byte(text, start);
    let utf16_end = utf16_start + text[start..end].encode_utf16().count();
    Some(NSRange::new(utf16_start, utf16_end - utf16_start))
}

pub fn range_for_bytes(text: &str, start: usize, end: usize) -> NSRange {
    let start = offset_for_byte(text, start);
    let end = offset_for_byte(text, end);
    NSRange::new(start, end.saturating_sub(start))
}

pub fn byte_range(text: &str, range: NSRange) -> (usize, usize) {
    let start = byte_offset_clamped(text, range.location);
    let end = byte_offset_clamped(text, range.location.saturating_add(range.length));
    (start.min(end), start.max(end))
}

pub fn offset_for_line_column(text: &str, line: usize, column: usize) -> usize {
    let target_line = line.max(1);
    let target_column = column.max(1);
    let mut offset = 0;
    for (line_index, current) in text.split_inclusive('\n').enumerate() {
        if line_index + 1 == target_line {
            offset += current
                .trim_end_matches('\n')
                .chars()
                .take(target_column - 1)
                .map(char::len_utf16)
                .sum::<usize>();
            return offset;
        }
        offset += current.encode_utf16().count();
    }
    text.encode_utf16().count()
}
