set -- -a -b -c value -z extra
OPTERR=0
while getopts ":ab:c:" opt; do
  case "$opt" in
    a) echo "got a";;
    b) echo "got b=$OPTARG";;
    c) echo "got c=$OPTARG";;
    \?) echo "unknown=$OPTARG";;
    :) echo "missing arg for $OPTARG";;
  esac
done
echo "shift=$((OPTIND - 1))"
shift $((OPTIND - 1))
echo "remaining=$*"
