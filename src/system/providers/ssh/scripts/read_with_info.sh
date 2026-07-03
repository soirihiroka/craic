path=$1
max_bytes=$2

if [ -d "$path" ]; then
  kind=dir
elif [ -f "$path" ]; then
  kind=file
elif [ -L "$path" ]; then
  kind=symlink
else
  kind=other
fi

metadata=$(stat -Lc '%s %Y' -- "$path") || exit 1
set -- $metadata
len=$1
mtime=$2

if [ -w "$path" ]; then
  readonly=0
else
  readonly=1
fi

if [ -x "$path" ]; then
  executable=1
else
  executable=0
fi

readable=0
if [ "$kind" = file ]; then
  if [ -z "$max_bytes" ] || [ "$len" -le "$max_bytes" ]; then
    readable=1
  fi
fi

printf 'CRAIC-FILE-READ\t%s\t%s\t%s\t%s\t%s\t%s\n' "$kind" "$len" "$mtime" "$readonly" "$executable" "$readable"
if [ "$readable" = 1 ]; then
  cat -- "$path"
fi
