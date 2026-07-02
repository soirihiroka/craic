use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

pub(super) enum RenderCell {
    Markdown {
        source: String,
    },
    Code {
        execution_count: Option<i32>,
        source: String,
    },
    Raw {
        source: String,
    },
}

impl RenderCell {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            RenderCell::Markdown { .. } => "markdown",
            RenderCell::Code { .. } => "code",
            RenderCell::Raw { .. } => "raw",
        }
    }

    pub(super) fn source_len(&self) -> usize {
        match self {
            RenderCell::Markdown { source }
            | RenderCell::Code { source, .. }
            | RenderCell::Raw { source } => source.len(),
        }
    }
}

pub(super) fn show(
    right: Rc<super::super::right::RightPane>,
    load_token: super::super::right::PreviewLoadToken,
    file_path: String,
    local_path: Option<PathBuf>,
    prefetched_bytes: Option<Vec<u8>>,
) {
    right.show_provider_loading_message(&file_path, "Rendering read-only notebook preview...");

    let apply_file_path = file_path.clone();
    super::spawn_preview_load(
        Rc::clone(&right),
        load_token,
        file_path.clone(),
        move || {
            log::info!("readonly notebook preview reading file_path={file_path}");
            let text = super::super::repository_text_from_prefetch(prefetched_bytes, &file_path)?;
            log::info!(
                "readonly notebook preview read file_path={file_path} bytes={}",
                text.len()
            );
            parse_notebook_cells(&text, &file_path)
        },
        move |right, result| match result {
            Ok(cells) => {
                log::info!(
                    "readonly notebook preview parsed cells ready file_path={apply_file_path} cells={}",
                    cells.len()
                );
                right.show_notebook_preview(&apply_file_path);
                right
                    .file_notebook_preview
                    .load_readonly_cells(cells, local_path.as_deref());
            }
            Err(message) => {
                log::warn!(
                    "readonly notebook preview failed file_path={apply_file_path}: {message}"
                );
                right.show_unavailable(&apply_file_path, &message);
            }
        },
    );
}

fn parse_notebook_cells(text: &str, file_path: &str) -> Result<Vec<RenderCell>, String> {
    let started = Instant::now();
    let notebook = nbformat::parse_notebook(text)
        .map_err(|err| format!("Unable to parse notebook JSON: {err}"))?;
    let cells = render_cells(notebook);
    log::info!(
        "readonly notebook preview parsed file_path={file_path} cells={} markdown={} code={} raw={} elapsed_ms={}",
        cells.len(),
        cells
            .iter()
            .filter(|cell| matches!(cell, RenderCell::Markdown { .. }))
            .count(),
        cells
            .iter()
            .filter(|cell| matches!(cell, RenderCell::Code { .. }))
            .count(),
        cells
            .iter()
            .filter(|cell| matches!(cell, RenderCell::Raw { .. }))
            .count(),
        started.elapsed().as_millis()
    );

    Ok(cells)
}

fn render_cells(notebook: nbformat::Notebook) -> Vec<RenderCell> {
    match notebook {
        nbformat::Notebook::V4(notebook) => render_v4_cells(notebook.cells),
        nbformat::Notebook::V4QuirksMode(quirks) => {
            log::info!(
                "readonly notebook preview accepted v4 quirks quirks={}",
                quirks.quirks().len()
            );
            render_v4_cells(quirks.repair().cells)
        }
        nbformat::Notebook::Legacy(notebook) => render_legacy_cells(notebook.cells),
        nbformat::Notebook::V3(notebook) => notebook
            .worksheets
            .unwrap_or_default()
            .into_iter()
            .flat_map(|worksheet| render_v3_cells(worksheet.cells))
            .collect(),
        _ => {
            log::warn!("readonly notebook preview received unsupported nbformat notebook variant");
            Vec::new()
        }
    }
}

fn render_v4_cells(cells: Vec<nbformat::v4::Cell>) -> Vec<RenderCell> {
    cells
        .into_iter()
        .map(|cell| match cell {
            nbformat::v4::Cell::Markdown { source, .. } => RenderCell::Markdown {
                source: source.join(""),
            },
            nbformat::v4::Cell::Code {
                execution_count,
                source,
                ..
            } => RenderCell::Code {
                execution_count,
                source: source.join(""),
            },
            nbformat::v4::Cell::Raw { source, .. } => RenderCell::Raw {
                source: source.join(""),
            },
        })
        .collect()
}

fn render_legacy_cells(cells: Vec<nbformat::legacy::Cell>) -> Vec<RenderCell> {
    cells
        .into_iter()
        .map(|cell| match cell {
            nbformat::legacy::Cell::Markdown { source, .. } => RenderCell::Markdown {
                source: source.join(""),
            },
            nbformat::legacy::Cell::Code {
                execution_count,
                source,
                ..
            } => RenderCell::Code {
                execution_count,
                source: source.join(""),
            },
            nbformat::legacy::Cell::Raw { source, .. } => RenderCell::Raw {
                source: source.join(""),
            },
        })
        .collect()
}

fn render_v3_cells(cells: Vec<nbformat::v3::Cell>) -> Vec<RenderCell> {
    cells
        .into_iter()
        .map(|cell| match cell {
            nbformat::v3::Cell::Heading { level, source, .. } => {
                let hashes = "#".repeat(level.clamp(1, 6) as usize);
                RenderCell::Markdown {
                    source: format!("{hashes} {}", source.join("")),
                }
            }
            nbformat::v3::Cell::Markdown { source, .. } => RenderCell::Markdown {
                source: source.join(""),
            },
            nbformat::v3::Cell::Code {
                prompt_number,
                input,
                ..
            } => RenderCell::Code {
                execution_count: prompt_number,
                source: input.unwrap_or_default().join(""),
            },
            nbformat::v3::Cell::Raw { source, .. } => RenderCell::Raw {
                source: source.join(""),
            },
        })
        .collect()
}
