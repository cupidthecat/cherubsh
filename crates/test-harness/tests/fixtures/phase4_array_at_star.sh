arr=(one "two three" four)
IFS=":"
echo "@:" "${arr[@]}"
echo "*:" "${arr[*]}"
for x in "${arr[@]}"; do
  echo "elem=$x"
done
for x in ${arr[*]}; do
  echo "split=$x"
done
