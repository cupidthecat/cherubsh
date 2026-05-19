x=outer
f() {
  local x=inner
  echo "in=$x"
}
f
echo "out=$x"
