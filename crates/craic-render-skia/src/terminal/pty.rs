use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use polling::{Event, Events, PollMode, Poller};
use rustix_openpty::{
    openpty,
    rustix::{
        fs::{OFlags, fcntl_getfl, fcntl_setfl},
        termios::{
            InputModes, OptionalActions, Winsize, tcgetattr, tcgetpgrp, tcsetattr, tcsetwinsize,
        },
    },
};

use super::{TerminalCommand, TerminalState, TerminalViewport};

#[derive(Default)]
struct Exit {
    status: Option<ExitStatus>,
    finished: bool,
    reported: bool,
}

pub(super) struct PtyWorker {
    child_pid: u32,
    control: File,
    sender: mpsc::SyncSender<Vec<u8>>,
    poller: Arc<Poller>,
    exit: Arc<Mutex<Exit>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    closing: bool,
    shutdown: Option<craic_platform::terminal::ProcessGroupShutdown>,
}

impl PtyWorker {
    pub(super) fn spawn(
        launch: TerminalCommand,
        viewport: TerminalViewport,
        state: Arc<Mutex<TerminalState>>,
    ) -> io::Result<Self> {
        let argv = launch.spawn_argv().map_err(io::Error::other)?;
        let pty = openpty(None, Some(&window_size(viewport)))?;
        let master = File::from(pty.controller);
        let slave = File::from(pty.user);
        let mut termios = tcgetattr(&slave)?;
        termios.input_modes.insert(InputModes::IUTF8);
        tcsetattr(&slave, OptionalActions::Now, &termios)?;
        fcntl_setfl(&master, fcntl_getfl(&master)? | OFlags::NONBLOCK)?;
        let control = master.try_clone()?;
        let registered = RegisteredPty::new(master)?;
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(launch.working_directory)
            .env_clear()
            .envs(launch.environment)
            .env_remove("DESKTOP_STARTUP_ID")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                for signal in [
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGALRM,
                ] {
                    if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                        return Err(io::Error::last_os_error());
                    }
                }
                let mut signals = std::mem::zeroed();
                libc::sigemptyset(&mut signals);
                if libc::sigprocmask(libc::SIG_SETMASK, &signals, std::ptr::null_mut()) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = ChildGuard(Some(command.spawn()?));
        // Command retains the parent's copies of the configured slave FDs.
        drop(command);
        let child_pid = child.0.as_ref().unwrap().id();
        let (sender, receiver) = mpsc::sync_channel(128);
        let poller = Arc::clone(&registered.poller);
        let exit = Arc::new(Mutex::new(Exit::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_exit = Arc::clone(&exit);
        let worker_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("terminal-pty".into())
            .spawn(move || {
                let mut child = child;
                if let Err(error) = run(
                    &registered,
                    &mut child,
                    &state,
                    &worker_exit,
                    &worker_stop,
                    receiver,
                ) {
                    log::warn!("terminal PTY stopped pid={child_pid}: {error}");
                }
                drop(registered);
                let status = {
                    let mut exit = worker_exit
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if exit.status.is_none() {
                        exit.status = child.terminate();
                    }
                    exit.finished = true;
                    exit.status
                };
                state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .wake
                    .send_replace(());
                log::debug!("terminal PTY drained pid={child_pid} status={status:?}");
            })?;
        Ok(Self {
            child_pid,
            control,
            sender,
            poller,
            exit,
            stop,
            thread: Some(thread),
            closing: false,
            shutdown: None,
        })
    }

    pub(super) fn pid(&self) -> u32 {
        self.child_pid
    }

    pub(super) fn foreground(&self) -> Option<i32> {
        tcgetpgrp(&self.control).ok().map(|id| id.as_raw_pid())
    }

    pub(super) fn running(&self) -> bool {
        let exit = self.exit.lock().unwrap_or_else(|error| error.into_inner());
        !self.closing && exit.status.is_none() && !exit.finished
    }

    pub(super) fn closing(&self) -> bool {
        self.closing
    }

    pub(super) fn input(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.sender.try_send(bytes).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                io::Error::new(io::ErrorKind::WouldBlock, "terminal input queue is full")
            }
            mpsc::TrySendError::Disconnected(_) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "terminal process is closed")
            }
        })?;
        self.poller.notify()
    }

    pub(super) fn resize(&self, viewport: TerminalViewport) -> io::Result<()> {
        tcsetwinsize(&self.control, window_size(viewport))?;
        Ok(())
    }

    pub(super) fn take_exit(&self) -> Option<Option<ExitStatus>> {
        let mut exit = self.exit.lock().unwrap_or_else(|error| error.into_inner());
        if exit.finished && !exit.reported {
            exit.reported = true;
            Some(exit.status)
        } else {
            None
        }
    }

    pub(super) fn begin_shutdown(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        let exit = self.exit.lock().unwrap_or_else(|error| error.into_inner());
        if exit.status.is_none() && !exit.finished {
            self.shutdown = craic_platform::terminal::begin_process_group_shutdown(
                self.child_pid as i32,
                self.foreground(),
            );
        }
    }

    pub(super) fn stop(&mut self) -> io::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.begin_shutdown();
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.finish();
        }
        self.stop.store(true, Ordering::Release);
        let _ = self.poller.notify();
        thread
            .join()
            .map_err(|_| io::Error::other("terminal I/O thread panicked"))
    }
}

impl Drop for PtyWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct RegisteredPty {
    poller: Arc<Poller>,
    file: File,
}

impl RegisteredPty {
    fn new(file: File) -> io::Result<Self> {
        let poller = Arc::new(Poller::new()?);
        // The file outlives the registration and is deregistered in Drop.
        unsafe { poller.add_with_mode(&file, Event::readable(0), PollMode::Level)? };
        Ok(Self { poller, file })
    }
}

impl Drop for RegisteredPty {
    fn drop(&mut self) {
        let _ = self.poller.delete(&self.file);
    }
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn terminate(&mut self) -> Option<ExitStatus> {
        let mut child = self.0.take()?;
        craic_platform::terminal::signal_process_groups(child.id() as i32, None, libc::SIGKILL);
        let _ = child.kill();
        child.wait().ok()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn run(
    pty: &RegisteredPty,
    child: &mut ChildGuard,
    state: &Mutex<TerminalState>,
    exit: &Mutex<Exit>,
    stop: &AtomicBool,
    receiver: mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    let mut events = Events::new();
    let mut buffer = [0; 32 * 1024];
    let mut writes: VecDeque<Vec<u8>> = VecDeque::new();
    let mut written = 0;
    let mut read_open = true;
    let mut interest = Event::readable(0);
    let mut drain_until = None;
    while !stop.load(Ordering::Acquire) {
        if let Some(process) = child.0.as_mut() {
            // Serializing reap with begin_shutdown prevents signaling an
            // already reaped child by its old process group ID.
            let mut exit = exit.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(status) = process.try_wait()? {
                exit.status = Some(status);
                child.0 = None;
                drain_until = Some(Instant::now() + Duration::from_millis(100));
            }
        }
        let mut drained = !read_open;
        if read_open {
            for _ in 0..8 {
                match (&pty.file).read(&mut buffer) {
                    Ok(0) => {
                        read_open = false;
                        drained = true;
                        break;
                    }
                    Ok(count) => {
                        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                        let reply = state.feed(&buffer[..count]);
                        if !reply.is_empty() && drain_until.is_none() {
                            writes.push_back(reply);
                        }
                    }
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                        read_open = false;
                        drained = true;
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        drained = true;
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        if let Some(deadline) = drain_until
            && (drained || Instant::now() >= deadline)
        {
            return Ok(());
        }
        if read_open && drain_until.is_none() {
            while writes.len() < 128 {
                let Ok(bytes) = receiver.try_recv() else {
                    break;
                };
                writes.push_back(bytes);
            }
            while let Some(bytes) = writes.front() {
                match (&pty.file).write(&bytes[written..]) {
                    Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                    Ok(count) => {
                        written += count;
                        if written == bytes.len() {
                            writes.pop_front();
                            written = 0;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(error) => return Err(error),
                }
            }
        }
        let next = Event::new(
            0,
            read_open,
            read_open && drain_until.is_none() && !writes.is_empty(),
        );
        if next != interest {
            if read_open {
                pty.poller
                    .modify_with_mode(&pty.file, next, PollMode::Level)?;
            } else {
                // HUP is level-triggered even with empty interest.
                pty.poller.delete(&pty.file)?;
            }
            interest = next;
        }
        if drain_until.is_none() && read_open && writes.is_empty() {
            // A bounded receive can leave input queued after the write list
            // emptied. Consume another batch before waiting for a new wakeup.
            if let Ok(bytes) = receiver.try_recv() {
                writes.push_back(bytes);
                continue;
            }
        }
        events.clear();
        pty.poller
            .wait(&mut events, Some(Duration::from_millis(50)))?;
    }
    Ok(())
}

fn window_size(viewport: TerminalViewport) -> Winsize {
    Winsize {
        ws_row: viewport.lines as u16,
        ws_col: viewport.columns as u16,
        ws_xpixel: (viewport.columns as u16).saturating_mul(viewport.cell_width),
        ws_ypixel: (viewport.lines as u16).saturating_mul(viewport.cell_height),
    }
}
