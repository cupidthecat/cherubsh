#include <stdio.h>
#include <stdlib.h>
#include <readline/readline.h>

static int replace_line(int count, int key)
{
    (void)count;
    (void)key;
    rl_replace_line("custom binding", 0);
    rl_point = rl_end;
    return 0;
}

int main(void)
{
    char *line;
    Keymap bare;
    Keymap original;

    rl_initialize();
    if (rl_add_defun("replace-line", replace_line, -1) != 0)
        return 2;
    if (rl_bind_keyseq("\\C-xq", replace_line) != 0)
        return 3;
    if (rl_macro_bind("\\C-xm", "macro text", rl_get_keymap()) != 0)
        return 4;
    line = readline("custom> ");
    if (line == NULL)
        return 5;
    printf("line=%s\n", line);
    free(line);
    line = readline("macro> ");
    if (line == NULL)
        return 6;
    printf("line=%s\n", line);
    free(line);
    original = rl_get_keymap();
    bare = rl_make_bare_keymap();
    if (bare == NULL)
        return 7;
    if (rl_bind_key_in_map('\n', rl_newline, bare) != 0 ||
        rl_bind_key_in_map('\r', rl_newline, bare) != 0 ||
        rl_bind_key_in_map('r', replace_line, bare) != 0)
        return 8;
    rl_set_keymap(bare);
    line = readline("bare> ");
    if (line == NULL)
        return 9;
    printf("line=%s\n", line);
    free(line);
    rl_set_keymap(original);
    rl_free_keymap(bare);
    return 0;
}
