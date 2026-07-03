emit_workspace() {
  kind=$1
  source=$2
  path=$3
  printf '%s\t%s\t%s\n' "$kind" "$source" "$path"
}

resolve_path() {
  path=$1
  if [ "$path" = '~' ]; then
    printf '%s\n' "$HOME"
  else
    case "$path" in
      "~/"*) printf '%s/%s\n' "$HOME" "${path#\~/}" ;;
      *) printf '%s\n' "$path" ;;
    esac
  fi
}

while IFS='	' read -r kind raw_path; do
  [ -n "$kind" ] || continue
  path=$(resolve_path "$raw_path")
  case "$kind" in
    W) [ -d "$path" ] && emit_workspace W "$raw_path" "$path" ;;
    R)
      [ -d "$path" ] &&
        find "$path" -mindepth 1 -maxdepth 1 -type d -print |
          while IFS= read -r child; do
            emit_workspace R "$raw_path" "$child"
          done
      ;;
  esac
done
