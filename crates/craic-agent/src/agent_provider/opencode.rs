use super::{
    AgentProvider, CancellationToken, CommandInvocation, ModelOption, append_model_args,
    check_command_available, command_path, parse_line_model_options, run_generation_command,
    run_model_command,
};

pub(super) static PROVIDER: Provider = Provider;

pub(super) struct Provider;

impl AgentProvider for Provider {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn check_available(&self) -> Result<(), String> {
        check_command_available(self.label(), &command_path("opencode"))
    }

    fn fetch_models(&self) -> Result<Vec<ModelOption>, String> {
        let output = run_model_command(self, command_path("opencode"), &["models"])?;
        parse_line_model_options(&output)
    }

    fn generate_text(
        &self,
        model: Option<&str>,
        prompt: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let mut args = vec!["run".to_string()];
        append_model_args(&mut args, model);
        run_generation_command(
            self,
            CommandInvocation::new(command_path("opencode"), args),
            prompt,
            cancellation,
        )
    }
}
