impl GitRepoHandle {
    pub fn new(
        workspace: WorkspaceRef,
        shell: Arc<dyn ShellAccess>,
        files: Arc<dyn FileAccess>,
    ) -> Self {
        Self {
            workspace,
            shell,
            files,
            terminal_links: None,
            hooks: Vec::new(),
        }
    }

    pub fn with_terminal_links(mut self, terminal_links: Arc<dyn TerminalLinkAccess>) -> Self {
        self.terminal_links = Some(terminal_links);
        self
    }

    pub fn with_hook(mut self, hook: Arc<dyn GitOperationHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    fn git(&self, args: &[String]) -> Result<String, String> {
        self.run_command_text("git", "git", args, None, &[0])
    }

    fn git_with_progress(
        &self,
        args: &[String],
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        let request = ShellCommandRunRequest::new("git", self.workspace.root.clone(), "git")
            .args(args.iter().cloned());
        let mut events = self.shell.stream_fast_command(request);
        while let Some(event) = events.blocking_recv() {
            match event {
                ShellCommandEvent::Record { text, .. } => {
                    progress(text);
                }
                ShellCommandEvent::Finished(result) => {
                    let output = result?;
                    if output.status_success(&[0]) {
                        return Ok(output.stdout);
                    }

                    let message = output.failure_message();
                    return Err(if message.is_empty() {
                        format!("git failed with status {:?}", output.status_code)
                    } else {
                        message
                    });
                }
            }
        }

        Err("git command stream stopped before returning a result.".to_string())
    }

    fn git_ok(&self, args: &[String]) -> Result<String, String> {
        self.git(args).map(|out| out.trim().to_string())
    }

    fn record_fetch(&self) {
        let workspace_key = self.workspace.id.to_string();
        match successful_fetch_times().lock() {
            Ok(mut fetch_times) => {
                fetch_times.insert(workspace_key, SystemTime::now());
            }
            Err(err) => {
                log::warn!(
                    "failed to record git fetch time workspace={}: {err}",
                    self.workspace.display_name
                );
            }
        }
    }

    fn last_fetch_at(&self) -> Option<SystemTime> {
        successful_fetch_times()
            .lock()
            .ok()
            .and_then(|fetch_times| fetch_times.get(&self.workspace.id.to_string()).copied())
    }

    fn run_script_output(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<ShellCommandOutput, String> {
        let mut request = ShellRunRequest::new(operation, self.workspace.root.clone(), script);
        if let Some(stdin) = stdin {
            request = request.stdin(stdin.to_vec());
        }
        let output = self
            .shell
            .run_fast_script(request)
            .blocking_recv()
            .map_err(|_| format!("{operation} shell command did not return a result."))??;
        if output.status_success(success_codes) {
            Ok(output)
        } else {
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
        }
    }

    fn run_script_text(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<String, String> {
        Ok(self
            .run_script_output(operation, script, stdin, success_codes)?
            .stdout_text_trimmed())
    }

    fn run_command_output(
        &self,
        operation: &str,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<ShellCommandOutput, String> {
        let mut request =
            ShellCommandRunRequest::new(operation, self.workspace.root.clone(), program)
                .args(args.iter().cloned());
        if let Some(stdin) = stdin {
            request = request.stdin(stdin.to_vec());
        }
        let output = self
            .shell
            .run_fast_command(request)
            .blocking_recv()
            .map_err(|_| format!("{operation} command did not return a result."))??;
        if output.status_success(success_codes) {
            Ok(output)
        } else {
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
        }
    }

    fn run_command_text(
        &self,
        operation: &str,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<String, String> {
        Ok(self
            .run_command_output(operation, program, args, stdin, success_codes)?
            .stdout_text_trimmed())
    }

    fn run_python_json<T: DeserializeOwned>(
        &self,
        operation: &str,
        script: &str,
        input: serde_json::Value,
    ) -> Result<T, String> {
        let stdin = serde_json::to_vec(&input)
            .map_err(|err| format!("{operation} request serialization failed: {err}"))?;
        let output = self.run_command_output(
            operation,
            "python3",
            &["-c".to_string(), script.to_string()],
            Some(&stdin),
            &[0],
        )?;
        serde_json::from_slice(&output.stdout)
            .map_err(|err| format!("{operation} returned invalid JSON: {err}"))
    }

    fn run_with_hooks<T, F>(&self, operation: &str, run: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        let mut post_hooks = Vec::new();
        for hook in &self.hooks {
            match hook.pre() {
                Ok(post_hook) => post_hooks.push(post_hook),
                Err(err) => {
                    for post_hook in post_hooks.into_iter().rev() {
                        if let Err(post_err) = post_hook.post() {
                            log::warn!(
                                "git operation hook cleanup failed operation={} error={}",
                                operation,
                                post_err
                            );
                        }
                    }
                    return Err(err);
                }
            }
        }

        let result = run();
        let mut post_error = None;
        for post_hook in post_hooks.into_iter().rev() {
            if let Err(err) = post_hook.post() {
                log::warn!(
                    "git operation post hook failed operation={} error={}",
                    operation,
                    err
                );
                if post_error.is_none() {
                    post_error = Some(err);
                }
            }
        }

        match (result, post_error) {
            (Ok(value), None) => Ok(value),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), _) => Err(err),
        }
    }
}
