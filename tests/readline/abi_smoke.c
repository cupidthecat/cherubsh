#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <readline/history.h>
#include <readline/readline.h>

int main(void) {
    int search_result;
    int search_position;
    char *expanded = NULL;
    char *argument = NULL;
    char *home = NULL;
    HIST_ENTRY *entry = NULL;

    clear_history();
    using_history();
    add_history("echo one");
    add_history("echo two");
    printf("version=%x/%s\n", rl_readline_version, rl_library_version);
    printf("history=%d,%d,%s\n", history_base, history_length, history_get(1)->line);

    history_set_pos(history_length);
    search_result = history_search("two", -1);
    search_position = where_history();
    printf("search=%d,%d\n", search_result, search_position);
    printf("expand=%d", history_expand("!!", &expanded));
    printf(",%s\n", expanded);
    free(expanded);

    argument = history_arg_extract(1, 1, "printf '%s' value");
    printf("argument=%s\n", argument);
    free(argument);

    rl_initialize();
    rl_replace_line("alpha beta", 1);
    rl_point = 5;
    rl_insert_text("-");
    rl_delete_text(0, 1);
    printf("line=%s\n", rl_line_buffer);
    printf("keymap=%s\n", rl_get_keymap() != NULL ? "yes" : "no");

    home = tilde_expand_word("~/sample");
    printf("tilde=%s\n", home);
    free(home);

    entry = remove_history(0);
    printf("removed=%s,%d\n", entry->line, history_length);
    free_history_entry(entry);
    clear_history();
    return 0;
}
