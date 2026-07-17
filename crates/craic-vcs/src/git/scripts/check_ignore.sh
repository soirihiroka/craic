cd "$1" || exit 2
git check-ignore --stdin -z
status=$?
[ "$status" -eq 0 ] || [ "$status" -eq 1 ] || exit "$status"
exit 0
