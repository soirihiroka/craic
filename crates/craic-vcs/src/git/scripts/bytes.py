import base64
import json
import pathlib
import subprocess
import sys


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(2)


def git_bytes(repo, args):
    output = subprocess.run(
        ["git", *args],
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if output.returncode != 0:
        message = (output.stderr or output.stdout).decode("utf-8", "replace").strip()
        fail(message or f"git {' '.join(args)} failed with status {output.returncode}")
    return output.stdout


def git_text(repo, args):
    return git_bytes(repo, args).decode("utf-8", "replace").strip()


def b64(value):
    if value is None:
        return None
    return base64.b64encode(value).decode("ascii")


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


def worktree_path_pair(repo, file_path):
    raw = git_bytes(
        repo,
        ["--no-optional-locks", "status", "--untracked-files=all", "--branch", "--porcelain=2", "-z"],
    )
    for status, path, old_path, untracked in parse_worktree_entries(raw):
        if path == file_path or old_path == file_path:
            return {
                "old": None if untracked else old_path or path,
                "new": path,
            }
    return {"old": file_path, "new": file_path}


def parse_commit_pairs(raw):
    tokens = [
        token.decode("utf-8", "replace")
        for token in raw.split(b"\0")
        if token
    ]
    pairs = []
    index = 0
    while index < len(tokens):
        status = tokens[index]
        index += 1
        kind = status[:1]
        if kind in ("R", "C"):
            old_path = tokens[index] if index < len(tokens) else None
            new_path = tokens[index + 1] if index + 1 < len(tokens) else None
            index += int(old_path is not None) + int(new_path is not None)
            pairs.append({"old": old_path, "new": new_path})
        elif kind == "A":
            new_path = tokens[index] if index < len(tokens) else None
            index += int(new_path is not None)
            pairs.append({"old": None, "new": new_path})
        elif kind == "D":
            old_path = tokens[index] if index < len(tokens) else None
            index += int(old_path is not None)
            pairs.append({"old": old_path, "new": None})
        else:
            path = tokens[index] if index < len(tokens) else None
            index += int(path is not None)
            pairs.append({"old": path, "new": path})
    return pairs


def commit_path_pair(repo, commit_hash, file_path):
    raw = git_bytes(
        repo,
        ["diff-tree", "--root", "--no-commit-id", "--name-status", "-r", "-M", "-z", commit_hash],
    )
    for pair in parse_commit_pairs(raw):
        if pair.get("old") == file_path or pair.get("new") == file_path:
            return pair
    return {"old": file_path, "new": file_path}


def parent_hash(repo, commit_hash):
    output = git_text(repo, ["rev-list", "--parents", "-n", "1", commit_hash])
    parts = output.split()
    return parts[1] if len(parts) > 1 else None


def tree_bytes(repo, rev, file_path, max_bytes):
    if not rev or not file_path:
        return None
    spec = f"{rev}:{file_path}"
    exists = subprocess.run(
        ["git", "cat-file", "-e", spec],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if exists.returncode != 0:
        return None
    size_text = git_text(repo, ["cat-file", "-s", spec])
    try:
        size = int(size_text)
    except ValueError:
        fail(f"Invalid git object size: {size_text}")
    if size > max_bytes:
        fail(f"{pathlib.PurePosixPath(file_path).name} is too large to preview.")
    return git_bytes(repo, ["show", spec])


def workdir_bytes(repo, file_path, max_bytes):
    path = pathlib.Path(repo) / file_path
    if not path.exists() or path.is_dir():
        return None
    size = path.stat().st_size
    if size > max_bytes:
        fail(f"{pathlib.PurePosixPath(file_path).name} is too large to preview.")
    return path.read_bytes()


request = json.load(sys.stdin)
repo = request["repo"]
mode = request["mode"]
file_path = request["path"]
max_binary_bytes = int(request["max_binary_bytes"])

if mode == "worktree":
    pair = worktree_path_pair(repo, file_path)
    before = tree_bytes(repo, "HEAD", pair.get("old") or file_path, max_binary_bytes)
    after = workdir_bytes(repo, pair.get("new") or file_path, max_binary_bytes)
elif mode == "commit":
    commit_hash = request["hash"]
    pair = commit_path_pair(repo, commit_hash, file_path)
    parent = parent_hash(repo, commit_hash)
    before = tree_bytes(repo, parent, pair.get("old") or file_path, max_binary_bytes)
    after = tree_bytes(repo, commit_hash, pair.get("new") or file_path, max_binary_bytes)
else:
    fail(f"Unsupported bytes mode: {mode}")

json.dump({"before_b64": b64(before), "after_b64": b64(after)}, sys.stdout)
