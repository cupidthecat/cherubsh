#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <readline/history.h>
#include <readline/readline.h>
#include <readline/tilde.h>

static char *completion_generator(const char *text, int state)
{
    static const char *values[] = {"alpha", "alpine", NULL};

    (void)text;
    if (state < 0 || values[state] == NULL)
        return NULL;
    return strdup(values[state]);
}

static void free_matches(char **matches)
{
    size_t index;

    if (matches == NULL)
        return;
    for (index = 0; matches[index] != NULL; index++)
        free(matches[index]);
    free(matches);
}

int main(void)
{
    char *expanded = NULL;
    char *argument;
    char *tilde;
    char **matches;
    HIST_ENTRY **removed;
    int history_status;
    size_t removed_count = 0;

    using_history();
    clear_history();
    add_history("printf first second");

    history_status = history_expand("!!", &expanded);
    printf("history-expand=%d,%s\n", history_status, expanded);
    free(expanded);

    argument = history_arg_extract(1, 2, "printf first second third");
    printf("history-arg=%s\n", argument);
    free(argument);

    setenv("HOME", "/tmp/readline-parity-home", 1);
    tilde = tilde_expand_word("~/file");
    printf("tilde=%s\n", tilde);
    free(tilde);

    matches = rl_completion_matches("al", completion_generator);
    printf("completion=%s,%s,%s\n", matches[0], matches[1], matches[2]);
    free_matches(matches);

    add_history("remove one");
    add_history("remove two");
    removed = remove_history_range(1, 2);
    if (removed != NULL) {
        while (removed[removed_count] != NULL) {
            free_history_entry(removed[removed_count]);
            removed_count++;
        }
        free(removed);
    }
    printf("removed=%zu,remaining=%d\n", removed_count, history_length);
    clear_history();
    return 0;
}
