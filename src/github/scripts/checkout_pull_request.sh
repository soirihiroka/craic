cd "$1" || exit 2

gh_cmd=$(command -v gh 2>/dev/null || {
  if [ -x /home/linuxbrew/.linuxbrew/bin/gh ]; then
    printf '%s\n' /home/linuxbrew/.linuxbrew/bin/gh
  elif [ -x "$HOME/.local/bin/gh" ]; then
    printf '%s\n' "$HOME/.local/bin/gh"
  else
    printf '%s\n' gh
  fi
})

"$gh_cmd" pr checkout "$2"
