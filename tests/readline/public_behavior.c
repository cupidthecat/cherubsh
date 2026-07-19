#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <readline/readline.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int probe_command(int count, int key)
{
    return count + key;
}

static int has_name(const char **names, const char *wanted)
{
    size_t index;

    for (index = 0; names && names[index]; index++)
        if (strcmp(names[index], wanted) == 0)
            return 1;
    return 0;
}

int main(void)
{
    Keymap map;
    Keymap print_map;
    char raw_sequence[] = {24, 'z', 0};
    char raw_bound[] = {24, 'q', 0};
    char raw_macro[] = {24, 'm', 0};
    char bind_line[] = "\"\\C-xq\": probe-command";
    char macro_line[] = "\"\\C-xm\": \"macro text\"";
    char state_line[] = "saved line";
    char trimmed_sequence[] = {'u', '1', '2', 'x', 0};
    char argument_only[] = {'u', '1', '2', 0};
    char template[] = "/tmp/cherubsh-inputrc-XXXXXX";
    struct readline_state state;
    rl_command_func_t *function;
    const char **names;
    int descriptor;
    int kind = -1;
    int old_keyboard;
    int current_keyboard;
    int timeout_status;
    int input_pipe[2];
    int saved_stdin;
    char *timed_line;
    unsigned int seconds = 77;
    unsigned int microseconds = 88;

    rl_initialize();
    map = rl_make_bare_keymap();
    rl_set_keymap(map);
    printf("bind=%d\n", rl_bind_keyseq_in_map("\\C-xz", probe_command, map));
    function = rl_function_of_keyseq(raw_sequence, map, &kind);
    printf("nested=%d,%d\n", function == probe_command, kind);
    printf("already=%d\n", rl_bind_keyseq_if_unbound_in_map("\\C-xz", rl_abort, map));
    printf("new=%d\n", rl_bind_keyseq_if_unbound_in_map("\\C-xn", probe_command, map));

    rl_macro_bind("\\C-xm", "macro text", map);
    function = rl_function_of_keyseq(raw_macro, map, &kind);
    printf("macro=%d,%d\n", function && strcmp((char *)function, "macro text") == 0, kind);

    rl_add_defun("probe-command", probe_command, -1);
    names = rl_funmap_names();
    printf("funmap=%d,%d\n", rl_named_function("probe-command") == probe_command,
           has_name(names, "probe-command"));
    free((void *)names);

    print_map = rl_make_bare_keymap();
    rl_bind_key_in_map('u', rl_universal_argument, print_map);
    rl_bind_key_in_map('x', probe_command, print_map);
    printf("trim=%d,%d\n",
           rl_trim_arg_from_keyseq(trimmed_sequence, 4, print_map),
           rl_trim_arg_from_keyseq(argument_only, 3, print_map));
    rl_print_keybinding("probe-command", print_map, 1);
    rl_print_keybinding("abort", print_map, 1);
    rl_print_keybinding("probe-command", print_map, 0);

    printf("parse=%d,%d\n", rl_parse_and_bind(bind_line), rl_parse_and_bind(macro_line));
    function = rl_function_of_keyseq(raw_bound, map, &kind);
    printf("parsed-function=%d,%d\n", function == probe_command, kind);
    function = rl_function_of_keyseq(raw_macro, map, &kind);
    printf("parsed-macro=%d,%d\n", function && strcmp((char *)function, "macro text") == 0,
           kind);

    descriptor = mkstemp(template);
    if (descriptor < 0)
        return 2;
    dprintf(descriptor, "$if probe\nset completion-query-items 42\n$else\n"
                        "set completion-query-items 7\n$endif\n");
    close(descriptor);
    rl_readline_name = "probe";
    printf("inputrc=%d\n", rl_read_init_file(template));
    unlink(template);
    printf("variable=%s\n", rl_variable_value("completion-query-items"));

    rl_replace_line(state_line, 0);
    rl_point = 3;
    rl_mark = 6;
    printf("save-state=%d\n", rl_save_state(&state));
    rl_point = 1;
    rl_mark = 2;
    printf("restore-state=%d\n", rl_restore_state(&state));
    printf("state=%s,%d,%d\n", rl_line_buffer, rl_point, rl_mark);

    rl_activate_mark();
    printf("mark=%d\n", rl_mark_active_p());
    rl_deactivate_mark();
    printf("unmark=%d\n", rl_mark_active_p());

    old_keyboard = rl_set_keyboard_input_timeout(250000);
    current_keyboard = rl_set_keyboard_input_timeout(-1);
    printf("keyboard=%d,%d\n", old_keyboard, current_keyboard);
    rl_set_timeout(1, 1500000);
    timeout_status = rl_timeout_remaining(&seconds, &microseconds);
    printf("timeout=%d,%u,%u\n", timeout_status, seconds, microseconds);

    if (pipe(input_pipe) != 0)
        return 3;
    saved_stdin = dup(STDIN_FILENO);
    if (saved_stdin < 0 || dup2(input_pipe[0], STDIN_FILENO) < 0)
        return 4;
    close(input_pipe[0]);
    clearerr(stdin);
    rl_instream = stdin;
    rl_set_timeout(0, 20000);
    errno = 0;
    timed_line = readline("");
    printf("timeout-read=%d,%d,%d\n", timed_line == NULL,
           (rl_readline_state & RL_STATE_TIMEOUT) != 0, errno);
    free(timed_line);
    dup2(saved_stdin, STDIN_FILENO);
    close(saved_stdin);
    close(input_pipe[1]);

    printf("unbind=%d\n", rl_unbind_command_in_map("probe-command", map));
    function = rl_function_of_keyseq(raw_sequence, map, &kind);
    printf("unbound=%d\n", function == NULL);
    rl_free_keymap(map);
    rl_free_keymap(print_map);
    return 0;
}
