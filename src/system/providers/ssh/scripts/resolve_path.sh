path=$1

if [ "$path" = '~' ]; then
  printf '%s\n' "$HOME"
else
  case "$path" in
    "~/"*) printf '%s/%s\n' "$HOME" "${path#\~/}" ;;
    *) printf '%s\n' "$path" ;;
  esac
fi
