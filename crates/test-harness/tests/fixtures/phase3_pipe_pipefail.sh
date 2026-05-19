set -o pipefail
false | true
echo "fail=$?"
true | true
echo "ok=$?"
