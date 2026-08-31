use std::collections::{HashMap, HashSet};
#[cfg(target_os = "linux")]
use std::fs;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentResourceUsage {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub process_count: usize,
}

impl AgentResourceUsage {
    pub fn sidebar_label(self) -> String {
        format!(
            "CPU {} · Mem {} · {}",
            format_cpu_percent(self.cpu_percent),
            format_memory(self.memory_bytes),
            format_process_count(self.process_count)
        )
    }
}

pub struct ProcessUsageTracker {
    previous_samples: HashMap<u64, UsageSample>,
    #[cfg(target_os = "linux")]
    cpu_count: f64,
}

impl ProcessUsageTracker {
    pub fn new() -> Self {
        Self {
            previous_samples: HashMap::new(),
            #[cfg(target_os = "linux")]
            cpu_count: std::thread::available_parallelism()
                .map(|count| count.get() as f64)
                .unwrap_or(1.0),
        }
    }

    pub fn sample(
        &mut self,
        session_id: u64,
        root_pid: libc::pid_t,
        snapshot: &ProcessSnapshot,
    ) -> Option<AgentResourceUsage> {
        let subtree = snapshot.subtree_usage(root_pid)?;
        let current = UsageSample {
            process_ticks: subtree.cpu_ticks,
            system_ticks: snapshot.system_cpu_ticks,
        };
        let cpu_percent = self
            .previous_samples
            .insert(session_id, current)
            .map(|previous| {
                let process_delta =
                    current.process_ticks.saturating_sub(previous.process_ticks) as f64;
                let system_delta =
                    current.system_ticks.saturating_sub(previous.system_ticks) as f64;

                if system_delta <= 0.0 {
                    0.0
                } else {
                    let normalized = process_delta / system_delta;
                    #[cfg(target_os = "linux")]
                    let normalized = normalized * self.cpu_count;
                    normalized * 100.0
                }
            })
            .unwrap_or(0.0);

        Some(AgentResourceUsage {
            cpu_percent,
            memory_bytes: subtree.memory_bytes,
            process_count: subtree.process_count,
        })
    }

    pub fn clear(&mut self, session_id: u64) {
        self.previous_samples.remove(&session_id);
    }

    pub fn retain_sessions(&mut self, session_ids: &[u64]) {
        let session_ids = session_ids.iter().copied().collect::<HashSet<_>>();
        self.previous_samples
            .retain(|session_id, _| session_ids.contains(session_id));
    }
}

#[derive(Clone, Copy)]
struct UsageSample {
    process_ticks: u64,
    system_ticks: u64,
}

pub struct ProcessSnapshot {
    processes: HashMap<libc::pid_t, ProcessStat>,
    children: HashMap<libc::pid_t, Vec<libc::pid_t>>,
    system_cpu_ticks: u64,
}

impl ProcessSnapshot {
    pub fn read() -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            return Self::read_linux();
        }
        #[cfg(target_os = "macos")]
        {
            return Self::read_macos();
        }
        #[allow(unreachable_code)]
        None
    }

    #[cfg(target_os = "linux")]
    fn read_linux() -> Option<Self> {
        let system_cpu_ticks = system_cpu_ticks()?;
        let page_size = page_size();
        let mut processes = HashMap::new();
        let mut children: HashMap<libc::pid_t, Vec<libc::pid_t>> = HashMap::new();

        for entry in fs::read_dir("/proc").ok()?.filter_map(Result::ok) {
            let Some(pid) = entry
                .file_name()
                .to_string_lossy()
                .parse::<libc::pid_t>()
                .ok()
            else {
                continue;
            };
            let Ok(stat_text) = fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some(stat) = parse_process_stat(pid, &stat_text, page_size) else {
                continue;
            };
            children.entry(stat.parent_pid).or_default().push(pid);
            processes.insert(pid, stat);
        }

        Some(Self {
            processes,
            children,
            system_cpu_ticks,
        })
    }

    #[cfg(target_os = "macos")]
    fn read_macos() -> Option<Self> {
        const PROC_PIDTBSDINFO: i32 = 3;
        const RUSAGE_INFO_V4: i32 = 4;

        unsafe extern "C" {
            fn proc_listallpids(buffer: *mut libc::c_void, buffer_size: i32) -> i32;
            fn proc_pidinfo(
                pid: i32,
                flavor: i32,
                arg: u64,
                buffer: *mut libc::c_void,
                buffer_size: i32,
            ) -> i32;
            fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut libc::c_void) -> i32;
        }

        let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return None;
        }
        let mut pids = vec![0i32; count as usize + 64];
        let bytes = i32::try_from(pids.len().checked_mul(std::mem::size_of::<i32>())?).ok()?;
        let listed = unsafe { proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if listed <= 0 {
            return None;
        }
        pids.truncate(listed as usize);

        let mut processes = HashMap::new();
        let mut children: HashMap<libc::pid_t, Vec<libc::pid_t>> = HashMap::new();
        for pid in pids.into_iter().filter(|pid| *pid > 0) {
            let mut bsd_info = [0u8; 256];
            let bsd_size = unsafe {
                proc_pidinfo(
                    pid,
                    PROC_PIDTBSDINFO,
                    0,
                    bsd_info.as_mut_ptr().cast(),
                    bsd_info.len() as i32,
                )
            };
            if bsd_size < 20 {
                continue;
            }
            let parent_pid = i32::from_ne_bytes(bsd_info[16..20].try_into().ok()?);

            let mut usage = [0u64; 48];
            if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, usage.as_mut_ptr().cast()) } != 0 {
                continue;
            }
            let process = ProcessStat {
                parent_pid,
                // rusage_info_v4 reports the process' user/system time at indices 2/3 and
                // already-reaped descendants at 12/13. Live descendants are added while
                // walking the process tree below, matching the Linux cutime/cstime totals.
                cpu_ticks: usage[2]
                    .saturating_add(usage[3])
                    .saturating_add(usage[12])
                    .saturating_add(usage[13]),
                memory_bytes: usage[8],
            };
            children.entry(process.parent_pid).or_default().push(pid);
            processes.insert(pid, process);
        }

        let mut monotonic = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut monotonic) } != 0 {
            return None;
        }
        let system_cpu_ticks = u64::try_from(monotonic.tv_sec)
            .ok()?
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::try_from(monotonic.tv_nsec).ok()?);

        Some(Self {
            processes,
            children,
            system_cpu_ticks,
        })
    }

    fn subtree_usage(&self, root_pid: libc::pid_t) -> Option<ProcessSubtreeUsage> {
        if !self.processes.contains_key(&root_pid) {
            return None;
        }

        let mut seen = HashSet::new();
        let mut stack = vec![root_pid];
        let mut cpu_ticks = 0u64;
        let mut memory_bytes = 0u64;
        let mut process_count = 0usize;

        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }

            let Some(process) = self.processes.get(&pid) else {
                continue;
            };
            cpu_ticks = cpu_ticks.saturating_add(process.cpu_ticks);
            memory_bytes = memory_bytes.saturating_add(process.memory_bytes);
            process_count += 1;

            if let Some(children) = self.children.get(&pid) {
                stack.extend(children.iter().copied());
            }
        }

        Some(ProcessSubtreeUsage {
            cpu_ticks,
            memory_bytes,
            process_count,
        })
    }
}

struct ProcessSubtreeUsage {
    cpu_ticks: u64,
    memory_bytes: u64,
    process_count: usize,
}

struct ProcessStat {
    parent_pid: libc::pid_t,
    cpu_ticks: u64,
    memory_bytes: u64,
}

#[cfg(target_os = "linux")]
fn parse_process_stat(
    expected_pid: libc::pid_t,
    stat_text: &str,
    page_size: u64,
) -> Option<ProcessStat> {
    let pid = stat_text.split_whitespace().next()?.parse().ok()?;
    if pid != expected_pid {
        return None;
    }

    let fields = stat_text
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let parent_pid = fields.get(1)?.parse().ok()?;
    let utime = parse_u64_field(&fields, 11)?;
    let stime = parse_u64_field(&fields, 12)?;
    let cutime = parse_i64_field(&fields, 13)?.max(0) as u64;
    let cstime = parse_i64_field(&fields, 14)?.max(0) as u64;
    let resident_pages = parse_i64_field(&fields, 21)?.max(0) as u64;

    Some(ProcessStat {
        parent_pid,
        cpu_ticks: utime
            .saturating_add(stime)
            .saturating_add(cutime)
            .saturating_add(cstime),
        memory_bytes: resident_pages.saturating_mul(page_size),
    })
}

#[cfg(target_os = "linux")]
fn parse_u64_field(fields: &[&str], index: usize) -> Option<u64> {
    fields.get(index)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn parse_i64_field(fields: &[&str], index: usize) -> Option<i64> {
    fields.get(index)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn system_cpu_ticks() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let cpu_line = stat.lines().next()?;
    let mut parts = cpu_line.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }

    Some(parts.filter_map(|part| part.parse::<u64>().ok()).sum())
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size > 0 {
        page_size as u64
    } else {
        4096
    }
}

fn format_cpu_percent(percent: f64) -> String {
    let percent = percent.max(0.0);
    if percent < 0.05 {
        "0%".to_string()
    } else if percent < 10.0 {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

fn format_memory(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes = bytes as f64;
    if bytes < KIB {
        format!("{bytes:.0} B")
    } else if bytes < MIB {
        format!("{:.0} KB", bytes / KIB)
    } else if bytes < GIB {
        format_compact_unit(bytes / MIB, "MB")
    } else {
        format_compact_unit(bytes / GIB, "GB")
    }
}

fn format_compact_unit(value: f64, unit: &str) -> String {
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

fn format_process_count(process_count: usize) -> String {
    if process_count == 1 {
        "1 proc".to_string()
    } else {
        format!("{process_count} procs")
    }
}
