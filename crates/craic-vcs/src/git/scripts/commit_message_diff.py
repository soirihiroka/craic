import base64
import json
import os
import subprocess
import sys


def git_bytes(args, allow_fail=False):
    proc = subprocess.run(["git", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if proc.returncode != 0:
        if allow_fail:
            return b""
        sys.stderr.write(proc.stderr.decode("utf-8", "replace"))
        raise SystemExit(proc.returncode)
    return proc.stdout


def git_success(args):
    return subprocess.run(
        ["git", *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    ).returncode == 0


def selected_diff(files):
    diff = git_bytes(
        [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            "HEAD",
            "--",
            *files,
        ],
        allow_fail=True,
    )
    if not diff:
        diff = git_bytes(
            ["diff", "--no-color", "--no-ext-diff", "--find-renames", "--", *files],
            allow_fail=True,
        )
    return diff.decode("utf-8", "replace")


def untracked_details(files):
    details = []
    for file_path in files:
        if git_success(["ls-files", "--error-unmatch", "--", file_path]):
            continue
        if os.path.isdir(file_path):
            details.append(
                f"diff --git a/{file_path} b/{file_path}\n"
                "new file mode 040000\n"
                "--- /dev/null\n"
                f"+++ b/{file_path}\n"
                "@@\n"
                "[untracked directory omitted]"
            )
            continue
        try:
            with open(file_path, "rb") as handle:
                data = handle.read()
        except OSError:
            details.append(
                f"diff --git a/{file_path} b/{file_path}\n"
                "new file mode 100644\n"
                "--- /dev/null\n"
                f"+++ b/{file_path}\n"
                "@@\n"
                "[untracked file could not be read]"
            )
            continue
        if b"\0" in data:
            details.append(
                f"diff --git a/{file_path} b/{file_path}\n"
                "new file mode 100644\n"
                "--- /dev/null\n"
                f"+++ b/{file_path}\n"
                "@@\n"
                "[untracked binary file omitted]"
            )
            continue
        text = data.decode("utf-8", "replace")
        details.append(
            f"diff --git a/{file_path} b/{file_path}\n"
            "new file mode 100644\n"
            "--- /dev/null\n"
            f"+++ b/{file_path}\n"
            "@@\n"
            + "\n".join(f"+{line}" for line in text.splitlines())
        )
    return "\n\n".join(details)


request = json.load(sys.stdin)
files = [path for path in request.get("files", []) if path]
diff = selected_diff(files)
untracked = untracked_details(files)
if untracked:
    if diff.strip():
        diff += "\n\n"
    diff += untracked

print(json.dumps({"diff_b64": base64.b64encode(diff.encode("utf-8")).decode("ascii")}))
