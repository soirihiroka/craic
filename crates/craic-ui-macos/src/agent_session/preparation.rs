fn prepare(
    workspace: &craic_config::ConfiguredWorkspace,
) -> Result<
    (
        craic_codex_app_server::AppServerConfig,
        String,
        Option<RemoteMediaContext>,
    ),
    String,
> {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    let workspace_ref = access.workspace;
    let shell = access
        .provider
        .shell(&workspace_ref)
        .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
    let provider_kind = access.provider.kind();
    let root = workspace_ref.root.absolute.clone();
    let config = craic_agent::app_server::config(shell.as_ref(), &workspace_ref, provider_kind)?;
    let remote_media = (provider_kind != ProviderKind::Local).then(|| RemoteMediaContext {
        shell,
        working_dir: workspace_ref.root,
    });
    Ok((config, root, remote_media))
}

fn materialize_attachments(
    context: RemoteMediaContext,
    attachments: Vec<Attachment>,
) -> Result<(Vec<Attachment>, Vec<RemoteMedia>), String> {
    let mut resolved = Vec::with_capacity(attachments.len());
    let mut uploaded = Vec::with_capacity(attachments.len());
    for mut attachment in attachments {
        if matches!(
            attachment.kind,
            AttachmentKind::Mention | AttachmentKind::Skill
        ) {
            resolved.push(attachment);
            continue;
        }
        let kind = match attachment.kind {
            AttachmentKind::Image => RemoteMediaKind::Image,
            AttachmentKind::Audio => RemoteMediaKind::Audio,
            AttachmentKind::Mention | AttachmentKind::Skill => {
                unreachable!("references are never uploaded")
            }
        };
        match remote_media::materialize(
            context.shell.clone(),
            context.working_dir.clone(),
            attachment.path.clone(),
            kind,
        ) {
            Ok(remote) => {
                attachment.path = PathBuf::from(&remote.path);
                resolved.push(attachment);
                uploaded.push(remote);
            }
            Err(error) => {
                remote_media::remove(context.shell, context.working_dir, uploaded);
                return Err(error);
            }
        }
    }
    Ok((resolved, uploaded))
}

fn remove_remote_media(context: Option<&RemoteMediaContext>, uploaded: Vec<RemoteMedia>) {
    if let Some(context) = context {
        remote_media::remove(context.shell.clone(), context.working_dir.clone(), uploaded);
    }
}
