exec {out}>&1
if (( out >= 10 )); then echo out-ok; else echo out-bad; fi
echo dup-ok >&$out
exec {out}>&-
echo after-close:${out-unset}
echo closed >&$out
echo close-status:$?

exec {in}<<EOF
one
two
EOF
read first <&$in
read second <&$in
echo heredoc:$first/$second

exec {fd[0]}</dev/null
exec {copy}<&${fd[0]}-
if (( fd[0] >= 10 && copy >= 10 )); then echo array-move-ok; else echo array-move-bad; fi
exec {copy}<&-

readonly ro=42
exec {ro}>/dev/null
echo readonly-status:$?

shopt -s varredir_close
: {tmp}<>/dev/null
echo varclose >&$tmp
echo varclose-status:$?
