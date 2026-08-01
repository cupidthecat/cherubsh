#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <readline/readline.h>

static char *generator(const char *text, int state)
{
    static const char *values[] = {"source", "sound", NULL};

    (void)text;
    if (state < 0 || values[state] == NULL)
        return NULL;
    return strdup(values[state]);
}

static void free_matches(char **matches)
{
    size_t index;

    for (index = 0; matches != NULL && matches[index] != NULL; index++)
        free(matches[index]);
    free(matches);
}

int main(void)
{
    char directory[] = "/tmp/readline-inputrc-XXXXXX";
    char parent[256];
    char child[256];
    FILE *file;
    char **matches;
    char app_value[16];
    char mode_value[16];
    char term_value[16];
    char include_value[16];
    const char *first_borrowed;
    const char *second_borrowed;

    if (mkdtemp(directory) == NULL)
        return 2;
    snprintf(parent, sizeof(parent), "%s/inputrc", directory);
    snprintf(child, sizeof(child), "%s/child.inputrc", directory);

    file = fopen(child, "w");
    if (file == NULL)
        return 3;
    fputs("set keyseq-timeout 44\n", file);
    fclose(file);

    file = fopen(parent, "w");
    if (file == NULL)
        return 4;
    fprintf(file,
            "set editing-mode vi\n"
            "$if probe\nset completion-query-items 41\n$endif\n"
            "$if mode=vi\nset completion-prefix-display-length 42\n$endif\n"
            "$if term=xterm\nset history-size 43\n$endif\n"
            "set comment-begin first\n"
            "set emacs-mode-string second\n"
            "$include %s\n",
            child);
    fclose(file);

    rl_initialize();
    rl_readline_name = "probe";
    setenv("TERM", "xterm-256color", 1);
    printf("inputrc=%d\n", rl_read_init_file(parent));
    snprintf(app_value, sizeof(app_value), "%s",
             rl_variable_value("completion-query-items"));
    snprintf(mode_value, sizeof(mode_value), "%s",
             rl_variable_value("completion-prefix-display-length"));
    snprintf(term_value, sizeof(term_value), "%s",
             rl_variable_value("history-size"));
    snprintf(include_value, sizeof(include_value), "%s",
             rl_variable_value("keyseq-timeout"));
    printf("conditions=%s,%s,%s,%s\n",
           app_value, mode_value, term_value, include_value);
    first_borrowed = rl_variable_value("comment-begin");
    second_borrowed = rl_variable_value("emacs-mode-string");
    printf("borrowed=%s,%s,%d\n", first_borrowed, second_borrowed,
           first_borrowed != second_borrowed);

    matches = rl_completion_matches("so", generator);
    printf("matches=%s,%s,%s\n", matches[0], matches[1], matches[2]);
    free_matches(matches);

    unlink(parent);
    unlink(child);
    rmdir(directory);
    return 0;
}
