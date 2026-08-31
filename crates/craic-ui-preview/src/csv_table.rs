const MAX_VISIBLE_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
}

pub fn parse_csv_table(source: &str) -> Result<Option<CsvTable>, String> {
    let records = parse_csv_records(source)?;
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Ok(None);
    };
    let rows = records.collect::<Vec<_>>();
    let column_count = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(header.len());
    if column_count > MAX_COLUMNS {
        return Err(format!(
            "This CSV has {column_count} columns; table preview supports up to {MAX_COLUMNS}."
        ));
    }

    let columns = (0..column_count)
        .map(|index| {
            header
                .get(index)
                .filter(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| format!("Column {}", index + 1))
        })
        .collect();
    let total_rows = rows.len();
    let rows = rows.into_iter().take(MAX_VISIBLE_ROWS).collect();
    Ok(Some(CsvTable {
        columns,
        rows,
        total_rows,
    }))
}

fn parse_csv_records(source: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut field_started = false;
    let mut quoted = false;
    let mut chars = source.chars().peekable();

    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }

        match ch {
            '"' if !field_started => {
                quoted = true;
                field_started = true;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                field_started = false;
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                field_started = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                field_started = false;
            }
            _ => {
                field.push(ch);
                field_started = true;
            }
        }
    }

    if quoted {
        return Err("The CSV contains an unterminated quoted field.".to_string());
    }
    if field_started || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}
