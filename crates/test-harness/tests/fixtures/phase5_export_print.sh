FOO=bar
export FOO
echo "$FOO"
unset FOO
echo "after=${FOO-unset}"
