tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
exec 3>"$tmp"
echo "via fd 3 line 1" >&3
echo "via fd 3 line 2" >&3
exec 3>&-
cat "$tmp"
