path=$1

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

printf '%s\t%s\t%s\t%s\t%s\n' "$kind" "$1" "$2" "$readonly" "$executable"
