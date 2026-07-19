bar=one
flow=two
flip=three

foo=ba
typeset -n foo
typeset -n fee=flow

echo "$foo"
echo "$fee"

typeset -n fee=flip
echo "$fee"
typeset -n

typeset +n foo=othe
echo "$foo"
echo "bar=$bar"

foo=ba
typeset -n foo
declare foo=two
echo "after-declare:$foo/$bar"
declare -n foo=baz
echo "after-retarget:${foo-unset}/${bar-unset}/${baz-unset}"
declare -p foo ba

first=alpha
second=beta
typeset -n iter=first
for iter in first second; do
  echo "loop:${!iter}:$iter"
done
echo "loop-final:${!iter}:$iter"

typeset -n roref=first
readonly roref
declare -p roref first
