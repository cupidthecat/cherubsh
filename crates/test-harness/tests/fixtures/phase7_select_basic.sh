PS3='pick> '
select item in red blue; do
  echo "reply=$REPLY item=$item"
  break
done <<< 2
