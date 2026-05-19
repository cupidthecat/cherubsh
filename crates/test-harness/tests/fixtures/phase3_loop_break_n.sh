for i in 1 2 3; do
  for j in a b c; do
    if [ "$j" = "b" ]; then
      break 2
    fi
    echo "$i$j"
  done
done
echo end
