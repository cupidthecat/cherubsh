#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <readline/history.h>
#include <readline/readline.h>

static char *completion_generator(const char *text, int state) {
    static const char *values[] = {"source", "sort", NULL};

    (void)text;
    if (state < 0 || values[state] == NULL) {
        return NULL;
    }
    return strdup(values[state]);
}

static char **attempted_completion(const char *text, int start, int end) {
    (void)start;
    (void)end;
    return rl_completion_matches(text, completion_generator);
}

int main(void) {
    char *line;

    rl_attempted_completion_function = attempted_completion;
    while ((line = readline("rl> ")) != NULL) {
        if (*line != '\0') {
            add_history(line);
        }
        printf("line=<%s>\n", line);
        free(line);
    }
    return 0;
}
