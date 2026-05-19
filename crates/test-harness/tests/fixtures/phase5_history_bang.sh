set +o histexpand
echo "no expand: !"
set -o histexpand 2>/dev/null || true
# !! and !N are interactive-history features; non-interactive shells should
# treat them as literal text. Both bash and cherubsh must agree.
printf '%s\n' "literal !! and !1 stay raw"
