cb() { echo "cb idx=$1 line=$2"; }
printf 'a\nb\nc\nd\n' | mapfile -t -C cb -c 2 arr
echo "len=${#arr[@]}"
printf '%s\n' "${arr[@]}"
