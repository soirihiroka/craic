import datetime
import json
import re
import sys
import tarfile
import zipfile

fmt, path = sys.argv[1], sys.argv[2]
drive_prefix = re.compile(r"^[A-Za-z]:")


def normalize_member(raw):
    raw = raw.rstrip("/")
    if not raw:
        return None
    if raw.startswith("/") or raw.startswith("\\") or "\\" in raw or drive_prefix.match(raw):
        return None
    parts = raw.split("/")
    if any(part in ("", ".", "..") or drive_prefix.match(part) for part in parts):
        return None
    return "/".join(parts)


def zip_modified(info):
    try:
        return datetime.datetime(
            *info.date_time, tzinfo=datetime.timezone.utc
        ).timestamp()
    except Exception:
        return None


def zip_mode(info):
    mode = (info.external_attr >> 16) & 0o777
    return mode or None


members = []
invalid = 0
if fmt == "zip":
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            name = normalize_member(info.filename)
            if name is None:
                invalid += 1
                continue
            kind = "dir" if info.is_dir() else "file"
            members.append(
                {
                    "name": name,
                    "kind": kind,
                    "len": None if kind == "dir" else info.file_size,
                    "modified": zip_modified(info),
                    "mode": zip_mode(info),
                }
            )
else:
    mode = {"tar": "r:", "tar.gz": "r:gz", "tar.xz": "r:xz", "tar.bz2": "r:bz2"}[
        fmt
    ]
    with tarfile.open(path, mode) as archive:
        for member in archive.getmembers():
            name = normalize_member(member.name)
            if name is None:
                invalid += 1
                continue
            if member.isdir():
                kind = "dir"
            elif member.issym() or member.islnk():
                kind = "symlink"
            elif member.isfile():
                kind = "file"
            else:
                kind = "other"
            members.append(
                {
                    "name": name,
                    "kind": kind,
                    "len": None if kind == "dir" else member.size,
                    "modified": member.mtime,
                    "mode": member.mode & 0o777 if member.mode is not None else None,
                }
            )
print(json.dumps({"members": members, "invalid": invalid}))
