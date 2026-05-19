arr[0]=old
arr[3]=keep
printf 'a\nb\n' | mapfile -t arr
declare -p arr
arr[0]=old
arr[3]=keep
printf 'x\n' | mapfile -t -O 2 arr
declare -p arr
