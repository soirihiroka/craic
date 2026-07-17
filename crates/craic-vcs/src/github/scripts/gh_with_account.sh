gh_cmd=$1
host=$2
login=$3
shift 3

token=$("$gh_cmd" auth token --hostname "$host" --user "$login") || exit $?
export GH_TOKEN="$token"
export GH_ENTERPRISE_TOKEN="$token"
export GH_HOST="$host"
exec "$gh_cmd" "$@"
