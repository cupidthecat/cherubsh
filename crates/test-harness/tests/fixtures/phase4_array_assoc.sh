declare -A m
m[red]=1
m[green]=2
m[blue]=3
echo "${m[red]}"
echo "${m[green]}"
echo "${m[blue]}"
echo "len=${#m[@]}"
for k in "${!m[@]}"; do
  echo "$k=${m[$k]}"
done | sort
