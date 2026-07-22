pub(super) fn signal_terminal_process_groups(
    child_pid: libc::pid_t,
    foreground_pgid: Option<libc::pid_t>,
    signal: libc::c_int,
) {
    for (role, pgid) in [
        ("foreground", foreground_pgid),
        ("session-leader", Some(child_pid)),
    ] {
        let Some(pgid) = pgid.filter(|pgid| *pgid > 0) else {
            continue;
        };
        if role == "session-leader" && foreground_pgid == Some(pgid) {
            continue;
        }
        let result = unsafe { libc::kill(-pgid, signal) };
        if result == 0 {
            log::info!(
                "VTE terminal process group signaled role={role} pgid={pgid} signal={signal} pid={child_pid}"
            );
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                log::debug!(
                    "VTE terminal process group already exited role={role} pgid={pgid} signal={signal} pid={child_pid}"
                );
            } else {
                log::warn!(
                    "VTE terminal process group signal failed role={role} pgid={pgid} signal={signal} pid={child_pid} error={error}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::signal_terminal_process_groups;
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    struct ProcessGroupGuard {
        child: Child,
        pgid: libc::pid_t,
    }

    impl Drop for ProcessGroupGuard {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }

    #[test]
    fn terminal_close_terminates_the_program_process_group() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }

        let mut child = command
            .spawn()
            .expect("isolated terminal process should start");
        let session_leader = child.id() as libc::pid_t;
        let stdout = child.stdout.take().expect("terminal process stdout");
        let program_pid = BufReader::new(stdout)
            .lines()
            .next()
            .expect("terminal program pid line")
            .expect("terminal program pid should be readable")
            .parse::<libc::pid_t>()
            .expect("terminal program should print its pid");
        let mut group = ProcessGroupGuard {
            child,
            pgid: session_leader,
        };

        signal_terminal_process_groups(session_leader, Some(session_leader), libc::SIGHUP);

        assert!(
            wait_for_child_exit(&mut group.child, Duration::from_secs(2)),
            "terminal session leader remained alive after close"
        );
        assert!(
            wait_for_process_exit(program_pid, Duration::from_secs(2)),
            "program in the terminal process group remained alive after close"
        );
    }

    #[test]
    fn terminal_close_terminates_distinct_foreground_and_session_leader_groups() {
        let mut session_leader = isolated_sleep_process();
        let mut foreground = isolated_sleep_process();
        let session_leader_pid = session_leader.pgid;
        let foreground_pid = foreground.pgid;

        signal_terminal_process_groups(session_leader_pid, Some(foreground_pid), libc::SIGHUP);

        assert!(
            wait_for_child_exit(&mut foreground.child, Duration::from_secs(2)),
            "foreground terminal process group remained alive after close"
        );
        assert!(
            wait_for_child_exit(&mut session_leader.child, Duration::from_secs(2)),
            "terminal session leader process group remained alive after close"
        );
    }

    fn isolated_sleep_process() -> ProcessGroupGuard {
        let mut command = Command::new("sleep");
        command.arg("30");
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command
            .spawn()
            .expect("isolated terminal process should start");
        let pgid = child.id() as libc::pid_t;
        ProcessGroupGuard { child, pgid }
    }

    fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn wait_for_process_exit(pid: libc::pid_t, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let result = unsafe { libc::kill(pid, 0) };
            if result < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }
}
