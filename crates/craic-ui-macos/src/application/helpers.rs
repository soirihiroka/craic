fn native_create_workspace_request(
    root: PathBuf,
    name: &str,
    remote: &str,
) -> Result<NativeCreateWorkspaceRequest, String> {
    let remote = remote.trim();
    let remote = (!remote.is_empty()).then(|| remote.to_string());
    let name = if name.trim().is_empty() {
        remote
            .as_deref()
            .and_then(native_workspace_name_from_remote)
            .unwrap_or_default()
    } else {
        name.trim().to_string()
    };
    let name = native_validated_workspace_name(&name)?;
    Ok(NativeCreateWorkspaceRequest { root, name, remote })
}

fn native_workspace_name_from_remote(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/');
    if remote.is_empty() {
        return None;
    }
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let name = remote.rsplit(['/', ':']).next().unwrap_or(remote);
    native_validated_workspace_name(name).ok()
}

fn native_validated_workspace_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Repository name is required.".to_string());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("Repository name must be a single folder name.".to_string());
    }
    Ok(name.to_string())
}

fn create_native_workspace(
    request: NativeCreateWorkspaceRequest,
) -> Result<(PathBuf, String), String> {
    if !request.root.is_dir() {
        return Err(format!(
            "Workspace root is not a directory: {}",
            request.root.display()
        ));
    }
    let destination = request.root.join(&request.name);
    if destination.exists() {
        if !destination.is_dir() {
            return Err(format!(
                "Destination already exists and is not a folder: {}",
                destination.display()
            ));
        }
        let mut entries = std::fs::read_dir(&destination).map_err(|error| {
            format!(
                "Could not inspect destination folder {}: {error}",
                destination.display()
            )
        })?;
        if entries.next().is_some() {
            return Err(format!(
                "Destination folder is not empty: {}",
                destination.display()
            ));
        }
    }

    if let Some(remote) = request.remote {
        let provider = LocalProvider::new();
        let workspace = LocalProvider::workspace_for_path(&request.root);
        let shell = provider
            .shell(&workspace)
            .ok_or_else(|| "Local shell access is unavailable.".to_string())?;
        let message = craic_vcs::git::clone_repository_with_shell(
            shell,
            workspace.root,
            &remote,
            &request.name,
        )?;
        return Ok((destination, message));
    }

    log::info!(
        "native workspace folder create start path={}",
        destination.display()
    );
    std::fs::create_dir_all(&destination).map_err(|error| {
        format!(
            "Could not create workspace folder {}: {error}",
            destination.display()
        )
    })?;
    log::info!(
        "native workspace folder create complete path={}",
        destination.display()
    );
    Ok((destination, "Workspace created.".to_string()))
}

fn load_workspace_snapshot(
    workspace: &craic_config::ConfiguredWorkspace,
) -> Result<(Arc<GitRepoHandle>, WorkspaceSnapshot), String> {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    let workspace_ref = access.workspace;
    let files = access
        .provider
        .files(&workspace_ref)
        .ok_or_else(|| "File access is unavailable for this workspace.".to_string())?;
    let shell = access
        .provider
        .shell(&workspace_ref)
        .ok_or_else(|| "Shell access is unavailable for this workspace.".to_string())?;
    let terminal_links = access
        .provider
        .terminal_links(&workspace_ref)
        .ok_or_else(|| "File-link navigation is unavailable for this workspace.".to_string())?;
    let handle = Arc::new(
        GitRepoHandle::new(workspace_ref, shell, files).with_terminal_links(terminal_links),
    );
    let snapshot = handle.load_workspace_snapshot()?;
    Ok((handle, snapshot))
}

fn workspace_metadata_handle(
    workspace: &craic_config::ConfiguredWorkspace,
) -> Result<(GitRepoHandle, Option<Arc<dyn GitHubAccess>>), String> {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    let workspace_ref = access.workspace;
    let files = access
        .provider
        .files(&workspace_ref)
        .ok_or_else(|| "File access is unavailable for this workspace.".to_string())?;
    let shell = access
        .provider
        .shell(&workspace_ref)
        .ok_or_else(|| "Shell access is unavailable for this workspace.".to_string())?;
    let github_access = github_access_for_provider(access.provider.as_ref(), &workspace_ref);
    let account = workspace_config::git_config_from_file_access(files.as_ref()).github_auth_account;
    let mut handle = GitRepoHandle::new(workspace_ref.clone(), shell.clone(), files);
    if let Some(hook) = github::git_auth_hook(shell, workspace_ref.root.clone(), account) {
        handle = handle.with_hook(hook);
    }
    Ok((handle, github_access))
}

async fn load_native_workspace_metadata(
    entry: WorkspaceEntry,
) -> Result<NativeWorkspaceMetadata, String> {
    let workspace = entry.workspace;
    let host = match &workspace.provider {
        craic_config::WorkspaceProvider::Ssh { host } => Some(host.clone()),
        craic_config::WorkspaceProvider::Local => None,
    };
    let metadata_workspace = workspace.clone();
    let (handle, github_access) =
        tokio::task::spawn_blocking(move || workspace_metadata_handle(&metadata_workspace))
            .await
            .map_err(|error| {
                format!("Workspace metadata setup task did not complete: {error}")
            })??;
    let WorkspaceRepositoryMetadata { kind, remote_url } =
        handle.workspace_metadata_async(github_access).await?;
    Ok(NativeWorkspaceMetadata {
        kind,
        remote_label: remote_url
            .as_deref()
            .and_then(|remote| native_remote_workspace_label(remote, host.as_deref())),
    })
}

async fn load_native_workspace_github_accounts(
    workspace: craic_config::ConfiguredWorkspace,
) -> Result<Vec<GitHubAuthAccount>, String> {
    let (_, github_access) =
        tokio::task::spawn_blocking(move || workspace_metadata_handle(&workspace))
            .await
            .map_err(|error| format!("GitHub account setup task did not complete: {error}"))??;
    let github_access = github_access
        .ok_or_else(|| "GitHub CLI access is unavailable for this workspace.".to_string())?;
    tokio::task::spawn_blocking(move || github_access.authenticated_accounts())
        .await
        .map_err(|error| format!("GitHub account loading task did not complete: {error}"))?
}

fn native_remote_workspace_label(remote_url: &str, workspace_host: Option<&str>) -> Option<String> {
    let slug = github::parse_github_url(remote_url)
        .or_else(|| craic_vcs::gitlab::parse_gitlab_url(remote_url))
        .or_else(|| craic_vcs::bitbucket::parse_bitbucket_url(remote_url))
        .or_else(|| native_generic_remote_slug(remote_url))?;
    let host = workspace_host
        .map(str::trim)
        .and_then(|host| {
            host.rsplit_once('@')
                .map_or(Some(host), |(_, host)| Some(host))
        })
        .map(|host| host.trim_matches('/'))
        .filter(|host| !host.is_empty() && !native_workspace_host_is_local(host));
    Some(host.map_or(slug.clone(), |host| format!("{slug}@{host}")))
}

fn native_workspace_host_is_local(host: &str) -> bool {
    let normalized = host
        .trim()
        .trim_matches('/')
        .trim_matches(|character| character == '[' || character == ']')
        .to_ascii_lowercase();
    if matches!(normalized.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return true;
    }
    let without_port = normalized
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|character| character.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(normalized.as_str());
    matches!(without_port, "localhost" | "127.0.0.1")
}

fn native_generic_remote_slug(remote_url: &str) -> Option<String> {
    let remote_url = remote_url.trim();
    if remote_url.is_empty() {
        return None;
    }
    let path = if let Some((_, tail)) = remote_url.split_once("://") {
        tail.split_once('/').map(|(_, path)| path)?
    } else if let Some((_, path)) = remote_url.split_once(':') {
        path
    } else {
        remote_url
    };
    let path = path.trim().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (parts.len() >= 2).then(|| format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]))
}

fn native_workspace_metadata_symbol(kind: RepoMetadata) -> &'static str {
    match kind {
        RepoMetadata::Fork => "arrow.triangle.branch",
        RepoMetadata::Private => "lock.fill",
        RepoMetadata::Public => "globe",
        RepoMetadata::Local => "folder.badge.gearshape",
        RepoMetadata::Unknown => "questionmark.folder",
        RepoMetadata::Folder => "folder",
    }
}

fn native_workspace_metadata_description(kind: RepoMetadata) -> &'static str {
    match kind {
        RepoMetadata::Fork => "Forked repository",
        RepoMetadata::Private => "Private repository",
        RepoMetadata::Public => "Public repository",
        RepoMetadata::Local => "Local Git repository",
        RepoMetadata::Unknown => "Repository",
        RepoMetadata::Folder => "Folder",
    }
}

fn docker_access_for_workspace(
    workspace: &craic_config::ConfiguredWorkspace,
) -> Result<Arc<dyn DockerAccess>, String> {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    access
        .provider
        .docker(&access.workspace)
        .ok_or_else(|| "Docker is unavailable for this workspace.".to_string())
}

fn native_remote_action_pulls(action: NativeRemoteAction, snapshot: &RepositorySnapshot) -> bool {
    match action {
        NativeRemoteAction::Pull => true,
        NativeRemoteAction::Push => false,
        NativeRemoteAction::Contextual => snapshot.has_upstream && snapshot.behind > 0,
    }
}

fn local_changes_overwritten_body(
    action: NativeRemoteAction,
    snapshot: &RepositorySnapshot,
    files: &[String],
) -> String {
    let pull_before_push = matches!(action, NativeRemoteAction::Contextual)
        && snapshot.has_upstream
        && snapshot.behind > 0
        && snapshot.ahead > 0;
    craic_vcs::git::local_changes_overwritten_body(files, pull_before_push)
}

fn git_action_events(
    handle: &GitRepoHandle,
    snapshot: &RepositorySnapshot,
    action: NativeRemoteAction,
) -> Result<GitCommandGenerator, String> {
    match action {
        NativeRemoteAction::Pull => return Ok(handle.pull_with_progress()),
        NativeRemoteAction::Push => return Ok(handle.push_with_progress()),
        NativeRemoteAction::Contextual => {}
    }
    if !snapshot.has_upstream {
        let remote = snapshot
            .remote_name
            .as_deref()
            .ok_or_else(|| "Publishing requires a configured Git remote.".to_string())?;
        if snapshot.branch.is_empty() {
            return Err("Repository is not initialized.".to_string());
        }
        return Ok(handle.publish_with_progress(remote, &snapshot.branch));
    }
    if snapshot.behind > 0 && snapshot.ahead > 0 {
        return Ok(handle.pull_push_with_progress());
    }
    if snapshot.behind > 0 {
        return Ok(handle.pull_with_progress());
    }
    if snapshot.ahead > 0 {
        return Ok(handle.push_with_progress());
    }
    Ok(handle.fetch_with_progress(snapshot.remote_name.as_deref()))
}

fn changed_file_matches_query(path: &str, status: &str, query: &str) -> bool {
    query.is_empty() || path.to_lowercase().contains(query) || status.to_lowercase().contains(query)
}

fn native_quick_action_symbol(target: &RunItem) -> &'static str {
    match target.command {
        RunCommand::MakeTarget { .. } => "hammer",
        RunCommand::BunScript { .. } => "play.rectangle",
        RunCommand::ShellCommand { .. } => "terminal",
    }
}

fn duplicate_file_name(name: &str, directory: bool) -> String {
    if directory {
        return format!("{name} copy");
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            format!("{stem} copy.{extension}")
        }
        _ => format!("{name} copy"),
    }
}

fn file_signature_from_info(info: &FileNodeInfo) -> FileSignature {
    FileSignature {
        kind: info.kind,
        len: info.len_or_zero(),
        modified: info.modified,
    }
}

fn workspace_file_drag_type() -> Retained<NSString> {
    NSString::from_str("dev.craic.workspace-file-path")
}

struct NativeWorkspaceFileDrag {
    workspace_selection_id: String,
    workspace_id: String,
    relative: String,
}

fn workspace_file_drag_payload(
    workspace: &craic_config::ConfiguredWorkspace,
    access: &dyn FileAccess,
    path: &FileNodePath,
) -> Option<String> {
    let relative = path.native_relative()?;
    if relative.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({
        "version": 2,
        "workspace_selection_id": workspace.selection_id(),
        "workspace_id": access.workspace().id.as_str(),
        "relative": relative,
    }))
    .ok()
}

fn workspace_file_clipboard_type() -> Retained<NSString> {
    NSString::from_str("dev.craic.workspace-file-clipboard")
}

fn workspace_file_clipboard_from_pasteboard(
    pasteboard: &NSPasteboard,
    access: &dyn FileAccess,
) -> Option<(FileNodePath, bool)> {
    let payload = pasteboard
        .stringForType(&workspace_file_clipboard_type())?
        .to_string();
    let mut parts = payload.splitn(3, '\n');
    if parts.next()? != access.workspace().id.as_str() {
        return None;
    }
    let move_item = match parts.next()? {
        "copy" => false,
        "move" => true,
        _ => return None,
    };
    let relative = parts.next()?.trim().trim_start_matches('/');
    (!relative.is_empty()).then(|| (access.root().join_child(relative), move_item))
}

fn workspace_file_drag_source(
    info: &ProtocolObject<dyn NSDraggingInfo>,
) -> Option<NativeWorkspaceFileDrag> {
    let drag_type = workspace_file_drag_type();
    let payload = info
        .draggingPasteboard()
        .stringForType(&drag_type)?
        .to_string();
    let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
    if value.get("version")?.as_u64()? != 2 {
        return None;
    }
    let workspace_selection_id = value.get("workspace_selection_id")?.as_str()?;
    let workspace_id = value.get("workspace_id")?.as_str()?;
    let relative = value.get("relative")?.as_str()?;
    if workspace_selection_id.is_empty() || workspace_id.is_empty() || relative.is_empty() {
        return None;
    }
    Some(NativeWorkspaceFileDrag {
        workspace_selection_id: workspace_selection_id.to_string(),
        workspace_id: workspace_id.to_string(),
        relative: relative.to_string(),
    })
}

fn current_drag_requests_copy() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Option))
}

fn local_file_paths_from_drag(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let classes = NSArray::from_slice(&[NSURL::class()]);
    let Some(objects) = (unsafe {
        info.draggingPasteboard()
            .readObjectsForClasses_options(&classes, None)
    }) else {
        return Vec::new();
    };
    objects
        .iter()
        .filter_map(|object| object.downcast::<NSURL>().ok())
        .filter(|url| url.isFileURL())
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

fn load_native_file_tree(
    handle: &GitRepoHandle,
    expanded: &HashSet<FileNodePath>,
) -> Result<Vec<NativeFileRow>, String> {
    let files = handle.workspace_files();
    let root = files.root();
    let mut rows = Vec::new();
    append_native_file_children(files.as_ref(), &root, 0, expanded, &mut rows)?;
    Ok(rows)
}

fn append_native_file_children(
    files: &dyn FileAccess,
    parent: &FileNodePath,
    depth: usize,
    expanded: &HashSet<FileNodePath>,
    rows: &mut Vec<NativeFileRow>,
) -> Result<(), String> {
    if rows.len() >= FILE_TREE_ROW_LIMIT {
        return Ok(());
    }
    let listing = files
        .list_dirs(std::slice::from_ref(parent))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            format!(
                "No directory listing was returned for {}.",
                parent.display()
            )
        })?;
    let mut infos = files.info_many(&listing.entries)?;
    infos.retain(|info| info.display_name != ".git");
    infos.sort_by(|left, right| {
        right
            .capabilities
            .listable
            .cmp(&left.capabilities.listable)
            .then_with(|| {
                left.display_name
                    .to_lowercase()
                    .cmp(&right.display_name.to_lowercase())
            })
    });
    for info in infos {
        if rows.len() >= FILE_TREE_ROW_LIMIT {
            break;
        }
        let should_expand = info.capabilities.listable && expanded.contains(&info.path);
        let path = info.path.clone();
        rows.push(NativeFileRow { info, depth });
        if should_expand {
            append_native_file_children(files, &path, depth + 1, expanded, rows)?;
        }
    }
    Ok(())
}

fn ns_color_from_hex(value: &str) -> Option<Retained<NSColor>> {
    let hex = value.trim().strip_prefix('#')?;
    let (red, green, blue, alpha) = match hex.len() {
        3 | 4 => {
            let mut digits = hex.chars().map(|digit| digit.to_digit(16));
            let red = digits.next()?? as u8 * 17;
            let green = digits.next()?? as u8 * 17;
            let blue = digits.next()?? as u8 * 17;
            let alpha = if hex.len() == 4 {
                digits.next()?? as u8 * 17
            } else {
                255
            };
            (red, green, blue, alpha)
        }
        6 | 8 => {
            let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let alpha = if hex.len() == 8 {
                u8::from_str_radix(&hex[6..8], 16).ok()?
            } else {
                255
            };
            (red, green, blue, alpha)
        }
        _ => return None,
    };
    Some(NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        f64::from(alpha) / 255.0,
    ))
}

fn is_image_preview_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic"
            )
        })
}

fn is_changed_binary_preview_path(path: &str) -> bool {
    is_image_preview_path(path)
        || is_pdf_preview_path(path)
        || is_font_preview_path(path)
        || media_preview_mime(path).is_some()
}

fn is_preview_limit_message(message: &str) -> bool {
    message.contains("too large to preview")
        || message.contains("cannot be previewed as text")
        || message.contains("exceeds the preview limit")
}

fn is_pdf_preview_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn is_font_preview_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "otf" | "otc" | "ttf" | "ttc" | "woff" | "woff2"
            )
        })
}

fn is_sqlite_preview_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "db" | "db3" | "s3db" | "sqlite" | "sqlite3"
            )
        })
}

fn has_sqlite_header(bytes: &[u8]) -> bool {
    bytes.starts_with(b"SQLite format 3\0")
}

fn media_preview_mime(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "oga" | "ogg" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "m4v" => Some("video/x-m4v"),
        "mov" => Some("video/quicktime"),
        "mp4" => Some("video/mp4"),
        "ogv" => Some("video/ogg"),
        "webm" => Some("video/webm"),
        _ => None,
    }
}

fn inline_local_preview_assets(html: &str, document_path: &Path, workspace_root: &Path) -> String {
    static RESOURCE_ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    let resource_attribute = RESOURCE_ATTRIBUTE.get_or_init(|| {
        Regex::new(r#"(?i)(\b(?:src|poster)\s*=\s*)(["'])([^"']+)["']"#)
            .expect("preview resource attribute regex is valid")
    });
    let Some(document_parent) = document_path.parent() else {
        return html.to_string();
    };
    let Ok(workspace_root) = workspace_root.canonicalize() else {
        return html.to_string();
    };
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;

    for captures in resource_attribute.captures_iter(html) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let source = captures
            .get(3)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let path_only = source.split(['?', '#']).next().unwrap_or_default();
        if path_only.is_empty()
            || path_only.starts_with('/')
            || path_only.starts_with("//")
            || path_only.contains("://")
            || path_only.starts_with("data:")
        {
            continue;
        }
        let candidate = document_parent.join(path_only);
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if !candidate.starts_with(&workspace_root) {
            log::warn!(
                "native Markdown preview rejected relative asset outside workspace path={}",
                candidate.display()
            );
            continue;
        }
        let Some(mime) = preview_asset_mime(&candidate) else {
            continue;
        };
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if metadata.len() > FILE_CONTENT_PREVIEW_LIMIT {
            log::warn!(
                "native Markdown preview skipped oversized asset path={} bytes={}",
                candidate.display(),
                metadata.len()
            );
            continue;
        }
        let Ok(bytes) = std::fs::read(&candidate) else {
            continue;
        };
        log::debug!(
            "native Markdown preview embedded local asset path={} bytes={} mime={mime}",
            candidate.display(),
            bytes.len()
        );
        let data_url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        output.push_str(&html[cursor..whole.start()]);
        output.push_str(
            captures
                .get(1)
                .map(|value| value.as_str())
                .unwrap_or("src="),
        );
        let quote = captures.get(2).map(|value| value.as_str()).unwrap_or("\"");
        output.push_str(quote);
        output.push_str(&data_url);
        output.push_str(quote);
        cursor = whole.end();
    }
    output.push_str(&html[cursor..]);
    output
}

fn preview_asset_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "svg" => Some("image/svg+xml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        "heic" => Some("image/heic"),
        _ => media_preview_mime(path.to_string_lossy().as_ref()),
    }
}

fn submit_ui_effect_completion(handle: Option<&AppHandle>, id: UiEffectId, result: UiEffectResult) {
    let Some(handle) = handle else {
        log::warn!("native UI effect completion dropped because app-core is unavailable");
        return;
    };
    if let Err(command) = handle.try_send(AppCommand::CompleteUiEffect(UiEffectCompletion {
        id,
        result,
    })) {
        log::warn!("native UI effect completion queue rejected command={command:?}");
    }
}

fn changed_file_symbol(status: &str) -> (&'static str, &'static str) {
    if status.contains('D') {
        ("minus", "Deleted")
    } else if status.contains('A') || status.contains('?') {
        ("plus", "Added")
    } else if status.contains('R') {
        ("arrow.right", "Renamed")
    } else if status.contains('U') {
        ("exclamationmark.triangle", "Conflict")
    } else {
        ("pencil", "Modified")
    }
}

fn diff_document(comparison: &FileComparison) -> DiffDocument {
    DiffDocument {
        rows: comparison
            .rows
            .iter()
            .map(|row| DiffRow {
                left_number: row.left_number,
                right_number: row.right_number,
                left_text: row.left_text.clone(),
                right_text: row.right_text.clone(),
                left_kind: diff_row_kind(row.left_kind),
                right_kind: diff_row_kind(row.right_kind),
            })
            .collect(),
    }
}

fn diff_row_kind(kind: craic_vcs::git::DiffKind) -> DiffRowKind {
    match kind {
        craic_vcs::git::DiffKind::Context => DiffRowKind::Context,
        craic_vcs::git::DiffKind::Deleted => DiffRowKind::Deleted,
        craic_vcs::git::DiffKind::Added => DiffRowKind::Added,
        craic_vcs::git::DiffKind::Fold => DiffRowKind::Fold,
    }
}

fn populate_agent_selector(
    popup: &NSPopUpButton,
    options: &[NativeAgentSelectorOption],
    selected: Option<&str>,
    empty_label: &str,
) {
    popup.removeAllItems();
    if options.is_empty() {
        popup.addItemWithTitle(&NSString::from_str(empty_label));
        return;
    }
    for option in options {
        popup.addItemWithTitle(&NSString::from_str(&option.label));
    }
    let selected_index = selected
        .and_then(|selected| options.iter().position(|option| option.id == selected))
        .unwrap_or(0);
    popup.selectItemAtIndex(selected_index as isize);
}

fn load_native_string_default(key: &str) -> Option<String> {
    NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str(key))
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
}

fn save_native_string_default(key: &str, value: &str) {
    let key = NSString::from_str(key);
    let value = NSString::from_str(value);
    unsafe {
        NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&value), &key);
    }
}

fn save_native_optional_default(key: &str, value: Option<&str>) {
    if let Some(value) = value {
        save_native_string_default(key, value);
    } else {
        NSUserDefaults::standardUserDefaults().removeObjectForKey(&NSString::from_str(key));
    }
}

fn history_avatar_key(email: &str) -> Option<String> {
    let email = email.trim();
    (!email.is_empty()).then(|| format!("email:{}", email.to_ascii_lowercase()))
}

fn append_native_agent_link_text(
    rendered: &mut String,
    ranges: &mut Vec<(NSRange, String)>,
    text: &str,
) {
    let base = rendered.encode_utf16().count();
    for (start, end, target) in detected_links(text) {
        let location = base + text[..start].encode_utf16().count();
        let length = text[start..end].encode_utf16().count();
        let value = match target {
            LinkTarget::Url(url) => url,
            LinkTarget::File { path, line, column } => match (line, column) {
                (Some(line), Some(column)) => format!("{path}:{line}:{column}"),
                (Some(line), None) => format!("{path}:{line}"),
                _ => path,
            },
        };
        ranges.push((NSRange::new(location, length), value));
    }
    rendered.push_str(text);
}

fn native_agent_attributed_text(
    text: &str,
    monospace: bool,
    font_size: f64,
) -> Retained<NSMutableAttributedString> {
    let mut rendered = String::new();
    let mut links = Vec::new();
    append_native_agent_link_text(&mut rendered, &mut links, text);
    let attributed = NSMutableAttributedString::from_nsstring(&NSString::from_str(&rendered));
    let range = NSRange::new(0, rendered.encode_utf16().count());
    if range.length != 0 {
        let font = if monospace {
            let monospace_size = (font_size - 1.5).max(craic_config::MIN_FONT_SIZE);
            NSFont::userFixedPitchFontOfSize(monospace_size)
                .unwrap_or_else(|| NSFont::systemFontOfSize(monospace_size))
        } else {
            NSFont::systemFontOfSize(font_size)
        };
        let color = if monospace {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::labelColor()
        };
        unsafe {
            attributed.addAttribute_value_range(NSFontAttributeName, &font, range);
            attributed.addAttribute_value_range(NSForegroundColorAttributeName, &color, range);
        }
        for (range, value) in links {
            let value = NSString::from_str(&value);
            unsafe { attributed.addAttribute_value_range(NSLinkAttributeName, &value, range) };
        }
    }
    attributed
}

fn native_agent_transcript_natural_text_height(text: &str, width: f64, font_size: f64) -> f64 {
    let character_width = (font_size * 7.0 / 13.0).max(4.0);
    let characters_per_line = ((width - 8.0).max(96.0) / character_width).floor() as usize;
    let lines = text
        .lines()
        .map(|line| line.chars().count().max(1).div_ceil(characters_per_line))
        .sum::<usize>()
        .max(1);
    lines as f64 * (font_size + 4.0) + 8.0
}

fn native_agent_transcript_section_height(
    text: &str,
    width: f64,
    font_size: f64,
    maximum: f64,
) -> f64 {
    native_agent_transcript_natural_text_height(text, width, font_size)
        .min(maximum)
        .max(25.0)
}

fn native_agent_transcript_is_compact(kind: NativeAgentTranscriptKind) -> bool {
    !matches!(
        kind,
        NativeAgentTranscriptKind::User
            | NativeAgentTranscriptKind::Assistant
            | NativeAgentTranscriptKind::Developer
    )
}

fn native_agent_transcript_row_height(
    item: &NativeAgentTranscriptItem,
    width: f64,
    font_size: f64,
) -> f64 {
    let content_width = (width.max(320.0) - 52.0).max(1.0);
    let mut card_height = 42.0;
    if !item.body.trim().is_empty() {
        card_height += native_agent_transcript_section_height(
            item.body.trim_end(),
            content_width,
            font_size,
            if native_agent_transcript_is_compact(item.kind) {
                180.0
            } else {
                300.0
            },
        ) + 8.0;
    }
    if item.image.is_some() {
        card_height += 208.0;
    }
    if let Some(detail) = item
        .detail
        .as_deref()
        .filter(|detail| !detail.trim().is_empty())
    {
        card_height +=
            20.0 + native_agent_transcript_section_height(detail, content_width, font_size, 220.0);
    }
    card_height.max(52.0) + 12.0
}

fn is_native_agent_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "mp3" | "m4a" | "ogg" | "flac" | "aac" | "aiff" | "caf"
            )
        })
}

fn agent_image_node_path(
    access: &dyn FileAccess,
    source_path: &str,
) -> Result<FileNodePath, String> {
    let source_path = source_path
        .strip_prefix("file://")
        .unwrap_or(source_path)
        .trim();
    if source_path.is_empty() {
        return Err("Codex returned an empty image path.".to_string());
    }
    let workspace = access.workspace();
    let root = workspace.root.absolute.trim_end_matches('/');
    let relative = if source_path == root {
        ""
    } else if let Some(relative) = source_path
        .strip_prefix(root)
        .and_then(|relative| relative.strip_prefix('/'))
    {
        relative
    } else if Path::new(source_path).is_absolute() {
        return Err("Codex image is outside the active workspace.".to_string());
    } else {
        source_path.trim_start_matches("./")
    };
    if relative
        .split('/')
        .any(|component| component == ".." || component.is_empty() && !relative.is_empty())
    {
        return Err("Codex image path is not a valid workspace-relative path.".to_string());
    }
    Ok(access.root().join_child(relative))
}

fn decode_agent_image_data_uri(data_uri: &str) -> Result<Vec<u8>, String> {
    let Some((metadata, encoded)) = data_uri.split_once(',') else {
        return Err("Codex returned an invalid inline image.".to_string());
    };
    if !metadata.starts_with("data:image/") || !metadata.ends_with(";base64") {
        return Err("Codex returned an unsupported inline image encoding.".to_string());
    }
    let encoded_limit = AGENT_IMAGE_PREVIEW_LIMIT
        .saturating_mul(4)
        .div_ceil(3)
        .saturating_add(8);
    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > encoded_limit {
        return Err(format!(
            "Codex image exceeds the {} MiB preview limit.",
            AGENT_IMAGE_PREVIEW_LIMIT / (1024 * 1024)
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("Codex inline image is not valid base64: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > AGENT_IMAGE_PREVIEW_LIMIT {
        return Err(format!(
            "Codex image exceeds the {} MiB preview limit.",
            AGENT_IMAGE_PREVIEW_LIMIT / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

fn launch_native_workspace_location(
    provider_id: &str,
    workspace_path: &str,
    selected_path: &str,
    line: Option<usize>,
    column: Option<usize>,
) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the Craic executable: {error}"))?;
    let mut command = Command::new(&executable);
    command
        .arg("--workspace-provider")
        .arg(provider_id)
        .arg("--workspace-path")
        .arg(workspace_path)
        .arg("--open-path")
        .arg(selected_path);
    if let Some(line) = line {
        command.arg("--line").arg(line.to_string());
    }
    if let Some(column) = column {
        command.arg("--column").arg(column.to_string());
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch a new Craic window: {error}"))
}

fn resolve_local_workspace_arg(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Could not resolve current directory: {error}"))?
            .join(path)
    };
    let metadata = std::fs::metadata(&absolute).map_err(|error| {
        format!(
            "Workspace path does not exist or cannot be read: {} ({error})",
            path.display()
        )
    })?;
    let workspace = if metadata.is_dir() {
        absolute
    } else if metadata.is_file() {
        absolute
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("File has no parent directory: {}", path.display()))?
    } else {
        return Err(format!(
            "Workspace path must be a file or directory: {}",
            path.display()
        ));
    };
    Ok(workspace.canonicalize().unwrap_or(workspace))
}

fn upload_native_terminal_remote_images(
    context: &NativeTerminalRemoteMedia,
    sources: Vec<PathBuf>,
) -> Result<Vec<RemoteMedia>, String> {
    let mut uploaded = Vec::with_capacity(sources.len());
    for source in sources {
        if context.cancellation.is_cancelled() {
            remote_media::remove_bounded(
                context.shell.clone(),
                context.working_dir.clone(),
                uploaded,
            );
            return Err("Remote image upload was cancelled.".to_string());
        }
        match remote_media::materialize_cancellable(
            context.shell.clone(),
            context.working_dir.clone(),
            source,
            RemoteMediaKind::Image,
            || context.cancellation.is_cancelled(),
        ) {
            Ok(media) => uploaded.push(media),
            Err(error) => {
                remote_media::remove_bounded(
                    context.shell.clone(),
                    context.working_dir.clone(),
                    uploaded,
                );
                return Err(error);
            }
        }
        if context.cancellation.is_cancelled() {
            remote_media::remove_bounded(
                context.shell.clone(),
                context.working_dir.clone(),
                uploaded,
            );
            return Err("Remote image upload was cancelled.".to_string());
        }
    }
    Ok(uploaded)
}

#[derive(Default)]
struct NativeStartupOptions {
    workspace: Option<craic_config::ConfiguredWorkspace>,
    open_path: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
}

fn startup_options() -> Result<NativeStartupOptions, String> {
    let args = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(NativeStartupOptions::default());
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--workspace-provider")
    {
        let mut provider = None;
        let mut workspace_path = None;
        let mut open_path = None;
        let mut line = None;
        let mut column = None;
        let mut index = 0;
        while index < args.len() {
            let flag = args[index]
                .to_str()
                .ok_or_else(|| "Startup option names must be valid UTF-8.".to_string())?;
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("Missing value for {flag}."))?
                .to_str()
                .ok_or_else(|| format!("{flag} must be valid UTF-8."))?;
            index += 1;
            match flag {
                "--workspace-provider" => provider = Some(value.to_string()),
                "--workspace-path" => workspace_path = Some(value.to_string()),
                "--open-path" => open_path = Some(value.to_string()),
                "--line" => {
                    line =
                        Some(value.parse::<usize>().map_err(|_| {
                            format!("--line must be a positive integer, got {value}.")
                        })?)
                }
                "--column" => {
                    column = Some(value.parse::<usize>().map_err(|_| {
                        format!("--column must be a positive integer, got {value}.")
                    })?)
                }
                _ => return Err(format!("Unknown startup option: {flag}.")),
            }
        }
        let provider = provider.ok_or_else(|| "Missing --workspace-provider.".to_string())?;
        let workspace_path =
            workspace_path.ok_or_else(|| "Missing --workspace-path.".to_string())?;
        if line == Some(0) {
            return Err("--line must be greater than zero.".to_string());
        }
        if column == Some(0) {
            return Err("--column must be greater than zero.".to_string());
        }
        if column.is_some() && line.is_none() {
            return Err("--column requires --line.".to_string());
        }
        if line.is_some() && open_path.is_none() {
            return Err("--line requires --open-path.".to_string());
        }
        let provider = match provider.as_str() {
            "local" => craic_config::WorkspaceProvider::Local,
            value => {
                let host = value
                    .strip_prefix("ssh:")
                    .filter(|host| !host.is_empty())
                    .ok_or_else(|| format!("Unsupported workspace provider: {value}."))?;
                craic_config::WorkspaceProvider::Ssh {
                    host: host.to_string(),
                }
            }
        };
        let workspace_path = if provider == craic_config::WorkspaceProvider::Local {
            resolve_local_workspace_arg(Path::new(&workspace_path))?
                .to_string_lossy()
                .into_owned()
        } else {
            workspace_path
        };
        return Ok(NativeStartupOptions {
            workspace: Some(craic_config::ConfiguredWorkspace {
                path: workspace_path,
                provider,
                display_name: None,
                color: None,
            }),
            open_path,
            line,
            column,
        });
    }
    if args.len() > 1 {
        return Err("Expected at most one workspace path.".to_string());
    }

    let path = Path::new(&args[0]);
    let workspace = resolve_local_workspace_arg(path)?;
    log::info!(
        "native startup workspace argument resolved path={}",
        workspace.display()
    );
    Ok(NativeStartupOptions {
        workspace: Some(craic_config::ConfiguredWorkspace::local(
            workspace.to_string_lossy().into_owned(),
        )),
        ..NativeStartupOptions::default()
    })
}
