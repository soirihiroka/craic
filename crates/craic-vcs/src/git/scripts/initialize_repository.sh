cd "$1" || exit 2

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf '%s\n' 'Workspace is already a Git repository.'
  exit 0
fi

git init
