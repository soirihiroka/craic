#!/usr/bin/env bash

set -u

if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "usage: $0 PID [INTERVAL_SECONDS]" >&2
    exit 2
fi

target_pid=$1
interval_seconds=${2:-5}

if [[ ! $target_pid =~ ^[0-9]+$ || ! -d /proc/$target_pid ]]; then
    echo "Craic PID is not running: $target_pid" >&2
    exit 1
fi
if [[ ! $interval_seconds =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    echo "Interval must be a positive number of seconds: $interval_seconds" >&2
    exit 2
fi

cpu_count=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
previous_process_ticks=
previous_system_ticks=

cpu_percent() {
    local process_stat process_fields process_ticks system_ticks
    process_stat=$(<"/proc/$target_pid/stat")
    process_fields=${process_stat##*) }
    read -r -a fields <<< "$process_fields"
    process_ticks=$((fields[11] + fields[12] + fields[13] + fields[14]))
    system_ticks=$(awk '/^cpu / { total = 0; for (i = 2; i <= NF; i++) total += $i; print total; exit }' /proc/stat)
    sampled_cpu_percent=0
    if [[ -n $previous_process_ticks && $system_ticks -gt $previous_system_ticks ]]; then
        sampled_cpu_percent=$(awk -v process_delta="$((process_ticks - previous_process_ticks))" \
            -v system_delta="$((system_ticks - previous_system_ticks))" \
            -v cpus="$cpu_count" 'BEGIN { printf "%.1f", process_delta / system_delta * cpus * 100 }')
    fi
    previous_process_ticks=$process_ticks
    previous_system_ticks=$system_ticks
}

descendant_count() {
    ps -eo pid=,ppid= | awk -v root="$target_pid" '
        { parent[$1] = $2 }
        END {
            descendants[root] = 1
            changed = 1
            while (changed) {
                changed = 0
                for (pid in parent) {
                    if (!descendants[pid] && descendants[parent[pid]]) {
                        descendants[pid] = 1
                        count += 1
                        changed = 1
                    }
                }
            }
            print count + 0
        }
    '
}

printf 'timestamp\tpid\tcpu_percent\trss_kib\tthreads\tfds\tdescendants\tinotify_marks\n'
while [[ -d /proc/$target_pid ]]; do
    timestamp=$(date --iso-8601=seconds)
    cpu_percent
    rss_kib=$(awk '/^VmRSS:/ { print $2 }' "/proc/$target_pid/status" 2>/dev/null)
    threads=$(awk '/^Threads:/ { print $2 }' "/proc/$target_pid/status" 2>/dev/null)
    fds=$(find "/proc/$target_pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    descendants=$(descendant_count)
    inotify_marks=$(grep -h '^inotify' /proc/"$target_pid"/fdinfo/* 2>/dev/null | wc -l)
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$timestamp" "$target_pid" "$sampled_cpu_percent" "${rss_kib:-0}" \
        "${threads:-0}" "$fds" "$descendants" "$inotify_marks"
    sleep "$interval_seconds"
done

echo "Craic PID exited: $target_pid" >&2
