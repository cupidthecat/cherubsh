arr=(alpha beta gamma delta)
echo "${arr[0]}"
echo "${arr[2]}"
echo "len=${#arr[@]}"
echo "slice=${arr[@]:1:2}"
arr[5]=epsilon
echo "sparse=${arr[5]}"
echo "indices=${!arr[@]}"
