exec 4>&1
echo "to original stdout via fd 4" >&4
exec 4>&-
echo "after closing 4: still on stdout"
