#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "cupid/repl.h"
#include "cupid/shell.h"

static const char *base_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash ? slash + 1 : path;
}

static int shebang_mode_for_script(const char *path, enum cupid_mode *mode_out) {
    FILE *f;
    char line[256];
    const char *p;
    const char *start;
    const char *end;
    char tok[128];
    const char *name;

    if (path == NULL || mode_out == NULL) return 0;
    f = fopen(path, "r");
    if (f == NULL) return 0;
    if (fgets(line, sizeof(line), f) == NULL) {
        fclose(f);
        return 0;
    }
    fclose(f);
    p = line;
    if (p[0] != '#' || p[1] != '!') return 0;
    p += 2;
    while (*p == ' ' || *p == '\t') p++;
    start = p;
    while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
    end = p;
    if (end <= start || (size_t)(end - start) >= sizeof(tok)) return 0;
    memcpy(tok, start, (size_t)(end - start));
    tok[end - start] = '\0';
    name = base_name(tok);
    if (strcmp(name, "env") == 0) {
        while (*p == ' ' || *p == '\t') p++;
        while (*p == '-') {
            while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
            while (*p == ' ' || *p == '\t') p++;
        }
        start = p;
        while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
        end = p;
        if (end <= start || (size_t)(end - start) >= sizeof(tok)) return 0;
        memcpy(tok, start, (size_t)(end - start));
        tok[end - start] = '\0';
        name = base_name(tok);
    }
    if (strcmp(name, "sh") == 0 || strcmp(name, "dash") == 0) {
        *mode_out = CUPID_MODE_POSIX;
        return 1;
    }
    if (strcmp(name, "bash") == 0 || strcmp(name, "zsh") == 0 ||
        strcmp(name, "ksh") == 0 || strcmp(name, "cupid") == 0) {
        *mode_out = CUPID_MODE_BASH;
        return 1;
    }
    return 0;
}

static int shell_eval_c_script(struct cupid_shell *shell, const char *script) {
    if (shell == NULL || script == NULL) return 1;
    shell->current_file = NULL;
    shell->lineno = 1;
    return cupid_shell_eval_text(shell, script, 1);
}

int main(int argc, char **argv) {
    int argi = 1;
    int posix_mode = 0;

    while (argi < argc) {
        if (strcmp(argv[argi], "--posix") == 0) {
            posix_mode = 1;
            argi++;
            continue;
        }
        if (strcmp(argv[argi], "-o") == 0 && argi + 1 < argc && strcmp(argv[argi + 1], "posix") == 0) {
            posix_mode = 1;
            argi += 2;
            continue;
        }
        break;
    }

    if (argi < argc && strcmp(argv[argi], "-c") == 0) {
        struct cupid_shell shell;
        int status;
        if (argi + 1 >= argc) {
            return 2;
        }
        cupid_shell_init(&shell);
        shell.is_dash_c = 1;
        if (posix_mode) {
            shell.mode = CUPID_MODE_POSIX;
        }
        if (argi + 2 < argc) {
            shell.arg0 = strdup(argv[argi + 2]);
        } else {
            shell.arg0 = strdup(argv[0]);
        }
        if (argi + 3 < argc) {
            int i;
            shell.params.count = (size_t)(argc - (argi + 3));
            shell.params.args = calloc(shell.params.count, sizeof(char *));
            for (i = argi + 3; i < argc; i++) {
                shell.params.args[i - (argi + 3)] = strdup(argv[i]);
            }
        }
        status = shell_eval_c_script(&shell, argv[argi + 1]);
        if (shell.should_exit) status = shell.exit_code;
        cupid_shell_run_exit_trap(&shell);
        cupid_shell_destroy(&shell);
        return status;
    }
    if (argi < argc) {
        struct cupid_shell shell;
        int status;
        cupid_shell_init(&shell);
        if (posix_mode) {
            shell.mode = CUPID_MODE_POSIX;
        } else {
            enum cupid_mode shebang_mode;
            if (shebang_mode_for_script(argv[argi], &shebang_mode)) {
                shell.mode = shebang_mode;
            }
        }
        shell.arg0 = strdup(argv[argi]);
        if (argi + 1 < argc) {
            int i;
            shell.params.count = (size_t)(argc - (argi + 1));
            shell.params.args = calloc(shell.params.count, sizeof(char *));
            for (i = argi + 1; i < argc; i++) shell.params.args[i - (argi + 1)] = strdup(argv[i]);
        }
        status = cupid_shell_eval_file(&shell, argv[argi]);
        if (shell.should_exit) {
            status = shell.exit_code;
        }
        cupid_shell_run_exit_trap(&shell);
        cupid_shell_destroy(&shell);
        return status;
    }
    return cupid_repl_run(posix_mode ? 1 : 0);
}
