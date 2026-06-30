const INDENT_UNIT: &str = "    ";

static PLAIN_TEXT_NEWLINE: PlainTextNewline = PlainTextNewline;
static RUST_NEWLINE: RustNewline = RustNewline;

pub(crate) struct NewlineContext<'a> {
    pub(crate) text: &'a str,
    pub(crate) cursor: usize,
    pub(crate) language: &'a str,
}

pub(crate) struct EnterNewline {
    pub(crate) inserted: String,
    pub(crate) cursor: usize,
}

trait NewlineService {
    fn indent_for_newline(&self, text: &str, cursor: usize) -> String {
        current_line_indent(text, cursor).to_string()
    }
}

pub(crate) fn enter_newline(context: NewlineContext<'_>) -> EnterNewline {
    let cursor = previous_char_boundary(context.text, context.cursor);
    let indent = service_for_language(context.language).indent_for_newline(context.text, cursor);
    let inserted = format!("\n{indent}");
    let cursor = cursor + inserted.len();

    EnterNewline { inserted, cursor }
}

fn service_for_language(language: &str) -> &'static dyn NewlineService {
    match normalize_language_name(language).as_str() {
        "rust" | "rs" => &RUST_NEWLINE,
        _ => &PLAIN_TEXT_NEWLINE,
    }
}

struct PlainTextNewline;

impl NewlineService for PlainTextNewline {}

struct RustNewline;

impl NewlineService for RustNewline {
    fn indent_for_newline(&self, text: &str, cursor: usize) -> String {
        rust_chain_indent(text, cursor).unwrap_or_else(|| current_line_indent(text, cursor).into())
    }
}

fn rust_chain_indent(text: &str, cursor: usize) -> Option<String> {
    let cursor = previous_char_boundary(text, cursor.min(text.len()));
    let line_start = current_line_start(text, cursor);
    let line = &text[line_start..cursor];
    let code = rust_code_before_line_comment(line).trim_end();
    let content = code.trim_start();

    if content.is_empty() {
        return None;
    }

    if content.starts_with('.') {
        if rust_chain_line_ends_chain(content) {
            return chain_head_indent(text, line_start);
        }
        return Some(leading_whitespace(code).to_string());
    }

    if rust_line_starts_chain(content) {
        return Some(format!("{}{}", leading_whitespace(code), INDENT_UNIT));
    }

    None
}

fn rust_chain_line_ends_chain(content: &str) -> bool {
    let content = content.trim_end();
    if content.ends_with(';') || content.ends_with(',') {
        return true;
    }

    last_dot_method_name(content).is_some_and(|name| name == "build")
}

fn rust_line_starts_chain(content: &str) -> bool {
    let content = content.trim_end();
    if content.ends_with(';')
        || content.ends_with(',')
        || content.ends_with('{')
        || content.ends_with('}')
        || content.ends_with(':')
        || content.ends_with("=>")
    {
        return false;
    }

    if !content.ends_with(')') {
        return false;
    }

    content.contains("::builder()") || last_dot_method_name(content).is_some()
}

fn chain_head_indent(text: &str, mut line_start: usize) -> Option<String> {
    while line_start > 0 {
        let previous_end = line_start.saturating_sub(1);
        let previous_start = current_line_start(text, previous_end);
        let previous_line = &text[previous_start..previous_end];
        let previous_code = rust_code_before_line_comment(previous_line).trim_end();
        let trimmed = previous_code.trim_start();

        if trimmed.is_empty() || trimmed.starts_with('.') {
            line_start = previous_start;
            continue;
        }

        return Some(leading_whitespace(previous_code).to_string());
    }

    None
}

fn last_dot_method_name(code: &str) -> Option<&str> {
    let mut offset = 0;
    let mut last = None;

    while let Some(dot_offset) = code[offset..].find('.') {
        let name_start = offset + dot_offset + '.'.len_utf8();
        let Some((_, first)) = code[name_start..].char_indices().next() else {
            break;
        };
        if !is_identifier_start(first) {
            offset = name_start;
            continue;
        }

        let name_end = name_start
            + code[name_start..]
                .char_indices()
                .take_while(|(_, ch)| is_identifier_char(*ch))
                .map(|(offset, ch)| offset + ch.len_utf8())
                .last()
                .unwrap_or(first.len_utf8());
        let after_name = code[name_end..].trim_start();
        if after_name.starts_with('(') {
            last = Some(&code[name_start..name_end]);
        }
        offset = name_end;
    }

    last
}

fn rust_code_before_line_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    for (offset, ch) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if in_char {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '\'' {
                in_char = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            continue;
        }

        if ch == '\'' {
            in_char = true;
            continue;
        }

        if line[offset..].starts_with("//") {
            return &line[..offset];
        }
    }

    line
}

fn current_line_indent(text: &str, cursor: usize) -> &str {
    let line_start = current_line_start(text, cursor);
    let line_end = current_line_end(text, line_start);
    leading_whitespace(&text[line_start..line_end])
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .take_while(|(_, ch)| matches!(ch, ' ' | '\t'))
        .map(|(offset, ch)| offset + ch.len_utf8())
        .last()
        .unwrap_or(0);
    &line[..end]
}

fn current_line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .rfind('\n')
        .map(|offset| offset + 1)
        .unwrap_or(0)
}

fn current_line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn previous_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn normalize_language_name(language: &str) -> String {
    language.trim().to_ascii_lowercase()
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn newline_at_end(text: &str, language: &str) -> EnterNewline {
        enter_newline(NewlineContext {
            text,
            cursor: text.len(),
            language,
        })
    }

    #[test]
    fn default_newline_preserves_current_line_indent() {
        let text = "root\n\t    child";

        let newline = newline_at_end(text, "plain");

        assert_eq!(newline.inserted, "\n\t    ");
        assert_eq!(newline.cursor, text.len() + newline.inserted.len());
    }

    #[test]
    fn rust_chain_method_newline_keeps_dot_line_indent() {
        let text = concat!(
            "fn view() {\n",
            "    let widget = gtk::Box::builder()\n",
            "        .margin_start(10)",
        );

        let newline = newline_at_end(text, "rust");

        assert_eq!(newline.inserted, "\n        ");
        assert_eq!(newline.cursor, text.len() + newline.inserted.len());
    }

    #[test]
    fn rust_build_terminator_newline_returns_to_let_indent() {
        let text = concat!(
            "fn view() {\n",
            "    let widget = gtk::Box::builder()\n",
            "        .orientation(gtk::Orientation::Vertical)\n",
            "        .margin_start(10)\n",
            "        .build();",
        );

        let newline = newline_at_end(text, "rust");

        assert_eq!(newline.inserted, "\n    ");
        assert_eq!(newline.cursor, text.len() + newline.inserted.len());
    }
}
