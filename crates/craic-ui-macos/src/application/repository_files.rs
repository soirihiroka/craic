async fn handle_repository_files(
    request: RepositoryRequest,
    repository_completion_tx: std::sync::mpsc::Sender<FrontendCompletion>,
    retired_jobs: RetiredJobSender,
) {
    match request {
        RepositoryRequest::LoadFilesTree {
            workspace_id,
            handle,
            expanded,
            generation,
            cancellation,
        } => {
            let task = tokio::task::spawn_blocking(move || {
                load_native_file_tree(&handle, &expanded)
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "files-tree",
                task,
            )
            .await
            else {
                log::debug!("workspace file tree canceled workspace={workspace_id}");
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Workspace file loading task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FilesTree {
                        workspace_id,
                        generation,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("workspace file tree dropped during shutdown");
            }
        }
        RepositoryRequest::LoadWorkspaceFile {
            workspace_id,
            handle,
            path,
            request_id,
            cancellation,
        } => {
            let cancel_requested = Arc::new(AtomicBool::new(false));
            let max_bytes = if is_font_preview_path(&path.display()) {
                FONT_CONTENT_PREVIEW_LIMIT
            } else {
                FILE_CONTENT_PREVIEW_LIMIT
            };
            let mut events = handle.workspace_files().read_with_info_events(
                FileReadRequest {
                    path: path.clone(),
                    max_bytes: Some(max_bytes),
                    cancel_requested: Some(cancel_requested.clone()),
                },
            );
            let load = async move {
                while let Some(event) = events.recv().await {
                    if let craic_system::system::capabilities::files::FileOperationEvent::Finished(result) = event {
                        return result.map_err(|error| error.to_string());
                    }
                }
                Err("Workspace file read ended without a result.".to_string())
            };
            let load = tokio::time::timeout(REPOSITORY_CALLBACK_TIMEOUT, load);
            let load = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    cancel_requested.store(true, Ordering::SeqCst);
                    log::debug!(
                        "workspace file preview canceled workspace={workspace_id} path={}",
                        path.display()
                    );
                    return;
                }
                result = load => result,
            };
            let mut result = match load {
                Ok(result) => result,
                Err(_) => {
                    cancel_requested.store(true, Ordering::SeqCst);
                    Err("Workspace file read timed out.".to_string())
                }
            };
            let path_display = path.display();
            let is_safetensors = Path::new(&path_display)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("safetensors")
                });
            let needs_local_header = is_safetensors
                && result
                    .as_ref()
                    .is_ok_and(|read| read.bytes.is_none());
            if needs_local_header
                && let Some(local_path) =
                    handle.workspace_files().local_path(&path)
            {
                log::debug!(
                    "native Safetensors bounded header read path={path_display}"
                );
                let header_path = path_display.clone();
                let task = tokio::task::spawn_blocking(move || {
                    read_metadata_header(&local_path, &header_path)
                });
                let Some(header) =
                    until_workspace_change(
                        &cancellation,
                        &retired_jobs,
                        "safetensors-header",
                        task,
                    )
                    .await
                else {
                    log::debug!(
                        "native Safetensors header read canceled workspace={workspace_id} path={path_display}"
                    );
                    return;
                };
                match header {
                    Ok(Ok(bytes)) => {
                        if let Ok(read) = &mut result {
                            read.bytes = Some(bytes);
                        }
                    }
                    Ok(Err(error)) => result = Err(error),
                    Err(error) => {
                        result = Err(format!(
                            "Safetensors metadata task failed: {error}"
                        ));
                    }
                }
            }
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceFile {
                        workspace_id,
                        path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("workspace file preview dropped during shutdown");
            }
        }
        RepositoryRequest::LoadWorkspaceFolder {
            workspace_id,
            handle,
            path,
            info,
            request_id,
            cancellation,
        } => {
            let completion_path = path.clone();
            let access = handle.workspace_files();
            let task = tokio::task::spawn_blocking(move || {
                log::debug!(
                    "native folder preview load start path={}",
                    path.display()
                );
                let provider_path = access.copy_path(&path);
                let entries = access
                    .list_dirs(std::slice::from_ref(&path))?
                    .into_iter()
                    .next()
                    .map(|listing| listing.entries)
                    .unwrap_or_default();
                let mut file_count = 0usize;
                let mut folder_count = 0usize;
                for entry in access.info_many(&entries)? {
                    if entry.kind == FileNodeKind::Directory {
                        folder_count += 1;
                    } else if entry.kind.is_file() {
                        file_count += 1;
                    }
                }
                log::debug!(
                    "native folder preview load complete path={} files={} folders={}",
                    path.display(),
                    file_count,
                    folder_count
                );
                Ok::<_, String>(NativeFolderPreview {
                    info,
                    provider_path,
                    file_count,
                    folder_count,
                })
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "folder-preview",
                task,
            )
            .await
            else {
                log::debug!(
                    "native folder preview canceled workspace={workspace_id} path={}",
                    completion_path.display()
                );
                return;
            };
            let result = result.unwrap_or_else(|error| {
                Err(format!("Folder preview task failed: {error}"))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceFolder {
                        workspace_id,
                        path: completion_path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("workspace folder preview dropped during shutdown");
            }
        }
        RepositoryRequest::LoadWorkspaceSqliteSchema {
            workspace_id,
            handle,
            path,
            info,
            prefetched_bytes,
            request_id,
            cancellation,
        } => {
            let completion_path = path.clone();
            let access = handle.workspace_files();
            let local_path = access.local_path(&path);
            let bytes = if local_path.is_some() {
                None
            } else if let Some(bytes) = prefetched_bytes {
                Some(Ok(bytes))
            } else {
                let cancel_requested = Arc::new(AtomicBool::new(false));
                let events = access.read_with_info_events(FileReadRequest {
                    path: path.clone(),
                    max_bytes: Some(SQLITE_MATERIALIZE_LIMIT),
                    cancel_requested: Some(cancel_requested.clone()),
                });
                let Some(result) = wait_file_operation(
                    events,
                    &cancellation,
                    cancel_requested,
                )
                .await
                else {
                    log::debug!(
                        "native SQLite schema read canceled workspace={workspace_id} path={}",
                        path.display()
                    );
                    return;
                };
                Some(result.and_then(FileRead::into_bytes))
            };
            let task = tokio::task::spawn_blocking(move || {
                let (db_path, materialized) = match (local_path, bytes) {
                    (Some(local_path), _) => (local_path, None),
                    (None, Some(Ok(bytes))) => {
                        let materialized = materialize_bytes_for_view(
                            &info,
                            bytes,
                            Some(SQLITE_MATERIALIZE_LIMIT),
                        )?;
                        (materialized.path().to_path_buf(), Some(materialized))
                    }
                    (None, Some(Err(error))) => return Err(error),
                    (None, None) => {
                        return Err(
                            "SQLite database could not be materialized.".to_string()
                        );
                    }
                };
                let tables = sqlite_preview::load_schema(&db_path)?;
                Ok(NativeSqliteSchema {
                    db_path,
                    materialized,
                    tables,
                })
            });
            let result = match wait_workspace_job(
                &cancellation,
                &retired_jobs,
                "sqlite-schema",
                task,
                None,
            )
            .await
            {
                NativeJobWait::WorkspaceChanged => {
                    log::debug!(
                        "native SQLite schema load canceled workspace={workspace_id} path={}",
                        completion_path.display()
                    );
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("SQLite schema task timed out.".to_string())
                }
                NativeJobWait::Completed(joined) => joined.unwrap_or_else(|error| {
                    Err(format!("SQLite schema task failed: {error}"))
                }),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceSqliteSchema {
                        workspace_id,
                        path: completion_path,
                        request_id,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("workspace SQLite schema preview dropped during shutdown");
            }
        }
        RepositoryRequest::LoadWorkspaceSqlitePage {
            workspace_id,
            path,
            db_path,
            table,
            page,
            filter_column,
            filter,
            sort,
            generation,
            cancellation,
        } => {
            let task = tokio::task::spawn_blocking(move || {
                sqlite_preview::load_page(
                    &db_path,
                    table,
                    page,
                    filter_column,
                    &filter,
                    sort,
                )
            });
            let result = match wait_workspace_job(
                &cancellation,
                &retired_jobs,
                "sqlite-page",
                task,
                None,
            )
            .await
            {
                NativeJobWait::WorkspaceChanged => {
                    log::debug!(
                        "native SQLite page load canceled workspace={workspace_id} path={}",
                        path.display()
                    );
                    return;
                }
                NativeJobWait::TimedOut => {
                    Err("SQLite page task timed out.".to_string())
                }
                NativeJobWait::Completed(joined) => joined.unwrap_or_else(|error| {
                    Err(format!("SQLite page task failed: {error}"))
                }),
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceSqlitePage {
                        workspace_id,
                        path,
                        generation,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("workspace SQLite page preview dropped during shutdown");
            }
        }
        RepositoryRequest::RunFileMutation {
            workspace_id,
            access,
            mutation,
            allow_sudo_retry,
            cancellation,
        } => {
            let completion_access = access.clone();
            let cancel_requested = Arc::new(AtomicBool::new(false));
            let result = match mutation.clone() {
                NativeFileMutation::CreateFile { path } => {
                    let events = access.write_node_events(FileWriteRequest {
                        path: path.clone(),
                        mode: FileWriteMode::CreateNew,
                        payload: FileWritePayload::File(Vec::new()),
                        cancel_requested: Some(cancel_requested.clone()),
                    });
                    wait_file_operation(
                        events,
                        &cancellation,
                        cancel_requested.clone(),
                    )
                    .await
                    .map(|result| result.map(|()| Some(path)))
                }
                NativeFileMutation::CreateDirectory { path } => {
                    let events = access.write_node_events(FileWriteRequest {
                        path: path.clone(),
                        mode: FileWriteMode::CreateNew,
                        payload: FileWritePayload::Directory,
                        cancel_requested: Some(cancel_requested.clone()),
                    });
                    wait_file_operation(
                        events,
                        &cancellation,
                        cancel_requested.clone(),
                    )
                    .await
                    .map(|result| result.map(|()| Some(path)))
                }
                NativeFileMutation::Rename {
                    source,
                    destination_parent,
                    new_name,
                }
                | NativeFileMutation::Move {
                    source,
                    destination_parent,
                    new_name,
                } => {
                    let events = access.move_node_events(FileMoveRequest {
                        source,
                        destination_parent,
                        new_name,
                        cancel_requested: Some(cancel_requested.clone()),
                    });
                    wait_file_operation(
                        events,
                        &cancellation,
                        cancel_requested.clone(),
                    )
                    .await
                    .map(|result| result.map(Some))
                }
                NativeFileMutation::Copy {
                    source,
                    destination,
                } => {
                    let events = access.copy_node_events(FileCopyRequest {
                        source,
                        destination,
                        cancel_requested: Some(cancel_requested.clone()),
                    });
                    wait_file_operation(
                        events,
                        &cancellation,
                        cancel_requested.clone(),
                    )
                    .await
                    .map(|result| result.map(Some))
                }
                NativeFileMutation::Transfer {
                    source_workspace,
                    source_workspace_id,
                    source_relative,
                    destination,
                } => {
                    let worker_cancel = cancel_requested.clone();
                    let worker = tokio::task::spawn_blocking(move || {
                        let source_access = craic_system::workspace::file_access_for_configured_workspace(
                            &source_workspace,
                        )?;
                        if source_access.workspace().id.as_str() != source_workspace_id {
                            return Err(
                                "The dragged workspace identity is no longer valid."
                                    .to_string(),
                            );
                        }
                        let source = source_access.root().join_child(source_relative);
                        transfer_file_node(
                            source_access,
                            access,
                            source,
                            destination,
                            worker_cancel,
                        )
                        .map(Some)
                    });
                    match wait_workspace_job(
                        &cancellation,
                        &retired_jobs,
                        "cross-provider-transfer",
                        worker,
                        Some(&cancel_requested),
                    )
                    .await
                    {
                        NativeJobWait::WorkspaceChanged => None,
                        NativeJobWait::TimedOut => Some(Err(
                            "Cross-provider transfer timed out.".to_string(),
                        )),
                        NativeJobWait::Completed(joined) => Some(joined.unwrap_or_else(|error| {
                            Err(format!("Cross-provider transfer task failed: {error}"))
                        })),
                    }
                }
                NativeFileMutation::Upload {
                    sources,
                    destination_parent,
                } => {
                    let worker_cancel = cancel_requested.clone();
                    let worker = tokio::task::spawn_blocking(move || {
                        craic_system::system::transfer::transfer_local_paths(
                            access,
                            sources,
                            destination_parent,
                            worker_cancel,
                        )
                        .map(|paths| paths.into_iter().next())
                    });
                    match wait_workspace_job(
                        &cancellation,
                        &retired_jobs,
                        "workspace-upload",
                        worker,
                        Some(&cancel_requested),
                    )
                    .await
                    {
                        NativeJobWait::WorkspaceChanged => None,
                        NativeJobWait::TimedOut => {
                            Some(Err("Upload task timed out.".to_string()))
                        }
                        NativeJobWait::Completed(joined) => Some(joined.unwrap_or_else(|error| {
                            Err(format!("Upload task failed: {error}"))
                        })),
                    }
                }
                NativeFileMutation::Delete { path } => {
                    let events = access.delete_events(FileDeleteRequest {
                        path,
                        cancel_requested: Some(cancel_requested.clone()),
                    });
                    wait_file_operation(
                        events,
                        &cancellation,
                        cancel_requested.clone(),
                    )
                    .await
                    .map(|result| result.map(|()| None))
                }
            };
            let Some(result) = result else {
                log::debug!("file mutation canceled workspace={workspace_id}");
                return;
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FileMutationFinished {
                        workspace_id,
                        access: completion_access,
                        mutation,
                        allow_sudo_retry,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file mutation completion dropped during shutdown");
            }
        }
        RepositoryRequest::DownloadWorkspaceFile {
            workspace_id,
            access,
            source,
            destination,
            allow_sudo_retry,
            cancellation,
        } => {
            let completion_access = access.clone();
            let completion_source = source.clone();
            let completion_destination = destination.clone();
            let cancel_requested = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel_requested.clone();
            let worker = tokio::task::spawn_blocking(move || {
                access.download_to_local(FileDownloadRequest {
                    sources: vec![source],
                    destination,
                    cancel_requested: Some(worker_cancel),
                })
            });
            let result = match wait_workspace_job(
                &cancellation,
                &retired_jobs,
                "workspace-download",
                worker,
                Some(&cancel_requested),
            )
            .await
            {
                NativeJobWait::WorkspaceChanged => None,
                NativeJobWait::TimedOut => {
                    Some(Err("Download task timed out.".to_string()))
                }
                NativeJobWait::Completed(joined) => Some(joined.unwrap_or_else(|error| {
                    Err(format!("Download task failed: {error}"))
                })),
            };
            let Some(result) = result else {
                log::debug!("Files download canceled workspace={workspace_id}");
                return;
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FileDownloadFinished {
                        workspace_id,
                        access: completion_access,
                        source: completion_source,
                        destination: completion_destination,
                        allow_sudo_retry,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file download completion dropped during shutdown");
            }
        }
        RepositoryRequest::SaveWorkspaceFile {
            workspace_id,
            access,
            path,
            text,
            expected_signature,
            edit_generation,
            allow_sudo_retry,
            cancellation,
        } => {
            let completion_access = access.clone();
            let completion_path = path.clone();
            let completion_text = text.clone();
            let completion_signature = expected_signature.clone();
            let cancel_requested = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel_requested.clone();
            let worker = tokio::task::spawn_blocking(move || {
                let current = access.info(&path)?;
                if file_signature_from_info(&current) != expected_signature {
                    return Err(
                        "The file changed on disk. Your pending editor changes were preserved."
                            .to_string(),
                    );
                }
                craic_system::system::capabilities::files::wait_file_operation(
                    access.write_node_events(FileWriteRequest {
                        path: path.clone(),
                        mode: FileWriteMode::Replace,
                        payload: FileWritePayload::File(text.into_bytes()),
                        cancel_requested: Some(worker_cancel),
                    }),
                    craic_system::system::capabilities::files::FileOperation::Write,
                )
                .map_err(|error| error.to_string())?;
                access.info(&path)
            });
            let result = match wait_workspace_job(
                &cancellation,
                &retired_jobs,
                "workspace-save",
                worker,
                Some(&cancel_requested),
            )
            .await
            {
                NativeJobWait::WorkspaceChanged => None,
                NativeJobWait::TimedOut => {
                    Some(Err("File save task timed out.".to_string()))
                }
                NativeJobWait::Completed(joined) => Some(joined.unwrap_or_else(|error| {
                    Err(format!("File save task failed: {error}"))
                })),
            };
            let Some(result) = result else {
                log::debug!(
                    "native Files text save canceled workspace={workspace_id}"
                );
                return;
            };
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceFileSaved {
                        workspace_id,
                        access: completion_access,
                        path: completion_path,
                        text: completion_text,
                        expected_signature: completion_signature,
                        edit_generation,
                        allow_sudo_retry,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file save completion dropped during shutdown");
            }
        }
        RepositoryRequest::HighlightWorkspaceText {
            workspace_id,
            access,
            path,
            text,
            completion_cursor,
            completion_cursor_utf16,
            edit_generation,
            cancellation,
        } => {
            let completion_path = path.clone();
            let preview_path = path.clone();
            let path_display = path.display();
            let task = tokio::task::spawn_blocking(move || {
                let TextSyntaxAnalysis {
                    syntax,
                    mut diagnostics,
                    fold_ranges,
                    completions: completion,
                } = analyze_text_syntax(&path_display, &text, completion_cursor);
                let language = language_support_for_id(language_id_from_path(&path_display));
                let markdown_lint = if language.lint == LintKind::Markdown {
                    let ignored_rules = workspace_config::markdown_lint_ignored_rules_from_file_access(access.as_ref());
                    craic_language::markdown_lint::check_language_document(
                        language,
                        Some(&path_display),
                        &text,
                        &ignored_rules,
                    )
                } else {
                    Vec::new()
                };
                let spellcheck = craic_language::spellcheck::check_document(
                    language,
                    Some(&path_display),
                    &text,
                    &craic_language::spellcheck::SpellcheckAllowlist::default(),
                );
                diagnostics.extend(markdown_lint.iter().map(|issue| {
                    TextDiagnosticSpan {
                        start: issue.start,
                        end: issue.end,
                        kind: TextDiagnosticKind::Warning,
                    }
                }));
                diagnostics.extend(spellcheck.iter().map(|issue| TextDiagnosticSpan {
                    start: issue.start,
                    end: issue.end,
                    kind: TextDiagnosticKind::Spelling,
                }));
                diagnostics.sort_by_key(|issue| (issue.start, issue.end));
                let extension = Path::new(&path_display)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| extension.to_ascii_lowercase());
                let local_document_path = access.local_path(&preview_path);
                let local_workspace_root = access.local_path(&access.root());
                let web_preview = extension.as_deref().and_then(|extension| {
                    match extension {
                        "md" | "markdown" | "mdown" => {
                            let fragment = craic_ui_preview::markdown_preview_web::markdown_fragment_html(&text);
                            let html = craic_ui_preview::markdown_preview_web::html_document(
                                &fragment,
                                text.len(),
                            );
                            Some(Ok(NativeWebPreview {
                                html: match (&local_document_path, &local_workspace_root) {
                                    (Some(document), Some(root)) => {
                                        inline_local_preview_assets(&html, document, root)
                                    }
                                    _ => html,
                                },
                                mode: NativeWebPreviewMode::BesideEditor,
                            }))
                        }
                        "html" | "htm" | "xhtml" | "svg" => {
                            Some(Ok(NativeWebPreview {
                                html: match (&local_document_path, &local_workspace_root) {
                                    (Some(document), Some(root)) => {
                                        inline_local_preview_assets(&text, document, root)
                                    }
                                    _ => text.clone(),
                                },
                                mode: NativeWebPreviewMode::BesideEditor,
                            }))
                        }
                        "ipynb" => Some(
                            craic_ui_preview::notebook_preview_web::html_document(&text)
                                .map(|html| NativeWebPreview {
                                    html,
                                    mode: NativeWebPreviewMode::FullPane,
                                }),
                        ),
                        _ => None,
                    }
                });
                let csv_table = (extension.as_deref() == Some("csv"))
                    .then(|| parse_csv_table(&text));
                NativeTextAnalysis {
                    syntax,
                    diagnostics,
                    fold_ranges,
                    markdown_lint,
                    completion,
                    completion_cursor_utf16,
                    web_preview,
                    csv_table,
                }
            });
            let Some(result) = until_workspace_change(
                &cancellation,
                &retired_jobs,
                "text-analysis",
                task,
            )
            .await
            else {
                log::debug!(
                    "native Files syntax canceled workspace={workspace_id} path={}",
                    path.display()
                );
                return;
            };
            let result = result
                .map_err(|error| format!("Syntax highlight task failed: {error}"));
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::WorkspaceTextHighlighted {
                        workspace_id,
                        path: completion_path,
                        edit_generation,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file syntax completion dropped during shutdown");
            }
        }
        RepositoryRequest::AuthorizeFileSudo {
            workspace_id,
            access,
            password,
            retry,
        } => {
            let authorization_access = access.clone();
            let result = tokio::task::spawn_blocking(move || {
                authorization_access.sudo_access(password)
            })
            .await
            .unwrap_or_else(|error| {
                Err(FileSudoError::new(
                    FileSudoErrorKind::Unavailable,
                    format!("Sudo authorization task failed: {error}"),
                ))
            });
            if repository_completion_tx
                .send(FrontendCompletion::Repository(
                    RepositoryCompletion::FileSudoAuthorized {
                        workspace_id,
                        access,
                        retry,
                        result,
                    },
                ))
                .is_err()
            {
                log::warn!("file sudo authorization completion dropped during shutdown");
            }
        }
        _ => unreachable!("repository request routed to the wrong handler"),
    }
}
