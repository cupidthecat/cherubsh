count() {
  if [ "$1" -gt 0 ]; then
    echo "$1"
    count $(($1 - 1))
  fi
}
# fall back to manual recursion since arithmetic isn't done yet
walk() {
  echo "n=$1"
}
walk one
walk two
