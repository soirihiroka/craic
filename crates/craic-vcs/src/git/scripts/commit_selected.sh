git_date=$1

if [ -n "$git_date" ]; then
  export GIT_AUTHOR_DATE="$git_date"
  export GIT_COMMITTER_DATE="$git_date"
fi

git commit -F -
