for i in 1 2 3 4 5; do
  echo "i=$i"
  if [ "$i" = "3" ]; then
    break
  fi
done
echo done
