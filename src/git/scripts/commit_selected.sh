cd "$1" || exit 2
shift

force_count=$1
shift
update_count=$1
shift

if git rev-parse --verify HEAD >/dev/null 2>&1; then
  git reset -- .
else
  git rm --cached -r --ignore-unmatch . >/dev/null 2>&1 || true
fi

while [ "$force_count" -gt 0 ]; do
  git update-index --force-remove -- "$1"
  shift
  force_count=$((force_count - 1))
done

if [ "$update_count" -gt 0 ]; then
  git update-index --add --remove --replace -- "$@"
fi

git commit -F -
