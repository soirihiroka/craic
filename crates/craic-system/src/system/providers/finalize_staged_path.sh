src=$1
dst=$2

mv -n -T -- "$src" "$dst" || exit 1
if [ -e "$src" ] || [ -L "$src" ]; then
  printf 'CRAIC-ERROR\talready-exists\t%s\n' "$dst" >&2
  printf '%s already exists.\n' "$dst" >&2
  exit 17
fi
