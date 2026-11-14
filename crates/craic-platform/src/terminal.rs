use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A terminal launch, independent of its native widget and PTY renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub working_directory: PathBuf,
    pub target_working_directory: String,
    pub environment: HashMap<String, String>,
}

impl TerminalCommand {
    pub fn new(
        program: impl Into<OsString>,
        args: Vec<OsString>,
        working_directory: PathBuf,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            target_working_directory: working_directory.to_string_lossy().into_owned(),
            working_directory,
            environment: crate::terminal_environment(),
        }
    }

    pub fn argv(&self) -> Vec<OsString> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }

    pub fn display(&self) -> String {
        self.argv()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Both backends reject invalid commands rather than silently replacing path bytes.
    pub fn utf8_argv(&self) -> Result<Vec<String>, String> {
        if self.program.is_empty() {
            return Err("Cannot start an empty terminal command.".into());
        }
        let directory = self
            .working_directory
            .to_str()
            .ok_or_else(|| "Terminal working directory is not valid UTF-8.".to_owned())?;
        if directory.contains('\0') || !self.working_directory.is_dir() {
            return Err("Terminal working directory does not exist or is invalid.".into());
        }
        if self.environment.iter().any(|(name, value)| {
            name.is_empty() || name.contains(['=', '\0']) || value.contains('\0')
        }) {
            return Err("Terminal environment contains an invalid name or value.".into());
        }
        self.argv()
            .into_iter()
            .map(|arg| {
                let arg = arg
                    .into_string()
                    .map_err(|_| "Terminal argument is not valid UTF-8.".to_owned())?;
                if arg.contains('\0') {
                    return Err("Terminal argument contains a null byte.".into());
                }
                Ok(arg)
            })
            .collect()
    }

    pub fn spawn_argv(&self) -> Result<Vec<String>, String> {
        let mut argv = vec!["/usr/bin/env".into(), "-u".into(), "NO_COLOR".into()];
        argv.extend(self.utf8_argv()?);
        Ok(argv)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns: usize,
    pub rows: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalLifecycle {
    Starting,
    Running,
    Exited,
    Closing,
}

/// Native adapters own presentation and PTY integration; callers use this process contract.
pub trait TerminalBackend {
    fn process_id(&self) -> Option<u32>;
    fn lifecycle(&self) -> TerminalLifecycle;
    fn send_input(&self, bytes: &[u8]) -> Result<(), String>;
    fn resize_terminal(&mut self, size: TerminalSize) -> Result<(), String>;
    fn terminate_process(&mut self) -> Result<(), String>;
}

pub fn signal_process_groups(child_pid: i32, foreground: Option<i32>, signal: i32) {
    for group in [Some(child_pid), foreground.filter(|id| *id != child_pid)]
        .into_iter()
        .flatten()
        .filter(|id| *id > 0)
    {
        let result = unsafe { libc::kill(-group, signal) };
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            log::warn!(
                "terminal process group signal failed pid={child_pid} group={group} signal={signal}: {}",
                io::Error::last_os_error()
            );
        }
    }
}

/// Starts shutdown before returning, so quitting cannot race the worker's first signal.
pub fn begin_process_group_shutdown(
    child_pid: i32,
    foreground: Option<i32>,
) -> Option<ProcessGroupShutdown> {
    if child_pid <= 0 {
        return None;
    }
    log::info!("terminal shutdown started pid={child_pid} foreground={foreground:?}");
    signal_process_groups(child_pid, foreground, libc::SIGHUP);
    Some(ProcessGroupShutdown {
        child_pid,
        foreground,
        started: Instant::now(),
    })
}

pub struct ProcessGroupShutdown {
    child_pid: i32,
    foreground: Option<i32>,
    started: Instant,
}

impl ProcessGroupShutdown {
    /// Run outside the UI thread. Both adapters use this bounded escalation policy.
    pub fn finish(self) {
        let Self {
            child_pid,
            foreground,
            started,
        } = self;
        for (signal, timeout) in [
            (None, 150),
            (Some(libc::SIGTERM), 250),
            (Some(libc::SIGKILL), 250),
        ] {
            let deadline = if let Some(signal) = signal {
                signal_process_groups(child_pid, foreground, signal);
                Instant::now() + Duration::from_millis(timeout)
            } else {
                started + Duration::from_millis(timeout)
            };
            loop {
                let alive = [foreground, Some(child_pid)]
                    .into_iter()
                    .flatten()
                    .filter(|id| *id > 0)
                    .any(|id| {
                        (unsafe { libc::kill(-id, 0) == 0 })
                            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                    });
                if !alive {
                    log::debug!("terminal shutdown completed pid={child_pid}");
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Blocking convenience for workers that have not started shutdown yet.
pub fn terminate_process_groups(child_pid: i32, foreground: Option<i32>) {
    if let Some(shutdown) = begin_process_group_shutdown(child_pid, foreground) {
        shutdown.finish();
    }
}
