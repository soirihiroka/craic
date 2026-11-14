use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Runs bounded discovery/version commands, including collection of both pipes.
/// The command gets null stdin and its own process group; interactive terminals
/// use their PTY adapter instead of this helper.
pub fn command_output(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let stdout = child.stdout.take().expect("configured stdout pipe");
    let stderr = child.stderr.take().expect("configured stderr pipe");
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let error = io::Error::last_os_error();
            stop_command(&mut child);
            return Err(error);
        }
    }
    let canceled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let stdout = scope.spawn(|| collect_pipe(stdout, deadline, &canceled));
        let stderr = scope.spawn(|| collect_pipe(stderr, deadline, &canceled));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    break Err(io::Error::new(io::ErrorKind::TimedOut, "Command timed out"));
                }
                Err(error) => break Err(error),
            }
        };
        if status.is_err() {
            canceled.store(true, Ordering::Release);
            stop_command(&mut child);
        }
        let stdout = stdout
            .join()
            .map_err(|_| io::Error::other("stdout collector panicked"));
        let stderr = stderr
            .join()
            .map_err(|_| io::Error::other("stderr collector panicked"));
        let result = status.and_then(|status| {
            Ok(Output {
                status,
                stdout: stdout??,
                stderr: stderr??,
            })
        });
        if result.is_err() && !canceled.swap(true, Ordering::AcqRel) {
            // A shell may exit while descendants retain its pipe descriptors.
            // Collectors enforce the same deadline and never leave a blocked join.
            stop_command(&mut child);
        }
        result
    })
}

fn collect_pipe(
    mut pipe: impl Read,
    deadline: Instant,
    canceled: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if canceled.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Command output collection timed out",
            ));
        }
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(output),
            Ok(count) => {
                if output.len().saturating_add(count) > 16 * 1024 * 1024 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Command output exceeded 16 MiB",
                    ));
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn stop_command(child: &mut Child) {
    let pid = child.id();
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                log::warn!("bounded command cleanup did not observe child exit pid={pid}");
                return;
            }
        }
    }
}
