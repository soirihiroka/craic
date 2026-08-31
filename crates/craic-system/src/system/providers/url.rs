use crate::system::capabilities::url::UrlOpenAccess;
use crate::system::path::WorkspaceRef;
use craic_platform::UiEffect;

#[derive(Clone, Debug)]
pub(super) struct UrlResolver {
    provider_label: String,
    workspace: WorkspaceRef,
    wildcard_host_replacement: Option<String>,
}

impl UrlResolver {
    pub(super) fn new(
        provider_label: impl Into<String>,
        workspace: WorkspaceRef,
        wildcard_host_replacement: Option<String>,
    ) -> Self {
        Self {
            provider_label: provider_label.into(),
            workspace,
            wildcard_host_replacement,
        }
    }
}

impl UrlOpenAccess for UrlResolver {
    fn resolve_url(&self, url: &str) -> Result<UiEffect, String> {
        let rewritten_url = self
            .wildcard_host_replacement
            .as_deref()
            .and_then(|host| replace_wildcard_url_host(url, host));
        if rewritten_url.is_some() {
            log::info!(
                "url wildcard host rewritten provider={} workspace={}",
                self.provider_label,
                self.workspace.display_name
            );
        }
        Ok(UiEffect::OpenUrl(
            rewritten_url.unwrap_or_else(|| url.to_string()),
        ))
    }
}

fn replace_wildcard_url_host(url: &str, remote_host: &str) -> Option<String> {
    let (scheme, remainder) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }

    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let port = if authority == "0.0.0.0" || authority == "[::]" {
        ""
    } else if let Some(port) = authority.strip_prefix("0.0.0.0:") {
        if !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            &authority["0.0.0.0".len()..]
        } else {
            return None;
        }
    } else if let Some(port) = authority.strip_prefix("[::]:") {
        if !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            &authority["[::]".len()..]
        } else {
            return None;
        }
    } else {
        return None;
    };

    let remote_host = remote_host
        .trim()
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(remote_host.trim());
    if remote_host.is_empty() {
        return None;
    }
    let remote_host = if remote_host.contains(':') && !remote_host.starts_with('[') {
        format!("[{remote_host}]")
    } else {
        remote_host.to_string()
    };

    Some(format!(
        "{scheme}://{remote_host}{port}{}",
        &remainder[authority_end..]
    ))
}
