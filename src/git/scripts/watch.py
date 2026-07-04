import base64
import json
import pathlib
import subprocess
import sys
import time


def git_bytes(repo, args):
    output = subprocess.run(
        ["git", *args],
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if output.returncode != 0:
        message = (output.stderr or output.stdout).decode("utf-8", "replace").strip()
        raise RuntimeError(message or f"git {' '.join(args)} failed with status {output.returncode}")
    return output.stdout


def git_text(repo, args):
    return git_bytes(repo, args).decode("utf-8", "replace").strip()


def optional_text(repo, args):
    try:
        return git_text(repo, args)
    except Exception:
        return ""


def parse_worktree_entries(raw):
    tokens = [
        token.decode("utf-8", "replace")
        for token in raw.split(b"\0")
        if token
    ]
    entries = []
    index = 0
    while index < len(tokens):
        field = tokens[index]
        index += 1
        if field.startswith("1 "):
            parts = field.split(" ", 8)
            if len(parts) >= 9:
                entries.append((parts[1], parts[8], None, False))
        elif field.startswith("2 "):
            old_path = tokens[index] if index < len(tokens) else None
            if old_path is not None:
                index += 1
            parts = field.split(" ", 9)
            if len(parts) >= 10:
                entries.append((parts[1], parts[9], old_path, False))
        elif field.startswith("u "):
            parts = field.split(" ", 10)
            if len(parts) >= 11:
                entries.append((parts[1], parts[10], None, False))
        elif field.startswith("? "):
            entries.append(("??", field[2:], None, True))
    return entries


def file_signature(repo, file_path):
    path = pathlib.Path(repo) / file_path
    try:
        stat = path.lstat()
    except FileNotFoundError:
        return {"state": "missing"}
    return {
        "state": "present",
        "is_dir": path.is_dir(),
        "len": stat.st_size,
        "modified_ns": stat.st_mtime_ns,
    }


def worktree_signatures(repo, status):
    signatures = {}
    for _, path, _, _ in parse_worktree_entries(status):
        signatures[path] = file_signature(repo, path)
    return signatures


def snapshot(repo):
    status = git_bytes(
        repo,
        ["--no-optional-locks", "status", "--untracked-files=all", "--branch", "--porcelain=2", "-z"],
    )
    return {
        "branch": optional_text(repo, ["rev-parse", "--abbrev-ref", "HEAD"]),
        "head": optional_text(repo, ["rev-parse", "HEAD"]),
        "upstream": optional_text(repo, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]),
        "status_b64": base64.b64encode(status).decode("ascii"),
        "worktree_signatures": worktree_signatures(repo, status),
    }


request = json.loads(sys.argv[1]) if len(sys.argv) > 1 else json.load(sys.stdin)
repo = request["repo"]
interval = max(1.0, float(request["interval_seconds"]))
previous = None
previous_error = None

while True:
    try:
        current = snapshot(repo)
        if previous_error:
            print("recovered", flush=True)
            previous_error = None
        if previous is None:
            previous = current
            print("ready", flush=True)
        elif current != previous:
            previous = current
            print("changed", flush=True)
    except Exception as error:
        message = str(error)
        if message != previous_error:
            print(f"error\t{message}", flush=True)
            previous_error = message
    time.sleep(interval)
