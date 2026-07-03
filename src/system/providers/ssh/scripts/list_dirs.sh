for path do
  printf 'CRAIC-DIR\037%s\n' "$path"
  find "$path" -mindepth 1 -maxdepth 1 -printf 'CRAIC-ENTRY\037%f\037%p\037%y\037%s\037%T@\037%m\n' 2>/dev/null || true
done
