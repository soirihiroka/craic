use super::{
    AgentProvider, CANCELED_ERROR, CancellationToken, GENERATION_TIMEOUT, MODEL_LIST_TIMEOUT,
    ModelOption, preview_text,
};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Response};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

pub(super) static PROVIDER: Provider = Provider;

pub(super) struct Provider;

impl AgentProvider for Provider {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn label(&self) -> &'static str {
        "Ollama"
    }

    fn default_model_label(&self) -> String {
        "Select an Ollama model".to_string()
    }

    fn model_cache_key(&self) -> String {
        format!("{}:{}", self.id(), base_url())
    }

    fn check_available(&self) -> Result<(), String> {
        let base_url = base_url();
        run_http(request_version(&base_url))
    }

    fn fetch_models(&self) -> Result<Vec<ModelOption>, String> {
        let base_url = base_url();
        let tags = run_http(request_tags(&base_url))?;
        let models = tags
            .models
            .into_iter()
            .filter_map(|model| {
                let id = model
                    .name
                    .or(model.model)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())?;
                Some(ModelOption {
                    id: id.clone(),
                    label: id,
                })
            })
            .collect::<Vec<_>>();

        if models.is_empty() {
            Err(
                "Ollama returned no local models. Pull a model with `ollama pull <name>`."
                    .to_string(),
            )
        } else {
            log::debug!(
                "ollama parsed local models base_url={} count={}",
                base_url,
                models.len()
            );
            Ok(models)
        }
    }

    fn generate_text(
        &self,
        model: Option<&str>,
        prompt: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
            return Err(
                "Choose an Ollama model in Preferences before generating with Ollama.".to_string(),
            );
        };
        if cancellation.is_canceled() {
            log::info!("ollama generation skipped after cancellation");
            return Err(CANCELED_ERROR.to_string());
        }
        let base_url = base_url();
        run_http(async {
            let url = format!("{base_url}/api/generate");
            let client = http_client(GENERATION_TIMEOUT)?;
            let body = serde_json::json!({
                "model": model,
                "prompt": prompt,
                "stream": false,
            });
            let body = serde_json::to_vec(&body)
                .map_err(|err| format!("Failed to build Ollama request: {err}"))?;
            let response = client
                .post(&url)
                .header(CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|err| {
                    format!(
                        "Could not connect to Ollama at {base_url}. Start Ollama or update ollama.base_url. ({err})"
                    )
                })?;
            if cancellation.is_canceled() {
                log::info!("ollama generation canceled after response base_url={base_url}");
                return Err(CANCELED_ERROR.to_string());
            }
            let text = response_text(response, &base_url, "generate text").await?;
            if cancellation.is_canceled() {
                log::info!("ollama generation canceled after body read base_url={base_url}");
                return Err(CANCELED_ERROR.to_string());
            }
            let generated = serde_json::from_str::<GenerateResponse>(&text).map_err(|err| {
                format!(
                    "Ollama returned an invalid generation response from {base_url}: {err}. Body: {}",
                    preview_text(&text)
                )
            })?;
            if generated.response.trim().is_empty() {
                Err(format!(
                    "Ollama model '{model}' returned an empty response from {base_url}."
                ))
            } else {
                Ok(generated.response)
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
}

async fn request_tags(base_url: &str) -> Result<TagsResponse, String> {
    let url = format!("{base_url}/api/tags");
    log::debug!("loading ollama models base_url={base_url}");
    let client = http_client(MODEL_LIST_TIMEOUT)?;
    let response = client.get(&url).send().await.map_err(|err| {
        format!(
            "Could not connect to Ollama at {base_url}. Start Ollama or update ollama.base_url. ({err})"
        )
    })?;
    let text = response_text(response, base_url, "load models").await?;
    serde_json::from_str::<TagsResponse>(&text).map_err(|err| {
        format!(
            "Ollama returned an invalid model list from {base_url}: {err}. Body: {}",
            preview_text(&text)
        )
    })
}

async fn request_version(base_url: &str) -> Result<(), String> {
    let url = format!("{base_url}/api/version");
    log::debug!("checking ollama availability base_url={base_url}");
    let client = http_client(MODEL_LIST_TIMEOUT)?;
    let response = client.get(&url).send().await.map_err(|err| {
        format!(
            "Could not connect to Ollama at {base_url}. Start Ollama or update ollama.base_url. ({err})"
        )
    })?;
    response_text(response, base_url, "check availability")
        .await
        .map(|_| ())
}

async fn response_text(response: Response, base_url: &str, action: &str) -> Result<String, String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format!("Failed to read Ollama response from {base_url}: {err}"))?;
    if status.is_success() {
        return Ok(text);
    }

    let message = serde_json::from_str::<ErrorResponse>(&text)
        .ok()
        .map(|error| error.error)
        .filter(|error| !error.trim().is_empty())
        .unwrap_or_else(|| preview_text(&text));
    Err(format!(
        "Ollama at {base_url} failed to {action}: HTTP {status}: {message}"
    ))
}

fn http_client(timeout: std::time::Duration) -> Result<Client, String> {
    Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|err| format!("Failed to create Ollama HTTP client: {err}"))
}

fn run_http<T>(future: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            return tokio::task::block_in_place(|| handle.block_on(future));
        }
        return Err(
            "Ollama cannot run through the synchronous provider API on a current-thread Tokio runtime."
                .to_string(),
        );
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Failed to start Ollama HTTP runtime: {err}"))?
        .block_on(future)
}

fn base_url() -> String {
    crate::config::load()
        .ollama_base_url
        .or_else(|| std::env::var("OLLAMA_HOST").ok())
        .as_deref()
        .and_then(normalize_base_url)
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn normalize_base_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        return None;
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        Some(format!("http://{url}"))
    }
}
