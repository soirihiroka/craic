use crate::github;
use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio};
use moka::sync::Cache;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

static AVATAR_CACHE: OnceLock<Cache<String, Vec<u8>>> = OnceLock::new();
static AVATAR_IN_FLIGHT: OnceLock<Mutex<HashMap<String, Vec<mpsc::Sender<AvatarResult>>>>> =
    OnceLock::new();

type AvatarResult = Result<Vec<u8>, String>;

pub fn blank_title() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build()
}

pub fn app_menu_button(menu: &gio::Menu, visible: bool) -> gtk::MenuButton {
    let button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(menu)
        .tooltip_text("Main menu")
        .visible(visible)
        .build();
    button.add_css_class("flat");
    button
}

pub fn title(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    label.add_css_class("title-1");
    label
}

pub fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    label.add_css_class("heading");
    label
}

pub fn muted(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    label.add_css_class("dim-label");
    label
}

#[derive(Clone)]
pub enum AvatarSource {
    Url(String),
    Email(String),
}

impl AvatarSource {
    pub fn key(&self) -> String {
        match self {
            Self::Url(url) => format!("url:{url}"),
            Self::Email(email) => format!("email:{email}"),
        }
    }
}

pub fn fetch_avatar(avatar: &adw::Avatar, source: AvatarSource) {
    let cache_key = source.key();
    avatar.set_widget_name(&cache_key);

    if let Some(bytes) = cached_avatar(&cache_key) {
        if avatar.widget_name().as_str() == cache_key.as_str()
            && let Some(texture) = texture_from_bytes(&bytes)
        {
            avatar.set_custom_image(Some(&texture));
        }
        return;
    }

    let expected_key = cache_key.clone();
    let (sender, receiver) = mpsc::channel();
    let should_fetch = register_avatar_request(cache_key.clone(), sender);

    if should_fetch {
        thread::spawn(move || {
            let result = match source {
                AvatarSource::Url(url) => github::download_avatar(&url),
                AvatarSource::Email(email) => github::avatar_url_for_email(&email)
                    .and_then(|url| github::download_avatar(&url)),
            };

            if let Ok(bytes) = result.as_ref() {
                cache_avatar(cache_key.clone(), bytes);
            }
            complete_avatar_request(cache_key, result);
        });
    }

    gtk::glib::timeout_add_local(Duration::from_millis(100), {
        let avatar = avatar.clone();

        move || match receiver.try_recv() {
            Ok(Ok(bytes)) => {
                if avatar.widget_name().as_str() == expected_key.as_str()
                    && let Some(texture) = texture_from_bytes(&bytes)
                {
                    avatar.set_custom_image(Some(&texture));
                }
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        }
    });
}

fn cached_avatar(key: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = avatar_cache().get(key) {
        return Some(bytes);
    }

    if let Some(bytes) = github::cached_avatar_bytes(key) {
        avatar_cache().insert(key.to_string(), bytes.clone());
        return Some(bytes);
    }

    None
}

fn cache_avatar(key: String, bytes: &[u8]) {
    avatar_cache().insert(key.clone(), bytes.to_vec());
    github::cache_avatar_bytes(&key, bytes);
}

fn avatar_cache() -> &'static Cache<String, Vec<u8>> {
    AVATAR_CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(256)
            .time_to_live(Duration::from_secs(60))
            .build()
    })
}

fn register_avatar_request(key: String, sender: mpsc::Sender<AvatarResult>) -> bool {
    let mut in_flight = avatar_in_flight().lock().expect("avatar in-flight lock");
    if let Some(senders) = in_flight.get_mut(&key) {
        senders.push(sender);
        false
    } else {
        in_flight.insert(key, vec![sender]);
        true
    }
}

fn complete_avatar_request(key: String, result: AvatarResult) {
    let senders = avatar_in_flight()
        .lock()
        .expect("avatar in-flight lock")
        .remove(&key)
        .unwrap_or_default();

    for sender in senders {
        let _ = sender.send(result.clone());
    }
}

fn avatar_in_flight() -> &'static Mutex<HashMap<String, Vec<mpsc::Sender<AvatarResult>>>> {
    AVATAR_IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn texture_from_bytes(bytes: &[u8]) -> Option<gdk::Texture> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    loader
        .pixbuf()
        .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf))
}
