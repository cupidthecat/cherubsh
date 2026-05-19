{ echo before; /bin/true; echo after; } | cat

f()
{
    echo f-before
    /bin/false
    echo f-after:$?
}
f | cat

for i in one
do
    /bin/true
    echo loop:$i
done | cat
