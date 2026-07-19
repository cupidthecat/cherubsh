#include <stdio.h>
#include <stdlib.h>

#include <readline/history.h>
#include <readline/readline.h>

int main(void) {
    char *line;
    while ((line = readline("rl> ")) != NULL) {
        if (*line != '\0') {
            add_history(line);
        }
        printf("line=<%s>\n", line);
        free(line);
    }
    return 0;
}
