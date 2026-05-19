set -- a b c
echo "$#: $1 $2 $3"
shift
echo "after-shift: $#: $1 $2"
