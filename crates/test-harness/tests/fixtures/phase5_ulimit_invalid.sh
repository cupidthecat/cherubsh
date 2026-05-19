ulimit -b >/dev/null 2>&1
echo "badopt=$?"
ulimit -n nope >/dev/null 2>&1
echo "badnum=$?"
{ ulimit -p 8; } >/dev/null 2>&1
echo "pipe-set=$?"
