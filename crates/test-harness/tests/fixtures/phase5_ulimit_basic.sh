type -t ulimit
ulimit
ulimit -n
ulimit -Sn
ulimit -Hn
ulimit -a | sed -n '/^file size/p;/^open files/p;/^pipe size/p'
