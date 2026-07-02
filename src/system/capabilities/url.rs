#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UrlOpenActivation {
    pub(crate) event_time: u32,
}

impl UrlOpenActivation {
    pub(crate) fn from_event_time(event_time: u32) -> Self {
        Self { event_time }
    }
}

pub(crate) trait UrlOpenAccess: Send + Sync {
    fn open_url(&self, url: &str, activation: UrlOpenActivation) -> Result<String, String>;
}
