host=$1
login=$2
shift 2

gh_cmd=$(command -v gh 2>/dev/null || {
  if [ -x /home/linuxbrew/.linuxbrew/bin/gh ]; then
    printf '%s\n' /home/linuxbrew/.linuxbrew/bin/gh
  elif [ -x "$HOME/.local/bin/gh" ]; then
    printf '%s\n' "$HOME/.local/bin/gh"
  else
    printf '%s\n' gh
  fi
})

token=$("$gh_cmd" auth token --hostname "$host" --user "$login") || exit $?
export GH_TOKEN="$token"
export GH_ENTERPRISE_TOKEN="$token"
export GH_HOST="$host"
exec "$gh_cmd" "$@"
