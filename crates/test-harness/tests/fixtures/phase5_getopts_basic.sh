set -- -a -b foo ba
while getopts "ab:" opt; do
  case $opt in
    a) echo "got a" ;;
    b) echo "got b=$OPTARG" ;;
  esac
done
shift $((OPTIND - 1))
echo "rest: $*"
