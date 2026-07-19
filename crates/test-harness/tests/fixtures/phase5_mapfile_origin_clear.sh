arr[0]=old
arr[3]=keep
printf 'a\nb\n' | mapfile -t ar
declare -p ar
arr[0]=old
arr[3]=keep
printf 'x\n' | mapfile -t -O 2 ar
declare -p ar
