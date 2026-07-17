#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UrlOpenActivation {
    pub event_time: u32,
}

impl UrlOpenActivation {
    pub fn from_event_time(event_time: u32) -> Self {
        Self { event_time }
    }
}

pub trait UrlOpenAccess: Send + Sync {
    fn open_url(&self, url: &str, activation: UrlOpenActivation) -> Result<String, String>;
}
