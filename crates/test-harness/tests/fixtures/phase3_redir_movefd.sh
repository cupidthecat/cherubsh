echo "stdout"
echo "stderr" >&2
exec 3>&1
exec 1>&2
echo "now stderr"
exec 1>&3
exec 3>&-
echo "back to stdout"
