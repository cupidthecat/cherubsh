tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
echo first >"$tmp"
set -C
# >| should override noclobber and overwrite
echo overwrite >|"$tmp"
cat "$tmp"
# >> should append even under noclobber
echo append >>"$tmp"
cat "$tmp"
