use jsonc_parser::ParseOptions;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextFormat {
    Json,
    Jsonc,
    JsonLines,
}

impl TextFormat {
    pub fn for_path(path: &str) -> Option<Self> {
        match Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "json" => Some(Self::Json),
            "jsonc" => Some(Self::Jsonc),
            "jsonl" | "ndjson" => Some(Self::JsonLines),
            _ => None,
        }
    }
}

impl fmt::Display for TextFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "JSON",
            Self::Jsonc => "JSONC",
            Self::JsonLines => "JSON Lines",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    Value,
    Records,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicDocument {
    pub kind: DocumentKind,
    pub root: Arc<DynamicValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DynamicValue {
    Null,
    Boolean(bool),
    Number(String),
    String(String),
    Sequence(Vec<Arc<DynamicValue>>),
    Mapping(Vec<DynamicEntry>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicEntry {
    pub key: Arc<DynamicValue>,
    pub value: Arc<DynamicValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub format: TextFormat,
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub record: Option<usize>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(record) = self.record {
            write!(
                formatter,
                "{} record {record}, line {}, column {}: {}",
                self.format, self.line, self.column, self.message
            )
        } else {
            write!(
                formatter,
                "{} line {}, column {}: {}",
                self.format, self.line, self.column, self.message
            )
        }
    }
}

impl std::error::Error for ParseError {}

pub fn parse_text(format: TextFormat, source: &str) -> Result<DynamicDocument, ParseError> {
    match format {
        TextFormat::Json => parse_json(source),
        TextFormat::Jsonc => parse_jsonc(source),
        TextFormat::JsonLines => parse_json_lines(source),
    }
}

fn parse_json(source: &str) -> Result<DynamicDocument, ParseError> {
    serde_json::from_str(source)
        .map(|value| DynamicDocument {
            kind: DocumentKind::Value,
            root: dynamic_value(value),
        })
        .map_err(|error| serde_error(TextFormat::Json, error, 0, None))
}

fn parse_jsonc(source: &str) -> Result<DynamicDocument, ParseError> {
    let options = ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    jsonc_parser::parse_to_serde_value(source, &options)
        .map(|value| DynamicDocument {
            kind: DocumentKind::Value,
            root: dynamic_value(value),
        })
        .map_err(|error| ParseError {
            format: TextFormat::Jsonc,
            message: error.kind().to_string(),
            line: error.line_display(),
            column: error.column_display(),
            record: None,
        })
}

fn parse_json_lines(source: &str) -> Result<DynamicDocument, ParseError> {
    let mut values = Vec::new();
    let mut record = 0;
    for (line_index, line) in source.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        record += 1;
        let value = serde_json::from_str(line)
            .map_err(|error| serde_error(TextFormat::JsonLines, error, line_index, Some(record)))?;
        values.push(dynamic_value(value));
    }
    Ok(DynamicDocument {
        kind: DocumentKind::Records,
        root: Arc::new(DynamicValue::Sequence(values)),
    })
}

fn serde_error(
    format: TextFormat,
    error: serde_json::Error,
    line_offset: usize,
    record: Option<usize>,
) -> ParseError {
    let line = error.line();
    let column = error.column();
    let rendered = error.to_string();
    let suffix = format!(" at line {line} column {column}");
    ParseError {
        format,
        message: rendered
            .strip_suffix(&suffix)
            .unwrap_or(&rendered)
            .to_string(),
        line: line + line_offset,
        column,
        record,
    }
}

fn dynamic_value(value: serde_json::Value) -> Arc<DynamicValue> {
    Arc::new(match value {
        serde_json::Value::Null => DynamicValue::Null,
        serde_json::Value::Bool(value) => DynamicValue::Boolean(value),
        serde_json::Value::Number(value) => DynamicValue::Number(value.to_string()),
        serde_json::Value::String(value) => DynamicValue::String(value),
        serde_json::Value::Array(values) => {
            DynamicValue::Sequence(values.into_iter().map(dynamic_value).collect())
        }
        serde_json::Value::Object(values) => DynamicValue::Mapping(
            values
                .into_iter()
                .map(|(key, value)| DynamicEntry {
                    key: Arc::new(DynamicValue::String(key)),
                    value: dynamic_value(value),
                })
                .collect(),
        ),
    })
}
