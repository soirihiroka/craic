use crate::system::capabilities::url::{UrlOpenAccess, UrlOpenActivation};
use crate::system::path::WorkspaceRef;
use gtk::gio;
use gtk::prelude::*;

#[derive(Clone, Debug)]
pub(super) struct GioUrlOpenAccess {
    provider_label: String,
    workspace: WorkspaceRef,
    wildcard_host_replacement: Option<String>,
}

impl GioUrlOpenAccess {
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

    fn app_launch_context(
        &self,
        activation: UrlOpenActivation,
    ) -> Result<gtk::gdk::AppLaunchContext, String> {
        let display = gtk::gdk::Display::default()
            .ok_or_else(|| "GTK display is unavailable for URL opening.".to_string())?;
        let context = display.app_launch_context();
        context.set_timestamp(if activation.event_time == 0 {
            gtk::gdk::CURRENT_TIME
        } else {
            activation.event_time
        });
        Ok(context)
    }
}

impl UrlOpenAccess for GioUrlOpenAccess {
    fn open_url(&self, url: &str, activation: UrlOpenActivation) -> Result<String, String> {
        let rewritten_url = self
            .wildcard_host_replacement
            .as_deref()
            .and_then(|host| replace_wildcard_url_host(url, host));
        let url = rewritten_url.as_deref().unwrap_or(url);
        if rewritten_url.is_some() {
            log::info!(
                "url wildcard host rewritten provider={} workspace={}",
                self.provider_label,
                self.workspace.display_name
            );
        }
        log::info!(
            "url open start provider={} workspace={} url_len={}",
            self.provider_label,
            self.workspace.display_name,
            url.len()
        );
        let context = self.app_launch_context(activation)?;
        gio::AppInfo::launch_default_for_uri(url, Some(&context))
            .map_err(|err| format!("Failed to open URL: {err}"))?;
        Ok("Opened URL.".to_string())
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
