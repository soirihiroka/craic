use super::{
    AgentProvider, CancellationToken, CommandInvocation, ModelOption, append_model_args,
    check_command_available, command_path, json_value_from_output, run_generation_command,
    run_model_command,
};
use std::path::PathBuf;

pub(super) static PROVIDER: Provider = Provider;

pub(super) struct Provider;

impl AgentProvider for Provider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn check_available(&self) -> Result<(), String> {
        check_command_available(self.label(), &codex_command())
    }

    fn fetch_models(&self) -> Result<Vec<ModelOption>, String> {
        let output = run_model_command(self, codex_command(), &["debug", "models", "--bundled"])?;
        parse_codex_model_options(&output)
    }

    fn generate_text(
        &self,
        model: Option<&str>,
        prompt: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let mut args = vec![
            "exec".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
        ];
        append_model_args(&mut args, model);
        run_generation_command(
            self,
            CommandInvocation::new(codex_command(), args),
            prompt,
            cancellation,
        )
    }
}

fn parse_codex_model_options(output: &str) -> Result<Vec<ModelOption>, String> {
    let catalog = json_value_from_output(output)
        .ok_or_else(|| "Codex did not return a model catalog.".to_string())?;
    let top_level_keys = catalog
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    log::debug!("codex model catalog top_level_keys={top_level_keys:?}");
    let models = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Codex model catalog did not include a models array.".to_string())?
        .iter()
        .filter_map(|model| {
            let id = model.get("slug")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let visibility = model.get("visibility").and_then(serde_json::Value::as_str);
            if matches!(visibility, Some("hide" | "hidden")) {
                return None;
            }
            let label = model
                .get("display_name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| id.clone());
            Some(ModelOption { id, label })
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        Err("Codex returned no selectable models.".to_string())
    } else {
        let sample = models
            .iter()
            .take(8)
            .map(|model| format!("{}=>{}", model.id, model.label))
            .collect::<Vec<_>>()
            .join(", ");
        log::debug!(
            "codex parsed selectable models count={} sample={}",
            models.len(),
            sample
        );
        Ok(models)
    }
}

fn codex_command() -> PathBuf {
    let local = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("../codex/target/debug/codex");
    if local.is_file() {
        return local;
    }

    command_path("codex")
}
