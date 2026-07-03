mod agy;
mod codex;
mod ollama;
mod opencode;

use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const DEFAULT_PROVIDER_ID: &str = "opencode";
pub const CANCELED_ERROR: &str = "Agent request canceled.";

const GENERATION_TIMEOUT: Duration = Duration::from_secs(120);
const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_PREVIEW_CHARS: usize = 300;
const REPAIR_OUTPUT_CHARS: usize = 8_000;

static MODEL_OPTIONS_CACHE: OnceLock<Mutex<HashMap<String, Vec<ModelOption>>>> = OnceLock::new();

static PROVIDERS: [&'static dyn AgentProvider; 4] = [
    &opencode::PROVIDER,
    &codex::PROVIDER,
    &agy::PROVIDER,
    &ollama::PROVIDER,
];

pub trait AgentProvider: Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;

    fn default_model_label(&self) -> String {
        format!("{} Default", self.label())
    }

    fn model_cache_key(&self) -> String {
        self.id().to_string()
    }

    fn check_available(&self) -> Result<(), String>;
    fn fetch_models(&self) -> Result<Vec<ModelOption>, String>;
    fn generate_text(
        &self,
        model: Option<&str>,
        prompt: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String>;
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    canceled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug)]
struct CommandInvocation {
    program: PathBuf,
    args: Vec<String>,
}

impl CommandInvocation {
    fn new(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

pub fn registered_providers() -> &'static [&'static dyn AgentProvider] {
    log_registered_providers();
    &PROVIDERS
}

pub fn find_provider(id: &str) -> Option<&'static dyn AgentProvider> {
    let id = id.trim();
    registered_providers()
        .iter()
        .copied()
        .find(|provider| provider.id() == id)
}

pub fn default_provider() -> &'static dyn AgentProvider {
    find_provider(DEFAULT_PROVIDER_ID).unwrap_or(PROVIDERS[0])
}

pub fn check_availability(provider_id: &str) -> Result<(), String> {
    let provider = provider_by_id(provider_id)?;
    match provider.check_available() {
        Ok(()) => {
            log::debug!("agent provider available provider_id={}", provider.id());
            Ok(())
        }
        Err(err) => {
            log::warn!(
                "agent provider availability failed provider_id={} error={}",
                provider.id(),
                preview_text(&err)
            );
            Err(err)
        }
    }
}

pub fn model_options(provider_id: &str) -> Result<Vec<ModelOption>, String> {
    let provider = provider_by_id(provider_id)?;
    let cache_key = provider.model_cache_key();
    if let Some(models) = cached_model_options(&cache_key) {
        log::debug!(
            "agent model cache hit provider_id={} cache_key={} models={}",
            provider.id(),
            cache_key,
            models.len()
        );
        return Ok(models);
    }

    log::debug!(
        "agent model cache miss provider_id={} cache_key={}",
        provider.id(),
        cache_key
    );
    if let Err(err) = check_availability(provider.id()) {
        log::warn!(
            "agent model listing failed provider_id={} cache_key={} error={}",
            provider.id(),
            cache_key,
            preview_text(&err)
        );
        return Err(err);
    }
    let models = provider.fetch_models().map_err(|err| {
        log::warn!(
            "agent model listing failed provider_id={} cache_key={} error={}",
            provider.id(),
            cache_key,
            preview_text(&err)
        );
        err
    })?;
    let sample = models
        .iter()
        .take(8)
        .map(|model| format!("{}=>{}", model.id, model.label))
        .collect::<Vec<_>>()
        .join(", ");
    log::debug!(
        "agent model listing complete provider_id={} cache_key={} models={} sample={}",
        provider.id(),
        cache_key,
        models.len(),
        sample
    );
    model_cache()
        .lock()
        .map_err(|_| "Agent model cache is unavailable.".to_string())?
        .insert(cache_key, models.clone());
    Ok(models)
}

pub fn generate_text(
    provider_id: &str,
    model: Option<&str>,
    prompt: &str,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    let provider = provider_by_id(provider_id)?;
    let model = normalized_model(model);
    if cancellation.is_canceled() {
        log::info!(
            "agent generation skipped after cancellation provider_id={}",
            provider.id()
        );
        return Err(CANCELED_ERROR.to_string());
    }
    log::info!(
        "agent generation attempt provider_id={} model_configured={} prompt_bytes={}",
        provider.id(),
        model.is_some(),
        prompt.len()
    );
    let result = provider.generate_text(model, prompt, cancellation);
    match result {
        Ok(output) => {
            log::debug!(
                "agent generation complete provider_id={} output_bytes={}",
                provider.id(),
                output.len()
            );
            Ok(output)
        }
        Err(err) => {
            log::error!(
                "agent generation failed provider_id={} error={}",
                provider.id(),
                preview_text(&err)
            );
            Err(err)
        }
    }
}

pub fn generate_structured<T, U, F>(
    provider_id: &str,
    model: Option<&str>,
    prompt: &str,
    response_name: &str,
    validate: F,
    cancellation: &CancellationToken,
) -> Result<U, String>
where
    T: DeserializeOwned,
    F: Fn(T) -> Result<U, String>,
{
    let output = generate_text(provider_id, model, prompt, cancellation)?;
    match parse_and_validate_structured::<T, U, F>(&output, response_name, &validate) {
        Ok(value) => Ok(value),
        Err(err) => {
            log::warn!(
                "structured generation invalid provider_id={} output_bytes={} error={}",
                provider_id,
                output.len(),
                err
            );
            log::info!(
                "structured generation repair retry provider_id={} response_name={}",
                provider_id,
                response_name
            );
            let repair_prompt = structured_repair_prompt(prompt, &output, &err, response_name);
            let retry_output = generate_text(provider_id, model, &repair_prompt, cancellation)
                .map_err(|err| {
                    log::error!(
                        "structured generation repair failed provider_id={} error={}",
                        provider_id,
                        preview_text(&err)
                    );
                    err
                })?;
            parse_and_validate_structured::<T, U, F>(&retry_output, response_name, &validate)
                .map_err(|retry_err| {
                    let final_err = format!(
                        "Agent returned invalid response after one repair attempt: {retry_err}"
                    );
                    log::error!(
                        "structured generation final error provider_id={} output_bytes={} error={}",
                        provider_id,
                        retry_output.len(),
                        final_err
                    );
                    final_err
                })
        }
    }
}

fn provider_by_id(provider_id: &str) -> Result<&'static dyn AgentProvider, String> {
    find_provider(provider_id).ok_or_else(|| {
        let ids = registered_providers()
            .iter()
            .map(|provider| provider.id())
            .collect::<Vec<_>>()
            .join(", ");
        let err = format!("Unknown agent provider '{provider_id}'. Available providers: {ids}.");
        log::error!(
            "agent provider lookup failed provider_id={} error={}",
            provider_id,
            preview_text(&err)
        );
        err
    })
}

fn cached_model_options(cache_key: &str) -> Option<Vec<ModelOption>> {
    model_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned())
}

fn model_cache() -> &'static Mutex<HashMap<String, Vec<ModelOption>>> {
    MODEL_OPTIONS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn log_registered_providers() {
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        for provider in &PROVIDERS {
            log::info!(
                "registered agent provider provider_id={} label={}",
                provider.id(),
                provider.label()
            );
        }
    });
}

fn normalized_model(model: Option<&str>) -> Option<&str> {
    model.map(str::trim).filter(|model| !model.is_empty())
}

fn parse_and_validate_structured<T, U, F>(
    output: &str,
    response_name: &str,
    validate: &F,
) -> Result<U, String>
where
    T: DeserializeOwned,
    F: Fn(T) -> Result<U, String>,
{
    let value = json_value_from_output(output)
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| format!("Agent did not return a {response_name}."))?;
    let parsed = serde_json::from_value::<T>(value)
        .map_err(|err| format!("Agent returned invalid JSON: {err}"))?;
    validate(parsed)
}

fn structured_repair_prompt(
    prompt: &str,
    output: &str,
    error: &str,
    response_name: &str,
) -> String {
    format!(
        r#"The previous response could not be used:
{error}

Return only a valid {response_name}. Do not include Markdown fences or commentary.

Original request:
{prompt}

Previous response:
```
{}
```
"#,
        truncate_chars(output, REPAIR_OUTPUT_CHARS)
    )
}

pub fn is_canceled_error(error: &str) -> bool {
    error == CANCELED_ERROR
}

fn append_model_args(args: &mut Vec<String>, model: Option<&str>) {
    if let Some(model) = normalized_model(model) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
}

fn check_command_available(label: &str, program: &Path) -> Result<(), String> {
    if program.is_file() {
        Ok(())
    } else {
        Err(format!(
            "{label} is not installed or is not available on PATH."
        ))
    }
}

fn run_generation_command(
    provider: &dyn AgentProvider,
    invocation: CommandInvocation,
    prompt: &str,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    if cancellation.is_canceled() {
        log::info!(
            "agent command skipped after cancellation provider_id={}",
            provider.id()
        );
        return Err(CANCELED_ERROR.to_string());
    }

    let mut child = Command::new(&invocation.program)
        .args(&invocation.args)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to start {}. Make sure it is installed and available on PATH. ({err})",
                provider.label()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|err| format!("Failed to send prompt to {}: {err}", provider.label()))?;
    }

    let started = Instant::now();
    loop {
        if cancellation.is_canceled() {
            let _ = child.kill();
            let _ = child.wait();
            log::info!("agent command canceled provider_id={}", provider.id());
            return Err(CANCELED_ERROR.to_string());
        }

        match child
            .try_wait()
            .map_err(|err| format!("Failed while waiting for {}: {err}", provider.label()))?
        {
            Some(status) => {
                let output = child.wait_with_output().map_err(|err| {
                    format!("Failed to collect {} output: {err}", provider.label())
                })?;
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                log::debug!(
                    "agent command finished provider_id={} status={} stdout_bytes={} stderr_bytes={}",
                    provider.id(),
                    status,
                    stdout.len(),
                    stderr.len()
                );
                if status.success() {
                    return if stdout.is_empty() {
                        Ok(stderr)
                    } else {
                        Ok(stdout)
                    };
                }

                return Err(if stderr.is_empty() {
                    format!("{} exited with status {status}.", provider.label())
                } else {
                    stderr
                });
            }
            None if started.elapsed() >= GENERATION_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} did not finish within {} seconds.",
                    provider.label(),
                    GENERATION_TIMEOUT.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn run_model_command(
    provider: &dyn AgentProvider,
    program: PathBuf,
    args: &[&str],
) -> Result<String, String> {
    log::debug!(
        "loading agent models provider_id={} command={} {}",
        provider.id(),
        program.display(),
        args.join(" ")
    );
    let mut child = Command::new(&program)
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to load {} models. Make sure {} is installed and available on PATH. ({err})",
                provider.label(),
                program.display()
            )
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Failed to read {} model output.", provider.label()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("Failed to read {} model errors.", provider.label()))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|err| err.to_string())
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|err| err.to_string())
    });

    let started = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|err| format!("Failed while loading {} models: {err}", provider.label()))?
        {
            Some(status) => {
                let stdout_bytes = stdout_reader
                    .join()
                    .map_err(|_| format!("Failed to collect {} model output.", provider.label()))?
                    .map_err(|err| {
                        format!("Failed to collect {} model output: {err}", provider.label())
                    })?;
                let stderr_bytes = stderr_reader
                    .join()
                    .map_err(|_| format!("Failed to collect {} model errors.", provider.label()))?
                    .map_err(|err| {
                        format!("Failed to collect {} model errors: {err}", provider.label())
                    })?;
                let stdout = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
                log::debug!(
                    "agent model command finished provider_id={} status={} stdout_bytes={} stderr_bytes={} stdout_preview={} stderr_preview={}",
                    provider.id(),
                    status,
                    stdout.len(),
                    stderr.len(),
                    preview_text(&stdout),
                    preview_text(&stderr)
                );
                if status.success() {
                    return Ok(if stdout.is_empty() { stderr } else { stdout });
                }

                return Err(if stderr.is_empty() {
                    format!(
                        "{} model command exited with status {status}.",
                        provider.label()
                    )
                } else {
                    stderr
                });
            }
            None if started.elapsed() >= MODEL_LIST_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} models did not load within {} seconds.",
                    provider.label(),
                    MODEL_LIST_TIMEOUT.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn parse_line_model_options(output: &str) -> Result<Vec<ModelOption>, String> {
    let models = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| ModelOption {
            id: line.to_string(),
            label: line.to_string(),
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        Err("Provider returned no models.".to_string())
    } else {
        Ok(models)
    }
}

fn json_value_from_output(output: &str) -> Option<serde_json::Value> {
    for (index, ch) in output.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut stream = serde_json::Deserializer::from_str(&output[index..]).into_iter();
        if let Some(Ok(value)) = stream.next() {
            log::debug!("parsed JSON value from agent output at byte_offset={index}");
            return Some(value);
        }
    }
    log::warn!(
        "failed to parse JSON value from agent output bytes={} preview={}",
        output.len(),
        preview_text(output)
    );
    None
}

fn command_path(name: &str) -> PathBuf {
    if let Some(path) = default_shell_command_path(name) {
        return path;
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let p = path.join(name);
            if p.is_file() {
                return p;
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(name);
        if p.is_file() {
            return p;
        }
    }

    for dir in &[
        "/home/linuxbrew/.linuxbrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ] {
        let p = PathBuf::from(dir).join(name);
        if p.is_file() {
            return p;
        }
    }

    PathBuf::from(name)
}

fn default_shell_command_path(name: &str) -> Option<PathBuf> {
    if !command_name_can_use_path(name) {
        return None;
    }

    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let script = format!("command -v {}", shell_quote(name));
    let output = Command::new(&shell)
        .arg("-i")
        .arg("-c")
        .arg(&script)
        .current_dir(std::env::temp_dir())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            log::warn!(
                "agent command shell lookup failed program={} shell={} error={}",
                name,
                shell.to_string_lossy(),
                err
            );
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        let resolved = shell_which_output(&stdout).map(PathBuf::from);
        log::debug!(
            "agent command shell lookup program={} resolved={:?}",
            name,
            resolved
        );
        resolved
    } else {
        log::debug!(
            "agent command shell lookup missing program={} status={} stderr={}",
            name,
            output.status,
            stderr.trim()
        );
        None
    }
}

fn command_name_can_use_path(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && program
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn shell_which_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn preview_text(text: &str) -> String {
    let mut preview = text
        .chars()
        .take(OUTPUT_PREVIEW_CHARS)
        .collect::<String>()
        .replace('\n', "\\n");
    if text.chars().count() > OUTPUT_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let truncated = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        format!("{truncated}\n[truncated]")
    } else {
        truncated
    }
}
