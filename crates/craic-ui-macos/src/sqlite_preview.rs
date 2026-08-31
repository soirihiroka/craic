use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use std::path::Path;
use std::time::Duration;

pub const PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Table {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sort {
    pub column_index: usize,
    pub direction: SortDirection,
}

pub struct Page {
    pub table: Table,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
    pub page: usize,
}

pub fn load_schema(db_path: &Path) -> Result<Vec<Table>, String> {
    let conn = open_database(db_path)?;
    let mut statement = conn
        .prepare(
            "SELECT name, type FROM sqlite_schema \
             WHERE type IN ('table', 'view') \
             ORDER BY lower(name), name",
        )
        .map_err(|error| format!("Unable to read SQLite schema: {error}"))?;
    statement
        .query_map([], |row| {
            Ok(Table {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        })
        .map_err(|error| format!("Unable to read SQLite schema: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Unable to read SQLite schema: {error}"))
}

pub fn load_page(
    db_path: &Path,
    table: Table,
    page: usize,
    filter_column: Option<usize>,
    filter: &str,
    sort: Option<Sort>,
) -> Result<Page, String> {
    let conn = open_database(db_path)?;
    let columns = load_columns(&conn, &table)?;
    if columns.is_empty() {
        return Ok(Page {
            table,
            columns,
            rows: Vec::new(),
            total_rows: 0,
            page,
        });
    }

    let table_sql = quote_identifier(&table.name);
    let select_sql = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let filter = filter.trim();
    let filter_columns = if filter.is_empty() {
        Vec::new()
    } else if let Some(index) = filter_column {
        columns.get(index).into_iter().collect::<Vec<_>>()
    } else {
        columns.iter().collect::<Vec<_>>()
    };
    let where_sql = if filter_columns.is_empty() {
        String::new()
    } else {
        format!(
            " WHERE {}",
            filter_columns
                .iter()
                .map(|column| format!(
                    "CAST({} AS TEXT) LIKE ? ESCAPE '\\'",
                    quote_identifier(&column.name)
                ))
                .collect::<Vec<_>>()
                .join(" OR ")
        )
    };
    let pattern = like_pattern(filter);
    let parameters = (0..filter_columns.len())
        .map(|_| pattern.as_str())
        .collect::<Vec<_>>();
    let count_sql = format!("SELECT COUNT(*) FROM {table_sql}{where_sql}");
    let total_rows = conn
        .query_row(&count_sql, params_from_iter(parameters.iter()), |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as usize)
        .map_err(|error| format!("Unable to count rows for {}: {error}", table.name))?;

    let order_sql = sort
        .as_ref()
        .and_then(|sort| {
            columns.get(sort.column_index).map(|column| {
                format!(
                    " ORDER BY {} {}",
                    quote_identifier(&column.name),
                    match sort.direction {
                        SortDirection::Ascending => "ASC",
                        SortDirection::Descending => "DESC",
                    }
                )
            })
        })
        .unwrap_or_default();
    let offset = page.saturating_mul(PAGE_SIZE);
    let rows_sql = format!(
        "SELECT {select_sql} FROM {table_sql}{where_sql}{order_sql} LIMIT {PAGE_SIZE} OFFSET {offset}"
    );
    let mut statement = conn
        .prepare(&rows_sql)
        .map_err(|error| format!("Unable to read rows from {}: {error}", table.name))?;
    let mut cursor = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(|error| format!("Unable to read rows from {}: {error}", table.name))?;
    let mut rows = Vec::new();
    while let Some(row) = cursor
        .next()
        .map_err(|error| format!("Unable to read row from {}: {error}", table.name))?
    {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let value = row
                .get_ref(index)
                .map(sqlite_value_text)
                .map_err(|error| format!("Unable to read value from {}: {error}", table.name))?;
            values.push(value);
        }
        rows.push(values);
    }

    Ok(Page {
        table,
        columns,
        rows,
        total_rows,
        page,
    })
}

fn load_columns(conn: &Connection, table: &Table) -> Result<Vec<Column>, String> {
    let pragma = conn
        .prepare(
            "SELECT name, type, pk FROM pragma_table_xinfo(?) \
             WHERE hidden = 0 ORDER BY cid",
        )
        .and_then(|mut statement| {
            statement
                .query_map([table.name.as_str()], |row| {
                    Ok(Column {
                        name: row.get(0)?,
                        data_type: row.get(1)?,
                        primary_key: row.get::<_, i64>(2)? > 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        });
    match pragma {
        Ok(columns) if !columns.is_empty() => Ok(columns),
        Ok(_) => fallback_columns(conn, table),
        Err(error) => {
            log::debug!(
                "native SQLite pragma_table_xinfo fallback table={} error={error}",
                table.name
            );
            fallback_columns(conn, table)
        }
    }
}

fn fallback_columns(conn: &Connection, table: &Table) -> Result<Vec<Column>, String> {
    let sql = format!("SELECT * FROM {} LIMIT 0", quote_identifier(&table.name));
    let statement = conn
        .prepare(&sql)
        .map_err(|error| format!("Unable to read columns for {}: {error}", table.name))?;
    Ok(statement
        .column_names()
        .into_iter()
        .map(|name| Column {
            name: name.to_string(),
            data_type: String::new(),
            primary_key: false,
        })
        .collect())
}

fn open_database(db_path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Unable to open SQLite database: {error}"))?;
    connection
        .busy_timeout(Duration::from_millis(750))
        .map_err(|error| format!("Unable to configure SQLite timeout: {error}"))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| format!("Unable to mark SQLite connection read-only: {error}"))?;
    Ok(connection)
}

fn sqlite_value_text(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        ValueRef::Blob(value) => format!("<blob {} bytes>", value.len()),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn like_pattern(filter: &str) -> String {
    let mut pattern = String::with_capacity(filter.len() + 2);
    pattern.push('%');
    for character in filter.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}
