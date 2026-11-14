use super::{
    AgentProvider, CancellationToken, CommandInvocation, ModelOption, append_model_args,
    check_command_available, command_path, json_value_from_output, run_generation_command,
    run_model_command,
};

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
        check_command_available(self.label(), &command_path("codex"))
    }

    fn fetch_models(&self) -> Result<Vec<ModelOption>, String> {
        let output = run_model_command(
            self,
            command_path("codex"),
            &["debug", "models", "--bundled"],
        )?;
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
            "--skip-git-repo-check".to_string(),
            "--json".to_string(),
        ];
        append_model_args(&mut args, model);
        let output = run_generation_command(
            self,
            CommandInvocation::new(command_path("codex"), args),
            prompt,
            cancellation,
        )?;
        output
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| {
                event.get("type").and_then(serde_json::Value::as_str) == Some("item.completed")
            })
            .filter_map(|event| {
                let item = event.get("item")?;
                if item.get("type").and_then(serde_json::Value::as_str) != Some("agent_message") {
                    return None;
                }
                item.get("text")?.as_str().map(str::to_string)
            })
            .filter(|message| !message.trim().is_empty())
            .last()
            .ok_or_else(|| "Codex did not return a final response.".to_string())
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
