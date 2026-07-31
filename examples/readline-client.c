#include <stdio.h>
#include <stdlib.h>

#include <readline/history.h>
#include <readline/readline.h>

int main(void) {
    char *name = readline("name> ");
    if (name == NULL) {
        return 0;
    }
    if (*name != '\0') {
        add_history(name);
    }
    printf("hello %s\n", name);
    free(name);
    return 0;
}
