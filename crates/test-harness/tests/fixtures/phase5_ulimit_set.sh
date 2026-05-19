cur=$(ulimit -Sn)
ulimit -Sn "$cur"
echo "n=$(ulimit -Sn)"
cur=$(ulimit -Sf)
ulimit -Sf "$cur"
echo "f=$(ulimit -Sf)"
