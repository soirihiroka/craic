use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::fs;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const LATEST_CRASH_FILE: &str = "latest-crash.log";

pub(crate) struct CrashNotice {
    pub(crate) path: PathBuf,
    pub(crate) summary: String,
}

pub(crate) fn install() -> Option<PathBuf> {
    let crash_dir = crate::config::craic_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("craic"))
        .join("crashes");

    if let Err(err) = fs::create_dir_all(&crash_dir) {
        eprintln!(
            "craic: failed to create crash dump directory {}: {err}",
            crash_dir.display()
        );
        return None;
    }

    install_signal_handlers(&crash_dir);
    install_panic_hook(crash_dir.clone());
    Some(crash_dir)
}

pub(crate) fn take_latest_crash_notice() -> Option<CrashNotice> {
    let crash_dir = crate::config::craic_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("craic"))
        .join("crashes");
    let latest_path = crash_dir.join(LATEST_CRASH_FILE);
    let dump = match fs::read_to_string(&latest_path) {
        Ok(dump) => dump,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            log::warn!(
                "latest crash dump could not be read path={} error={err}",
                latest_path.display()
            );
            return None;
        }
    };

    if let Err(err) = fs::remove_file(&latest_path) {
        log::warn!(
            "latest crash marker could not be removed path={} error={err}",
            latest_path.display()
        );
    }

    Some(CrashNotice {
        path: latest_path,
        summary: crash_summary(&dump),
    })
}

fn install_panic_hook(crash_dir: PathBuf) {
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        match write_panic_dump(&crash_dir, info) {
            Ok(path) => {
                log::error!("panic captured crash_dump={}", path.display());
                eprintln!("craic: crash dump written to {}", path.display());
            }
            Err(err) => {
                log::error!("panic captured but crash dump write failed: {err}");
                eprintln!("craic: failed to write crash dump: {err}");
            }
        }
        previous_hook(info);
    }));
}

fn crash_summary(dump: &str) -> String {
    let mut summary = Vec::new();

    for prefix in ["kind: ", "signal: ", "message: ", "location: "] {
        if let Some(line) = dump
            .lines()
            .find_map(|line| line.strip_prefix(prefix).filter(|value| !value.is_empty()))
        {
            summary.push(format!("{}: {line}", prefix.trim_end_matches(": ")));
        }
    }

    if summary.is_empty() {
        "Crash details are available in the dump file.".to_string()
    } else {
        summary.join("\n")
    }
}

fn write_panic_dump(crash_dir: &Path, info: &PanicHookInfo<'_>) -> std::io::Result<PathBuf> {
    fs::create_dir_all(crash_dir)?;
    let timestamp = unix_timestamp();
    let path = crash_dir.join(format!("crash-{timestamp}-{}-panic.log", process::id()));
    let latest_path = crash_dir.join(LATEST_CRASH_FILE);
    let dump = panic_dump(timestamp, info);

    fs::write(&path, dump.as_bytes())?;
    fs::write(&latest_path, dump.as_bytes())?;
    Ok(path)
}

fn panic_dump(timestamp: u64, info: &PanicHookInfo<'_>) -> String {
    let mut dump = String::new();
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("<unnamed>");
    let payload = panic_payload(info);

    let _ = writeln!(dump, "Craic crash dump");
    let _ = writeln!(dump, "kind: rust panic");
    let _ = writeln!(dump, "unix_timestamp: {timestamp}");
    let _ = writeln!(dump, "pid: {}", process::id());
    let _ = writeln!(dump, "thread: {thread_name}");
    let _ = writeln!(dump, "version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        dump,
        "target: {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    if let Ok(exe) = std::env::current_exe() {
        let _ = writeln!(dump, "executable: {}", exe.display());
    }

    let args = std::env::args().collect::<Vec<_>>();
    let _ = writeln!(dump, "args: {args:?}");

    match info.location() {
        Some(location) => {
            let _ = writeln!(
                dump,
                "location: {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
        }
        None => {
            let _ = writeln!(dump, "location: <unknown>");
        }
    }

    let _ = writeln!(dump, "message: {payload}");
    let _ = writeln!(dump);
    let _ = writeln!(dump, "Backtrace:");
    let _ = writeln!(dump, "{}", Backtrace::force_capture());
    dump
}

fn panic_payload(info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = info.payload().downcast_ref::<String>() {
        return message.clone();
    }
    "<non-string panic payload>".to_string()
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(unix)]
mod signal {
    use super::{LATEST_CRASH_FILE, unix_timestamp};
    use libc::{c_int, c_void};
    use std::cell::UnsafeCell;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::process;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SIGNAL_PATH_CAPACITY: usize = 4096;

    struct SignalPath {
        len: AtomicUsize,
        bytes: UnsafeCell<[u8; SIGNAL_PATH_CAPACITY]>,
    }

    unsafe impl Sync for SignalPath {}

    impl SignalPath {
        const fn new() -> Self {
            Self {
                len: AtomicUsize::new(0),
                bytes: UnsafeCell::new([0; SIGNAL_PATH_CAPACITY]),
            }
        }

        fn set(&self, path: &Path) -> bool {
            let bytes = path.as_os_str().as_bytes();
            if bytes.is_empty() || bytes.len() + 1 > SIGNAL_PATH_CAPACITY {
                return false;
            }

            unsafe {
                let target = &mut *self.bytes.get();
                target[..bytes.len()].copy_from_slice(bytes);
                target[bytes.len()] = 0;
            }
            self.len.store(bytes.len(), Ordering::Release);
            true
        }

        unsafe fn as_c_ptr(&self) -> *const libc::c_char {
            if self.len.load(Ordering::Acquire) == 0 {
                return ptr::null();
            }
            self.bytes.get().cast::<libc::c_char>()
        }
    }

    static SIGNAL_CRASH_PATH: SignalPath = SignalPath::new();
    static LATEST_CRASH_PATH: SignalPath = SignalPath::new();

    pub(super) fn install_signal_handlers(crash_dir: &Path) {
        let timestamp = unix_timestamp();
        let signal_path = crash_dir.join(format!("crash-{timestamp}-{}-signal.log", process::id()));
        let latest_path = crash_dir.join(LATEST_CRASH_FILE);

        if !SIGNAL_CRASH_PATH.set(&signal_path) || !LATEST_CRASH_PATH.set(&latest_path) {
            eprintln!(
                "craic: crash dump path is too long for signal handler: {}",
                signal_path.display()
            );
            return;
        }

        for signal in [
            libc::SIGABRT,
            libc::SIGBUS,
            libc::SIGFPE,
            libc::SIGILL,
            libc::SIGSEGV,
        ] {
            unsafe {
                install_signal_handler(signal);
            }
        }
    }

    unsafe fn install_signal_handler(signal: c_int) {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle_signal as *const () as usize;
        action.sa_flags = libc::SA_RESETHAND;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }

        if unsafe { libc::sigaction(signal, &action, ptr::null_mut()) } != 0 {
            eprintln!("craic: failed to install crash signal handler for signal {signal}");
        }
    }

    unsafe extern "C" fn handle_signal(signal: c_int) {
        let crash_path = unsafe { SIGNAL_CRASH_PATH.as_c_ptr() };
        if !crash_path.is_null() {
            unsafe {
                write_signal_dump(crash_path, signal);
            }
        }

        let latest_path = unsafe { LATEST_CRASH_PATH.as_c_ptr() };
        if !latest_path.is_null() {
            unsafe {
                write_signal_dump(latest_path, signal);
            }
        }

        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }

    unsafe fn write_signal_dump(path: *const libc::c_char, signal: c_int) {
        let fd = unsafe {
            libc::open(
                path,
                libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return;
        }

        unsafe {
            write_all(fd, b"Craic crash dump\n");
            write_all(fd, b"kind: fatal native signal\n");
            write_all(fd, b"signal: ");
            write_i32(fd, signal);
            write_all(fd, b"\npid: ");
            write_i32(fd, libc::getpid());
            write_all(
                fd,
                b"\n\nThis minimal dump was written from a fatal signal handler.\nRust panics include a full backtrace in the panic crash dump.\n",
            );
            libc::close(fd);
        }
    }

    unsafe fn write_all(fd: c_int, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let written = unsafe { libc::write(fd, bytes.as_ptr().cast::<c_void>(), bytes.len()) };
            if written <= 0 {
                break;
            }
            bytes = &bytes[written as usize..];
        }
    }

    unsafe fn write_i32(fd: c_int, value: c_int) {
        let mut value = value;
        if value < 0 {
            unsafe {
                write_all(fd, b"-");
            }
            value = value.saturating_neg();
        }

        let mut digits = [0_u8; 12];
        let mut index = digits.len();
        let mut remaining = value as u32;
        if remaining == 0 {
            unsafe {
                write_all(fd, b"0");
            }
            return;
        }

        while remaining > 0 && index > 0 {
            index -= 1;
            digits[index] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
        }
        unsafe {
            write_all(fd, &digits[index..]);
        }
    }
}

#[cfg(unix)]
fn install_signal_handlers(crash_dir: &Path) {
    signal::install_signal_handlers(crash_dir);
}

#[cfg(not(unix))]
fn install_signal_handlers(_: &Path) {}
