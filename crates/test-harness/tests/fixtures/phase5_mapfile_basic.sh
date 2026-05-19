printf 'alpha\nbeta\ngamma\n' | { mapfile -t lines; declare -p lines; }
