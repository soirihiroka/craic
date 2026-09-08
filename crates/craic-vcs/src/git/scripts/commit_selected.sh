git_timezone=$1

if [ -n "$git_timezone" ]; then
  export TZ="$git_timezone"
fi

git commit -F -
