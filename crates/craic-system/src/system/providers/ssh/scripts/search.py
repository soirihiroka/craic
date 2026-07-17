import json
import os
import re
import sys

root, query = sys.argv[1], sys.argv[2]
skip = set(json.loads(sys.argv[3]))
case_sensitive = sys.argv[4] == "1"
whole_word = sys.argv[5] == "1"
is_regex = sys.argv[6] == "1"
max_results = int(sys.argv[7])
max_file_bytes = int(sys.argv[8])

pattern = query if is_regex else re.escape(query)
if whole_word:
    pattern = r"\b(?:" + pattern + r")\b"

flags = re.MULTILINE | re.DOTALL
if not case_sensitive:
    flags |= re.IGNORECASE
rx = re.compile(pattern, flags)

text_matches = []
file_name_matches = []
limited = False


def result_count():
    return len(text_matches) + len(file_name_matches)


def finish():
    print(
        json.dumps(
            {
                "text_matches": text_matches,
                "file_name_matches": file_name_matches,
                "limited": limited,
            }
        )
    )


def preview_for_match(text, start, end):
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end == -1:
        line_end = len(text)
    return text[line_start:line_end].strip().replace("\r", " ").replace("\n", " ")[:180]


def has_nonempty_match(text):
    for found in rx.finditer(text):
        if found.start() != found.end():
            return True
    return False


for base, dirs, files in os.walk(root):
    dirs[:] = [d for d in dirs if d not in skip]
    for name in files:
        if name in skip:
            continue
        path = os.path.join(base, name)
        if has_nonempty_match(name):
            if result_count() >= max_results:
                limited = True
                finish()
                raise SystemExit(0)
            file_name_matches.append(path)
            if result_count() >= max_results:
                limited = True
                finish()
                raise SystemExit(0)
        try:
            if os.path.getsize(path) > max_file_bytes:
                continue
            data = open(path, "rb").read()
        except OSError:
            continue
        if b"\0" in data:
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            continue
        for found in rx.finditer(text):
            if found.start() == found.end():
                continue
            if result_count() >= max_results:
                limited = True
                finish()
                raise SystemExit(0)
            text_matches.append(
                {
                    "path": path,
                    "line_number": text.count("\n", 0, found.start()) + 1,
                    "start": found.start(),
                    "end": found.end(),
                    "line_text": preview_for_match(text, found.start(), found.end()),
                }
            )
            if result_count() >= max_results:
                limited = True
                finish()
                raise SystemExit(0)

finish()
