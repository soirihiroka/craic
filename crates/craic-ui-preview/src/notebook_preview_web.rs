use serde_json::Value;

const MAX_CELLS: usize = 1_000;

pub fn html_document(source: &str) -> Result<String, String> {
    let notebook: Value = serde_json::from_str(source)
        .map_err(|error| format!("Unable to parse notebook JSON: {error}"))?;
    let version = notebook
        .get("nbformat")
        .and_then(Value::as_u64)
        .ok_or_else(|| "This file does not contain a valid notebook format version.".to_string())?;
    let cells = notebook_cells(&notebook, version)?;

    let mut body = String::new();
    for (index, cell) in cells.iter().take(MAX_CELLS).enumerate() {
        let kind = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("raw");
        let source = cell_source(cell, version);
        match kind {
            "markdown" => {
                body.push_str("<section class=\"notebook-cell markdown-cell markdown-body\">");
                body.push_str(&crate::markdown_preview_web::markdown_fragment_html(
                    &source,
                ));
                body.push_str("</section>");
            }
            "heading" => {
                let level = cell
                    .get("level")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 6) as usize;
                let markdown = format!("{} {source}", "#".repeat(level));
                body.push_str("<section class=\"notebook-cell markdown-cell markdown-body\">");
                body.push_str(&crate::markdown_preview_web::markdown_fragment_html(
                    &markdown,
                ));
                body.push_str("</section>");
            }
            "code" => {
                let count = cell
                    .get("execution_count")
                    .or_else(|| cell.get("prompt_number"))
                    .and_then(Value::as_i64)
                    .map_or_else(|| " ".to_string(), |count| count.to_string());
                body.push_str("<section class=\"notebook-cell code-cell\">");
                body.push_str(&format!(
                    "<div class=\"prompt\">In&nbsp;[{count}]:</div><pre><code>{}</code></pre>",
                    escape_html(&source)
                ));
                body.push_str("</section>");
            }
            _ => {
                body.push_str("<section class=\"notebook-cell raw-cell\"><pre>");
                body.push_str(&escape_html(&source));
                body.push_str("</pre></section>");
            }
        }

        if index + 1 == MAX_CELLS && cells.len() > MAX_CELLS {
            body.push_str(&format!(
                "<p class=\"truncated\">Showing the first {MAX_CELLS} of {} cells.</p>",
                cells.len()
            ));
        }
    }

    if cells.is_empty() {
        body.push_str("<p class=\"empty\">This notebook has no cells.</p>");
    }

    Ok(format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; padding: 24px 28px 48px; color: CanvasText; background: Canvas; font: -apple-system-body; }}
.notebook-cell {{ max-width: 980px; margin: 0 auto 18px; }}
.markdown-cell {{ line-height: 1.55; }}
.markdown-cell > :first-child {{ margin-top: 0; }}
.markdown-cell > :last-child {{ margin-bottom: 0; }}
.code-cell {{ display: grid; grid-template-columns: minmax(62px, auto) minmax(0, 1fr); gap: 12px; align-items: start; }}
.prompt {{ color: color-mix(in srgb, CanvasText 58%, transparent); font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; padding-top: 12px; text-align: right; white-space: nowrap; }}
pre {{ margin: 0; padding: 12px 14px; overflow-x: auto; border-radius: 8px; background: color-mix(in srgb, CanvasText 7%, transparent); font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre; }}
.raw-cell pre {{ white-space: pre-wrap; }}
.empty, .truncated {{ max-width: 980px; margin: 40px auto; color: color-mix(in srgb, CanvasText 58%, transparent); text-align: center; }}
a {{ color: LinkText; }}
img {{ max-width: 100%; height: auto; }}
@media (prefers-color-scheme: dark) {{ pre {{ background: color-mix(in srgb, CanvasText 10%, transparent); }} }}
</style>
</head>
<body>{body}</body>
</html>"#
    ))
}

fn notebook_cells(notebook: &Value, version: u64) -> Result<Vec<&Value>, String> {
    if version >= 4 {
        return notebook
            .get("cells")
            .and_then(Value::as_array)
            .map(|cells| cells.iter().collect())
            .ok_or_else(|| "This notebook does not contain a valid cells array.".to_string());
    }

    let worksheets = notebook
        .get("worksheets")
        .and_then(Value::as_array)
        .ok_or_else(|| "This legacy notebook does not contain a worksheets array.".to_string())?;
    Ok(worksheets
        .iter()
        .filter_map(|worksheet| worksheet.get("cells").and_then(Value::as_array))
        .flatten()
        .collect())
}

fn cell_source(cell: &Value, version: u64) -> String {
    let key = if version < 4 && cell.get("cell_type").and_then(Value::as_str) == Some("code") {
        "input"
    } else {
        "source"
    };
    match cell.get(key) {
        Some(Value::String(source)) => source.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect::<String>(),
        _ => String::new(),
    }
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
