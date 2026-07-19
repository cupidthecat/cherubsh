#include <stdio.h>

#include <readline/history.h>

int main(void) {
    clear_history();
    add_history("linked");
    printf("%s\n", history_get(history_base)->line);
    clear_history();
    return 0;
}
