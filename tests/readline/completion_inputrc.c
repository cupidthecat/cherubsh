#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <readline/readline.h>

static char *generator(const char *text, int state)
{
    static const char *values[] = {"source", "sort", NULL};
    static const char *exact[] = {"source", NULL};
    const char **selected = strcmp(text, "source") == 0 ? exact : values;

    if (state < 0 || selected[state] == NULL)
        return NULL;
    return strdup(selected[state]);
}

static char **attempted_completion(const char *text, int start, int end)
{
    (void)start;
    (void)end;
    return rl_completion_matches(text, generator);
}

static int display_count;
static int display_length;

static void display_matches(char **matches, int len, int max)
{
    (void)matches;
    (void)max;
    display_count++;
    display_length = len;
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
    int normal_mode;
    int show_all_mode;
    int show_unmodified_mode;
    int repeat_mode;

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
            "$if term=xterm\nset comment-begin 43\n$endif\n"
            "set show-all-if-ambiguous 1\n"
            "set show-all-if-unmodified off\n"
            "set emacs-mode-string second\n"
            "$include %s\n",
            child);
    fclose(file);

    setenv("TERM", "xterm-256color", 1);
    rl_initialize();
    rl_readline_name = "probe";
    printf("inputrc=%d\n", rl_read_init_file(parent));
    snprintf(app_value, sizeof(app_value), "%s",
             rl_variable_value("completion-query-items"));
    snprintf(mode_value, sizeof(mode_value), "%s",
             rl_variable_value("completion-prefix-display-length"));
    snprintf(term_value, sizeof(term_value), "%s",
             rl_variable_value("comment-begin"));
    snprintf(include_value, sizeof(include_value), "%s",
             rl_variable_value("keyseq-timeout"));
    printf("conditions=%s,%s,%s,%s\n",
           app_value, mode_value, term_value, include_value);
    first_borrowed = rl_variable_value("comment-begin");
    second_borrowed = rl_variable_value("emacs-mode-string");
    printf("borrowed=%s,%s,%d\n", first_borrowed, second_borrowed,
           first_borrowed != second_borrowed);
    printf("completion-policy=%s,%s\n",
           rl_variable_value("show-all-if-ambiguous"),
           rl_variable_value("show-all-if-unmodified"));

    rl_last_func = rl_complete;
    repeat_mode = rl_completion_mode(rl_complete);
    rl_last_func = NULL;
    show_all_mode = rl_completion_mode(rl_complete);
    rl_variable_bind("show-all-if-ambiguous", "off");
    rl_variable_bind("show-all-if-unmodified", "on");
    show_unmodified_mode = rl_completion_mode(rl_complete);
    rl_variable_bind("show-all-if-unmodified", "off");
    normal_mode = rl_completion_mode(rl_complete);
    printf("completion-modes=%c,%c,%c,%c\n",
           normal_mode, show_all_mode, show_unmodified_mode, repeat_mode);
    rl_variable_bind("show-all-if-ambiguous", "");
    printf("boolean-empty=%s\n",
           rl_variable_value("show-all-if-ambiguous"));
    rl_variable_bind("show-all-if-ambiguous", "ON");
    printf("boolean-on=%s\n",
           rl_variable_value("show-all-if-ambiguous"));
    rl_variable_bind("show-all-if-ambiguous", "1");
    printf("boolean-one=%s\n",
           rl_variable_value("show-all-if-ambiguous"));
    rl_variable_bind("show-all-if-ambiguous", "yes");
    printf("boolean-other=%s\n",
           rl_variable_value("show-all-if-ambiguous"));

    matches = rl_completion_matches("so", generator);
    printf("matches=%s,%s,%s\n", matches[0], matches[1], matches[2]);
    free_matches(matches);

    rl_attempted_completion_function = attempted_completion;
    rl_completion_display_matches_hook = display_matches;
    rl_replace_line("so", 0);
    rl_point = rl_end;
    rl_complete_internal('@');
    printf("unmodified=%s,%d,%d,%c\n", rl_line_buffer, display_count,
           display_length, rl_completion_type);

    display_count = 0;
    display_length = 0;
    rl_replace_line("s", 0);
    rl_point = rl_end;
    rl_complete_internal('!');
    printf("show-all=%s,%d,%d,%c\n", rl_line_buffer, display_count,
           display_length, rl_completion_type);

    display_count = 0;
    display_length = 0;
    rl_variable_bind("editing-mode", "emacs");
    rl_variable_bind("show-all-if-ambiguous", "off");
    rl_variable_bind("bell-style", "audible");
    rl_tty_set_echoing(1);
    rl_last_func = NULL;
    rl_replace_line("so", 0);
    rl_point = rl_end;
    rl_complete(1, '\t');
    rl_last_func = rl_complete;
    rl_complete(1, '\t');
    printf("repeated=%s,%d,%d,%c\n", rl_line_buffer, display_count,
           display_length, rl_completion_type);

    rl_last_func = NULL;
    rl_replace_line("source", 0);
    rl_point = rl_end;
    rl_complete_internal('\t');
    printf("exact=<%s>,%c\n", rl_line_buffer, rl_completion_type);

    unlink(parent);
    unlink(child);
    rmdir(directory);
    return 0;
}
