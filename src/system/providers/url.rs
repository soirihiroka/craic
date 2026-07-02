use crate::system::capabilities::url::{UrlOpenAccess, UrlOpenActivation};
use crate::system::path::WorkspaceRef;
use gtk::gio;
use gtk::prelude::*;

#[derive(Clone, Debug)]
pub(super) struct GioUrlOpenAccess {
    provider_label: String,
    workspace: WorkspaceRef,
}

impl GioUrlOpenAccess {
    pub(super) fn new(provider_label: impl Into<String>, workspace: WorkspaceRef) -> Self {
        Self {
            provider_label: provider_label.into(),
            workspace,
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
