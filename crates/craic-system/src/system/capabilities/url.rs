use craic_platform::UiEffect;

pub trait UrlOpenAccess: Send + Sync {
    fn resolve_url(&self, url: &str) -> Result<UiEffect, String>;
}
