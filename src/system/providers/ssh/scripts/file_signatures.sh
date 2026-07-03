for path do
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then
    printf 'CRAIC-WATCH\037%s\037missing\n' "$path"
    continue
  fi

  if [ -d "$path" ]; then
    kind=dir
  elif [ -f "$path" ]; then
    kind=file
  elif [ -L "$path" ]; then
    kind=symlink
  else
    kind=other
  fi

  metadata=$(stat -Lc '%s %Y' -- "$path") || {
    printf 'CRAIC-WATCH\037%s\037missing\n' "$path"
    continue
  }
  len=${metadata%% *}
  modified=${metadata#* }
  printf 'CRAIC-WATCH\037%s\037present\037%s\037%s\037%s\n' "$path" "$kind" "$len" "$modified"
done
