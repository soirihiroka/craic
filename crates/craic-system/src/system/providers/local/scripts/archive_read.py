import re
import sys
import tarfile
import zipfile

fmt, path, member_name, max_bytes_raw = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
max_bytes = int(max_bytes_raw)
drive_prefix = re.compile(r"^[A-Za-z]:")


def safe_member(raw):
    if not raw or raw.startswith("/") or raw.startswith("\\") or "\\" in raw:
        return False
    if drive_prefix.match(raw):
        return False
    return not any(part in ("", ".", "..") or drive_prefix.match(part) for part in raw.split("/"))


if not safe_member(member_name):
    raise SystemExit("unsafe archive member name")

if fmt == "zip":
    with zipfile.ZipFile(path) as archive:
        info = archive.getinfo(member_name)
        if info.is_dir():
            raise SystemExit("archive member is not a file")
        if max_bytes >= 0 and info.file_size > max_bytes:
            raise SystemExit("archive member exceeds read limit")
        with archive.open(info) as member:
            data = member.read()
else:
    mode = {"tar": "r:", "tar.gz": "r:gz", "tar.xz": "r:xz", "tar.bz2": "r:bz2"}[
        fmt
    ]
    with tarfile.open(path, mode) as archive:
        member_info = archive.getmember(member_name)
        if not member_info.isfile():
            raise SystemExit("archive member is not a file")
        if max_bytes >= 0 and member_info.size > max_bytes:
            raise SystemExit("archive member exceeds read limit")
        member = archive.extractfile(member_info)
        if member is None:
            raise SystemExit("archive member is not a file")
        data = member.read()

if max_bytes >= 0 and len(data) > max_bytes:
    raise SystemExit("archive member exceeds read limit")
sys.stdout.buffer.write(data)
