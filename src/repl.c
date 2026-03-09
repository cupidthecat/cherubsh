#include "cupid/repl.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "cupid/history.h"
#include "cupid/line_edit.h"
#include "cupid/prompt.h"
#include "cupid/shell.h"
#include "cupid/signal.h"
#include "cupid/vars.h"

static int run_interactive(struct cupid_shell *shell) {
    int editor_ok;

    cupid_history_init();
    cupid_history_load();
    editor_ok = (cupid_line_edit_init() == 0) ? 1 : 0;

    if (cupid_vars_get(shell, "PS1") == NULL) {
        cupid_vars_set(shell, "PS1", "\\u@\\h:\\w\\$ ");
    }
    if (cupid_vars_get(shell, "PS2") == NULL) {
        cupid_vars_set(shell, "PS2", "> ");
    }

    for (;;) {
        char *line;

        {
            const char *pc = cupid_vars_get(shell, "PROMPT_COMMAND");
            if (pc != NULL && pc[0] != '\0') {
                char *cmd = strdup(pc);
                if (cmd != NULL) {
                    (void)cupid_shell_eval_line(shell, cmd, 0);
                    free(cmd);
                }
            }
        }

        if (editor_ok) {
            const char *ps1 = cupid_vars_get(shell, "PS1");
            char *prompt = cupid_prompt_expand(shell, ps1);
            line = cupid_line_read(prompt, shell);
            free(prompt);
        } else {
            const char *ps1 = cupid_vars_get(shell, "PS1");
            char *prompt = cupid_prompt_expand(shell, ps1);
            char *buf = NULL;
            size_t cap = 0;
            ssize_t nread;

            if (prompt != NULL) {
                fputs(prompt, stdout);
                fflush(stdout);
            }
            free(prompt);

            cupid_signal_set_prompt_mode(1);
            cupid_signal_clear_interrupt();
            nread = getline(&buf, &cap, stdin);
            cupid_signal_set_prompt_mode(0);

            if (nread < 0) {
                if (errno == EINTR) {
                    clearerr(stdin);
                    shell->last_status = 130;
                    free(buf);
                    continue;
                }
                free(buf);
                line = NULL;
            } else {
                if (nread > 0 && buf[nread - 1] == '\n') buf[nread - 1] = '\0';
                line = buf;
            }
        }

        if (line == NULL) {
            int status = shell->last_status;
            cupid_history_cleanup();
            if (editor_ok) cupid_line_edit_cleanup();
            cupid_shell_run_exit_trap(shell);
            cupid_shell_destroy(shell);
            return status;
        }

        if (line[0] == '\0') {
            free(line);
            continue;
        }

        cupid_history_add(line);
        (void)cupid_shell_eval_line(shell, line, 1);
        free(line);

        if (shell->should_exit) {
            int code = shell->exit_code;
            cupid_history_cleanup();
            if (editor_ok) cupid_line_edit_cleanup();
            cupid_shell_run_exit_trap(shell);
            cupid_shell_destroy(shell);
            return code;
        }
    }
}

static int run_piped(struct cupid_shell *shell) {
    char *line = NULL;
    size_t cap = 0;

    cupid_signal_setup();
    (void)setvbuf(stdin, NULL, _IONBF, 0);

    for (;;) {
        ssize_t nread;

        cupid_signal_set_prompt_mode(1);
        cupid_signal_clear_interrupt();
        nread = getline(&line, &cap, stdin);
        cupid_signal_set_prompt_mode(0);

        if (nread < 0) {
            if (errno == EINTR) {
                clearerr(stdin);
                shell->last_status = 130;
                continue;
            }
            free(line);
            {
                int status = shell->last_status;
                cupid_shell_run_exit_trap(shell);
                cupid_shell_destroy(shell);
                return status;
            }
        }

        if (nread > 0 && line[nread - 1] == '\n') line[nread - 1] = '\0';
        if (line[0] == '\0') continue;

        (void)cupid_shell_eval_line(shell, line, 1);

        if (shell->should_exit) {
            free(line);
            {
                int code = shell->exit_code;
                cupid_shell_run_exit_trap(shell);
                cupid_shell_destroy(shell);
                return code;
            }
        }
    }
}

int cupid_repl_run(int posix_mode) {
    struct cupid_shell shell;
    cupid_shell_init(&shell);
    shell.mode = posix_mode ? CUPID_MODE_POSIX : CUPID_MODE_BASH;
    shell.arg0 = strdup("cupid");
    cupid_signal_setup();

    if (isatty(STDIN_FILENO)) {
        shell.is_interactive = 1;
        return run_interactive(&shell);
    }
    shell.is_interactive = 0;
    return run_piped(&shell);
}
