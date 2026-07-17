import base64
import json
import subprocess
import sys


def git_text(args, allow_fail=False):
    proc = subprocess.run(["git", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if proc.returncode != 0:
        if allow_fail:
            return ""
        sys.stderr.write(proc.stderr.decode("utf-8", "replace"))
        raise SystemExit(proc.returncode)
    return proc.stdout.decode("utf-8", "replace").rstrip("\n")


def b64(value):
    return base64.b64encode(value.encode("utf-8", "replace")).decode("ascii")


def stats_for(hash_value):
    insertions = 0
    deletions = 0
    output = git_text(["show", "--numstat", "--format=", hash_value], allow_fail=True)
    for line in output.splitlines():
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        if fields[0].isdigit():
            insertions += int(fields[0])
        if fields[1].isdigit():
            deletions += int(fields[1])
    return insertions, deletions


def tags_for(hash_value):
    return [
        tag
        for tag in git_text(["tag", "--points-at", hash_value], allow_fail=True).splitlines()
        if tag
    ]


def row_for(hash_value):
    insertions, deletions = stats_for(hash_value)
    return {
        "hash": hash_value,
        "short_hash": git_text(["show", "-s", "--format=%h", hash_value], allow_fail=True),
        "author_b64": b64(git_text(["show", "-s", "--format=%an", hash_value], allow_fail=True)),
        "author_email_b64": b64(
            git_text(["show", "-s", "--format=%ae", hash_value], allow_fail=True)
        ),
        "subject_b64": b64(git_text(["show", "-s", "--format=%s", hash_value], allow_fail=True)),
        "timestamp": int(git_text(["show", "-s", "--format=%ct", hash_value], allow_fail=True) or 0),
        "insertions": insertions,
        "deletions": deletions,
        "tags_b64": [b64(tag) for tag in tags_for(hash_value)],
    }


def matches_query(hash_value, query):
    if not query:
        return True
    haystack = "\n".join(
        [
            hash_value,
            git_text(["show", "-s", "--format=%h", hash_value], allow_fail=True),
            git_text(["show", "-s", "--format=%B", hash_value], allow_fail=True),
            git_text(["show", "-s", "--format=%an", hash_value], allow_fail=True),
            git_text(["show", "-s", "--format=%ae", hash_value], allow_fail=True),
            "\n".join(tags_for(hash_value)),
        ]
    )
    return query in haystack.casefold()


request = json.load(sys.stdin)
after = request.get("after") or ""
limit = int(request.get("limit") or 0)
query = (request.get("query") or "").casefold()
fetch_limit = limit + 1

hashes = []
seen_after = not after
for hash_value in git_text(["rev-list", "HEAD"], allow_fail=True).splitlines():
    if not seen_after:
        if hash_value == after:
            seen_after = True
        continue
    if not matches_query(hash_value, query):
        continue
    hashes.append(hash_value)
    if len(hashes) >= fetch_limit:
        break

print(
    json.dumps(
        {
            "commits": [row_for(hash_value) for hash_value in hashes[:limit]],
            "has_more": len(hashes) > limit,
        }
    )
)
