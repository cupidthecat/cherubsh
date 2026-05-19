test -z ""; echo $?
test -n abc; echo $?
test abc = abc; echo $?
test 3 -gt 2; echo $?
[ "x" != "y" ]; echo $?
