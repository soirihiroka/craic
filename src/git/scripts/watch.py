import base64
import json
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
