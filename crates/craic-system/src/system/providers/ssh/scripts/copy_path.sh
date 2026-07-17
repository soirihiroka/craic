src=$1
dst=$2

if [ -e "$dst" ] || [ -L "$dst" ]; then
  printf 'CRAIC-ERROR\talready-exists\t%s\n' "$dst" >&2
  printf '%s already exists.\n' "$dst" >&2
  exit 17
fi

cp -a -- "$src" "$dst"
