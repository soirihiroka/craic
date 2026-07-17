use super::{DiffKind, FileComparison, FileDiffRow, MAX_TEXT_PREVIEW_BYTES};

pub const MAX_BINARY_PREVIEW_BYTES: usize = 32 * 1024 * 1024;

pub fn comparison_from_unified_diff(
    diff: &str,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> FileComparison {
    FileComparison::from_rows(complete_diff_rows(
        parse_unified_diff(diff),
        left_lines,
        right_lines,
        complete_empty,
    ))
}

pub fn text_preview_lines(bytes: Option<&[u8]>) -> Result<Vec<String>, String> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(bytes)?;
    Ok(lines_from_bytes(bytes))
}

pub fn lines_from_bytes(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(ToString::to_string)
        .collect()
}

pub fn ensure_blob_text_previewable(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_TEXT_PREVIEW_BYTES {
        return Err("File is too large to preview as text.".to_string());
    }
    if is_binary_bytes(bytes) {
        return Err("Binary files cannot be previewed as text.".to_string());
    }
    Ok(())
}

pub fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

fn complete_diff_rows(
    rows: Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> Vec<FileDiffRow> {
    if rows.is_empty() {
        if complete_empty {
            let mut complete = Vec::new();
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                1,
                left_lines.len().saturating_add(1),
                1,
                right_lines.len().saturating_add(1),
            );
            return complete;
        }
        return rows;
    }

    let mut complete = Vec::new();
    let mut next_left = 1;
    let mut next_right = 1;

    for row in rows {
        if let (Some(left_number), Some(right_number)) = (row.left_number, row.right_number) {
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                next_left,
                left_number,
                next_right,
                right_number,
            );
        }

        if let Some(number) = row.left_number {
            next_left = number.saturating_add(1);
        }
        if let Some(number) = row.right_number {
            next_right = number.saturating_add(1);
        }

        complete.push(row);
    }

    append_context_gap(
        &mut complete,
        left_lines,
        right_lines,
        next_left,
        left_lines.len().saturating_add(1),
        next_right,
        right_lines.len().saturating_add(1),
    );

    complete
}

fn append_context_gap(
    rows: &mut Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) {
    let count = left_end
        .saturating_sub(left_start)
        .min(right_end.saturating_sub(right_start));

    for offset in 0..count {
        let left_number = left_start + offset;
        let right_number = right_start + offset;
        let text = left_lines
            .get(left_number.saturating_sub(1))
            .or_else(|| right_lines.get(right_number.saturating_sub(1)))
            .cloned()
            .unwrap_or_default();

        rows.push(FileDiffRow {
            left_number: Some(left_number),
            right_number: Some(right_number),
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }
}

#[derive(Default)]
struct DiffRowsBuilder {
    rows: Vec<FileDiffRow>,
    deleted: Vec<PendingDiffLine>,
    added: Vec<PendingDiffLine>,
}

impl DiffRowsBuilder {
    fn push_context(
        &mut self,
        left_number: Option<usize>,
        right_number: Option<usize>,
        text: String,
    ) {
        self.flush();
        self.rows.push(FileDiffRow {
            left_number,
            right_number,
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }

    fn push_deleted(&mut self, number: Option<usize>, text: String) {
        self.deleted.push(PendingDiffLine { number, text });
    }

    fn push_added(&mut self, number: Option<usize>, text: String) {
        self.added.push(PendingDiffLine { number, text });
    }

    fn flush(&mut self) {
        for index in 0..self.deleted.len().max(self.added.len()) {
            let deleted = self.deleted.get(index);
            let added = self.added.get(index);

            self.rows.push(FileDiffRow {
                left_number: deleted.and_then(|line| line.number),
                right_number: added.and_then(|line| line.number),
                left_text: deleted.map(|line| line.text.clone()),
                right_text: added.map(|line| line.text.clone()),
                left_kind: if deleted.is_some() {
                    DiffKind::Deleted
                } else {
                    DiffKind::Context
                },
                right_kind: if added.is_some() {
                    DiffKind::Added
                } else {
                    DiffKind::Context
                },
            });
        }

        self.deleted.clear();
        self.added.clear();
    }
}

struct PendingDiffLine {
    number: Option<usize>,
    text: String,
}

fn parse_unified_diff(diff: &str) -> Vec<FileDiffRow> {
    let mut builder = DiffRowsBuilder::default();
    let mut next_left = None::<usize>;
    let mut next_right = None::<usize>;
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            builder.flush();
            next_left = None;
            next_right = None;
            in_hunk = false;
            continue;
        }
        if !in_hunk && is_unified_metadata_line(line) {
            builder.flush();
            continue;
        }
        if line.starts_with("@@") {
            builder.flush();
            if let Some((left, right)) = parse_hunk_line_numbers(line) {
                next_left = Some(left);
                next_right = Some(right);
            }
            in_hunk = true;
            continue;
        }
        if line.starts_with("\\ ") {
            continue;
        }
        if !in_hunk {
            continue;
        }

        if let Some(text) = line.strip_prefix('-') {
            builder.push_deleted(next_left, text.to_string());
            next_left = next_left.map(|value| value.saturating_add(1));
        } else if let Some(text) = line.strip_prefix('+') {
            builder.push_added(next_right, text.to_string());
            next_right = next_right.map(|value| value.saturating_add(1));
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line).to_string();
            builder.push_context(next_left, next_right, text);
            next_left = next_left.map(|value| value.saturating_add(1));
            next_right = next_right.map(|value| value.saturating_add(1));
        }
    }

    builder.flush();
    builder.rows
}

fn is_unified_metadata_line(line: &str) -> bool {
    line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("dissimilarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
}

fn parse_hunk_line_numbers(line: &str) -> Option<(usize, usize)> {
    let end = line[2..].find("@@")? + 2;
    let header = line[2..end].trim();
    let mut parts = header.split_whitespace();
    let left = parts.next()?;
    let right = parts.next()?;
    Some((parse_hunk_start(left)?, parse_hunk_start(right)?))
}

fn parse_hunk_start(part: &str) -> Option<usize> {
    let number = part.trim_start_matches(['-', '+']);
    let number = number.split(',').next().unwrap_or(number);
    number.parse::<usize>().ok()
}
