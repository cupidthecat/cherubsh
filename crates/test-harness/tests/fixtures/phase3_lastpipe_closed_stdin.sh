shopt -s lastpipe
exec 0<&-
echo x | read x
echo "x=$x"
unset x
echo x | cat | read x
echo "x=$x"
