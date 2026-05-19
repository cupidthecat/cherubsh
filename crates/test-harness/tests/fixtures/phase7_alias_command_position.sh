shopt -s expand_aliases

alias e=echo
< /dev/null e redir
a=true e assign
eval 'a=true e eval'

alias comment=#
comment this should be ignored
echo after
