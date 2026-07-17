import json
import os
import shutil
import subprocess
import sys


def git(args, check=True):
    proc = subprocess.run(["git", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and proc.returncode != 0:
        message = (proc.stderr or proc.stdout).decode("utf-8", "replace").strip()
        raise RuntimeError(message or f"git {' '.join(args)} failed with status {proc.returncode}")
    return proc


def safe_relative_path(path):
    if not path or path in (".", "..") or os.path.isabs(path):
        raise RuntimeError("Refusing to discard an invalid path.")
    normalized = os.path.normpath(path)
    if normalized == ".." or normalized.startswith("../"):
        raise RuntimeError("Refusing to discard a path outside the repository.")
    return path


def status_entry(path):
    proc = git(
        ["--no-optional-locks", "status", "--untracked-files=all", "--porcelain=2", "-z", "--", path]
    )
    fields = proc.stdout.decode("utf-8", "surrogateescape").split("\0")
    index = 0
    while index < len(fields):
        field = fields[index]
        index += 1
        if not field:
            continue
        kind = field[:1]
        parts = field.split(" ")
        if kind == "1" and len(parts) >= 9:
            candidate = parts[8]
            if candidate == path:
                return {"kind": "changed", "path": candidate, "old_path": None}
        if kind == "2" and len(parts) >= 10:
            new_path = parts[9]
            old_path = fields[index] if index < len(fields) else None
            if old_path is not None:
                index += 1
            if new_path == path or old_path == path:
                return {"kind": "renamed", "path": new_path, "old_path": old_path}
        if kind == "u" and len(parts) >= 11:
            candidate = parts[10]
            if candidate == path:
                return {"kind": "unmerged", "path": candidate, "old_path": None}
        if kind == "?" and field.startswith("? "):
            candidate = field[2:]
            if candidate == path:
                return {"kind": "untracked", "path": candidate, "old_path": None}
    return None


def tracked_in_head(path):
    return git(["cat-file", "-e", f"HEAD:{path}"], check=False).returncode == 0


def tracked_in_index(path):
    return git(["ls-files", "--error-unmatch", "--", path], check=False).returncode == 0


def remove_worktree_path(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path)
    elif os.path.lexists(path):
        os.remove(path)


request = json.load(sys.stdin)
path = safe_relative_path(request["path"])
entry = status_entry(path)

if entry and entry.get("old_path") and entry["old_path"] != entry["path"]:
    new_path = safe_relative_path(entry["path"])
    old_path = safe_relative_path(entry["old_path"])
    if tracked_in_index(new_path):
        git(["rm", "-f", "--", new_path])
    else:
        remove_worktree_path(new_path)
    git(["restore", "--staged", "--worktree", "--", old_path])
elif tracked_in_head(path):
    git(["restore", "--staged", "--worktree", "--", path])
elif tracked_in_index(path):
    git(["rm", "-f", "--", path])
else:
    remove_worktree_path(path)

print(json.dumps({"message": ""}))
