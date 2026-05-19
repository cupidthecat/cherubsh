got=0
trap 'got=$((got+1)); echo "usr1#${got}"' USR1
kill -USR1 $$
kill -USR1 $$
echo "total=${got}"
