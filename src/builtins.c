#include "cupid/builtins.h"

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/times.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "cupid/arith.h"
#include "cupid/ast.h"
#include "cupid/expand.h"
#include "cupid/history.h"
#include "cupid/lexer.h"
#include "cupid/shell.h"
#include "cupid/vars.h"

extern char **environ;

/* ------------------------------------------------------------------ */
/*  Helpers                                                           */
/* ------------------------------------------------------------------ */

static int parse_exit_code(const char *s, int *out) {
    char *end = NULL;
    long v;
    errno = 0;
    v = strtol(s, &end, 10);
    if (errno != 0 || end == s || *end != '\0') {
        return -1;
    }
    *out = (int)(v & 0xff);
    return 0;
}

static int assignment_name_start(char c) {
    return isalpha((unsigned char)c) || c == '_';
}

static int assignment_name_char(char c) {
    return isalnum((unsigned char)c) || c == '_';
}

static int split_assignment_word_ext(const char *word, const char **name, size_t *name_len,
                                     const char **value, int *append_out) {
    const char *p;
    const char *lhs_end;
    int append = 0;

    if (word == NULL || !assignment_name_start(word[0])) return 0;
    p = word + 1;
    while (*p != '\0' && assignment_name_char(*p)) p++;
    if (*p == '[') {
        int depth = 0;
        while (*p != '\0') {
            if (*p == '[') depth++;
            else if (*p == ']') {
                depth--;
                if (depth == 0) {
                    p++;
                    break;
                }
            }
            p++;
        }
        if (depth != 0) return 0;
    }
    lhs_end = p;
    if (*p == '=') {
        /* plain assignment */
    } else if (*p == '+' && p[1] == '=') {
        append = 1;
        p++;
    } else {
        return 0;
    }
    if (name != NULL) *name = word;
    if (name_len != NULL) *name_len = (size_t)(lhs_end - word);
    if (value != NULL) *value = p + 1;
    if (append_out != NULL) *append_out = append;
    return 1;
}

static int apply_append_assignment_value(struct cupid_shell *shell, const char *name, const char *value) {
    const char *old;
    char *merged;
    int rc;

    if (shell == NULL || name == NULL || value == NULL) return 1;
    old = cupid_vars_get(shell, name);
    if (cupid_vars_is_integer(shell, name)) {
        const char *base = (old != NULL && old[0] != '\0') ? old : "0";
        size_t need = strlen(base) + strlen(value) + 4;
        merged = calloc(need, 1);
        if (merged == NULL) return 1;
        snprintf(merged, need, "%s+(%s)", base, value);
    } else {
        size_t old_len = (old != NULL) ? strlen(old) : 0;
        size_t need = old_len + strlen(value) + 1;
        merged = calloc(need, 1);
        if (merged == NULL) return 1;
        if (old_len > 0) memcpy(merged, old, old_len);
        memcpy(merged + old_len, value, strlen(value));
    }
    rc = cupid_vars_set(shell, name, merged);
    free(merged);
    return rc == 0 ? 0 : 1;
}

static int print_declared_function(struct cupid_shell *shell, const char *name);
static char *normalize_function_word_text(const char *text);

static int posix_special_builtin_error(struct cupid_shell *shell, int status) {
    if (shell != NULL &&
        shell->mode == CUPID_MODE_POSIX &&
        !shell->is_interactive &&
        !shell->suppress_special_builtin_fatal) {
        shell->should_exit = 1;
        shell->exit_code = status;
    }
    return status;
}

static int disabled_builtin_index(const struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return -1;
    for (i = 0; i < shell->disabled_builtins.count; i++) {
        if (strcmp(shell->disabled_builtins.entries[i].name, name) == 0) {
            return (int)i;
        }
    }
    return -1;
}

static int builtin_is_enabled(const struct cupid_shell *shell, const char *name) {
    if (!cupid_is_builtin(name)) return 0;
    return disabled_builtin_index(shell, name) < 0;
}

static int set_builtin_enabled(struct cupid_shell *shell, const char *name, int enabled) {
    struct cupid_alias *next;
    int idx;
    if (shell == NULL || name == NULL || !cupid_is_builtin(name)) return -1;
    idx = disabled_builtin_index(shell, name);
    if (enabled) {
        if (idx < 0) return 0;
        free(shell->disabled_builtins.entries[idx].name);
        free(shell->disabled_builtins.entries[idx].value);
        if ((size_t)idx + 1 < shell->disabled_builtins.count) {
            shell->disabled_builtins.entries[idx] =
                shell->disabled_builtins.entries[shell->disabled_builtins.count - 1];
        }
        shell->disabled_builtins.count--;
        return 0;
    }
    if (idx >= 0) return 0;
    next = realloc(shell->disabled_builtins.entries,
                   sizeof(*next) * (shell->disabled_builtins.count + 1));
    if (next == NULL) return -1;
    shell->disabled_builtins.entries = next;
    shell->disabled_builtins.entries[shell->disabled_builtins.count].name = strdup(name);
    if (shell->disabled_builtins.entries[shell->disabled_builtins.count].name == NULL) {
        return -1;
    }
    shell->disabled_builtins.entries[shell->disabled_builtins.count].value = NULL;
    shell->disabled_builtins.count++;
    return 0;
}

static int is_shell_keyword_name(const char *name) {
    static const char *keywords[] = {
        "if", "then", "else", "elif", "fi",
        "case", "esac", "for", "select", "while", "until",
        "do", "done", "in", "function", "time", "coproc", "{", "}", NULL
    };
    const char **kw;
    if (name == NULL) return 0;
    for (kw = keywords; *kw != NULL; kw++) {
        if (strcmp(*kw, name) == 0) return 1;
    }
    return 0;
}

static const char *const g_special_builtin_names[] = {
    ".", ":", "break", "continue", "eval", "exec", "exit", "export",
    "readonly", "return", "set", "shift", "source", "times", "trap", "unset", NULL
};

enum name_kind {
    NAME_NONE = 0,
    NAME_ALIAS,
    NAME_KEYWORD,
    NAME_FUNCTION,
    NAME_BUILTIN,
    NAME_FILE,
    NAME_HASHED
};

static enum name_kind resolve_name_kind(struct cupid_shell *shell, const char *name,
                                        int allow_alias, int allow_func, int force_path,
                                        const char **alias_val_out, char **path_out,
                                        int *hashed_out);
static int hash_table_set(struct cupid_shell *shell, const char *name, const char *path);
static void hash_table_clear(struct cupid_shell *shell);
static struct cupid_hash_entry *hash_table_get_mut(struct cupid_shell *shell, const char *name);

static struct cupid_hash_entry *hash_table_get_mut(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->hashes.count; i++) {
        if (strcmp(shell->hashes.entries[i].name, name) == 0) {
            return &shell->hashes.entries[i];
        }
    }
    return NULL;
}

static int hash_table_set(struct cupid_shell *shell, const char *name, const char *path) {
    struct cupid_hash_entry *entry;
    struct cupid_hash_entry *next;
    char *name_dup;
    char *path_dup;
    size_t nc;
    if (shell == NULL || name == NULL || path == NULL) return -1;
    entry = (struct cupid_hash_entry *)hash_table_get_mut(shell, name);
    if (entry != NULL) {
        path_dup = strdup(path);
        if (path_dup == NULL) return -1;
        free(entry->path);
        entry->path = path_dup;
        entry->hits = 0;
        return 0;
    }
    if (shell->hashes.count == shell->hashes.capacity) {
        nc = (shell->hashes.capacity == 0) ? 8 : shell->hashes.capacity * 2;
        next = realloc(shell->hashes.entries, sizeof(*next) * nc);
        if (next == NULL) return -1;
        shell->hashes.entries = next;
        shell->hashes.capacity = nc;
    }
    name_dup = strdup(name);
    path_dup = strdup(path);
    if (name_dup == NULL || path_dup == NULL) {
        free(name_dup);
        free(path_dup);
        return -1;
    }
    shell->hashes.entries[shell->hashes.count].name = name_dup;
    shell->hashes.entries[shell->hashes.count].path = path_dup;
    shell->hashes.entries[shell->hashes.count].hits = 0;
    shell->hashes.count++;
    return 0;
}

static void hash_table_clear(struct cupid_shell *shell) {
    size_t i;
    if (shell == NULL) return;
    for (i = 0; i < shell->hashes.count; i++) {
        free(shell->hashes.entries[i].name);
        free(shell->hashes.entries[i].path);
    }
    free(shell->hashes.entries);
    shell->hashes.entries = NULL;
    shell->hashes.count = 0;
    shell->hashes.capacity = 0;
}

static enum name_kind resolve_name_kind(struct cupid_shell *shell, const char *name,
                                        int allow_alias, int allow_func, int force_path,
                                        const char **alias_val_out, char **path_out,
                                        int *hashed_out) {
    if (alias_val_out != NULL) *alias_val_out = NULL;
    if (path_out != NULL) *path_out = NULL;
    if (hashed_out != NULL) *hashed_out = 0;
    if (name == NULL) return NAME_NONE;
    if (force_path) {
        if (path_out != NULL) *path_out = cupid_find_in_path(name);
        return (path_out != NULL && *path_out != NULL) ? NAME_FILE : NAME_NONE;
    }
    if (allow_alias && shell != NULL && shell->opt_expand_aliases) {
        const char *alias_val = cupid_alias_get(shell, name);
        if (alias_val != NULL) {
            if (alias_val_out != NULL) *alias_val_out = alias_val;
            return NAME_ALIAS;
        }
    }
    if (is_shell_keyword_name(name)) return NAME_KEYWORD;
    if (allow_func && shell != NULL && cupid_func_get(shell, name) != NULL) return NAME_FUNCTION;
    if (builtin_is_enabled(shell, name)) return NAME_BUILTIN;
    if (shell != NULL) {
        struct cupid_hash_entry *entry = hash_table_get_mut(shell, name);
        if (entry != NULL) {
            if (path_out != NULL) *path_out = strdup(entry->path);
            entry->hits++;
            if (hashed_out != NULL) *hashed_out = 1;
            return NAME_HASHED;
        }
    }
    if (path_out != NULL) *path_out = cupid_find_in_path(name);
    return (path_out != NULL && *path_out != NULL) ? NAME_FILE : NAME_NONE;
}

static void describe_name(FILE *fp, const char *name, enum name_kind kind,
                          const char *alias_val, const char *path) {
    switch (kind) {
        case NAME_ALIAS:
            fprintf(fp, "%s is aliased to `%s'\n", name, alias_val ? alias_val : "");
            break;
        case NAME_KEYWORD:
            fprintf(fp, "%s is a shell keyword\n", name);
            break;
        case NAME_FUNCTION:
            fprintf(fp, "%s is a function\n", name);
            break;
        case NAME_BUILTIN:
            fprintf(fp, "%s is a shell builtin\n", name);
            break;
        case NAME_FILE:
            fprintf(fp, "%s is %s\n", name, path ? path : name);
            break;
        case NAME_HASHED:
            fprintf(fp, "%s is hashed (%s)\n", name, path ? path : name);
            break;
        default:
            break;
    }
}

static char *read_stream_alloc(FILE *fp) {
    long end;
    char *buf;
    size_t nread;
    if (fp == NULL) return NULL;
    if (fflush(fp) != 0) return NULL;
    if (fseek(fp, 0, SEEK_END) != 0) return NULL;
    end = ftell(fp);
    if (end < 0) return NULL;
    if (fseek(fp, 0, SEEK_SET) != 0) return NULL;
    buf = calloc((size_t)end + 1, 1);
    if (buf == NULL) return NULL;
    nread = fread(buf, 1, (size_t)end, fp);
    buf[nread] = '\0';
    return buf;
}

/* ------------------------------------------------------------------ */
/*  Existing builtins                                                 */
/* ------------------------------------------------------------------ */

static int builtin_exit(struct cupid_shell *shell, int argc, char **argv, bool in_child) {
    int code = 0;
    if (argc > 1 && parse_exit_code(argv[1], &code) != 0) {
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "exit: %s: numeric argument required\n", argv[1]);
        if (!in_child && shell != NULL) {
            shell->should_exit = 1;
            shell->exit_code = 2;
        }
        return 2;
    }
    if (!in_child && shell != NULL) {
        shell->should_exit = 1;
        shell->exit_code = code;
    }
    return code;
}

static int builtin_cd(struct cupid_shell *shell, int argc, char **argv) {
    const char *dest;
    char *chosen = NULL;
    char *oldpwd = NULL;
    char *pwd = NULL;
    const char *cdpath;
    if (argc > 2) {
        return 1;
    }
    if (argc == 1) {
        dest = cupid_vars_get(shell, "HOME");
        if (dest == NULL) {
            return 1;
        }
    } else {
        dest = argv[1];
    }
    if (dest == NULL) {
        return 1;
    }
    {
        const char *cur_pwd = cupid_vars_get(shell, "PWD");
        if (cur_pwd != NULL && cur_pwd[0] != '\0') {
            oldpwd = strdup(cur_pwd);
        }
        if (oldpwd == NULL) {
            oldpwd = getcwd(NULL, 0);
        }
    }
    if (dest[0] == '/' || strchr(dest, '/') != NULL) {
        if (chdir(dest) != 0) {
            free(oldpwd);
            return 1;
        }
    } else {
        cdpath = cupid_vars_get(shell, "CDPATH");
        if (cdpath != NULL && cdpath[0] != '\0') {
            const char *p = cdpath;
            while (1) {
                const char *end = strchr(p, ':');
                size_t len = (end != NULL) ? (size_t)(end - p) : strlen(p);
                char *candidate = calloc(len + strlen(dest) + 2, 1);
                if (candidate == NULL) {
                    free(oldpwd);
                    free(chosen);
                    return 1;
                }
                if (len > 0) memcpy(candidate, p, len);
                if (len > 0) candidate[len++] = '/';
                memcpy(candidate + len, dest, strlen(dest));
                if (chdir(candidate) == 0) {
                    chosen = candidate;
                    break;
                }
                free(candidate);
                if (end == NULL) break;
                p = end + 1;
            }
        }
        if (chosen == NULL && chdir(dest) != 0) {
            free(oldpwd);
            return 1;
        }
    }
    pwd = getcwd(NULL, 0);
    if (pwd != NULL) {
        if (oldpwd != NULL) {
            (void)setenv("OLDPWD", oldpwd, 1);
            if (shell != NULL) {
                (void)cupid_vars_set(shell, "OLDPWD", oldpwd);
            }
        }
        (void)setenv("PWD", pwd, 1);
        if (shell != NULL) {
            (void)cupid_vars_set(shell, "PWD", pwd);
        }
        free(pwd);
    }
    free(oldpwd);
    if (chosen != NULL && chosen[0] != '\0' && chosen[0] != '.') {
        puts(chosen);
    }
    free(chosen);
    return 0;
}

static int builtin_export(struct cupid_shell *shell, int argc, char **argv) {
    int i;
    for (i = 1; i < argc; i++) {
        const char *name = NULL;
        const char *value = NULL;
        size_t name_len = 0;
        int append = 0;
        if (!split_assignment_word_ext(argv[i], &name, &name_len, &value, &append)) {
            if (cupid_vars_export(shell, argv[i], NULL) != 0) {
                return 1;
            }
            continue;
        }
        {
            char *key = calloc(name_len + 1, 1);
            if (key == NULL) {
                return 1;
            }
            memcpy(key, name, name_len);
            if (append) {
                if (apply_append_assignment_value(shell, key, value) != 0) {
                    free(key);
                    return 1;
                }
                value = cupid_vars_get(shell, key);
            }
            if (cupid_vars_export(shell, key, value) != 0) {
                free(key);
                return 1;
            }
            free(key);
        }
    }
    return 0;
}

static int builtin_unset(struct cupid_shell *shell, int argc, char **argv) {
    int unset_nameref = 0;
    int unset_vars = 1;
    int unset_funcs = 1;
    int i;
    int arg_start = 1;
    for (i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        }
        if (argv[i][0] != '-' || argv[i][1] == '\0') {
            break;
        }
        {
            const char *p = argv[i] + 1;
            while (*p != '\0') {
                if (*p == 'n') {
                    unset_nameref = 1;
                    unset_vars = 1;
                    unset_funcs = 0;
                } else if (*p == 'v') {
                    unset_nameref = 0;
                    unset_vars = 1;
                    unset_funcs = 0;
                }
                else if (*p == 'f') {
                    unset_nameref = 0;
                    unset_vars = 0;
                    unset_funcs = 1;
                } else {
                    return 1;
                }
                p++;
            }
        }
    }
    arg_start = i;
    for (i = arg_start; i < argc; i++) {
        if (unset_vars) {
            if ((unset_nameref ? cupid_vars_unset_binding(shell, argv[i])
                               : cupid_vars_unset(shell, argv[i])) != 0) {
                if (shell != NULL && shell->mode == CUPID_MODE_POSIX) {
                    return posix_special_builtin_error(shell, 2);
                }
                return 1;
            }
        }
        if (unset_funcs && cupid_func_get(shell, argv[i]) != NULL) {
            cupid_func_unset(shell, argv[i]);
        }
    }
    return 0;
}

static int builtin_source(struct cupid_shell *shell, int argc, char **argv) {
    char *resolved = NULL;
    struct cupid_params old_params = {0};
    int restore_old_params = 0;
    const char *path;
    int rc;
    if (argc < 2) {
        return 1;
    }
    path = argv[1];
    if (strchr(path, '/') == NULL && shell != NULL && shell->opt_sourcepath) {
        const char *path_env = getenv("PATH");
        if (path_env != NULL) {
            const char *p = path_env;
            while (*p != '\0') {
                const char *end = strchr(p, ':');
                size_t len = (end != NULL) ? (size_t)(end - p) : strlen(p);
                char *candidate = calloc(len + strlen(path) + 2, 1);
                if (candidate == NULL) break;
                if (len > 0) memcpy(candidate, p, len);
                if (len > 0) candidate[len++] = '/';
                memcpy(candidate + len, path, strlen(path));
                if (access(candidate, R_OK) == 0) {
                    resolved = candidate;
                    break;
                }
                free(candidate);
                if (end == NULL) break;
                p = end + 1;
            }
        }
        if (resolved != NULL) path = resolved;
    }
    if (access(path, R_OK) != 0) {
        cupid_shell_error_prefix(stderr, shell);
        if (shell != NULL &&
            shell->mode == CUPID_MODE_POSIX &&
            shell->current_file == NULL &&
            strcmp(argv[0], ".") == 0) {
            fprintf(stderr, ".: %s: file not found\n", argv[1]);
        } else {
            fprintf(stderr, "%s: %s\n", argv[1], strerror(errno));
        }
        free(resolved);
        return (shell != NULL && shell->mode == CUPID_MODE_POSIX)
            ? posix_special_builtin_error(shell, 1)
            : 1;
    }

    if (shell != NULL && argc > 2) {
        size_t j;
        old_params = shell->params;
        shell->params.args = calloc((size_t)(argc - 2), sizeof(char *));
        if (shell->params.args == NULL) {
            shell->params = old_params;
            free(resolved);
            return 1;
        }
        shell->params.count = 0;
        for (j = 0; j < (size_t)(argc - 2); j++) {
            shell->params.args[j] = strdup(argv[j + 2]);
            if (shell->params.args[j] == NULL) {
                size_t k;
                for (k = 0; k < j; k++) free(shell->params.args[k]);
                free(shell->params.args);
                shell->params = old_params;
                free(resolved);
                return 1;
            }
            shell->params.count++;
        }
        restore_old_params = 1;
    }

    rc = cupid_shell_eval_file(shell, path);
    if (shell->return_flag) {
        shell->return_flag = 0;
        shell->last_status = rc;
    }
    if (restore_old_params) {
        size_t j;
        int changed = 0;
        if (shell->params.count != (size_t)(argc - 2)) {
            changed = 1;
        } else {
            for (j = 0; j < shell->params.count; j++) {
                if (strcmp(shell->params.args[j], argv[j + 2]) != 0) {
                    changed = 1;
                    break;
                }
            }
        }
        if (!changed) {
            for (j = 0; j < shell->params.count; j++) free(shell->params.args[j]);
            free(shell->params.args);
            shell->params = old_params;
        } else {
            for (j = 0; j < old_params.count; j++) free(old_params.args[j]);
            free(old_params.args);
        }
    }
    free(resolved);
    return rc;
}

static int builtin_break(struct cupid_shell *shell, int argc, char **argv) {
    int n = 1;
    if (argc > 1) {
        char *end;
        long v = strtol(argv[1], &end, 10);
        if (*end != '\0' || v < 1) {
            return 1;
        }
        n = (int)v;
    }
    if (shell->loop_depth == 0) {
        return 0;
    }
    shell->break_count = n;
    return shell->last_status;
}

static int builtin_continue(struct cupid_shell *shell, int argc, char **argv) {
    int n = 1;
    if (argc > 1) {
        char *end;
        long v = strtol(argv[1], &end, 10);
        if (*end != '\0' || v < 1) {
            return 1;
        }
        n = (int)v;
    }
    if (shell->loop_depth == 0) {
        return 0;
    }
    shell->continue_flag = n;
    return shell->last_status;
}

static int builtin_return(struct cupid_shell *shell, int argc, char **argv) {
    int code = 0;
    if (argc > 1 && parse_exit_code(argv[1], &code) != 0) {
        return 2;
    }
    shell->return_flag = 1;
    shell->last_status = code;
    return code;
}

static int builtin_shift(struct cupid_shell *shell, int argc, char **argv) {
    int n = 1;
    size_t i;
    if (argc > 1) {
        char *end;
        long v = strtol(argv[1], &end, 10);
        if (*end != '\0' || v < 0) {
            return 1;
        }
        n = (int)v;
    }
    if ((size_t)n > shell->params.count) {
        return 1;
    }
    for (i = 0; i < (size_t)n; i++) {
        free(shell->params.args[i]);
    }
    if (shell->params.count > (size_t)n) {
        memmove(shell->params.args, shell->params.args + n,
                (shell->params.count - (size_t)n) * sizeof(char *));
    }
    shell->params.count -= (size_t)n;
    return 0;
}

static int shell_replace_params(struct cupid_shell *shell, int argc, char **argv, int start_index) {
    size_t j;
    size_t new_count;
    char **new_args = NULL;

    if (shell == NULL) return 1;
    if (start_index < 0 || start_index > argc) return 1;
    new_count = (size_t)(argc - start_index);

    if (new_count > 0) {
        new_args = calloc(new_count, sizeof(char *));
        if (new_args == NULL) return 1;
        for (j = 0; j < new_count; j++) {
            new_args[j] = strdup(argv[start_index + (int)j]);
            if (new_args[j] == NULL) {
                size_t k;
                for (k = 0; k < j; k++) free(new_args[k]);
                free(new_args);
                return 1;
            }
        }
    }

    for (j = 0; j < shell->params.count; j++) {
        free(shell->params.args[j]);
    }
    free(shell->params.args);
    shell->params.args = new_args;
    shell->params.count = new_count;
    return 0;
}

static int builtin_local(struct cupid_shell *shell, int argc, char **argv) {
    int i = 1;
    int do_nameref = 0;
    if (shell->scope_depth == 0) {
        fprintf(stderr, "cupid: local: can only be used in a function\n");
        return 1;
    }
    while (i < argc && argv[i][0] == '-' && argv[i][1] != '\0') {
        const char *p = argv[i] + 1;
        while (*p != '\0') {
            if (*p == 'n') do_nameref = 1;
            else {
                return 1;
            }
            p++;
        }
        i++;
    }
    for (; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        if (eq == NULL) {
            if (do_nameref) {
                if (cupid_vars_set_local_nameref(shell, argv[i], "") != 0) return 1;
            } else {
                if (cupid_vars_set_local(shell, argv[i], "") != 0) return 1;
            }
        } else {
            size_t key_len = (size_t)(eq - argv[i]);
            char *key = calloc(key_len + 1, 1);
            if (key == NULL) return 1;
            memcpy(key, argv[i], key_len);
            if (do_nameref) {
                if (cupid_vars_set_local_nameref(shell, key, eq + 1) != 0) {
                    free(key);
                    return 1;
                }
            } else {
                if (cupid_vars_set_local(shell, key, eq + 1) != 0) {
                    free(key);
                    return 1;
                }
            }
            free(key);
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  New builtins: true, false, colon                                  */
/* ------------------------------------------------------------------ */

static int builtin_true(void) { return 0; }
static int builtin_false(void) { return 1; }

static int builtin_pwd(void) {
    char buf[4096];
    if (getcwd(buf, sizeof(buf)) == NULL) {
        return 1;
    }
    puts(buf);
    fflush(stdout);
    return 0;
}

static int builtin_alias(struct cupid_shell *shell, int argc, char **argv) {
    int status = 0;
    int i;
    if (argc == 1 || (argc == 2 && strcmp(argv[1], "-p") == 0)) {
        size_t ai;
        for (ai = 0; ai < shell->aliases.count; ai++) {
            printf("alias %s='%s'\n", shell->aliases.entries[ai].name, shell->aliases.entries[ai].value);
        }
        return 0;
    }
    for (i = 1; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        if (eq != NULL) {
            size_t nlen = (size_t)(eq - argv[i]);
            char *name = calloc(nlen + 1, 1);
            if (name == NULL) return 1;
            memcpy(name, argv[i], nlen);
            if (cupid_alias_set(shell, name, eq + 1) != 0) status = 1;
            free(name);
                continue;
        }
        {
            const char *val = cupid_alias_get(shell, argv[i]);
            if (val == NULL) {
                cupid_shell_error_prefix(stderr, shell);
                fprintf(stderr, "alias: %s: not found\n", argv[i]);
                status = 1;
            } else {
                printf("alias %s='%s'\n", argv[i], val);
            }
        }
    }
    return status;
}

static int builtin_unalias(struct cupid_shell *shell, int argc, char **argv) {
    int i;
    int status = 0;
    if (argc < 2) {
        fprintf(stderr, "cupid: unalias: usage: unalias [-a] name [name...]\n");
        return 1;
    }
    if (argc == 2 && strcmp(argv[1], "-a") == 0) {
        while (shell->aliases.count > 0) {
            free(shell->aliases.entries[shell->aliases.count - 1].name);
            free(shell->aliases.entries[shell->aliases.count - 1].value);
            shell->aliases.count--;
        }
        return 0;
    }
    for (i = 1; i < argc; i++) {
        if (cupid_alias_unset(shell, argv[i]) != 0) {
            fprintf(stderr, "cupid: unalias: %s: not found\n", argv[i]);
            status = 1;
        }
    }
    return status;
}

static int builtin_shopt(struct cupid_shell *shell, int argc, char **argv) {
    int i;
    int quiet = 0;
    int set_mode = 0;
    int unset_mode = 0;
    int status = 0;

    if (argc == 1) {
        printf("cmdhist\t%s\n", shell->opt_cmdhist ? "on" : "off");
        printf("expand_aliases\t%s\n", shell->opt_expand_aliases ? "on" : "off");
        printf("extglob\t%s\n", shell->opt_extglob ? "on" : "off");
        printf("nullglob\t%s\n", shell->opt_nullglob ? "on" : "off");
        printf("lastpipe\t%s\n", shell->opt_lastpipe ? "on" : "off");
        printf("sourcepath\t%s\n", shell->opt_sourcepath ? "on" : "off");
        printf("varredir_close\t%s\n", shell->opt_varredir_close ? "on" : "off");
        printf("xpg_echo\t%s\n", shell->opt_xpg_echo ? "on" : "off");
        return 0;
    }

    for (i = 1; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-q") == 0) quiet = 1;
        else if (strcmp(argv[i], "-s") == 0) set_mode = 1;
        else if (strcmp(argv[i], "-u") == 0) unset_mode = 1;
        else return 1;
    }
    if (i >= argc) return 0;

    for (; i < argc; i++) {
        int *opt_ptr = NULL;
        if (strcmp(argv[i], "cmdhist") == 0) opt_ptr = &shell->opt_cmdhist;
        else if (strcmp(argv[i], "expand_aliases") == 0) opt_ptr = &shell->opt_expand_aliases;
        else if (strcmp(argv[i], "extglob") == 0) opt_ptr = &shell->opt_extglob;
        else if (strcmp(argv[i], "nullglob") == 0) opt_ptr = &shell->opt_nullglob;
        else if (strcmp(argv[i], "lastpipe") == 0) opt_ptr = &shell->opt_lastpipe;
        else if (strcmp(argv[i], "sourcepath") == 0) opt_ptr = &shell->opt_sourcepath;
        else if (strcmp(argv[i], "varredir_close") == 0) opt_ptr = &shell->opt_varredir_close;
        else if (strcmp(argv[i], "xpg_echo") == 0) opt_ptr = &shell->opt_xpg_echo;
        else {
            status = 1;
            if (!quiet) fprintf(stderr, "cupid: shopt: %s: invalid shell option name\n", argv[i]);
            continue;
        }
        if (set_mode) *opt_ptr = 1;
        else if (unset_mode) *opt_ptr = 0;
        else if (!quiet) {
            printf("%s\t%s\n", argv[i], *opt_ptr ? "on" : "off");
        }
        if (quiet && !set_mode && !unset_mode && !*opt_ptr) status = 1;
    }
    return status;
}

static int builtin_hash(struct cupid_shell *shell, int argc, char **argv) {
    int i;
    int status = 0;
    int print_table = 0;
    if (argc == 1) {
        print_table = 1;
    } else if (argc == 2 && strcmp(argv[1], "-r") == 0) {
        hash_table_clear(shell);
        return 0;
    }
    if (print_table) {
        size_t hi;
        if (shell == NULL || shell->hashes.count == 0) {
            puts("hash: hash table empty");
            return 0;
        }
        puts("hits\tcommand");
        for (hi = 0; hi < shell->hashes.count; hi++) {
            printf("%4d\t%s\n", shell->hashes.entries[hi].hits, shell->hashes.entries[hi].path);
        }
        return 0;
    }
    for (i = 1; i < argc; i++) {
        char *path = NULL;
        if (strcmp(argv[i], "-p") == 0 && i + 2 < argc) {
            if (hash_table_set(shell, argv[i + 2], argv[i + 1]) != 0) {
                status = 1;
            }
            i += 2;
            continue;
        }
        path = cupid_find_in_path(argv[i]);
        if (path == NULL) {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "hash: %s: not found\n", argv[i]);
            status = 1;
        } else if (hash_table_set(shell, argv[i], path) != 0) {
            status = 1;
        }
        free(path);
    }
    return status;
}

static int builtin_builtin(struct cupid_shell *shell, int argc, char **argv, bool in_child) {
    int status;
    if (argc < 2) return 1;
    status = cupid_run_builtin(shell, argc - 1, argv + 1, in_child);
    if (status == CUPID_BUILTIN_NOT_FOUND) {
        fprintf(stderr, "cupid: builtin: %s: not a shell builtin\n", argv[1]);
        return 1;
    }
    return status;
}

/* ------------------------------------------------------------------ */
/*  echo                                                              */
/* ------------------------------------------------------------------ */

static int is_echo_flag(const char *s) {
    const char *p;
    if (s[0] != '-' || s[1] == '\0') return 0;
    p = s + 1;
    while (*p != '\0') {
        if (*p != 'n' && *p != 'e' && *p != 'E') return 0;
        p++;
    }
    return 1;
}

static int echo_hex_value(char ch) {
    if (ch >= '0' && ch <= '9') return ch - '0';
    if (ch >= 'a' && ch <= 'f') return 10 + (ch - 'a');
    if (ch >= 'A' && ch <= 'F') return 10 + (ch - 'A');
    return -1;
}

static int echo_process_escapes(const char *s, FILE *fp) {
    const char *p = s;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            switch (p[1]) {
                case 'n': fputc('\n', fp); p += 2; continue;
                case 't': fputc('\t', fp); p += 2; continue;
                case '\\': fputc('\\', fp); p += 2; continue;
                case 'e':
                case 'E': fputc(27, fp); p += 2; continue;
                case 'a': fputc('\a', fp); p += 2; continue;
                case 'b': fputc('\b', fp); p += 2; continue;
                case 'r': fputc('\r', fp); p += 2; continue;
                case 'c': return 1;
                case '"': fputc('"', fp); p += 2; continue;
                case '\'': fputc('\'', fp); p += 2; continue;
                case '?': fputc('?', fp); p += 2; continue;
                case 'x': {
                    int h1, h2;
                    p += 2;
                    h1 = echo_hex_value(*p);
                    if (h1 < 0) {
                        fputc('\\', fp);
                        fputc('x', fp);
                        continue;
                    }
                    p++;
                    h2 = echo_hex_value(*p);
                    if (h2 >= 0) {
                        fputc(((h1 << 4) | h2) & 0xff, fp);
                        p++;
                    } else {
                        fputc(h1 & 0xff, fp);
                    }
                    continue;
                }
                case '0': {
                    unsigned int val = 0;
                    int digits = 0;
                    p += 2;
                    while (digits < 3 && *p >= '0' && *p <= '7') {
                        val = val * 8 + (unsigned int)(*p - '0');
                        digits++;
                        p++;
                    }
                    fputc((int)(val & 0xff), fp);
                    continue;
                }
                default: break;
            }
        }
        fputc(*p, fp);
        p++;
    }
    return 0;
}

static int builtin_echo(struct cupid_shell *shell, int argc, char **argv) {
    int newline = 1;
    int escape = (shell != NULL && shell->opt_xpg_echo) ? 1 : 0;
    int start = 1;
    int i;

    while (start < argc && is_echo_flag(argv[start])) {
        const char *p = argv[start] + 1;
        while (*p != '\0') {
            if (*p == 'n') newline = 0;
            else if (*p == 'e') escape = 1;
            else if (*p == 'E') escape = 0;
            p++;
        }
        start++;
    }

    for (i = start; i < argc; i++) {
        int stop = 0;
        if (i > start) fputc(' ', stdout);
        if (escape) {
            stop = echo_process_escapes(argv[i], stdout);
            if (stop) {
                newline = 0;
                break;
            }
        } else {
            fputs(argv[i], stdout);
        }
    }
    if (newline) fputc('\n', stdout);
    fflush(stdout);
    return 0;
}

/* ------------------------------------------------------------------ */
/*  printf                                                            */
/* ------------------------------------------------------------------ */

struct printf_spec {
    char flags[8];
    int width_set;
    int width;
    int precision_set;
    int precision;
    char conv;
    int uses_arg;
    int is_time;
    int invalid_time_spec;
    const char *timefmt_start;
    size_t timefmt_len;
    const char *raw_start;
    size_t raw_len;
    const char *next;
};

struct printf_bytes {
    char *data;
    size_t len;
    size_t cap;
};

#define PRINTF_FIELD_LIMIT 10000
#define PRINTF_FORMAT_LIMIT 1000000
#define PRINTF_ERROR_FRAGMENT_MAX 64

static void printf_error(struct cupid_shell *shell, const char *fmt, ...) {
    va_list ap;
    cupid_shell_error_prefix(stderr, shell);
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
}

static void printf_format_fragment(const char *start, const char *end, char *buf, size_t cap) {
    size_t n = 0;
    if (cap == 0) return;
    if (start == NULL) start = "";
    if (end == NULL) end = start + strnlen(start, PRINTF_ERROR_FRAGMENT_MAX);
    while (start < end && *start != '\0' && n + 1 < cap) {
        unsigned char c = (unsigned char)*start++;
        if (c == '\n' || c == '\r' || c == '\t') break;
        if (isprint(c)) {
            buf[n++] = (char)c;
        } else {
            break;
        }
    }
    buf[n] = '\0';
}

static int printf_missing_format(struct cupid_shell *shell, const char *start, const char *end,
                                 int *status_out) {
    char fragment[PRINTF_ERROR_FRAGMENT_MAX];
    printf_format_fragment(start, end, fragment, sizeof(fragment));
    if (fragment[0] == '\0') {
        printf_error(shell, "printf: missing format character\n");
    } else {
        printf_error(shell, "printf: `%s': missing format character\n", fragment);
    }
    if (status_out != NULL) *status_out = 1;
    return -1;
}

static int printf_write_bytes(FILE *fp, size_t *count, const void *buf, size_t len) {
    if (len == 0) return 0;
    if (fwrite(buf, 1, len, fp) != len) return -1;
    if (count != NULL) *count += len;
    return 0;
}

static int printf_write_char(FILE *fp, size_t *count, char ch) {
    return printf_write_bytes(fp, count, &ch, 1);
}

static int printf_emit_padded(FILE *fp, size_t *count, const char *data, size_t len,
                              int width, int left) {
    size_t pad = 0;
    size_t i;
    if (width > 0 && (size_t)width > len) pad = (size_t)width - len;
    if (!left) {
        for (i = 0; i < pad; i++) {
            if (printf_write_char(fp, count, ' ') != 0) return -1;
        }
    }
    if (printf_write_bytes(fp, count, data, len) != 0) return -1;
    if (left) {
        for (i = 0; i < pad; i++) {
            if (printf_write_char(fp, count, ' ') != 0) return -1;
        }
    }
    return 0;
}

static int printf_hex_value(char ch) {
    if (ch >= '0' && ch <= '9') return ch - '0';
    if (ch >= 'a' && ch <= 'f') return 10 + (ch - 'a');
    if (ch >= 'A' && ch <= 'F') return 10 + (ch - 'A');
    return -1;
}

static int printf_process_format_backslash(const char **pp, FILE *fp, size_t *count) {
    const char *p = *pp + 1;
    if (*p == '\0') {
        if (printf_write_char(fp, count, '\\') != 0) return -1;
        *pp = p;
        return 0;
    }
    switch (*p) {
        case 'n': if (printf_write_char(fp, count, '\n') != 0) return -1; p++; break;
        case 't': if (printf_write_char(fp, count, '\t') != 0) return -1; p++; break;
        case '\\': if (printf_write_char(fp, count, '\\') != 0) return -1; p++; break;
        case '"': if (printf_write_char(fp, count, '"') != 0) return -1; p++; break;
        case '\'': if (printf_write_char(fp, count, '\'') != 0) return -1; p++; break;
        case '?': if (printf_write_char(fp, count, '?') != 0) return -1; p++; break;
        case 'e':
        case 'E': if (printf_write_char(fp, count, 27) != 0) return -1; p++; break;
        case 'a': if (printf_write_char(fp, count, '\a') != 0) return -1; p++; break;
        case 'b': if (printf_write_char(fp, count, '\b') != 0) return -1; p++; break;
        case 'f': if (printf_write_char(fp, count, '\f') != 0) return -1; p++; break;
        case 'r': if (printf_write_char(fp, count, '\r') != 0) return -1; p++; break;
        case 'v': if (printf_write_char(fp, count, '\v') != 0) return -1; p++; break;
        case 'x': {
            int h1, h2;
            p++;
            h1 = printf_hex_value(*p);
            if (h1 < 0) {
                if (printf_write_char(fp, count, '\\') != 0 ||
                    printf_write_char(fp, count, 'x') != 0) {
                    return -1;
                }
                break;
            }
            p++;
            h2 = printf_hex_value(*p);
            if (h2 >= 0) {
                if (printf_write_char(fp, count, (char)((h1 << 4) | h2)) != 0) return -1;
                p++;
            } else {
                if (printf_write_char(fp, count, (char)h1) != 0) return -1;
            }
            break;
        }
        case '0': {
            unsigned int val = 0;
            int digits = 0;
            p++;
            while (digits < 2 && *p >= '0' && *p <= '7') {
                val = val * 8 + (unsigned int)(*p - '0');
                digits++;
                p++;
            }
            if (printf_write_char(fp, count, (char)(val & 0xff)) != 0) return -1;
            break;
        }
        default:
            if (printf_write_char(fp, count, '\\') != 0 ||
                printf_write_char(fp, count, *p) != 0) {
                return -1;
            }
            p++;
            break;
    }
    *pp = p;
    return 0;
}

static int printf_q_is_safe(unsigned char c) {
    return (isalnum(c) || strchr("_@%+=:,./-", (int)c) != NULL);
}

static void printf_quote_shell(const char *s, FILE *fp) {
    const unsigned char *p = (const unsigned char *)(s ? s : "");
    int use_ansi = 0;
    if (*p == '\0') {
        fputs("''", fp);
        return;
    }
    for (; *p != '\0'; p++) {
        if (!isprint(*p)) {
            use_ansi = 1;
            break;
        }
    }

    p = (const unsigned char *)(s ? s : "");
    if (use_ansi) {
        fputs("$'", fp);
        while (*p != '\0') {
            unsigned char c = *p++;
            switch (c) {
                case '\a': fputs("\\a", fp); break;
                case '\b': fputs("\\b", fp); break;
                case '\t': fputs("\\t", fp); break;
                case '\n': fputs("\\n", fp); break;
                case '\v': fputs("\\v", fp); break;
                case '\f': fputs("\\f", fp); break;
                case '\r': fputs("\\r", fp); break;
                case '\\': fputs("\\\\", fp); break;
                case '\'': fputs("\\'", fp); break;
                default:
                    fprintf(fp, "\\x%02x", (unsigned int)c);
                    break;
            }
        }
        fputc('\'', fp);
        return;
    }

    while (*p != '\0') {
        unsigned char c = *p++;
        if (printf_q_is_safe(c)) {
            fputc((int)c, fp);
        } else {
            fputc('\\', fp);
            fputc((int)c, fp);
        }
    }
}

static char *printf_quote_shell_alloc(const char *s) {
    FILE *tmp = tmpfile();
    char *out;
    if (tmp == NULL) return NULL;
    printf_quote_shell(s, tmp);
    out = read_stream_alloc(tmp);
    fclose(tmp);
    return out;
}

static int printf_bytes_append(struct printf_bytes *b, char ch) {
    char *next;
    size_t nc;
    if (b->len + 1 > b->cap) {
        nc = (b->cap == 0) ? 32 : b->cap * 2;
        while (b->len + 1 > nc) nc *= 2;
        next = realloc(b->data, nc);
        if (next == NULL) return -1;
        b->data = next;
        b->cap = nc;
    }
    b->data[b->len++] = ch;
    return 0;
}

static int printf_decode_utf8(const char *s, unsigned int *cp) {
    const unsigned char *p = (const unsigned char *)s;
    if (p[0] < 0x80) {
        *cp = p[0];
        return 1;
    }
    if ((p[0] & 0xe0) == 0xc0 && (p[1] & 0xc0) == 0x80) {
        *cp = ((unsigned int)(p[0] & 0x1f) << 6) | (unsigned int)(p[1] & 0x3f);
        return 2;
    }
    if ((p[0] & 0xf0) == 0xe0 && (p[1] & 0xc0) == 0x80 && (p[2] & 0xc0) == 0x80) {
        *cp = ((unsigned int)(p[0] & 0x0f) << 12) |
              ((unsigned int)(p[1] & 0x3f) << 6) |
              (unsigned int)(p[2] & 0x3f);
        return 3;
    }
    if ((p[0] & 0xf8) == 0xf0 && (p[1] & 0xc0) == 0x80 &&
        (p[2] & 0xc0) == 0x80 && (p[3] & 0xc0) == 0x80) {
        *cp = ((unsigned int)(p[0] & 0x07) << 18) |
              ((unsigned int)(p[1] & 0x3f) << 12) |
              ((unsigned int)(p[2] & 0x3f) << 6) |
              (unsigned int)(p[3] & 0x3f);
        return 4;
    }
    *cp = p[0];
    return 1;
}

static long long printf_parse_signed(struct cupid_shell *shell, const char *s, int *invalid) {
    char *end = NULL;
    long long v;
    unsigned int cp;
    if (s != NULL && (s[0] == '\'' || s[0] == '"') && s[1] != '\0') {
        (void)printf_decode_utf8(s + 1, &cp);
        return (long long)cp;
    }
    errno = 0;
    v = strtoll(s ? s : "0", &end, 0);
    if (errno != 0 || end == s || (end != NULL && *end != '\0')) {
        if (invalid != NULL) *invalid = 1;
        printf_error(shell, "printf: %s: invalid number\n", s ? s : "");
        return 0;
    }
    return v;
}

static unsigned long long printf_parse_unsigned(struct cupid_shell *shell, const char *s, int *invalid) {
    char *end = NULL;
    unsigned long long v;
    unsigned int cp;
    if (s != NULL && (s[0] == '\'' || s[0] == '"') && s[1] != '\0') {
        (void)printf_decode_utf8(s + 1, &cp);
        return (unsigned long long)cp;
    }
    errno = 0;
    v = strtoull(s ? s : "0", &end, 0);
    if (errno != 0 || end == s || (end != NULL && *end != '\0')) {
        if (invalid != NULL) *invalid = 1;
        printf_error(shell, "printf: %s: invalid number\n", s ? s : "");
        return 0;
    }
    return v;
}

static double printf_parse_double(struct cupid_shell *shell, const char *s, int *invalid) {
    char *end = NULL;
    double v;
    unsigned int cp;
    if (s != NULL && (s[0] == '\'' || s[0] == '"') && s[1] != '\0') {
        (void)printf_decode_utf8(s + 1, &cp);
        return (double)cp;
    }
    errno = 0;
    v = strtod(s ? s : "0", &end);
    if (errno != 0 || end == s || (end != NULL && *end != '\0')) {
        if (invalid != NULL) *invalid = 1;
        printf_error(shell, "printf: %s: invalid number\n", s ? s : "");
        return 0.0;
    }
    return v;
}

static int printf_parse_star_int(const char *s, int *out) {
    char *end = NULL;
    long v;
    errno = 0;
    v = strtol(s ? s : "0", &end, 10);
    if (errno != 0 || end == s || (end != NULL && *end != '\0')) {
        *out = 0;
        return -1;
    }
    if (v > PRINTF_FIELD_LIMIT) v = PRINTF_FIELD_LIMIT;
    if (v < -PRINTF_FIELD_LIMIT) v = -PRINTF_FIELD_LIMIT;
    *out = (int)v;
    return 0;
}

static int printf_parse_spec(struct printf_spec *spec, struct cupid_shell *shell,
                             const char **pp, const char *fmt_end,
                             int *ai, int argc, char **argv, int *status_out) {
    const char *p = *pp;
    const char *spec_start = p - 1;
    size_t flen = 0;
    memset(spec, 0, sizeof(*spec));
    while (p < fmt_end && strchr("-+ #0", (int)*p) != NULL) {
        if (flen + 1 < sizeof(spec->flags)) spec->flags[flen++] = *p;
        p++;
    }
    spec->flags[flen] = '\0';

    if (p < fmt_end && *p == '*') {
        int w = 0;
        if (*ai < argc) (void)printf_parse_star_int(argv[(*ai)++], &w);
        spec->width_set = 1;
        spec->width = w;
        p++;
    } else {
        while (p < fmt_end && *p >= '0' && *p <= '9') {
            spec->width_set = 1;
            if (spec->width < PRINTF_FIELD_LIMIT) {
                if (spec->width <= (PRINTF_FIELD_LIMIT - 9) / 10) {
                    spec->width = spec->width * 10 + (*p - '0');
                } else {
                    spec->width = PRINTF_FIELD_LIMIT;
                }
            }
            p++;
        }
    }
    if (spec->width_set && spec->width < 0) {
        spec->width = -spec->width;
        if (strchr(spec->flags, '-') == NULL && flen + 1 < sizeof(spec->flags)) {
            spec->flags[flen++] = '-';
            spec->flags[flen] = '\0';
        }
    }

    if (p < fmt_end && *p == '.') {
        p++;
        spec->precision_set = 1;
        spec->precision = 0;
        if (p < fmt_end && *p == '*') {
            int pr = 0;
            if (*ai < argc) (void)printf_parse_star_int(argv[(*ai)++], &pr);
            if (pr < 0) {
                spec->precision_set = 0;
            } else {
                spec->precision = pr;
            }
            p++;
        } else {
            while (p < fmt_end && *p >= '0' && *p <= '9') {
                if (spec->precision < PRINTF_FIELD_LIMIT) {
                    if (spec->precision <= (PRINTF_FIELD_LIMIT - 9) / 10) {
                        spec->precision = spec->precision * 10 + (*p - '0');
                    } else {
                        spec->precision = PRINTF_FIELD_LIMIT;
                    }
                }
                p++;
            }
        }
    }

    if ((p + 1) < fmt_end && ((*p == 'h' && p[1] == 'h') || (*p == 'l' && p[1] == 'l'))) p += 2;
    else if (p < fmt_end && strchr("hlLjzt", (int)*p) != NULL) p++;

    if (p < fmt_end && *p == '(') {
        const char *start;
        int depth = 1;
        p++;
        start = p;
        while (p < fmt_end && depth > 0) {
            if (*p == '(') depth++;
            else if (*p == ')') depth--;
            p++;
        }
        if (depth != 0) return printf_missing_format(shell, spec_start, p, status_out);
        if (p >= fmt_end) return printf_missing_format(shell, spec_start, p, status_out);
        spec->is_time = 1;
        spec->timefmt_start = start;
        spec->timefmt_len = (size_t)((p - 1) - start);
        spec->conv = *p++;
        spec->raw_start = spec_start;
        spec->raw_len = (size_t)(p - spec_start);
        spec->invalid_time_spec = (spec->conv != 'T');
        spec->uses_arg = spec->invalid_time_spec ? 0 : 1;
        spec->next = p;
        *pp = p;
        return 0;
    }

    if (p >= fmt_end) {
        return printf_missing_format(shell, spec_start, p, status_out);
    }
    spec->conv = *p++;
    spec->uses_arg = (spec->conv != '%');
    spec->next = p;
    *pp = p;
    return 0;
}

static int printf_format_and_emit(FILE *out, size_t *count, const char *fmt,
                                  long long sv, unsigned long long uv, double dv,
                                  const char *str, int cv, char conv_kind) {
    int need;
    char stack[256];
    char *buf = stack;
    size_t cap = sizeof(stack);
    if (conv_kind == 'i') need = snprintf(NULL, 0, fmt, sv);
    else if (conv_kind == 'u') need = snprintf(NULL, 0, fmt, uv);
    else if (conv_kind == 'f') need = snprintf(NULL, 0, fmt, dv);
    else if (conv_kind == 's') need = snprintf(NULL, 0, fmt, str ? str : "");
    else need = snprintf(NULL, 0, fmt, cv);
    if (need < 0) return -1;
    if ((size_t)need + 1 > cap) {
        buf = calloc((size_t)need + 1, 1);
        if (buf == NULL) return -1;
        cap = (size_t)need + 1;
    }
    if (conv_kind == 'i') snprintf(buf, cap, fmt, sv);
    else if (conv_kind == 'u') snprintf(buf, cap, fmt, uv);
    else if (conv_kind == 'f') snprintf(buf, cap, fmt, dv);
    else if (conv_kind == 's') snprintf(buf, cap, fmt, str ? str : "");
    else snprintf(buf, cap, fmt, cv);
    if (printf_write_bytes(out, count, buf, (size_t)need) != 0) {
        if (buf != stack) free(buf);
        return -1;
    }
    if (buf != stack) free(buf);
    return 0;
}

static int printf_expand_b(struct cupid_shell *shell, const char *arg, struct printf_bytes *b,
                           int *stop_output, int *status_out) {
    const char *p = arg ? arg : "";
    while (*p != '\0') {
        if (*p != '\\') {
            if (printf_bytes_append(b, *p++) != 0) return -1;
            continue;
        }
        p++;
        switch (*p) {
            case '\0':
                if (printf_bytes_append(b, '\\') != 0) return -1;
                return 0;
            case 'a': if (printf_bytes_append(b, '\a') != 0) return -1; p++; break;
            case 'b': if (printf_bytes_append(b, '\b') != 0) return -1; p++; break;
            case 'e':
            case 'E': if (printf_bytes_append(b, 27) != 0) return -1; p++; break;
            case 'f': if (printf_bytes_append(b, '\f') != 0) return -1; p++; break;
            case 'n': if (printf_bytes_append(b, '\n') != 0) return -1; p++; break;
            case 'r': if (printf_bytes_append(b, '\r') != 0) return -1; p++; break;
            case 't': if (printf_bytes_append(b, '\t') != 0) return -1; p++; break;
            case 'v': if (printf_bytes_append(b, '\v') != 0) return -1; p++; break;
            case '\\': if (printf_bytes_append(b, '\\') != 0) return -1; p++; break;
            case 'c':
                p++;
                if (stop_output != NULL) *stop_output = 1;
                return 0;
            case 'x': {
                int h1, h2;
                p++;
                h1 = printf_hex_value(*p);
                if (h1 < 0) {
                    printf_error(shell, "printf: missing hex digit for \\x\n");
                    if (status_out != NULL) *status_out = 1;
                    if (printf_bytes_append(b, '\\') != 0 ||
                        printf_bytes_append(b, 'x') != 0) {
                        return -1;
                    }
                    break;
                }
                p++;
                h2 = printf_hex_value(*p);
                if (h2 >= 0) {
                    if (printf_bytes_append(b, (char)((h1 << 4) | h2)) != 0) return -1;
                    p++;
                } else {
                    if (printf_bytes_append(b, (char)h1) != 0) return -1;
                }
                break;
            }
            case '0': {
                int digits = 0;
                unsigned int val = 0;
                p++;
                while (digits < 3 && *p >= '0' && *p <= '7') {
                    val = val * 8 + (unsigned int)(*p - '0');
                    p++;
                    digits++;
                }
                if (printf_bytes_append(b, (char)(val & 0xff)) != 0) return -1;
                break;
            }
            case '1':
            case '2':
            case '3':
            case '4':
            case '5':
            case '6':
            case '7': {
                int digits = 0;
                unsigned int val = 0;
                while (digits < 3 && *p >= '0' && *p <= '7') {
                    val = val * 8 + (unsigned int)(*p - '0');
                    p++;
                    digits++;
                }
                if (printf_bytes_append(b, (char)(val & 0xff)) != 0) return -1;
                break;
            }
            default:
                if (printf_bytes_append(b, '\\') != 0 ||
                    printf_bytes_append(b, *p) != 0) {
                    return -1;
                }
                p++;
                break;
        }
    }
    return 0;
}

static int builtin_printf(struct cupid_shell *shell, int argc, char **argv) {
    const char *fmt;
    const char *fmt_end;
    size_t fmt_len;
    int fmt_index = 1;
    const char *assign_var = NULL;
    FILE *out = stdout;
    FILE *capture = NULL;
    char *captured = NULL;
    int ai;
    int status = 0;
    size_t out_count = 0;
    int stop_output = 0;
    time_t now_snapshot = time(NULL);

    if (argc >= 2 && strcmp(argv[1], "--") == 0) {
        fmt_index = 2;
    } else if (argc >= 2 && strcmp(argv[1], "-v") == 0) {
        if (argc < 4) {
            fprintf(stderr, "printf: usage: printf [-v var] format [arguments]\n");
            return 2;
        }
        assign_var = argv[2];
        fmt_index = 3;
        if (fmt_index < argc && strcmp(argv[fmt_index], "--") == 0) {
            fmt_index++;
        }
    }

    if (argc <= fmt_index) {
        fprintf(stderr, "printf: usage: printf [-v var] format [arguments]\n");
        return 2;
    }

    if (assign_var != NULL) {
        capture = tmpfile();
        if (capture == NULL) return 1;
        out = capture;
    }

    fmt = argv[fmt_index];
    fmt_len = strnlen(fmt ? fmt : "", PRINTF_FORMAT_LIMIT + 1);
    if (fmt_len > PRINTF_FORMAT_LIMIT) {
        printf_error(shell, "printf: format string too long\n");
        if (capture != NULL) fclose(capture);
        return 1;
    }
    fmt_end = fmt + fmt_len;
    ai = fmt_index + 1;

    do {
        const char *p = fmt;
        int used_arg_conv = 0;
        while (p < fmt_end && !stop_output) {
            if (*p == '\\') {
                if (printf_process_format_backslash(&p, out, &out_count) != 0) {
                    status = 1;
                    break;
                }
                continue;
            }
            if (*p != '%') {
                if (printf_write_char(out, &out_count, *p++) != 0) {
                    status = 1;
                    break;
                }
                continue;
            }
            p++;
            if (p < fmt_end && *p == '%') {
                if (printf_write_char(out, &out_count, '%') != 0) {
                    status = 1;
                    break;
                }
                p++;
                continue;
            }
            {
                struct printf_spec spec;
                char cfmt[64];
                char *w = cfmt;
                const char *arg;
                int left = 0;
                if (printf_parse_spec(&spec, shell, &p, fmt_end, &ai, argc, argv, &status) != 0) {
                    break;
                }
                arg = (spec.uses_arg && ai < argc) ? argv[ai] : NULL;
                used_arg_conv |= spec.uses_arg;
                left = (strchr(spec.flags, '-') != NULL);

                if (spec.invalid_time_spec) {
                    printf_error(shell, "printf: warning: `%c': invalid time format specification\n", spec.conv);
                    if (printf_write_bytes(out, &out_count, spec.raw_start, spec.raw_len) != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }

                if (spec.conv == 'q') {
                    char *q = printf_quote_shell_alloc(arg ? arg : "");
                    size_t qlen;
                    if (arg != NULL) ai++;
                    if (q == NULL) {
                        status = 1;
                        break;
                    }
                    qlen = strlen(q);
                    if (spec.precision_set && (size_t)spec.precision < qlen) qlen = (size_t)spec.precision;
                    if (printf_emit_padded(out, &out_count, q, qlen, spec.width_set ? spec.width : 0, left) != 0) {
                        free(q);
                        status = 1;
                        break;
                    }
                    free(q);
                    continue;
                }
                if (spec.conv == 'b') {
                    struct printf_bytes bytes = {0};
                    size_t n = 0;
                    if (arg != NULL) ai++;
                    if (printf_expand_b(shell, arg ? arg : "", &bytes, &stop_output, &status) != 0) {
                        free(bytes.data);
                        status = 1;
                        break;
                    }
                    n = bytes.len;
                    if (spec.precision_set && (size_t)spec.precision < n) n = (size_t)spec.precision;
                    if (printf_emit_padded(out, &out_count, bytes.data ? bytes.data : "", n,
                                           spec.width_set ? spec.width : 0, left) != 0) {
                        free(bytes.data);
                        status = 1;
                        break;
                    }
                    free(bytes.data);
                    continue;
                }
                if (spec.conv == 'n') {
                    char nbuf[64];
                    if (arg != NULL) {
                        snprintf(nbuf, sizeof(nbuf), "%zu", out_count);
                        if (cupid_vars_set(shell, arg, nbuf) != 0) status = 1;
                        ai++;
                    }
                    continue;
                }
                if (spec.conv == 'T' && spec.is_time) {
                    time_t t;
                    struct tm tmv;
                    char tfmt[256];
                    char outbuf[1024];
                    size_t olen;
                    if (arg != NULL) {
                        long long tv = printf_parse_signed(shell, arg, &status);
                        ai++;
                        if (tv == -1) t = now_snapshot;
                        else if (tv == -2) t = shell ? shell->start_time : now_snapshot;
                        else t = (time_t)tv;
                    } else {
                        t = now_snapshot;
                    }
                    if (spec.timefmt_len == 0) {
                        strcpy(tfmt, "%X");
                    } else {
                        size_t cp = spec.timefmt_len;
                        if (cp >= sizeof(tfmt)) cp = sizeof(tfmt) - 1;
                        memcpy(tfmt, spec.timefmt_start, cp);
                        tfmt[cp] = '\0';
                    }
                    if (localtime_r(&t, &tmv) == NULL) memset(&tmv, 0, sizeof(tmv));
                    olen = strftime(outbuf, sizeof(outbuf), tfmt, &tmv);
                    if (printf_emit_padded(out, &out_count, outbuf, olen,
                                           spec.width_set ? spec.width : 0, left) != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }

                *w++ = '%';
                if (spec.flags[0] != '\0') {
                    size_t fl = strlen(spec.flags);
                    memcpy(w, spec.flags, fl);
                    w += fl;
                }
                if (spec.width_set) w += snprintf(w, (size_t)(cfmt + sizeof(cfmt) - w), "%d", spec.width);
                if (spec.precision_set) w += snprintf(w, (size_t)(cfmt + sizeof(cfmt) - w), ".%d", spec.precision);

                if (strchr("diuoxX", (int)spec.conv) != NULL) {
                    *w++ = 'l';
                    *w++ = 'l';
                }
                *w++ = spec.conv;
                *w = '\0';

                if (strchr("di", (int)spec.conv) != NULL) {
                    long long v = printf_parse_signed(shell, arg ? arg : "0", &status);
                    if (arg != NULL) ai++;
                    if (printf_format_and_emit(out, &out_count, cfmt, v, 0, 0.0, NULL, 0, 'i') != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }
                if (strchr("uoxX", (int)spec.conv) != NULL) {
                    unsigned long long v = printf_parse_unsigned(shell, arg ? arg : "0", &status);
                    if (arg != NULL) ai++;
                    if (printf_format_and_emit(out, &out_count, cfmt, 0, v, 0.0, NULL, 0, 'u') != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }
                if (strchr("aAeEfFgG", (int)spec.conv) != NULL) {
                    double v = printf_parse_double(shell, arg ? arg : "0", &status);
                    if (arg != NULL) ai++;
                    if (printf_format_and_emit(out, &out_count, cfmt, 0, 0, v, NULL, 0, 'f') != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }
                if (spec.conv == 's') {
                    if (arg != NULL) ai++;
                    if (printf_format_and_emit(out, &out_count, cfmt, 0, 0, 0.0, arg ? arg : "", 0, 's') != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }
                if (spec.conv == 'c') {
                    int ch = (arg != NULL && arg[0] != '\0') ? (unsigned char)arg[0] : 0;
                    if (arg != NULL) ai++;
                    if (printf_format_and_emit(out, &out_count, cfmt, 0, 0, 0.0, NULL, ch, 'c') != 0) {
                        status = 1;
                        break;
                    }
                    continue;
                }

                printf_error(shell, "printf: `%c': invalid format character\n", spec.conv);
                if (spec.uses_arg && arg != NULL) ai++;
                status = 1;
                break;
            }
        }
        if (!used_arg_conv || stop_output) break;
    } while (ai < argc);

    if (assign_var != NULL) {
        captured = read_stream_alloc(capture);
        fclose(capture);
        if (captured == NULL) return 1;
        if (cupid_vars_set(shell, assign_var, captured) != 0) {
            free(captured);
            return 1;
        }
        free(captured);
    } else {
        fflush(stdout);
    }
    return status;
}

/* ------------------------------------------------------------------ */
/*  test / [                                                          */
/* ------------------------------------------------------------------ */

struct test_ctx {
    int argc;
    char **argv;
    int pos;
};

static const char *test_current(struct test_ctx *ctx) {
    if (ctx->pos >= ctx->argc) return NULL;
    return ctx->argv[ctx->pos];
}

static void test_advance(struct test_ctx *ctx) {
    if (ctx->pos < ctx->argc) ctx->pos++;
}

static int test_is_binary_op(const char *s) {
    return (strcmp(s, "=") == 0 || strcmp(s, "!=") == 0 ||
            strcmp(s, "-eq") == 0 || strcmp(s, "-ne") == 0 ||
            strcmp(s, "-lt") == 0 || strcmp(s, "-le") == 0 ||
            strcmp(s, "-gt") == 0 || strcmp(s, "-ge") == 0);
}

static int test_is_unary_op(const char *s) {
    return (strcmp(s, "-z") == 0 || strcmp(s, "-n") == 0 ||
            strcmp(s, "-e") == 0 || strcmp(s, "-f") == 0 ||
            strcmp(s, "-d") == 0 || strcmp(s, "-r") == 0 ||
            strcmp(s, "-w") == 0 || strcmp(s, "-x") == 0 ||
            strcmp(s, "-v") == 0 ||
            strcmp(s, "-s") == 0);
}

static int parse_test_var_ref(const char *arg, char **name_out, char **subscript_out) {
    const char *lb;
    const char *rb;
    char *name;
    char *subscript;
    size_t i;
    if (arg == NULL || arg[0] == '\0') return 0;
    lb = strchr(arg, '[');
    if (lb == NULL) {
        if (!(isalpha((unsigned char)arg[0]) || arg[0] == '_')) return 0;
        for (i = 1; arg[i] != '\0'; i++) {
            if (!(isalnum((unsigned char)arg[i]) || arg[i] == '_')) return 0;
        }
        name = strdup(arg);
        if (name == NULL) return -1;
        *name_out = name;
        *subscript_out = NULL;
        return 1;
    }
    rb = strchr(lb + 1, ']');
    if (lb == arg || rb == NULL || rb[1] != '\0') return 0;
    name = calloc((size_t)(lb - arg) + 1, 1);
    subscript = calloc((size_t)(rb - lb), 1);
    if (name == NULL || subscript == NULL) {
        free(name);
        free(subscript);
        return -1;
    }
    memcpy(name, arg, (size_t)(lb - arg));
    memcpy(subscript, lb + 1, (size_t)(rb - lb - 1));
    if (!(isalpha((unsigned char)name[0]) || name[0] == '_')) {
        free(name);
        free(subscript);
        return 0;
    }
    for (i = 1; name[i] != '\0'; i++) {
        if (!(isalnum((unsigned char)name[i]) || name[i] == '_')) {
            free(name);
            free(subscript);
            return 0;
        }
    }
    if (subscript[0] == '\0') {
        free(name);
        free(subscript);
        return 0;
    }
    *name_out = name;
    *subscript_out = subscript;
    return 1;
}

static int test_var_is_set(struct cupid_shell *shell, const char *arg) {
    char *name = NULL;
    char *subscript = NULL;
    int parsed = parse_test_var_ref(arg, &name, &subscript);
    int result = 1;
    if (parsed <= 0) return 1;
    if (subscript == NULL) {
        result = (cupid_vars_get(shell, name) != NULL || cupid_array_has_index(shell, name, 0)) ? 0 : 1;
    } else if (strcmp(subscript, "@") == 0 || strcmp(subscript, "*") == 0) {
        if (cupid_array_exists(shell, name)) {
            if (cupid_array_is_associative(shell, name)) {
                result = cupid_array_has_key(shell, name, subscript) ? 0 : 1;
            } else {
                result = cupid_array_member_count(shell, name) > 0 ? 0 : 1;
            }
        } else {
            result = cupid_vars_get(shell, name) != NULL ? 0 : 1;
        }
    } else {
        char *end = NULL;
        unsigned long idx = strtoul(subscript, &end, 10);
        if (*subscript != '\0' && *end == '\0') {
            result = cupid_array_has_index(shell, name, (size_t)idx) ? 0 : 1;
            if (result != 0 && idx == 0 && cupid_vars_get(shell, name) != NULL) result = 0;
        } else {
            result = cupid_array_has_key(shell, name, subscript) ? 0 : 1;
        }
    }
    free(name);
    free(subscript);
    return result;
}

static int test_eval_unary(struct cupid_shell *shell, const char *op, const char *arg) {
    struct stat st;
    if (strcmp(op, "-z") == 0) return arg[0] == '\0' ? 0 : 1;
    if (strcmp(op, "-n") == 0) return arg[0] != '\0' ? 0 : 1;
    if (strcmp(op, "-e") == 0) return stat(arg, &st) == 0 ? 0 : 1;
    if (strcmp(op, "-f") == 0) return (stat(arg, &st) == 0 && S_ISREG(st.st_mode)) ? 0 : 1;
    if (strcmp(op, "-d") == 0) return (stat(arg, &st) == 0 && S_ISDIR(st.st_mode)) ? 0 : 1;
    if (strcmp(op, "-r") == 0) return access(arg, R_OK) == 0 ? 0 : 1;
    if (strcmp(op, "-w") == 0) return access(arg, W_OK) == 0 ? 0 : 1;
    if (strcmp(op, "-x") == 0) return access(arg, X_OK) == 0 ? 0 : 1;
    if (strcmp(op, "-v") == 0) return test_var_is_set(shell, arg);
    if (strcmp(op, "-s") == 0) return (stat(arg, &st) == 0 && st.st_size > 0) ? 0 : 1;
    return 1;
}

static int test_eval_binary(const char *left, const char *op, const char *right) {
    if (strcmp(op, "=") == 0) return strcmp(left, right) == 0 ? 0 : 1;
    if (strcmp(op, "!=") == 0) return strcmp(left, right) != 0 ? 0 : 1;
    {
        long lv = strtol(left, NULL, 10);
        long rv = strtol(right, NULL, 10);
        if (strcmp(op, "-eq") == 0) return lv == rv ? 0 : 1;
        if (strcmp(op, "-ne") == 0) return lv != rv ? 0 : 1;
        if (strcmp(op, "-lt") == 0) return lv < rv ? 0 : 1;
        if (strcmp(op, "-le") == 0) return lv <= rv ? 0 : 1;
        if (strcmp(op, "-gt") == 0) return lv > rv ? 0 : 1;
        if (strcmp(op, "-ge") == 0) return lv >= rv ? 0 : 1;
    }
    return 1;
}

static int test_expr(struct cupid_shell *shell, struct test_ctx *ctx);

static int test_primary(struct cupid_shell *shell, struct test_ctx *ctx) {
    const char *tok = test_current(ctx);
    if (tok == NULL) return 1;

    if (strcmp(tok, "(") == 0) {
        int result;
        test_advance(ctx);
        result = test_expr(shell, ctx);
        tok = test_current(ctx);
        if (tok == NULL || strcmp(tok, ")") != 0) return 2;
        test_advance(ctx);
        return result;
    }

    if (test_is_unary_op(tok)) {
        const char *arg;
        test_advance(ctx);
        arg = test_current(ctx);
        if (arg == NULL) return 2;
        test_advance(ctx);
        return test_eval_unary(shell, tok, arg);
    }

    if (ctx->pos + 1 < ctx->argc && test_is_binary_op(ctx->argv[ctx->pos + 1])) {
        const char *left = tok;
        const char *op;
        const char *right;
        test_advance(ctx);
        op = test_current(ctx);
        test_advance(ctx);
        right = test_current(ctx);
        if (right == NULL) return 2;
        test_advance(ctx);
        return test_eval_binary(left, op, right);
    }

    test_advance(ctx);
    return tok[0] != '\0' ? 0 : 1;
}

static int test_not_expr(struct cupid_shell *shell, struct test_ctx *ctx) {
    const char *tok = test_current(ctx);
    if (tok != NULL && strcmp(tok, "!") == 0) {
        int result;
        test_advance(ctx);
        result = test_not_expr(shell, ctx);
        return result == 0 ? 1 : 0;
    }
    return test_primary(shell, ctx);
}

static int test_and_expr(struct cupid_shell *shell, struct test_ctx *ctx) {
    int result = test_not_expr(shell, ctx);
    while (1) {
        const char *tok = test_current(ctx);
        int right;
        if (tok == NULL || strcmp(tok, "-a") != 0) break;
        test_advance(ctx);
        right = test_not_expr(shell, ctx);
        if (result == 0) result = right;
    }
    return result;
}

static int test_or_expr(struct cupid_shell *shell, struct test_ctx *ctx) {
    int result = test_and_expr(shell, ctx);
    while (1) {
        const char *tok = test_current(ctx);
        int right;
        if (tok == NULL || strcmp(tok, "-o") != 0) break;
        test_advance(ctx);
        right = test_and_expr(shell, ctx);
        if (result != 0) result = right;
    }
    return result;
}

static int test_expr(struct cupid_shell *shell, struct test_ctx *ctx) {
    return test_or_expr(shell, ctx);
}

static int builtin_test(struct cupid_shell *shell, int argc, char **argv) {
    struct test_ctx ctx;
    int is_bracket = (strcmp(argv[0], "[") == 0);

    if (is_bracket) {
        if (argc < 2 || strcmp(argv[argc - 1], "]") != 0) {
            fprintf(stderr, "cupid: [: missing ]\n");
            return 2;
        }
        argc--;
    }

    if (argc <= 1) return 1;
    if (argc == 2) return argv[1][0] != '\0' ? 0 : 1;

    ctx.argc = argc;
    ctx.argv = argv;
    ctx.pos = 1;
    return test_expr(shell, &ctx);
}

/* ------------------------------------------------------------------ */
/*  read                                                              */
/* ------------------------------------------------------------------ */

static ssize_t read_line_fd(int fd, char **line, size_t *cap) {
    char *buf;
    size_t len = 0;
    size_t capacity;
    while (1) {
        char ch;
        ssize_t n = read(fd, &ch, 1);
        if (n == 0) break;
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (len + 2 > *cap) {
            size_t nc = (*cap == 0) ? 128 : (*cap * 2);
            char *next = realloc(*line, nc);
            if (next == NULL) return -1;
            *line = next;
            *cap = nc;
        }
        (*line)[len++] = ch;
        if (ch == '\n') break;
    }
    if (len == 0) return -1;
    capacity = (*cap == 0) ? 1 : *cap;
    buf = *line;
    if (buf == NULL) {
        buf = calloc(capacity, 1);
        if (buf == NULL) return -1;
        *line = buf;
        *cap = capacity;
    }
    buf[len] = '\0';
    return (ssize_t)len;
}

static int builtin_read(struct cupid_shell *shell, int argc, char **argv) {
    int raw = 0;
    int input_fd = STDIN_FILENO;
    const char *prompt = NULL;
    const char *array_name = NULL;
    int i = 1;
    char *line = NULL;
    size_t line_cap = 0;
    ssize_t nread;
    int var_start;
    const char *ifs_val;
    const char *ifs;

    while (i < argc) {
        if (strcmp(argv[i], "-r") == 0) { raw = 1; i++; }
        else if (strcmp(argv[i], "-a") == 0) {
            if (i + 1 >= argc) return 1;
            array_name = argv[i + 1];
            i += 2;
        }
        else if (strcmp(argv[i], "-p") == 0) {
            if (i + 1 >= argc) return 1;
            prompt = argv[i + 1];
            i += 2;
        }
        else if (strcmp(argv[i], "-u") == 0) {
            char *end = NULL;
            long fd;
            if (i + 1 >= argc) return 1;
            fd = strtol(argv[i + 1], &end, 10);
            if (end == argv[i + 1] || *end != '\0' || fd < 0) return 1;
            input_fd = (int)fd;
            i += 2;
        }
        else break;
    }
    var_start = i;

    if (prompt != NULL) {
        fputs(prompt, stderr);
        fflush(stderr);
    }

    nread = read_line_fd(input_fd, &line, &line_cap);
    if (nread < 0) {
        free(line);
        return 1;
    }
    if (nread > 0 && line[nread - 1] == '\n') {
        line[nread - 1] = '\0';
    }

    ifs_val = cupid_vars_get(shell, "IFS");
    ifs = (ifs_val == NULL) ? " \t\n" : ifs_val;

    if (!raw) {
        char *src = line;
        char *dst = line;
        while (*src != '\0') {
            if (*src == '\\' && src[1] != '\0') {
                src++;
            }
            *dst++ = *src++;
        }
        *dst = '\0';
    }

    if (array_name != NULL) {
        char **items = NULL;
        size_t count = 0;
        const char *p = line;
        if (ifs[0] == '\0') {
            items = calloc(1, sizeof(char *));
            if (items == NULL) {
                free(line);
                return 1;
            }
            items[0] = strdup(line);
            if (items[0] == NULL) {
                free(items);
                free(line);
                return 1;
            }
            count = 1;
        } else {
            while (*p != '\0') {
                const char *start;
                const char *end;
                char *slice;
                char **next;
                while (*p != '\0' &&
                       strchr(ifs, *p) != NULL &&
                       (*p == ' ' || *p == '\t' || *p == '\n')) {
                    p++;
                }
                if (*p == '\0') break;
                start = p;
                while (*p != '\0' && strchr(ifs, *p) == NULL) p++;
                end = p;
                slice = calloc((size_t)(end - start) + 1, 1);
                if (slice == NULL) {
                    size_t ai;
                    for (ai = 0; ai < count; ai++) free(items[ai]);
                    free(items);
                    free(line);
                    return 1;
                }
                if (end > start) memcpy(slice, start, (size_t)(end - start));
                next = realloc(items, sizeof(*next) * (count + 1));
                if (next == NULL) {
                    size_t ai;
                    free(slice);
                    for (ai = 0; ai < count; ai++) free(items[ai]);
                    free(items);
                    free(line);
                    return 1;
                }
                items = next;
                items[count++] = slice;
                if (*p != '\0' && strchr(ifs, *p) != NULL &&
                    !(*p == ' ' || *p == '\t' || *p == '\n')) {
                    p++;
                }
            }
        }
        if (cupid_array_set_list(shell, array_name, items, count) != 0) {
            size_t ai;
            for (ai = 0; ai < count; ai++) free(items[ai]);
            free(items);
            free(line);
            return 1;
        }
        {
            size_t ai;
            for (ai = 0; ai < count; ai++) free(items[ai]);
            free(items);
        }
    } else if (var_start >= argc) {
        if (cupid_vars_set(shell, "REPLY", line) != 0) {
            free(line);
            return 1;
        }
    } else {
        int var_count = argc - var_start;
        int vi;
        if (ifs[0] == '\0') {
            for (vi = 0; vi < var_count; vi++) {
                const char *val = (vi == 0) ? line : "";
                if (cupid_vars_set(shell, argv[var_start + vi], val) != 0) {
                    free(line);
                    return 1;
                }
            }
            free(line);
            return 0;
        }

        if (var_count == 1) {
            const char *start = line;
            const char *end = line + strlen(line);
            while (start < end &&
                   strchr(ifs, *start) != NULL &&
                   (*start == ' ' || *start == '\t' || *start == '\n')) {
                start++;
            }
            while (end > start &&
                   strchr(ifs, end[-1]) != NULL &&
                   (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\n')) {
                end--;
            }
            {
                char *slice = calloc((size_t)(end - start) + 1, 1);
                if (slice == NULL) {
                    free(line);
                    return 1;
                }
                if (end > start) memcpy(slice, start, (size_t)(end - start));
                if (cupid_vars_set(shell, argv[var_start], slice) != 0) {
                    free(slice);
                    free(line);
                    return 1;
                }
                free(slice);
            }
        } else {
            const char *p = line;
            for (vi = 0; vi < var_count - 1; vi++) {
                const char *start;
                const char *end;
                char *slice;
                while (*p != '\0' &&
                       strchr(ifs, *p) != NULL &&
                       (*p == ' ' || *p == '\t' || *p == '\n')) {
                    p++;
                }
                start = p;
                while (*p != '\0' && strchr(ifs, *p) == NULL) p++;
                end = p;
                slice = calloc((size_t)(end - start) + 1, 1);
                if (slice == NULL) {
                    free(line);
                    return 1;
                }
                if (end > start) memcpy(slice, start, (size_t)(end - start));
                if (cupid_vars_set(shell, argv[var_start + vi], slice) != 0) {
                    free(slice);
                    free(line);
                    return 1;
                }
                free(slice);

                if (*p == '\0') {
                    continue;
                }
                if (strchr(ifs, *p) != NULL &&
                    !(*p == ' ' || *p == '\t' || *p == '\n')) {
                    p++;
                }
                while (*p != '\0' &&
                       strchr(ifs, *p) != NULL &&
                       (*p == ' ' || *p == '\t' || *p == '\n')) {
                    p++;
                }
            }
            while (*p != '\0' &&
                   strchr(ifs, *p) != NULL &&
                   (*p == ' ' || *p == '\t' || *p == '\n')) {
                p++;
            }
            {
                const char *start = p;
                const char *end = start + strlen(start);
                char *slice;
                while (end > start &&
                       strchr(ifs, end[-1]) != NULL &&
                       (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\n')) {
                    end--;
                }
                slice = calloc((size_t)(end - start) + 1, 1);
                if (slice == NULL) {
                    free(line);
                    return 1;
                }
                if (end > start) memcpy(slice, start, (size_t)(end - start));
                if (cupid_vars_set(shell, argv[var_start + var_count - 1], slice) != 0) {
                    free(slice);
                    free(line);
                    return 1;
                }
                free(slice);
            }
        }
    }

    free(line);
    return 0;
}

/* ------------------------------------------------------------------ */
/*  eval                                                              */
/* ------------------------------------------------------------------ */

static int builtin_eval(struct cupid_shell *shell, int argc, char **argv) {
    size_t total = 0;
    char *combined;
    char *p;
    int i;
    int status;

    if (argc <= 1) return 0;

    for (i = 1; i < argc; i++) {
        if (i > 1) total++;
        total += strlen(argv[i]);
    }

    combined = calloc(total + 1, 1);
    if (combined == NULL) return 1;

    p = combined;
    for (i = 1; i < argc; i++) {
        size_t len = strlen(argv[i]);
        if (i > 1) *p++ = ' ';
        memcpy(p, argv[i], len);
        p += len;
    }

    status = cupid_shell_eval_line(shell, combined, 1);
    free(combined);
    return status;
}

/* ------------------------------------------------------------------ */
/*  exec                                                              */
/* ------------------------------------------------------------------ */

static int builtin_exec(int argc, char **argv) {
    int clean_env = 0;
    int login_shell = 0;
    const char *argv0_override = NULL;
    int i = 1;
    const char *cmd;
    char *resolved = NULL;
    char *argv0 = NULL;
    char **exec_argv = NULL;
    char *empty_env[] = {NULL};
    int j;

    while (i < argc && argv[i][0] == '-' && argv[i][1] != '\0') {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        }
        if (strcmp(argv[i], "-c") == 0) {
            clean_env = 1;
            i++;
            continue;
        }
        if (strcmp(argv[i], "-l") == 0) {
            login_shell = 1;
            i++;
            continue;
        }
        if (strcmp(argv[i], "-a") == 0) {
            if (i + 1 >= argc) return 1;
            argv0_override = argv[i + 1];
            i += 2;
            continue;
        }
        break;
    }

    if (i >= argc) return 0;
    cmd = argv[i];
    resolved = (strchr(cmd, '/') != NULL) ? strdup(cmd) : cupid_find_in_path(cmd);
    if (resolved == NULL) {
        errno = ENOENT;
        fprintf(stderr, "cupid: exec: %s: %s\n", cmd, strerror(errno));
        return 127;
    }

    argv0 = strdup(argv0_override != NULL ? argv0_override : cmd);
    if (argv0 == NULL) {
        free(resolved);
        return 1;
    }
    if (login_shell) {
        char *prefixed = calloc(strlen(argv0) + 2, 1);
        if (prefixed == NULL) {
            free(argv0);
            free(resolved);
            return 1;
        }
        prefixed[0] = '-';
        memcpy(prefixed + 1, argv0, strlen(argv0));
        free(argv0);
        argv0 = prefixed;
    }

    exec_argv = calloc((size_t)(argc - i + 1), sizeof(char *));
    if (exec_argv == NULL) {
        free(argv0);
        free(resolved);
        return 1;
    }
    exec_argv[0] = argv0;
    for (j = i + 1; j < argc; j++) {
        exec_argv[j - i] = argv[j];
    }
    exec_argv[argc - i] = NULL;

    execve(resolved, exec_argv, clean_env ? empty_env : environ);
    fprintf(stderr, "cupid: exec: %s: %s\n", cmd, strerror(errno));
    free(exec_argv);
    free(argv0);
    free(resolved);
    return (errno == ENOENT) ? 127 : 126;
}

/* ------------------------------------------------------------------ */
/*  set                                                               */
/* ------------------------------------------------------------------ */

static void set_list_named_opts(const struct cupid_shell *shell) {
    printf("allexport\t%s\n", shell->opt_allexport ? "on" : "off");
    printf("errexit\t%s\n", shell->opt_errexit ? "on" : "off");
    printf("histexpand\t%s\n", shell->opt_histexpand ? "on" : "off");
    printf("history\t%s\n", shell->opt_histexpand ? "on" : "off");
    printf("noglob\t%s\n", shell->opt_noglob ? "on" : "off");
    printf("monitor\t%s\n", shell->opt_monitor ? "on" : "off");
    printf("nounset\t%s\n", shell->opt_nounset ? "on" : "off");
    printf("xtrace\t%s\n", shell->opt_xtrace ? "on" : "off");
    printf("pipefail\t%s\n", shell->opt_pipefail ? "on" : "off");
    printf("posix\t%s\n", shell->mode == CUPID_MODE_POSIX ? "on" : "off");
}

static int set_apply_named_opt(struct cupid_shell *shell, int enable, const char *name) {
    if (strcmp(name, "allexport") == 0) shell->opt_allexport = enable;
    else if (strcmp(name, "errexit") == 0) shell->opt_errexit = enable;
    else if (strcmp(name, "histexpand") == 0) shell->opt_histexpand = enable;
    else if (strcmp(name, "history") == 0) shell->opt_histexpand = enable;
    else if (strcmp(name, "noglob") == 0) shell->opt_noglob = enable;
    else if (strcmp(name, "monitor") == 0) shell->opt_monitor = enable;
    else if (strcmp(name, "nounset") == 0) shell->opt_nounset = enable;
    else if (strcmp(name, "xtrace") == 0) shell->opt_xtrace = enable;
    else if (strcmp(name, "pipefail") == 0) shell->opt_pipefail = enable;
    else if (strcmp(name, "posix") == 0) shell->mode = enable ? CUPID_MODE_POSIX : CUPID_MODE_BASH;
    else {
        fprintf(stderr, "cupid: set: %s: invalid option\n", name);
        return (shell != NULL && shell->mode == CUPID_MODE_POSIX) ? 2 : 1;
    }
    return 0;
}

static int builtin_set(struct cupid_shell *shell, int argc, char **argv) {
    int i;
    int print_named_opts = 0;
    int err_status = (shell != NULL && shell->mode == CUPID_MODE_POSIX) ? 2 : 1;

    if (argc == 1) {
        size_t vi;
        for (vi = 0; vi < shell->vars.count; vi++) {
            printf("%s=%s\n", shell->vars.entries[vi].name,
                   shell->vars.entries[vi].value);
        }
        return 0;
    }

    for (i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            return shell_replace_params(shell, argc, argv, i + 1);
        }

        if (argv[i][0] == '-' || argv[i][0] == '+') {
            int enable = (argv[i][0] == '-');
            const char *p = argv[i] + 1;

            if (*p == 'o' && p[1] == '\0') {
                if (i + 1 >= argc) {
                    set_list_named_opts(shell);
                    return 0;
                }
                i++;
                {
                    int rc = set_apply_named_opt(shell, enable, argv[i]);
                    if (rc != 0) return posix_special_builtin_error(shell, rc);
                }
                continue;
            }

            while (*p != '\0') {
                switch (*p) {
                    case 'a': shell->opt_allexport = enable; break;
                    case 'e': shell->opt_errexit = enable; break;
                    case 'f': shell->opt_noglob = enable; break;
                    case 'H': shell->opt_histexpand = enable; break;
                    case 'm': shell->opt_monitor = enable; break;
                    case 'p': break;
                    case 'u': shell->opt_nounset = enable; break;
                    case 'x': shell->opt_xtrace = enable; break;
                    case 'o':
                        if (p[1] != '\0') {
                            fprintf(stderr, "cupid: set: -%c: invalid option\n", p[1]);
                            return posix_special_builtin_error(shell, err_status);
                        }
                        if (i + 1 >= argc) {
                            print_named_opts = 1;
                        } else {
                            i++;
                            {
                                int rc = set_apply_named_opt(shell, enable, argv[i]);
                                if (rc != 0) return posix_special_builtin_error(shell, rc);
                            }
                        }
                        p = "";
                        continue;
                    default:
                        fprintf(stderr, "cupid: set: -%c: invalid option\n", *p);
                        return posix_special_builtin_error(shell, err_status);
                }
                p++;
            }
            continue;
        }

        return shell_replace_params(shell, argc, argv, i);
    }
    if (print_named_opts) {
        set_list_named_opts(shell);
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  trap                                                              */
/* ------------------------------------------------------------------ */

static int signal_name_to_num(const char *name) {
    char *end;
    long num;

    if (strcmp(name, "EXIT") == 0 || strcmp(name, "0") == 0) return 0;
    if (strcmp(name, "HUP") == 0 || strcmp(name, "SIGHUP") == 0 || strcmp(name, "1") == 0) return SIGHUP;
    if (strcmp(name, "INT") == 0 || strcmp(name, "SIGINT") == 0 || strcmp(name, "2") == 0) return SIGINT;
    if (strcmp(name, "QUIT") == 0 || strcmp(name, "SIGQUIT") == 0 || strcmp(name, "3") == 0) return SIGQUIT;
    if (strcmp(name, "TERM") == 0 || strcmp(name, "SIGTERM") == 0 || strcmp(name, "15") == 0) return SIGTERM;
    if (strcmp(name, "USR1") == 0 || strcmp(name, "SIGUSR1") == 0 || strcmp(name, "10") == 0) return SIGUSR1;
    if (strcmp(name, "USR2") == 0 || strcmp(name, "SIGUSR2") == 0 || strcmp(name, "12") == 0) return SIGUSR2;

    num = strtol(name, &end, 10);
    if (*end == '\0' && num >= 0 && num <= CUPID_MAX_TRAP_SIGNAL) return (int)num;
    return -1;
}

static const char *signal_num_to_name(int signo) {
    if (signo == 0) return "EXIT";
    if (signo == SIGHUP) return "HUP";
    if (signo == SIGINT) return "INT";
    if (signo == SIGQUIT) return "QUIT";
    if (signo == SIGTERM) return "TERM";
    if (signo == SIGUSR1) return "USR1";
    if (signo == SIGUSR2) return "USR2";
    return NULL;
}

struct cupid_signal_info {
    int signo;
    const char *name;
};

static const struct cupid_signal_info g_signal_info[] = {
    { SIGHUP, "HUP" },
    { SIGINT, "INT" },
    { SIGQUIT, "QUIT" },
    { SIGUSR1, "USR1" },
    { SIGUSR2, "USR2" },
    { SIGTERM, "TERM" },
};

static void print_signal_list(void) {
    size_t i;
    for (i = 0; i < sizeof(g_signal_info) / sizeof(g_signal_info[0]); i++) {
        printf(" %d) %s%s", g_signal_info[i].signo, g_signal_info[i].name,
               (i + 1 == sizeof(g_signal_info) / sizeof(g_signal_info[0])) ? "\n" : " ");
    }
}

static int builtin_trap(struct cupid_shell *shell, int argc, char **argv) {
    int i;

    if (argc == 1) {
        for (i = 0; i <= CUPID_MAX_TRAP_SIGNAL; i++) {
            if (shell->traps[i] != NULL) {
                const char *name = signal_num_to_name(i);
                printf("trap -- '%s' %s\n", shell->traps[i], name ? name : "???");
            }
        }
        return 0;
    }

    if (argc == 2 && strcmp(argv[1], "-l") == 0) {
        print_signal_list();
        return 0;
    }

    if (argc < 3) {
        fprintf(stderr, "cupid: trap: usage: trap command signal\n");
        return 1;
    }

    {
        const char *handler = argv[1];
        int is_default = (strcmp(handler, "-") == 0);

        for (i = 2; i < argc; i++) {
            int signo = signal_name_to_num(argv[i]);
            if (signo < 0 || signo > CUPID_MAX_TRAP_SIGNAL) {
                fprintf(stderr, "cupid: trap: %s: invalid signal\n", argv[i]);
                return 1;
            }

            free(shell->traps[signo]);
            if (is_default) {
                shell->traps[signo] = NULL;
                if (signo > 0) signal(signo, SIG_DFL);
            } else {
                shell->traps[signo] = strdup(handler);
                if (signo > 0 && handler[0] == '\0') {
                    signal(signo, SIG_IGN);
                }
            }
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  wait                                                              */
/* ------------------------------------------------------------------ */

static int builtin_wait(void) {
    int status = 0;
    int st;
    while (waitpid(-1, &st, 0) > 0) {
        if (WIFEXITED(st)) status = WEXITSTATUS(st);
        else if (WIFSIGNALED(st)) status = 128 + WTERMSIG(st);
    }
    return status;
}

static void append_umask_perm(char **p, mode_t perms, mode_t bit, char ch) {
    if ((perms & bit) != 0) *(*p)++ = ch;
}

static int parse_umask_perms(const char *text, mode_t *perms_out, size_t *used_out) {
    size_t used = 0;
    mode_t perms = 0;

    while (text[used] == 'r' || text[used] == 'w' || text[used] == 'x') {
        if (text[used] == 'r') perms |= 4;
        else if (text[used] == 'w') perms |= 2;
        else perms |= 1;
        used++;
    }
    *perms_out = perms;
    *used_out = used;
    return 0;
}

static int parse_umask_clause(const char *text, mode_t *mask_io, size_t *used_out) {
    size_t i = 0;
    int who_mask = 0;
    char op;
    mode_t perms;
    size_t perms_used;
    mode_t current;
    mode_t next;

    #define APPLY_UMASK_CLASS(shift_value, class_bit)                                      \
        do {                                                                               \
            if ((who_mask & (class_bit)) != 0) {                                           \
                current = (mode_t)(7 & ~((*mask_io >> (shift_value)) & 7));                \
                next = current;                                                            \
                if (op == '=') next = perms;                                              \
                else if (op == '+') next = (mode_t)(current | perms);                     \
                else next = (mode_t)(current & ~perms);                                   \
                *mask_io &= (mode_t)~(7u << (shift_value));                               \
                *mask_io |= (mode_t)((7 & ~next) << (shift_value));                       \
            }                                                                              \
        } while (0)

    while (text[i] == 'u' || text[i] == 'g' || text[i] == 'o' || text[i] == 'a') {
        if (text[i] == 'u') who_mask |= 1;
        else if (text[i] == 'g') who_mask |= 2;
        else if (text[i] == 'o') who_mask |= 4;
        else who_mask |= 7;
        i++;
    }
    if (who_mask == 0) who_mask = 7;
    op = text[i];
    if (op != '=' && op != '+' && op != '-') return -1;
    i++;
    if (parse_umask_perms(text + i, &perms, &perms_used) != 0) return -1;
    i += perms_used;
    if (text[i] != '\0' && text[i] != ',') return -1;

    APPLY_UMASK_CLASS(6, 1);
    APPLY_UMASK_CLASS(3, 2);
    APPLY_UMASK_CLASS(0, 4);

    *used_out = i;
    return 0;

    #undef APPLY_UMASK_CLASS
}

static int parse_umask_value(const char *text, mode_t current, mode_t *mask_out) {
    char *end = NULL;
    long val = strtol(text, &end, 8);
    mode_t mask = current;

    if (*text == '\0') return -1;
    if (*end == '\0') {
        if (val < 0 || val > 0777) return -1;
        *mask_out = (mode_t)val;
        return 0;
    }

    while (*text != '\0') {
        size_t used = 0;
        if (parse_umask_clause(text, &mask, &used) != 0) return -1;
        text += used;
        if (*text == ',') text++;
    }
    *mask_out = mask;
    return 0;
}

static void format_umask_symbolic(mode_t mask, char *buf, size_t size) {
    mode_t u = (mode_t)(7 & ~((mask >> 6) & 7));
    mode_t g = (mode_t)(7 & ~((mask >> 3) & 7));
    mode_t o = (mode_t)(7 & ~(mask & 7));
    char *p = buf;
    if (size == 0) return;
    *p++ = 'u';
    *p++ = '=';
    append_umask_perm(&p, u, 4, 'r');
    append_umask_perm(&p, u, 2, 'w');
    append_umask_perm(&p, u, 1, 'x');
    *p++ = ',';
    *p++ = 'g';
    *p++ = '=';
    append_umask_perm(&p, g, 4, 'r');
    append_umask_perm(&p, g, 2, 'w');
    append_umask_perm(&p, g, 1, 'x');
    *p++ = ',';
    *p++ = 'o';
    *p++ = '=';
    append_umask_perm(&p, o, 4, 'r');
    append_umask_perm(&p, o, 2, 'w');
    append_umask_perm(&p, o, 1, 'x');
    *p = '\0';
}

static int builtin_umask(int argc, char **argv) {
    int symbolic = 0;
    int print_cmd = 0;
    int i = 1;
    mode_t current;
    mode_t new_mask = 0;
    int have_mask = 0;

    while (i < argc) {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        }
        if (strcmp(argv[i], "-S") == 0) {
            symbolic = 1;
            i++;
            continue;
        }
        if (strcmp(argv[i], "-p") == 0) {
            print_cmd = 1;
            i++;
            continue;
        }
        break;
    }

    current = umask(0);
    if (i < argc) {
        if (parse_umask_value(argv[i], current, &new_mask) != 0) {
            umask(current);
            return 1;
        }
        have_mask = 1;
        i++;
    }
    if (i != argc) {
        umask(current);
        return 1;
    }

    umask(have_mask ? new_mask : current);
    if (have_mask) return 0;

    if (symbolic) {
        char buf[32];
        format_umask_symbolic(current, buf, sizeof(buf));
        if (print_cmd) printf("umask -S %s\n", buf);
        else printf("%s\n", buf);
    } else if (print_cmd) {
        printf("umask %04o\n", current);
    } else {
        printf("%04o\n", current);
    }
    return 0;
}

static int builtin_ulimit(int argc, char **argv) {
    int soft = 0;
    int hard = 0;
    int resource = RLIMIT_CORE;
    int i = 1;
    struct rlimit lim;

    while (i < argc) {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        }
        if (argv[i][0] != '-' || argv[i][1] == '\0') break;
        {
            const char *p = argv[i] + 1;
            while (*p != '\0') {
                if (*p == 'S') soft = 1;
                else if (*p == 'H') hard = 1;
                else if (*p == 'c') resource = RLIMIT_CORE;
                else if (*p == 'n') resource = RLIMIT_NOFILE;
                else return 1;
                p++;
            }
        }
        i++;
    }
    if (getrlimit(resource, &lim) != 0) return 1;
    if (i < argc) {
        rlim_t value;
        if (strcmp(argv[i], "unlimited") == 0) value = RLIM_INFINITY;
        else {
            char *end = NULL;
            long parsed = strtol(argv[i], &end, 10);
            if (*argv[i] == '\0' || *end != '\0' || parsed < 0) return 1;
            value = (rlim_t)parsed;
        }
        if (!soft && !hard) {
            lim.rlim_cur = value;
            lim.rlim_max = value;
        } else {
            if (soft) lim.rlim_cur = value;
            if (hard) lim.rlim_max = value;
        }
        if (setrlimit(resource, &lim) != 0) return 1;
        return 0;
    }
    if (soft || !hard) {
        if (lim.rlim_cur == RLIM_INFINITY) printf("unlimited\n");
        else printf("%lld\n", (long long)lim.rlim_cur);
    } else {
        if (lim.rlim_max == RLIM_INFINITY) printf("unlimited\n");
        else printf("%lld\n", (long long)lim.rlim_max);
    }
    return 0;
}

static int builtin_enable(struct cupid_shell *shell, int argc, char **argv) {
    int disable = 0;
    int print_only = 0;
    int show_all = 0;
    int show_special = 0;
    int i = 1;
    const char *const *names;
    int status = 0;
    while (i < argc && argv[i][0] == '-' && argv[i][1] != '\0') {
        const char *p = argv[i] + 1;
        while (*p != '\0') {
            if (*p == 'n') disable = 1;
            else if (*p == 'p') print_only = 1;
            else if (*p == 's') {
                print_only = 1;
                show_special = 1;
            } else if (*p == 'a') {
                print_only = 1;
                show_all = 1;
            }
            else return 1;
            p++;
        }
        i++;
    }
    if (i >= argc || print_only) {
        names = show_special ? g_special_builtin_names : cupid_builtin_names();
        for (; *names != NULL; names++) {
            int enabled = builtin_is_enabled(shell, *names);
            if (show_all || (disable ? !enabled : enabled)) {
                printf("enable %s%s\n", enabled ? "" : "-n ", *names);
            }
        }
        if (i >= argc) return 0;
    }
    for (; i < argc; i++) {
        if (!cupid_is_builtin(argv[i])) {
            fprintf(stderr, "cupid: enable: %s: not a shell builtin\n", argv[i]);
            status = 1;
            continue;
        }
        if (set_builtin_enabled(shell, argv[i], disable ? 0 : 1) != 0) {
            status = 1;
        }
    }
    return status;
}

static int builtin_times(void) {
    struct tms t;
    clock_t ticks = times(&t);
    long hz = sysconf(_SC_CLK_TCK);
    if (ticks == (clock_t)-1 || hz <= 0) return 1;
    printf("%ldm%0.3fs %ldm%0.3fs\n",
           (long)(t.tms_utime / hz), (double)(t.tms_utime % hz) / (double)hz,
           (long)(t.tms_stime / hz), (double)(t.tms_stime % hz) / (double)hz);
    printf("%ldm%0.3fs %ldm%0.3fs\n",
           (long)(t.tms_cutime / hz), (double)(t.tms_cutime % hz) / (double)hz,
           (long)(t.tms_cstime / hz), (double)(t.tms_cstime % hz) / (double)hz);
    return 0;
}

/* ------------------------------------------------------------------ */
/*  type                                                              */
/* ------------------------------------------------------------------ */

char *cupid_find_in_path(const char *name) {
    const char *path_env = getenv("PATH");
    const char *p;

    if (path_env == NULL) return NULL;
    if (strchr(name, '/') != NULL) {
        if (access(name, X_OK) == 0) return strdup(name);
        return NULL;
    }

    p = path_env;
    while (*p != '\0') {
        const char *end = strchr(p, ':');
        size_t dir_len;
        size_t name_len = strlen(name);
        char *full;

        if (end == NULL) end = p + strlen(p);
        dir_len = (size_t)(end - p);

        full = calloc(dir_len + name_len + 2, 1);
        if (full == NULL) return NULL;
        if (dir_len > 0) {
            memcpy(full, p, dir_len);
            full[dir_len] = '/';
            memcpy(full + dir_len + 1, name, name_len);
        } else {
            memcpy(full, name, name_len);
        }

        if (access(full, X_OK) == 0) return full;
        free(full);

        p = (*end == ':') ? end + 1 : end;
    }
    return NULL;
}

static int builtin_type(struct cupid_shell *shell, int argc, char **argv) {
    int flag_all = 0;
    int flag_suppress_func = 0;
    int flag_path = 0;
    int flag_force_path = 0;
    int flag_kind = 0;
    int status = 0;
    int i;

    for (i = 1; i < argc && argv[i][0] == '-' && argv[i][1] != '\0'; i++) {
        const char *p = argv[i] + 1;
        while (*p != '\0') {
            if (*p == 'a') flag_all = 1;
            else if (*p == 'f') flag_suppress_func = 1;
            else if (*p == 'p') flag_path = 1;
            else if (*p == 'P') flag_force_path = 1;
            else if (*p == 't') flag_kind = 1;
            else {
                cupid_shell_error_prefix(stderr, shell);
                fprintf(stderr, "type: -%c: invalid option\n", *p);
                fprintf(stderr, "type: usage: type [-afptP] name [name ...]\n");
                return 2;
            }
            p++;
        }
    }

    for (; i < argc; i++) {
        const char *alias_val = NULL;
        char *path = NULL;
        int hashed = 0;
        enum name_kind kind = resolve_name_kind(shell, argv[i], 1, !flag_suppress_func,
                                                flag_force_path, &alias_val, &path, &hashed);
        if (flag_kind) {
            if (kind == NAME_ALIAS) puts("alias");
            else if (kind == NAME_KEYWORD) puts("keyword");
            else if (kind == NAME_FUNCTION) puts("function");
            else if (kind == NAME_BUILTIN) puts("builtin");
            else if (kind == NAME_FILE || kind == NAME_HASHED) puts("file");
            else status = 1;
        } else if (flag_path || flag_force_path) {
            if ((kind == NAME_FILE || kind == NAME_HASHED) && path != NULL) {
                puts(path);
            } else {
                status = 1;
            }
        } else if (flag_all) {
            int found = 0;
            if (shell != NULL && shell->opt_expand_aliases) {
                alias_val = cupid_alias_get(shell, argv[i]);
                if (alias_val != NULL) {
                    describe_name(stdout, argv[i], NAME_ALIAS, alias_val, NULL);
                    found = 1;
                }
            }
            if (is_shell_keyword_name(argv[i])) {
                describe_name(stdout, argv[i], NAME_KEYWORD, NULL, NULL);
                found = 1;
            }
            if (!flag_suppress_func && cupid_func_get(shell, argv[i]) != NULL) {
                describe_name(stdout, argv[i], NAME_FUNCTION, NULL, NULL);
                found = 1;
            }
            if (builtin_is_enabled(shell, argv[i])) {
                describe_name(stdout, argv[i], NAME_BUILTIN, NULL, NULL);
                found = 1;
            }
            if (path == NULL && !hashed) path = cupid_find_in_path(argv[i]);
            if (path != NULL) {
                describe_name(stdout, argv[i], hashed ? NAME_HASHED : NAME_FILE, NULL, path);
                found = 1;
            }
            if (!found) status = 1;
        } else {
            if (kind != NAME_NONE) {
                describe_name(stdout, argv[i], kind, alias_val, path);
                if (kind == NAME_FUNCTION) {
                    if (print_declared_function(shell, argv[i]) != 0) {
                        status = 1;
                    }
                }
            } else {
                cupid_shell_error_prefix(stderr, shell);
                fprintf(stderr, "type: %s: not found\n", argv[i]);
                status = 1;
            }
        }
        if (path != NULL) {
            free(path);
        }
    }
    return status;
}

static int builtin_command(struct cupid_shell *shell, int argc, char **argv, bool in_child) {
    int i = 1;
    int verbose = 0;
    int plain = 0;
    int use_default_path = 0;

    while (i < argc && argv[i][0] == '-') {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        }
        if (strcmp(argv[i], "-v") == 0) {
            plain = 1;
            i++;
            continue;
        }
        if (strcmp(argv[i], "-V") == 0) {
            verbose = 1;
            i++;
            continue;
        }
        if (strcmp(argv[i], "-p") == 0) {
            use_default_path = 1;
            i++;
            continue;
        }
        break;
    }

    if (i >= argc) return 0;

    if (plain || verbose) {
        int j;
        int status = 0;
        for (j = i; j < argc; j++) {
            const char *alias_val = NULL;
            char *path = NULL;
            int hashed = 0;
            enum name_kind kind = resolve_name_kind(shell, argv[j], 1, 1, use_default_path ? 0 : 0,
                                                    &alias_val, &path, &hashed);
            if (kind == NAME_NONE) {
                status = 1;
            } else if (plain) {
                if (kind == NAME_ALIAS) printf("alias %s='%s'\n", argv[j], alias_val ? alias_val : "");
                else if (kind == NAME_FILE || kind == NAME_HASHED) printf("%s\n", path);
                else printf("%s\n", argv[j]);
            } else {
                describe_name(stdout, argv[j], kind, alias_val, path);
                if (kind == NAME_FUNCTION) {
                    if (print_declared_function(shell, argv[j]) != 0) status = 1;
                }
            }
            free(path);
        }
        return status;
    }

    {
        int new_argc = argc - i;
        char **new_argv = argv + i;
        int status;

        if (new_argc <= 0) return 0;
        if (new_argv[0] == NULL || new_argv[0][0] == '\0') {
            cupid_shell_error_prefix(stderr, shell);
            fputs(": command not found\n", stderr);
            return 127;
        }

        if (shell != NULL) shell->suppress_special_builtin_fatal++;
        status = cupid_run_builtin(shell, new_argc, new_argv, in_child);
        if (shell != NULL) shell->suppress_special_builtin_fatal--;
        if (status != CUPID_BUILTIN_NOT_FOUND) return status;

        if (in_child) {
            execvp(new_argv[0], new_argv);
            _exit(errno == ENOENT ? 127 : 126);
        }
        {
            pid_t pid = fork();
            int st;
            if (pid < 0) return 1;
            if (pid == 0) {
                execvp(new_argv[0], new_argv);
                _exit(errno == ENOENT ? 127 : 126);
            }
            waitpid(pid, &st, 0);
            if (WIFEXITED(st)) return WEXITSTATUS(st);
            if (WIFSIGNALED(st)) return 128 + WTERMSIG(st);
        }
    }
    return 1;
}

static char *expand_array_item_fragment(const char *text, struct cupid_shell *shell) {
    struct cupid_tokens toks = {0};
    char *result = NULL;
    size_t len = 0;
    size_t i;
    int saw_word = 0;

    if (text == NULL) return strdup("");
    if (cupid_lex(text, &toks) != 0) {
        return cupid_expand_text(text, CUPID_QUOTE_NONE, shell);
    }

    for (i = 0; i < toks.count; i++) {
        char *expanded;
        char *next;
        size_t add_len;
        if (toks.items[i].kind != TOK_WORD) continue;
        if (saw_word) {
            next = realloc(result, len + 2);
            if (next == NULL) {
                free(result);
                cupid_tokens_free(&toks);
                return NULL;
            }
            result = next;
            result[len++] = ' ';
            result[len] = '\0';
        }
        saw_word = 1;
        expanded = cupid_expand_word(&toks.items[i].word, shell);
        if (expanded == NULL) {
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        add_len = strlen(expanded);
        next = realloc(result, len + add_len + 1);
        if (next == NULL) {
            free(expanded);
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        result = next;
        memcpy(result + len, expanded, add_len);
        len += add_len;
        result[len] = '\0';
        free(expanded);
    }

    cupid_tokens_free(&toks);
    if (result == NULL) result = strdup("");
    return result;
}

static int split_array_item_source(const char *source, char **key_out, char **value_out) {
    const char *p;
    int mode = 0;

    if (key_out == NULL || value_out == NULL) return -1;
    *key_out = NULL;
    *value_out = NULL;
    if (source == NULL || source[0] != '[') return 0;

    p = source + 1;
    while (*p != '\0') {
        if (mode == 1) {
            if (*p == '\'') mode = 0;
            p++;
            continue;
        }
        if (mode == 2) {
            if (*p == '\\' && p[1] != '\0') {
                p += 2;
                continue;
            }
            if (*p == '"') mode = 0;
            p++;
            continue;
        }
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (*p == '\'') {
            mode = 1;
            p++;
            continue;
        }
        if (*p == '"') {
            mode = 2;
            p++;
            continue;
        }
        if (*p == ']' && p[1] == '=') {
            size_t key_len = (size_t)(p - (source + 1));
            *key_out = calloc(key_len + 1, 1);
            *value_out = strdup(p + 2);
            if (*key_out == NULL || *value_out == NULL) {
                free(*key_out);
                free(*value_out);
                *key_out = NULL;
                *value_out = NULL;
                return -1;
            }
            memcpy(*key_out, source + 1, key_len);
            return 1;
        }
        p++;
    }
    return 0;
}

static int parse_array_literal_words(struct cupid_shell *shell, const char *value,
                                     char ***out_items, size_t *out_count) {
    size_t len;
    char *inner;
    struct cupid_tokens toks = {0};
    char **items = NULL;
    size_t count = 0;
    size_t i;
    if (value == NULL) return -1;
    len = strlen(value);
    if (len < 2 || value[0] != '(' || value[len - 1] != ')') return -1;
    inner = calloc(len - 1, 1);
    if (inner == NULL) return -1;
    memcpy(inner, value + 1, len - 2);
    if (cupid_lex(inner, &toks) != 0) {
        free(inner);
        return -1;
    }
    free(inner);
    for (i = 0; i < toks.count; i++) {
        char *source;
        char *key_src = NULL;
        char *value_src = NULL;
        char *expanded = NULL;
        char *copy;
        char **next;
        int split_rc;
        if (toks.items[i].kind != TOK_WORD) continue;
        source = cupid_word_source_text(&toks.items[i].word);
        split_rc = (source != NULL) ? split_array_item_source(source, &key_src, &value_src) : 0;
        if (split_rc < 0) {
            free(source);
            cupid_tokens_free(&toks);
            while (count > 0) free(items[--count]);
            free(items);
            return -1;
        }
        if (split_rc > 0) {
            char *expanded_key = expand_array_item_fragment(key_src, shell);
            char *expanded_value = expand_array_item_fragment(value_src, shell);
            size_t total_len;
            if (expanded_key == NULL || expanded_value == NULL) {
                free(source);
                free(key_src);
                free(value_src);
                free(expanded_key);
                free(expanded_value);
                cupid_tokens_free(&toks);
                while (count > 0) free(items[--count]);
                free(items);
                return -1;
            }
            total_len = strlen(expanded_key) + strlen(expanded_value) + 4;
            expanded = calloc(total_len, 1);
            if (expanded == NULL) {
                free(source);
                free(key_src);
                free(value_src);
                free(expanded_key);
                free(expanded_value);
                cupid_tokens_free(&toks);
                while (count > 0) free(items[--count]);
                free(items);
                return -1;
            }
            snprintf(expanded, total_len, "[%s]=%s", expanded_key, expanded_value);
            free(expanded_key);
            free(expanded_value);
        } else {
            expanded = cupid_expand_word(&toks.items[i].word, shell);
        }
        free(source);
        free(key_src);
        free(value_src);
        if (expanded == NULL) {
            cupid_tokens_free(&toks);
            while (count > 0) free(items[--count]);
            free(items);
            return -1;
        }
        copy = strdup(expanded);
        free(expanded);
        if (copy == NULL) {
            cupid_tokens_free(&toks);
            while (count > 0) free(items[--count]);
            free(items);
            return -1;
        }
        next = realloc(items, sizeof(*next) * (count + 1));
        if (next == NULL) {
            free(copy);
            cupid_tokens_free(&toks);
            while (count > 0) free(items[--count]);
            free(items);
            return -1;
        }
        items = next;
        items[count++] = copy;
    }
    cupid_tokens_free(&toks);
    *out_items = items;
    *out_count = count;
    return 0;
}

static struct cupid_array *find_shell_array(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->arrays.count; i++) {
        if (strcmp(shell->arrays.entries[i].name, name) == 0) {
            return &shell->arrays.entries[i];
        }
    }
    return NULL;
}

static void print_declare_quoted_value(const char *value) {
    const unsigned char *p = (const unsigned char *)(value ? value : "");
    int needs_ansi_c = 0;

    while (*p != '\0') {
        if (*p < 32 || *p == 127 || *p >= 128) {
            needs_ansi_c = 1;
            break;
        }
        p++;
    }
    p = (const unsigned char *)(value ? value : "");

    if (needs_ansi_c) {
        printf("$'");
        while (*p != '\0') {
            if (*p == '\\' || *p == '\'') {
                putchar('\\');
                putchar((int)*p);
            } else if (*p == '\n') {
                fputs("\\n", stdout);
            } else if (*p == '\t') {
                fputs("\\t", stdout);
            } else if (*p < 32 || *p == 127 || *p >= 128) {
                printf("\\%03o", (unsigned int)*p);
            } else {
                putchar((int)*p);
            }
            p++;
        }
        putchar('\'');
        return;
    }

    putchar('"');
    while (*p != '\0') {
        if (*p == '\\' || *p == '"' || *p == '$' || *p == '`') putchar('\\');
        putchar((int)*p);
        p++;
    }
    putchar('"');
}

static int print_declared_name(struct cupid_shell *shell, const char *name) {
    struct cupid_var *var = NULL;
    struct cupid_array *array;
    size_t i;

    if (shell == NULL || name == NULL) return 1;

    for (i = 0; i < shell->vars.count; i++) {
        if (strcmp(shell->vars.entries[i].name, name) == 0) {
            var = &shell->vars.entries[i];
            break;
        }
    }
    array = find_shell_array(shell, name);

    if (array != NULL) {
        printf("declare -%c", array->associative ? 'A' : 'a');
        if (var != NULL && var->integer) printf("i");
        printf(" %s", name);
        if (array->count > 0) {
            printf("=(");
            for (i = 0; i < array->count; i++) {
                const char *key = cupid_array_member_key(shell, name, i);
                const char *value = cupid_array_member_value(shell, name, i);
                if (array->associative) {
                    printf("[%s]=", key);
                    print_declare_quoted_value(value);
                    putchar(' ');
                } else {
                    if (i > 0) putchar(' ');
                    printf("[%s]=", key);
                    print_declare_quoted_value(value);
                }
            }
            putchar(')');
        }
        putchar('\n');
        return 0;
    }

    if (var != NULL) {
        printf("declare ");
        if (var->exported) printf("-x ");
        if (var->readonly) printf("-r ");
        if (var->integer) printf("-i ");
        if (var->uppercase) printf("-u ");
        if (var->nameref_target != NULL && var->nameref_target[0] != '\0') {
            printf("-n %s=", name);
            print_declare_quoted_value(var->nameref_target);
        } else {
            if (!var->exported && !var->readonly && !var->integer && !var->uppercase) {
                printf("-- ");
            }
            printf("%s=", name);
            print_declare_quoted_value(var->value);
        }
        putchar('\n');
        return 0;
    }

    if (getenv(name) != NULL) {
        printf("declare -x %s=", name);
        print_declare_quoted_value(getenv(name));
        putchar('\n');
        return 0;
    }

    cupid_shell_error_prefix(stderr, shell);
    fprintf(stderr, "declare: %s: not found\n", name);
    return 1;
}

static void print_word_literal(const struct cupid_word *word) {
    char *text;
    char *normalized;
    if (word == NULL) return;
    text = cupid_word_source_text(word);
    if (text == NULL) return;
    normalized = NULL;
    normalized = normalize_function_word_text(text);
    if (normalized != NULL) {
        fputs(normalized, stdout);
        free(normalized);
    } else {
        fputs(text, stdout);
    }
    free(text);
}

static int function_source_needs_literal_print(const char *source) {
    if (source == NULL) return 0;
    return strstr(source, "coproc") != NULL;
}

static int append_text(char **buf, size_t *len, size_t *cap, const char *text) {
    char *next;
    size_t add = text ? strlen(text) : 0;
    if (add == 0) return 0;
    if (*len + add + 1 > *cap) {
        size_t new_cap = (*cap == 0) ? 128 : *cap;
        while (*len + add + 1 > new_cap) new_cap *= 2;
        next = realloc(*buf, new_cap);
        if (next == NULL) return -1;
        *buf = next;
        *cap = new_cap;
    }
    memcpy(*buf + *len, text, add);
    *len += add;
    (*buf)[*len] = '\0';
    return 0;
}

static char *collapse_relop_spaces(const char *expr) {
    char *out;
    size_t i = 0;
    size_t j = 0;
    size_t n;
    if (expr == NULL) return NULL;
    n = strlen(expr);
    out = calloc(n + 1, 1);
    if (out == NULL) return NULL;
    while (i < n) {
        if ((expr[i] == '<' || expr[i] == '>') && j > 0) {
            while (j > 0 && out[j - 1] == ' ') j--;
            out[j++] = expr[i++];
            if (i < n && expr[i] == '=') out[j++] = expr[i++];
            while (i < n && expr[i] == ' ') i++;
            continue;
        }
        out[j++] = expr[i++];
    }
    out[j] = '\0';
    return out;
}

static int append_char(char **buf, size_t *len, size_t *cap, char ch) {
    char tmp[2];
    tmp[0] = ch;
    tmp[1] = '\0';
    return append_text(buf, len, cap, tmp);
}

static char decode_ansi_c_escape(const char **pp) {
    const char *p = *pp;
    if (*p == 'n') {
        *pp = p + 1;
        return '\n';
    }
    if (*p == 't') {
        *pp = p + 1;
        return '\t';
    }
    if (*p == 'r') {
        *pp = p + 1;
        return '\r';
    }
    if (*p == 'a') {
        *pp = p + 1;
        return '\a';
    }
    if (*p == 'b') {
        *pp = p + 1;
        return '\b';
    }
    if (*p == 'e' || *p == 'E') {
        *pp = p + 1;
        return 27;
    }
    if (*p >= '0' && *p <= '7') {
        unsigned int val = 0;
        int digits = 0;
        while (digits < 3 && *p >= '0' && *p <= '7') {
            val = val * 8 + (unsigned int)(*p - '0');
            p++;
            digits++;
        }
        *pp = p;
        return (char)(val & 0xff);
    }
    if (*p != '\0') {
        *pp = p + 1;
        return *p;
    }
    return '\\';
}

static char *normalize_function_word_text(const char *text) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    const char *p = text;
    if (text == NULL) return NULL;
    while (*p != '\0') {
        if (p[0] == '$' && p[1] == '(' && p[2] == '<') {
            if (append_text(&out, &len, &cap, "$(< ") != 0) {
                free(out);
                return NULL;
            }
            p += 3;
            while (*p == ' ' || *p == '\t') p++;
            continue;
        }
        if (p[0] == '$' && p[1] == '\'') {
            if (append_char(&out, &len, &cap, '\'') != 0) {
                free(out);
                return NULL;
            }
            p += 2;
            while (*p != '\0' && *p != '\'') {
                char ch = *p++;
                if (ch == '\\') ch = decode_ansi_c_escape(&p);
                if (append_char(&out, &len, &cap, ch) != 0) {
                    free(out);
                    return NULL;
                }
            }
            if (*p == '\'') p++;
            if (append_char(&out, &len, &cap, '\'') != 0) {
                free(out);
                return NULL;
            }
            continue;
        }
        if (append_char(&out, &len, &cap, *p++) != 0) {
            free(out);
            return NULL;
        }
    }
    if (len > 0 && out[len - 1] == ';') out[--len] = '\0';
    return out;
}

static void rstrip_line(char *line) {
    size_t len;
    if (line == NULL) return;
    len = strlen(line);
    while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r' ||
                       line[len - 1] == ' ' || line[len - 1] == '\t')) {
        line[--len] = '\0';
    }
}

static char *lstrip_line(char *line) {
    while (line != NULL && (*line == ' ' || *line == '\t')) line++;
    return line;
}

static void normalize_function_line(char *line) {
    char *src = line;
    char *dst = line;
    while (src != NULL && *src != '\0') {
        if (src[0] == '$' && src[1] == '(' && src[2] == '<') {
            *dst++ = *src++;
            *dst++ = *src++;
            *dst++ = *src++;
            *dst++ = ' ';
            while (*src == ' ' || *src == '\t') src++;
            continue;
        }
        if (src[0] == '<' && src[1] == '<') {
            *dst++ = *src++;
            *dst++ = *src++;
            if (*src == '-') {
                *dst++ = *src++;
            }
            while (*src == ' ' || *src == '\t') src++;
            continue;
        }
        *dst++ = *src++;
    }
    *dst = '\0';
}

static int parse_heredoc_delim(const char *line, char *delim, size_t delim_size, int *strip_tabs_out) {
    const char *p = strstr(line, "<<");
    const char *start;
    size_t len = 0;
    int strip_tabs = 0;
    if (strip_tabs_out != NULL) *strip_tabs_out = 0;
    if (delim == NULL || delim_size == 0 || p == NULL) return 0;
    p += 2;
    if (*p == '-') {
        strip_tabs = 1;
        p++;
    }
    while (*p == ' ' || *p == '\t') p++;
    if (*p == '\0') return 0;
    start = p;
    while (*p != '\0' && *p != ' ' && *p != '\t' && *p != ';' &&
           *p != ')' && *p != '(') {
        p++;
    }
    len = (size_t)(p - start);
    if (len == 0 || len + 1 > delim_size) return 0;
    memcpy(delim, start, len);
    delim[len] = '\0';
    if (strip_tabs_out != NULL) *strip_tabs_out = strip_tabs;
    return 1;
}

static int format_function_body_line(char **out, size_t *len, size_t *cap, const char *line) {
    if (append_text(out, len, cap, "    ") != 0 ||
        append_text(out, len, cap, line) != 0 ||
        append_text(out, len, cap, "\n") != 0) {
        return -1;
    }
    return 0;
}

static int format_function_body_line_with_indent(char **out, size_t *len, size_t *cap,
                                                 const char *line, int indent) {
    while (indent-- > 0) {
        if (append_text(out, len, cap, " ") != 0) return -1;
    }
    if (append_text(out, len, cap, line) != 0 ||
        append_text(out, len, cap, "\n") != 0) {
        return -1;
    }
    return 0;
}

static char *format_function_source_for_print(const char *name, const char *source) {
    const char *open_brace;
    const char *body_start;
    const char *body_end;
    const char *cursor;
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    int in_heredoc = 0;
    int heredoc_strip_tabs = 0;
    int compact_close_paren = 0;
    char heredoc_delim[128] = {0};
    if (name == NULL || source == NULL) return NULL;

    body_end = source + strlen(source);
    while (body_end > source &&
           (body_end[-1] == '\n' || body_end[-1] == '\r' ||
            body_end[-1] == ' ' || body_end[-1] == '\t')) {
        body_end--;
    }
    if (body_end <= source || body_end[-1] != '}') return NULL;
    body_end--;
    open_brace = strchr(source, '{');
    if (open_brace == NULL || open_brace >= body_end) return NULL;
    body_start = open_brace + 1;
    if (*body_start == '\r') body_start++;
    if (*body_start == '\n') body_start++;

    if (append_text(&out, &len, &cap, name) != 0 ||
        append_text(&out, &len, &cap, " () \n{ \n") != 0) {
        free(out);
        return NULL;
    }

    cursor = body_start;
    while (cursor < body_end) {
        const char *line_end = cursor;
        size_t raw_len;
        char *line;
        char *trimmed;

        while (line_end < body_end && *line_end != '\n' && *line_end != '\r') line_end++;
        raw_len = (size_t)(line_end - cursor);
        line = calloc(raw_len + 1, 1);
        if (line == NULL) {
            free(out);
            return NULL;
        }
        if (raw_len > 0) memcpy(line, cursor, raw_len);
        rstrip_line(line);
        trimmed = lstrip_line(line);
        if (trimmed == NULL || trimmed[0] == '\0') {
            free(line);
            while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
            cursor = line_end;
            continue;
        }

        if (in_heredoc) {
            if (heredoc_strip_tabs) {
                while (*trimmed == '\t') trimmed++;
            }
            if (append_text(&out, &len, &cap, trimmed) != 0 ||
                append_text(&out, &len, &cap, "\n") != 0) {
                free(line);
                free(out);
                return NULL;
            }
            if (strcmp(trimmed, heredoc_delim) == 0) {
                in_heredoc = 0;
                if (!compact_close_paren &&
                    append_text(&out, &len, &cap, "\n") != 0) {
                    free(line);
                    free(out);
                    return NULL;
                }
            }
            free(line);
            while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
            cursor = line_end;
            continue;
        }

        normalize_function_line(trimmed);
        if (strncmp(trimmed, "coproc (", 8) == 0) {
            if (parse_heredoc_delim(trimmed, heredoc_delim, sizeof(heredoc_delim), &heredoc_strip_tabs)) {
                in_heredoc = 1;
                compact_close_paren = 1;
            }
            if (append_text(&out, &len, &cap, "    coproc COPROC") != 0 ||
                append_text(&out, &len, &cap, trimmed + strlen("coproc")) != 0 ||
                append_text(&out, &len, &cap, "\n") != 0) {
                free(line);
                free(out);
                return NULL;
            }
            free(line);
            while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
            cursor = line_end;
            continue;
        }
        if (strncmp(trimmed, "coproc ", 7) == 0) {
            char *brace = strchr(trimmed, '{');
            if (brace != NULL && brace[1] != '\0') {
                char *after = brace + 1;
                while (*after == ' ' || *after == '\t') after++;
                if (*after != '\0') {
                    brace[1] = '\0';
                    if (append_text(&out, &len, &cap, "    ") != 0 ||
                        append_text(&out, &len, &cap, trimmed) != 0 ||
                        append_text(&out, &len, &cap, " \n") != 0) {
                        free(line);
                        free(out);
                        return NULL;
                    }
                    memmove(trimmed, after, strlen(after) + 1);
                    normalize_function_line(trimmed);
                    if (parse_heredoc_delim(trimmed, heredoc_delim, sizeof(heredoc_delim), &heredoc_strip_tabs)) {
                        in_heredoc = 1;
                    }
                    if (format_function_body_line_with_indent(&out, &len, &cap, trimmed, 8) != 0) {
                        free(line);
                        free(out);
                        return NULL;
                    }
                    free(line);
                    while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
                    cursor = line_end;
                    continue;
                }
            }
        }

        if (compact_close_paren && strcmp(trimmed, ")") == 0) {
            if (append_text(&out, &len, &cap, " )\n") != 0) {
                free(line);
                free(out);
                return NULL;
            }
            compact_close_paren = 0;
            free(line);
            while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
            cursor = line_end;
            continue;
        }

        if (parse_heredoc_delim(trimmed, heredoc_delim, sizeof(heredoc_delim), &heredoc_strip_tabs)) {
            if (format_function_body_line(&out, &len, &cap, trimmed) != 0) {
                free(line);
                free(out);
                return NULL;
            }
            in_heredoc = 1;
        } else if (strncmp(trimmed, "for ", 4) == 0) {
            char *do_pos = strstr(trimmed, "; do");
            if (do_pos != NULL && do_pos[4] == '\0') {
                do_pos[1] = '\0';
                if (format_function_body_line(&out, &len, &cap, trimmed) != 0 ||
                    format_function_body_line(&out, &len, &cap, "do") != 0) {
                    free(line);
                    free(out);
                    return NULL;
                }
            } else if (format_function_body_line(&out, &len, &cap, trimmed) != 0) {
                free(line);
                free(out);
                return NULL;
            }
        } else if (format_function_body_line(&out, &len, &cap, trimmed) != 0) {
            free(line);
            free(out);
            return NULL;
        }
        free(line);
        while (line_end < body_end && (*line_end == '\n' || *line_end == '\r')) line_end++;
        cursor = line_end;
    }

    if (append_text(&out, &len, &cap, "}\n") != 0) {
        free(out);
        return NULL;
    }
    return out;
}

static void print_function_indent(int indent) {
    while (indent-- > 0) putchar(' ');
}

static int function_node_has_heredoc(const struct cupid_node *node) {
    size_t i;
    if (node == NULL) return 0;
    for (i = 0; i < node->redir_count; i++) {
        if (node->redirs[i].kind == CUPID_REDIR_HEREDOC) return 1;
    }
    return 0;
}

static int print_function_redir(const struct cupid_redir *redir) {
    int need_space = 1;
    char *target_source = NULL;
    if (redir == NULL) return -1;
    if (redir->has_target) {
        target_source = cupid_word_source_text(&redir->target);
    }
    if (redir->fd_var != NULL && redir->fd_var[0] != '\0') {
        putchar('{');
        fputs(redir->fd_var, stdout);
        putchar('}');
    }
    switch (redir->kind) {
        case CUPID_REDIR_IN: fputs("<", stdout); break;
        case CUPID_REDIR_OUT: fputs(">", stdout); break;
        case CUPID_REDIR_APPEND: fputs(">>", stdout); break;
        case CUPID_REDIR_CLOBBER: fputs(">|", stdout); break;
        case CUPID_REDIR_INOUT: fputs("<>", stdout); break;
        case CUPID_REDIR_DUP_OUT: fputs(">&", stdout); break;
        case CUPID_REDIR_DUP_IN:
            if (target_source != NULL && strcmp(target_source, "-") == 0) fputs(">&", stdout);
            else fputs("<&", stdout);
            break;
        case CUPID_REDIR_ERR_OUT: fputs("&>", stdout); break;
        case CUPID_REDIR_ERR_TO_OUT:
            fputs(">&1", stdout);
            free(target_source);
            return 0;
        case CUPID_REDIR_HEREDOC:
            if (redir->heredoc_strip_tabs) fputs("<<-", stdout);
            else fputs("<<", stdout);
            break;
        case CUPID_REDIR_HERESTRING: fputs("<<<", stdout); break;
    }
    if (redir->fd >= 0 &&
        redir->kind != CUPID_REDIR_ERR_TO_OUT &&
        redir->fd != 0 &&
        !(redir->kind == CUPID_REDIR_OUT && redir->fd == 1) &&
        !(redir->kind == CUPID_REDIR_APPEND && redir->fd == 1) &&
        !(redir->kind == CUPID_REDIR_CLOBBER && redir->fd == 1) &&
        !(redir->kind == CUPID_REDIR_INOUT && redir->fd == 0)) {
        /* Not currently needed by the type corpus. */
    }
    if (redir->has_target) {
        if (redir->kind == CUPID_REDIR_HEREDOC) {
            need_space = 0;
        } else if (redir->kind == CUPID_REDIR_DUP_IN || redir->kind == CUPID_REDIR_DUP_OUT) {
            need_space = 0;
        }
        if (need_space) putchar(' ');
        print_word_literal(&redir->target);
    }
    free(target_source);
    return 0;
}

static void print_function_heredoc_body(const struct cupid_redir *redir) {
    const char *p;
    if (redir == NULL || redir->heredoc_body == NULL) return;
    if (!redir->heredoc_strip_tabs) {
        fputs(redir->heredoc_body, stdout);
        return;
    }
    p = redir->heredoc_body;
    while (*p != '\0') {
        while (*p == '\t') p++;
        while (*p != '\0') {
            putchar(*p);
            if (*p++ == '\n') break;
        }
    }
}

static int print_function_simple_command(const struct cupid_node *node, int indent, int add_semi) {
    size_t i;
    int has_heredoc;
    if (node == NULL || node->kind != NODE_SIMPLE_CMD) return -1;
    has_heredoc = function_node_has_heredoc(node);
    print_function_indent(indent);
    for (i = 0; i < node->simple_cmd.argc; i++) {
        if (i > 0) putchar(' ');
        print_word_literal(&node->simple_cmd.argv[i]);
    }
    for (i = 0; i < node->redir_count; i++) {
        if (node->simple_cmd.argc > 0 || i > 0) putchar(' ');
        if (print_function_redir(&node->redirs[i]) != 0) return -1;
    }
    if (add_semi && !has_heredoc) putchar(';');
    putchar('\n');
    if (has_heredoc) {
        for (i = 0; i < node->redir_count; i++) {
            char *delim;
            if (node->redirs[i].kind != CUPID_REDIR_HEREDOC) continue;
            print_function_heredoc_body(&node->redirs[i]);
            delim = cupid_word_dequote_literal(&node->redirs[i].target);
            if (delim == NULL) return -1;
            fputs(delim, stdout);
            putchar('\n');
            putchar('\n');
            free(delim);
        }
    }
    return 0;
}

static int print_function_list(const struct cupid_list_ast *body, int indent);
static int print_function_block_list(const struct cupid_list_ast *body, int indent);
static int function_list_has_heredoc(const struct cupid_list_ast *body);

static int function_node_contains_heredoc(const struct cupid_node *node) {
    if (node == NULL) return 0;
    if (function_node_has_heredoc(node)) return 1;
    switch (node->kind) {
        case NODE_FOR:
            return function_list_has_heredoc(node->for_clause.body);
        case NODE_IF:
            if (function_list_has_heredoc(node->if_clause.condition)) return 1;
            if (function_list_has_heredoc(node->if_clause.then_body)) return 1;
            if (node->if_clause.elif_next != NULL) {
                const struct cupid_if_node *elifn = node->if_clause.elif_next;
                while (elifn != NULL) {
                    if (function_list_has_heredoc(elifn->condition)) return 1;
                    if (function_list_has_heredoc(elifn->then_body)) return 1;
                    if (function_list_has_heredoc(elifn->else_body)) return 1;
                    elifn = elifn->elif_next;
                }
            }
            return function_list_has_heredoc(node->if_clause.else_body);
        case NODE_BRACE_GROUP:
            return function_list_has_heredoc(node->brace_group);
        case NODE_SUBSHELL:
            return function_list_has_heredoc(node->subshell);
        default:
            return 0;
    }
}

static int function_list_has_heredoc(const struct cupid_list_ast *body) {
    size_t i;
    if (body == NULL) return 0;
    for (i = 0; i < body->count; i++) {
        const struct cupid_pipeline_ast *pl = &body->items[i].pipeline;
        size_t j;
        for (j = 0; j < pl->count; j++) {
            if (function_node_contains_heredoc(&pl->commands[j])) return 1;
        }
    }
    return 0;
}

static int print_function_compact_node(const struct cupid_node *node) {
    size_t i;
    if (node == NULL) return -1;
    switch (node->kind) {
        case NODE_SIMPLE_CMD:
            for (i = 0; i < node->simple_cmd.argc; i++) {
                if (i > 0) putchar(' ');
                print_word_literal(&node->simple_cmd.argv[i]);
            }
            for (i = 0; i < node->redir_count; i++) {
                if (node->simple_cmd.argc > 0 || i > 0) putchar(' ');
                if (print_function_redir(&node->redirs[i]) != 0) return -1;
            }
            return 0;
        case NODE_ARITH_CMD:
            fputs("(( ", stdout);
            if (node->arith_cmd.expr != NULL) fputs(node->arith_cmd.expr, stdout);
            fputs(" ))", stdout);
            return 0;
        default:
            return -1;
    }
}

static int print_function_compact_list(const struct cupid_list_ast *list) {
    size_t i;
    if (list == NULL) return -1;
    for (i = 0; i < list->count; i++) {
        const struct cupid_pipeline_item *item = &list->items[i];
        const struct cupid_pipeline_ast *pl = &item->pipeline;
        if (pl->count != 1) return -1;
        if (i > 0) {
            if (item->join_from_prev == CUPID_CHAIN_AND_IF) fputs(" && ", stdout);
            else if (item->join_from_prev == CUPID_CHAIN_OR_IF) fputs(" || ", stdout);
            else fputs("; ", stdout);
        }
        if (item->negate_status) fputs("! ", stdout);
        if (print_function_compact_node(&pl->commands[0]) != 0) return -1;
    }
    return 0;
}

static int print_function_node(const struct cupid_node *node, int indent, int add_semi) {
    if (node == NULL) return -1;
    switch (node->kind) {
        case NODE_SIMPLE_CMD:
            return print_function_simple_command(node, indent, add_semi);
        case NODE_ARITH_CMD:
            print_function_indent(indent);
            fputs("(( ", stdout);
            if (node->arith_cmd.expr != NULL) fputs(node->arith_cmd.expr, stdout);
            fputs(" ))", stdout);
            if (add_semi) putchar(';');
            putchar('\n');
            return 0;
        case NODE_IF: {
            const struct cupid_if_node *cur = &node->if_clause;
            print_function_indent(indent);
            fputs("if ", stdout);
            if (print_function_compact_list(cur->condition) != 0) return -1;
            fputs("; then\n", stdout);
            if (print_function_block_list(cur->then_body, indent + 4) != 0) return -1;
            while (cur->elif_next != NULL) {
                cur = cur->elif_next;
                print_function_indent(indent);
                fputs("elif ", stdout);
                if (print_function_compact_list(cur->condition) != 0) return -1;
                fputs("; then\n", stdout);
                if (print_function_block_list(cur->then_body, indent + 4) != 0) return -1;
            }
            if (cur->else_body != NULL) {
                print_function_indent(indent);
                fputs("else\n", stdout);
                if (print_function_block_list(cur->else_body, indent + 4) != 0) return -1;
            }
            print_function_indent(indent);
            fputs("fi", stdout);
            if (add_semi) putchar(';');
            putchar('\n');
            return 0;
        }
        case NODE_BRACE_GROUP:
            return print_function_list(node->brace_group, indent);
        case NODE_SUBSHELL:
            if (node->subshell != NULL &&
                node->subshell->count == 1 &&
                node->subshell->items[0].pipeline.count == 1 &&
                node->subshell->items[0].pipeline.commands[0].kind == NODE_SIMPLE_CMD &&
                function_node_has_heredoc(&node->subshell->items[0].pipeline.commands[0])) {
                const struct cupid_node *inner = &node->subshell->items[0].pipeline.commands[0];
                size_t i;
                print_function_indent(indent);
                fputs("( ", stdout);
                for (i = 0; i < inner->simple_cmd.argc; i++) {
                    if (i > 0) putchar(' ');
                    print_word_literal(&inner->simple_cmd.argv[i]);
                }
                for (i = 0; i < inner->redir_count; i++) {
                    putchar(' ');
                    if (print_function_redir(&inner->redirs[i]) != 0) return -1;
                }
                putchar('\n');
                for (i = 0; i < inner->redir_count; i++) {
                    char *delim;
                    if (inner->redirs[i].kind != CUPID_REDIR_HEREDOC) continue;
                    print_function_heredoc_body(&inner->redirs[i]);
                    delim = cupid_word_dequote_literal(&inner->redirs[i].target);
                    if (delim == NULL) return -1;
                    fputs(delim, stdout);
                    putchar('\n');
                    free(delim);
                }
                fputs(" )\n", stdout);
                return 0;
            }
            print_function_indent(indent);
            fputs("(\n", stdout);
            if (print_function_list(node->subshell, indent + 4) != 0) return -1;
            print_function_indent(indent);
            fputs(")\n", stdout);
            return 0;
        case NODE_FOR: {
            size_t i;
            print_function_indent(indent);
            if (node->for_clause.is_cstyle) {
                const int init_empty = (node->for_clause.c_init == NULL || node->for_clause.c_init[0] == '\0');
                const int cond_empty = (node->for_clause.c_cond == NULL || node->for_clause.c_cond[0] == '\0');
                const int step_empty = (node->for_clause.c_step == NULL || node->for_clause.c_step[0] == '\0');
                const char *init = (!init_empty)
                    ? node->for_clause.c_init : "1";
                const char *cond = (!cond_empty)
                    ? node->for_clause.c_cond : "1";
                const char *step = (!step_empty)
                    ? node->for_clause.c_step : "1";
                char *cond_compact = NULL;
                if (!step_empty) {
                    printf("for ((%s; %s; %s ))\n", init, cond, step);
                } else {
                    if (!init_empty && !cond_empty) {
                        cond_compact = collapse_relop_spaces(cond);
                    }
                    printf("for ((%s; %s; %s))\n", init,
                           cond_compact != NULL ? cond_compact : cond, step);
                }
                free(cond_compact);
            } else {
                if (node->for_clause.varname == NULL) return -1;
                fputs("for ", stdout);
                fputs(node->for_clause.varname, stdout);
                if (node->for_clause.has_wordlist) {
                    fputs(" in", stdout);
                    for (i = 0; i < node->for_clause.word_count; i++) {
                        putchar(' ');
                        print_word_literal(&node->for_clause.words[i]);
                    }
                }
                fputs(";\n", stdout);
            }
            print_function_indent(indent);
            fputs("do\n", stdout);
            if (print_function_block_list(node->for_clause.body, indent + 4) != 0) return -1;
            print_function_indent(indent);
            fputs("done", stdout);
            if (add_semi && !function_list_has_heredoc(node->for_clause.body)) putchar(';');
            putchar('\n');
            return 0;
        }
        default:
            return -1;
    }
}

static int print_function_list(const struct cupid_list_ast *body, int indent) {
    size_t i;
    if (body == NULL) return -1;
    for (i = 0; i < body->count; i++) {
        const struct cupid_pipeline_ast *pl = &body->items[i].pipeline;
        int add_semi;
        if (pl->count != 1) return -1;
        add_semi = (i + 1 < body->count) ? 1 : 0;
        if (print_function_node(&pl->commands[0], indent, add_semi) != 0) return -1;
    }
    return 0;
}

static int print_function_block_list(const struct cupid_list_ast *body, int indent) {
    size_t i;
    if (body == NULL) return -1;
    for (i = 0; i < body->count; i++) {
        const struct cupid_pipeline_ast *pl = &body->items[i].pipeline;
        if (pl->count != 1) return -1;
        if (print_function_node(&pl->commands[0], indent, 1) != 0) return -1;
    }
    return 0;
}

static int print_declared_function(struct cupid_shell *shell, const char *name) {
    struct cupid_list_ast *body = cupid_func_get(shell, name);
    const char *source = cupid_func_source_get(shell, name);
    if (body == NULL) {
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "declare: %s: not found\n", name);
        return 1;
    }
    if (function_source_needs_literal_print(source)) {
        char *formatted = format_function_source_for_print(name, source);
        if (formatted == NULL) return 1;
        fputs(formatted, stdout);
        if (formatted[0] != '\0' && formatted[strlen(formatted) - 1] != '\n') {
            putchar('\n');
        }
        free(formatted);
        return 0;
    }
    printf("%s () \n{ \n", name);
    if (print_function_list(body, 4) != 0) return 1;
    printf("}\n");
    return 0;
}

static int builtin_mapfile(struct cupid_shell *shell, int argc, char **argv) {
    int strip_nl = 0;
    const char *name = "MAPFILE";
    char **items = NULL;
    size_t count = 0;
    int i = 1;
    while (i < argc && argv[i][0] == '-') {
        if (strcmp(argv[i], "-t") == 0) {
            strip_nl = 1;
            i++;
        } else {
            return 1;
        }
    }
    if (i < argc) name = argv[i];
    while (1) {
        char *line = NULL;
        size_t cap = 0;
        ssize_t n = getline(&line, &cap, stdin);
        char **next;
        if (n < 0) {
            free(line);
            break;
        }
        if (strip_nl && n > 0 && line[n - 1] == '\n') line[n - 1] = '\0';
        next = realloc(items, sizeof(*next) * (count + 1));
        if (next == NULL) {
            free(line);
            goto fail;
        }
        items = next;
        items[count++] = line;
    }
    if (cupid_array_set_list(shell, name, items, count) != 0) goto fail;
    for (i = 0; (size_t)i < count; i++) free(items[i]);
    free(items);
    return 0;
fail:
    for (i = 0; (size_t)i < count; i++) free(items[i]);
    free(items);
    return 1;
}

/* ------------------------------------------------------------------ */
/*  declare / typeset                                                 */
/* ------------------------------------------------------------------ */

static int builtin_declare(struct cupid_shell *shell, int argc, char **argv) {
    int do_export = 0;
    int do_readonly = 0;
    int do_array = 0;
    int do_assoc = 0;
    int do_function = 0;
    int do_nameref = 0;
    int clear_nameref = 0;
    int do_integer = 0;
    int clear_integer = 0;
    int do_upper = 0;
    int clear_upper = 0;
    int print_only = 0;
    int i = 1;

    while (i < argc && (argv[i][0] == '-' || argv[i][0] == '+') && argv[i][1] != '\0') {
        int enable = (argv[i][0] == '-') ? 1 : 0;
        const char *p = argv[i] + 1;
        while (*p != '\0') {
            if (*p == 'x' || *p == 'r' || *p == 'a' || *p == 'A' || *p == 'g' || *p == 'f') {
                if (!enable) {
                    fprintf(stderr, "cupid: declare: +%c: unsupported option\n", *p);
                    return 1;
                }
                if (*p == 'x') do_export = 1;
                else if (*p == 'r') do_readonly = 1;
                else if (*p == 'a') do_array = 1;
                else if (*p == 'A') do_assoc = 1;
                else if (*p == 'f') do_function = 1;
            } else if (*p == 'p') {
                if (!enable) {
                    fprintf(stderr, "cupid: declare: +%c: unsupported option\n", *p);
                    return 1;
                }
                print_only = 1;
            } else if (*p == 'i') {
                if (enable) {
                    do_integer = 1;
                    clear_integer = 0;
                } else {
                    clear_integer = 1;
                    do_integer = 0;
                }
            } else if (*p == 'n') {
                if (enable) {
                    do_nameref = 1;
                    clear_nameref = 0;
                } else {
                    clear_nameref = 1;
                    do_nameref = 0;
                }
            } else if (*p == 'u') {
                if (enable) {
                    do_upper = 1;
                    clear_upper = 0;
                } else {
                    clear_upper = 1;
                    do_upper = 0;
                }
            } else {
                fprintf(stderr, "cupid: declare: %c%c: invalid option\n",
                        enable ? '-' : '+', *p);
                return 1;
            }
            p++;
        }
        i++;
    }

    if (i >= argc) {
        size_t vi;
        if (do_function) {
            for (vi = 0; vi < shell->funcs.count; vi++) {
                if (print_declared_function(shell, shell->funcs.entries[vi].name) != 0) return 1;
            }
            return 0;
        }
        for (vi = 0; vi < shell->vars.count; vi++) {
            const char *nref = shell->vars.entries[vi].nameref_target;
            if (do_export && !shell->vars.entries[vi].exported) continue;
            if (do_readonly && !shell->vars.entries[vi].readonly) continue;
            if (do_nameref && (nref == NULL || nref[0] == '\0')) continue;
            printf("declare ");
            if (shell->vars.entries[vi].exported) printf("-x ");
            if (shell->vars.entries[vi].readonly) printf("-r ");
            if (shell->vars.entries[vi].integer) printf("-i ");
            if (shell->vars.entries[vi].uppercase) printf("-u ");
            if (nref != NULL && nref[0] != '\0') {
                printf("-n %s=\"%s\"\n", shell->vars.entries[vi].name, nref);
            } else {
                if (!shell->vars.entries[vi].exported && !shell->vars.entries[vi].readonly &&
                    !shell->vars.entries[vi].integer && !shell->vars.entries[vi].uppercase) {
                    printf("-- ");
                }
                printf("%s=", shell->vars.entries[vi].name);
                print_declare_quoted_value(shell->vars.entries[vi].value);
                putchar('\n');
            }
        }
        if (print_only) {
            for (vi = 0; vi < shell->arrays.count; vi++) {
                (void)print_declared_name(shell, shell->arrays.entries[vi].name);
            }
        }
        return 0;
    }

    if (print_only) {
        int status = 0;
        for (; i < argc; i++) {
            if (do_function) {
                if (print_declared_function(shell, argv[i]) != 0) status = 1;
            } else if (print_declared_name(shell, argv[i]) != 0) {
                status = 1;
            }
        }
        return status;
    }

    if (do_function) {
        int status = 0;
        int has_assignment = 0;
        int j;
        for (j = i; j < argc; j++) {
            if (strchr(argv[j], '=') != NULL) {
                has_assignment = 1;
                break;
            }
        }
        if (!has_assignment) {
            for (; i < argc; i++) {
                if (cupid_func_get(shell, argv[i]) == NULL) {
                    continue;
                }
                if (print_declared_function(shell, argv[i]) != 0) status = 1;
            }
            return status;
        }
    }

    for (; i < argc; i++) {
        const char *name = NULL;
        const char *value = NULL;
        size_t name_len = 0;
        int append = 0;
        if (split_assignment_word_ext(argv[i], &name, &name_len, &value, &append)) {
            char *key = calloc(name_len + 1, 1);
            if (key == NULL) return 1;
            memcpy(key, name, name_len);
            if (clear_integer) {
                if (cupid_vars_set_integer_attr(shell, key, 0) != 0) {
                    free(key);
                    return 1;
                }
            }
            if (clear_upper) {
                if (cupid_vars_set_upper_attr(shell, key, 0) != 0) {
                    free(key);
                    return 1;
                }
            }
            if (do_integer) {
                if (cupid_vars_set_integer_attr(shell, key, 1) != 0) {
                    free(key);
                    return 1;
                }
            }
            if (do_upper) {
                if (cupid_vars_set_upper_attr(shell, key, 1) != 0) {
                    free(key);
                    return 1;
                }
            }
            if (do_nameref) {
                if (cupid_vars_set_nameref(shell, key, value) != 0) {
                    free(key);
                    return 1;
                }
            } else if (do_array || do_assoc) {
                char **items = NULL;
                size_t count = 0;
                if (parse_array_literal_words(shell, value, &items, &count) != 0) {
                    free(key);
                    return 1;
                }
                if (cupid_array_set_associative(shell, key, do_assoc ? 1 : 0) != 0) {
                    size_t ai;
                    for (ai = 0; ai < count; ai++) free(items[ai]);
                    free(items);
                    free(key);
                    return 1;
                }
                if (cupid_array_set_list(shell, key, items, count) != 0) {
                    size_t ai;
                    for (ai = 0; ai < count; ai++) free(items[ai]);
                    free(items);
                    free(key);
                    return 1;
                }
                {
                    size_t ai;
                    for (ai = 0; ai < count; ai++) free(items[ai]);
                    free(items);
                }
            } else {
                if (append) {
                    if (apply_append_assignment_value(shell, key, value) != 0) {
                        free(key);
                        return 1;
                    }
                } else if (cupid_vars_set(shell, key, value) != 0) {
                    free(key);
                    return 1;
                }
                if (do_export) {
                    const char *cur = cupid_vars_get(shell, key);
                    if (cupid_vars_export(shell, key, cur ? cur : "") != 0) {
                        free(key);
                        return 1;
                    }
                }
            }
            if (clear_nameref) {
                if (cupid_vars_clear_nameref(shell, key) != 0) {
                    free(key);
                    return 1;
                }
            }
            if (do_readonly) cupid_vars_mark_readonly(shell, key);
            free(key);
        } else {
            if (do_export) {
                const char *val = cupid_vars_get(shell, argv[i]);
                cupid_vars_export(shell, argv[i], val ? val : "");
            }
            if (do_readonly) cupid_vars_mark_readonly(shell, argv[i]);
            if (do_array || do_assoc) {
                if (cupid_array_set_associative(shell, argv[i], do_assoc ? 1 : 0) != 0) return 1;
            }
            if (do_integer) {
                if (cupid_vars_set_integer_attr(shell, argv[i], 1) != 0) return 1;
            } else if (clear_integer) {
                if (cupid_vars_set_integer_attr(shell, argv[i], 0) != 0) return 1;
            }
            if (do_upper) {
                if (cupid_vars_set_upper_attr(shell, argv[i], 1) != 0) return 1;
            } else if (clear_upper) {
                if (cupid_vars_set_upper_attr(shell, argv[i], 0) != 0) return 1;
            }
            if (do_nameref) {
                if (cupid_vars_set_nameref(shell, argv[i], "") != 0) return 1;
            } else if (clear_nameref) {
                if (cupid_vars_clear_nameref(shell, argv[i]) != 0) return 1;
            } else if (!do_export && !do_readonly && !do_array && !do_assoc &&
                       !do_integer && !clear_integer && !do_upper && !clear_upper) {
                cupid_vars_set(shell, argv[i], "");
            }
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  getopts                                                           */
/* ------------------------------------------------------------------ */

static int builtin_getopts(struct cupid_shell *shell, int argc, char **argv) {
    const char *optstring;
    const char *varname;
    const char *optind_str;
    int optind_val;
    char **args;
    int nargs;
    const char *arg;
    char result[2];
    char buf[32];
    const char *found;

    if (argc < 3 || (argc >= 2 && argv[1][0] == '-' && argv[1][1] != '\0')) {
        fprintf(stderr, "cupid: getopts: usage: getopts optstring name [args]\n");
        return 2;
    }

    optstring = argv[1];
    varname = argv[2];

    optind_str = cupid_vars_get(shell, "OPTIND");
    optind_val = (optind_str != NULL) ? (int)strtol(optind_str, NULL, 10) : 1;
    if (optind_val < 1) optind_val = 1;

    if (argc > 3) {
        args = argv + 3;
        nargs = argc - 3;
    } else {
        args = shell->params.args;
        nargs = (int)shell->params.count;
    }

    if (optind_val > nargs) {
        cupid_vars_set(shell, varname, "?");
        return 1;
    }

    arg = args[optind_val - 1];
    if (arg == NULL || arg[0] != '-' || arg[1] == '\0') {
        cupid_vars_set(shell, varname, "?");
        return 1;
    }
    if (strcmp(arg, "--") == 0) {
        snprintf(buf, sizeof(buf), "%d", optind_val + 1);
        cupid_vars_set(shell, "OPTIND", buf);
        cupid_vars_set(shell, varname, "?");
        return 1;
    }

    result[0] = arg[1];
    result[1] = '\0';
    found = strchr(optstring, arg[1]);

    snprintf(buf, sizeof(buf), "%d", optind_val + 1);
    cupid_vars_set(shell, "OPTIND", buf);

    if (found == NULL) {
        if (optstring[0] != ':') {
            fprintf(stderr, "cupid: illegal option -- %c\n", arg[1]);
        }
        result[0] = '?';
        cupid_vars_set(shell, varname, result);
        return 0;
    }

    if (found[1] == ':') {
        if (arg[2] != '\0') {
            cupid_vars_set(shell, "OPTARG", arg + 2);
        } else if (optind_val < nargs) {
            cupid_vars_set(shell, "OPTARG", args[optind_val]);
            snprintf(buf, sizeof(buf), "%d", optind_val + 2);
            cupid_vars_set(shell, "OPTIND", buf);
        } else {
            if (optstring[0] == ':') {
                result[0] = ':';
            } else {
                result[0] = '?';
                fprintf(stderr, "cupid: option requires an argument -- %c\n", arg[1]);
            }
        }
    }

    cupid_vars_set(shell, varname, result);
    return 0;
}

/* ------------------------------------------------------------------ */
/*  kill                                                              */
/* ------------------------------------------------------------------ */

static int builtin_kill(struct cupid_shell *shell, int argc, char **argv) {
    int signo = SIGTERM;
    int i = 1;
    int status = 0;

    if (argc < 2) {
        cupid_shell_error_prefix(stderr, shell);
        fputs("kill: usage: kill [-signal] pid...\n", stderr);
        return 1;
    }

    if (argv[1][0] == '-' && argv[1][1] != '\0') {
        if (strcmp(argv[1], "-l") == 0) {
            if (argc == 2) {
                print_signal_list();
                return 0;
            }
            for (i = 2; i < argc; i++) {
                int mapped;
                if (argv[i][0] >= '0' && argv[i][0] <= '9') {
                    char *end = NULL;
                    long raw = strtol(argv[i], &end, 10);
                    if (end != argv[i] && *end == '\0' && raw > 128) {
                        raw -= 128;
                        mapped = signal_name_to_num(argv[i]);
                        if (mapped < 0) mapped = (int)raw;
                    } else {
                        mapped = signal_name_to_num(argv[i]);
                    }
                } else {
                    mapped = signal_name_to_num(argv[i]);
                }
                if (mapped < 0) {
                    cupid_shell_error_prefix(stderr, shell);
                    fprintf(stderr, "kill: %s: invalid signal specification\n", argv[i]);
                    status = 1;
                    continue;
                }
                if (argv[i][0] >= '0' && argv[i][0] <= '9') {
                    const char *name = signal_num_to_name(mapped);
                    if (name == NULL) {
                        cupid_shell_error_prefix(stderr, shell);
                        fprintf(stderr, "kill: %s: invalid signal specification\n", argv[i]);
                        status = 1;
                        continue;
                    }
                    puts(name);
                } else {
                    printf("%d\n", mapped);
                }
            }
            return status;
        }
        signo = signal_name_to_num(argv[1] + 1);
        if (signo < 0) {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "kill: %s: invalid signal\n", argv[1] + 1);
            return 1;
        }
        i = 2;
    }

    for (; i < argc; i++) {
        char *end;
        long pid_val = strtol(argv[i], &end, 10);
        if (*end != '\0') {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "kill: %s: invalid pid\n", argv[i]);
            status = 1;
            continue;
        }
        if (kill((pid_t)pid_val, signo) != 0) {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "kill: (%ld): %s\n", pid_val, strerror(errno));
            status = 1;
        }
    }
    return status;
}

/* ------------------------------------------------------------------ */
/*  readonly                                                          */
/* ------------------------------------------------------------------ */

static int builtin_readonly(struct cupid_shell *shell, int argc, char **argv) {
    int i;

    if (argc == 1) {
        size_t vi;
        for (vi = 0; vi < shell->vars.count; vi++) {
            if (shell->vars.entries[vi].readonly) {
                printf("declare -r %s=\"%s\"\n", shell->vars.entries[vi].name,
                       shell->vars.entries[vi].value);
            }
        }
        return 0;
    }

    for (i = 1; i < argc; i++) {
        const char *name = NULL;
        const char *value = NULL;
        size_t name_len = 0;
        int append = 0;
        if (split_assignment_word_ext(argv[i], &name, &name_len, &value, &append)) {
            char *key = calloc(name_len + 1, 1);
            if (key == NULL) return 1;
            memcpy(key, name, name_len);
            if (append) {
                if (apply_append_assignment_value(shell, key, value) != 0) {
                    free(key);
                    return 1;
                }
            } else if (cupid_vars_set(shell, key, value) != 0) {
                free(key);
                return 1;
            }
            cupid_vars_mark_readonly(shell, key);
            free(key);
        } else {
            cupid_vars_mark_readonly(shell, argv[i]);
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  let                                                               */
/* ------------------------------------------------------------------ */

static int builtin_let(struct cupid_shell *shell, int argc, char **argv) {
    long result = 0;
    int i;

    if (argc < 2) return 1;

    for (i = 1; i < argc; i++) {
        int err = 0;
        result = cupid_arith_eval(shell, argv[i], &err);
        if (err) {
            fprintf(stderr, "cupid: let: syntax error in expression\n");
            return 1;
        }
    }

    return (result != 0) ? 0 : 1;
}

/* ------------------------------------------------------------------ */
/*  history                                                           */
/* ------------------------------------------------------------------ */

static int g_history_file_cursor = 0;

static void history_print_usage(void) {
    fprintf(stderr,
            "history: usage: history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]\n");
}

static char *history_resolve_path(struct cupid_shell *shell, const char *override_path) {
    const char *path = override_path;
    if (path == NULL || path[0] == '\0') path = cupid_vars_get(shell, "HISTFILE");
    if (path == NULL || path[0] == '\0') path = getenv("HISTFILE");
    if (path == NULL || path[0] == '\0') {
        const char *home = getenv("HOME");
        size_t need;
        char *fallback;
        if (home == NULL || home[0] == '\0') return NULL;
        need = strlen(home) + strlen("/.cupid_history") + 1;
        fallback = calloc(need, 1);
        if (fallback == NULL) return NULL;
        snprintf(fallback, need, "%s/.cupid_history", home);
        return fallback;
    }
    return strdup(path);
}

static int history_write_file(const char *path, int append_mode, int start_index) {
    FILE *f;
    int count = cupid_history_count();
    int i;
    if (path == NULL) return 1;
    if (start_index < 0) start_index = 0;
    if (start_index > count) start_index = count;
    f = fopen(path, append_mode ? "a" : "w");
    if (f == NULL) return 1;
    for (i = start_index; i < count; i++) {
        const char *entry = cupid_history_get(count - 1 - i);
        if (entry == NULL) continue;
        fprintf(f, "%s\n", entry);
    }
    fclose(f);
    return 0;
}

static int history_read_file(const char *path) {
    FILE *f;
    char *line = NULL;
    size_t cap = 0;
    ssize_t nread;
    if (path == NULL) return 1;
    f = fopen(path, "r");
    if (f == NULL) return 0;
    while ((nread = getline(&line, &cap, f)) >= 0) {
        if (nread > 0 && line[nread - 1] == '\n') line[nread - 1] = '\0';
        cupid_history_add(line);
    }
    free(line);
    fclose(f);
    return 0;
}

static int builtin_history(struct cupid_shell *shell, int argc, char **argv) {
    int count, i, start;

    if (argc >= 2 && argv[1][0] == '-' && argv[1][1] != '\0') {
        const char *opts = argv[1] + 1;
        int do_c = 0, do_p = 0, do_s = 0;
        int do_a = 0, do_n = 0, do_r = 0, do_w = 0;
        int anrw_count;
        const char *file_arg = (argc >= 3) ? argv[2] : NULL;

        while (*opts != '\0') {
            switch (*opts) {
                case 'c': do_c = 1; break;
                case 'p': do_p = 1; break;
                case 's': do_s = 1; break;
                case 'a': do_a = 1; break;
                case 'n': do_n = 1; break;
                case 'r': do_r = 1; break;
                case 'w': do_w = 1; break;
                default:
                    fprintf(stderr, "history: -%c: invalid option\n", *opts);
                    history_print_usage();
                    return 2;
            }
            opts++;
        }

        anrw_count = do_a + do_n + do_r + do_w;
        if (anrw_count > 1) {
            fprintf(stderr, "history: cannot use more than one of -anrw\n");
            return 1;
        }

        if (do_c) {
            cupid_history_clear();
            g_history_file_cursor = 0;
            if (!(do_p || do_s || do_a || do_n || do_r || do_w)) return 0;
        }

        if (do_p) {
            for (i = 2; i < argc; i++) {
                puts(argv[i]);
            }
            return 0;
        }

        if (do_s) {
            size_t total = 0;
            char *joined;
            char *p;
            if (argc < 3) return 0;
            for (i = 2; i < argc; i++) {
                if (i > 2) total++;
                total += strlen(argv[i]);
            }
            joined = calloc(total + 1, 1);
            if (joined == NULL) return 1;
            p = joined;
            for (i = 2; i < argc; i++) {
                size_t n = strlen(argv[i]);
                if (i > 2) *p++ = ' ';
                memcpy(p, argv[i], n);
                p += n;
            }
            cupid_history_add(joined);
            free(joined);
            return 0;
        }

        if (do_a || do_w || do_r || do_n) {
            char *path = history_resolve_path(shell, file_arg);
            int rc = 0;
            if (path == NULL) return 1;
            if (do_a) {
                rc = history_write_file(path, 1, g_history_file_cursor);
                if (rc == 0) g_history_file_cursor = cupid_history_count();
            } else if (do_w) {
                rc = history_write_file(path, 0, 0);
                if (rc == 0) g_history_file_cursor = cupid_history_count();
            } else if (do_r || do_n) {
                rc = history_read_file(path);
                if (rc == 0) g_history_file_cursor = cupid_history_count();
            }
            free(path);
            return rc;
        }
    }

    count = cupid_history_count();
    start = 0;

    if (argc >= 2) {
        char *end;
        long n = strtol(argv[1], &end, 10);
        if (*end != '\0' || n < 0) {
            fprintf(stderr, "cupid: history: %s: numeric argument required\n", argv[1]);
            return 1;
        }
        if ((int)n < count) start = count - (int)n;
    }

    for (i = start; i < count; i++) {
        const char *entry = cupid_history_get(count - 1 - i);
        if (entry != NULL) {
            printf("%5d  %s\n", i + 1, entry);
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/*  fc                                                                */
/* ------------------------------------------------------------------ */

static void fc_print_usage(void) {
    fprintf(stderr, "fc: usage: fc [-e ename] [-lnr] [first] [last] or fc -s [pat=rep] [command]\n");
}

static int fc_error_out_of_range(struct cupid_shell *shell) {
    cupid_shell_error_prefix(stderr, shell);
    fprintf(stderr, "fc: history specification out of range\n");
    return 1;
}

static int fc_error_no_command(struct cupid_shell *shell) {
    cupid_shell_error_prefix(stderr, shell);
    fprintf(stderr, "fc: no command found\n");
    return 1;
}

static int fc_error_invalid_option(struct cupid_shell *shell, char opt) {
    cupid_shell_error_prefix(stderr, shell);
    fprintf(stderr, "fc: -%c: invalid option\n", opt);
    fc_print_usage();
    return 2;
}

static int fc_event_count(void) {
    return cupid_history_count();
}

static const char *fc_history_event_command(int event_number) {
    int count = fc_event_count();
    if (event_number < 1 || event_number > count) return NULL;
    return cupid_history_get(count - event_number);
}

static int fc_cmd_is_fc(const char *cmd) {
    const char *p = cmd;
    if (p == NULL) return 0;
    while (*p == ' ' || *p == '\t') p++;
    if (p[0] != 'f' || p[1] != 'c') return 0;
    return p[2] == '\0' || p[2] == ' ' || p[2] == '\t';
}

static int fc_default_last_event(void) {
    int count = fc_event_count();
    const char *latest;
    if (count <= 0) return 0;
    latest = cupid_history_get(0);
    if (count > 1 && fc_cmd_is_fc(latest)) return count - 1;
    return count;
}

static int fc_parse_long_strict(const char *text, long *out) {
    char *end = NULL;
    long v;
    if (text == NULL || text[0] == '\0') return 0;
    v = strtol(text, &end, 10);
    if (end == text || *end != '\0') return 0;
    *out = v;
    return 1;
}

static int fc_resolve_event(struct cupid_shell *shell, const char *spec, int list_mode,
                            int default_event, int *event_out, int *not_found_out) {
    int count = fc_event_count();
    long num;
    int ev;
    int search_start;
    size_t plen;

    (void)shell;
    if (not_found_out != NULL) *not_found_out = 0;
    if (event_out == NULL) return -1;
    if (count <= 0) return -1;

    if (spec == NULL) {
        *event_out = default_event;
        return 0;
    }

    if (fc_parse_long_strict(spec, &num)) {
        if (num == 0) {
            if (!list_mode) return -1;
            *event_out = count;
            return 0;
        }
        if (num < 0) {
            ev = count + (int)num;
        } else {
            ev = (int)num;
        }
        if (list_mode) {
            if (ev < 1) ev = 1;
            if (ev > count) ev = count;
            *event_out = ev;
            return 0;
        }
        if (ev < 1 || ev > count) return -1;
        *event_out = ev;
        return 0;
    }

    plen = strlen(spec);
    search_start = fc_default_last_event();
    if (search_start < 1) search_start = count;
    for (ev = search_start; ev >= 1; ev--) {
        const char *cmd = fc_history_event_command(ev);
        if (cmd != NULL && strncmp(cmd, spec, plen) == 0) {
            *event_out = ev;
            return 0;
        }
    }

    if (not_found_out != NULL) *not_found_out = 1;
    return -1;
}

static char *fc_replace_all(const char *src, const char *pat, const char *rep) {
    const char *p;
    size_t src_len;
    size_t pat_len;
    size_t rep_len;
    size_t hits = 0;
    size_t out_len;
    char *out;
    char *w;

    if (src == NULL) return strdup("");
    if (pat == NULL || pat[0] == '\0') return strdup(src);
    if (rep == NULL) rep = "";

    src_len = strlen(src);
    pat_len = strlen(pat);
    rep_len = strlen(rep);

    p = src;
    while ((p = strstr(p, pat)) != NULL) {
        hits++;
        p += pat_len;
    }
    if (hits == 0) return strdup(src);

    out_len = src_len + (rep_len >= pat_len ? (rep_len - pat_len) * hits
                                            : 0) - (rep_len < pat_len ? (pat_len - rep_len) * hits : 0);
    out = calloc(out_len + 1, 1);
    if (out == NULL) return NULL;

    p = src;
    w = out;
    while (*p != '\0') {
        const char *m = strstr(p, pat);
        if (m == NULL) {
            size_t n = strlen(p);
            memcpy(w, p, n);
            w += n;
            break;
        }
        if (m > p) {
            size_t n = (size_t)(m - p);
            memcpy(w, p, n);
            w += n;
        }
        if (rep_len > 0) {
            memcpy(w, rep, rep_len);
            w += rep_len;
        }
        p = m + pat_len;
    }
    *w = '\0';
    return out;
}

static int fc_print_list(int first_ev, int last_ev, int reverse, int show_numbers) {
    int start = first_ev;
    int end = last_ev;
    int step;
    int ev;

    if (reverse) {
        start = last_ev;
        end = first_ev;
    }
    step = (start <= end) ? 1 : -1;

    ev = start;
    while (1) {
        const char *cmd = fc_history_event_command(ev);
        if (cmd != NULL) {
            if (show_numbers) printf("%d\t %s\n", ev, cmd);
            else printf("\t %s\n", cmd);
        }
        if (ev == end) break;
        ev += step;
    }
    return 0;
}

static int builtin_fc(struct cupid_shell *shell, int argc, char **argv) {
    int list_mode = 0;
    int reverse = 0;
    int show_numbers = 1;
    int subst_mode = 0;
    const char *editor = NULL;
    int i = 1;
    int count = fc_event_count();
    int default_last;
    int default_first;
    int first_ev, last_ev;
    int nf = 0;

    while (i < argc) {
        const char *arg = argv[i];
        const char *p;
        if (strcmp(arg, "--") == 0) {
            i++;
            break;
        }
        if (arg[0] != '-' || arg[1] == '\0' || isdigit((unsigned char)arg[1])) break;
        p = arg + 1;
        while (*p != '\0') {
            switch (*p) {
                case 'l': list_mode = 1; break;
                case 'n': show_numbers = 0; break;
                case 'r': reverse = 1; break;
                case 's': subst_mode = 1; break;
                case 'e':
                    p++;
                    if (*p != '\0') {
                        editor = p;
                        p = "";
                        continue;
                    }
                    if (i + 1 >= argc) return fc_error_invalid_option(shell, 'e');
                    i++;
                    editor = argv[i];
                    break;
                default:
                    return fc_error_invalid_option(shell, *p);
            }
            p++;
        }
        i++;
    }

    if (count <= 0) return fc_error_no_command(shell);
    default_last = fc_default_last_event();
    if (default_last < 1) default_last = count;
    default_first = default_last - 15;
    if (default_first < 1) default_first = 1;

    if (subst_mode) {
        char *cmd_text;
        struct { char *pat; char *rep; } *subs = NULL;
        size_t sub_count = 0;
        size_t si;
        const char *cmd_spec = NULL;
        int ev;
        int rc;

        while (i < argc && strchr(argv[i], '=') != NULL) {
            const char *eq = strchr(argv[i], '=');
            size_t p_len = (size_t)(eq - argv[i]);
            char *pat = calloc(p_len + 1, 1);
            char *rep = strdup(eq + 1);
            void *next;
            if (pat == NULL || rep == NULL) {
                free(pat);
                free(rep);
                for (si = 0; si < sub_count; si++) {
                    free(subs[si].pat);
                    free(subs[si].rep);
                }
                free(subs);
                return 1;
            }
            if (p_len > 0) memcpy(pat, argv[i], p_len);
            next = realloc(subs, sizeof(*subs) * (sub_count + 1));
            if (next == NULL) {
                free(pat);
                free(rep);
                for (si = 0; si < sub_count; si++) {
                    free(subs[si].pat);
                    free(subs[si].rep);
                }
                free(subs);
                return 1;
            }
            subs = next;
            subs[sub_count].pat = pat;
            subs[sub_count].rep = rep;
            sub_count++;
            i++;
        }
        if (i < argc) cmd_spec = argv[i];

        if (fc_resolve_event(shell, cmd_spec, 0, default_last, &ev, &nf) != 0) {
            for (si = 0; si < sub_count; si++) {
                free(subs[si].pat);
                free(subs[si].rep);
            }
            free(subs);
            return fc_error_no_command(shell);
        }

        cmd_text = strdup(fc_history_event_command(ev) ? fc_history_event_command(ev) : "");
        if (cmd_text == NULL) {
            for (si = 0; si < sub_count; si++) {
                free(subs[si].pat);
                free(subs[si].rep);
            }
            free(subs);
            return 1;
        }
        for (si = 0; si < sub_count; si++) {
            char *next = fc_replace_all(cmd_text, subs[si].pat, subs[si].rep);
            if (next == NULL) {
                free(cmd_text);
                for (si = 0; si < sub_count; si++) {
                    free(subs[si].pat);
                    free(subs[si].rep);
                }
                free(subs);
                return 1;
            }
            free(cmd_text);
            cmd_text = next;
        }
        for (si = 0; si < sub_count; si++) {
            free(subs[si].pat);
            free(subs[si].rep);
        }
        free(subs);

        puts(cmd_text);
        rc = cupid_shell_eval_line(shell, cmd_text, 1);
        free(cmd_text);
        return rc;
    }

    if (list_mode) {
        const char *first_spec = NULL;
        const char *last_spec = NULL;
        if (i < argc) first_spec = argv[i++];
        if (i < argc) last_spec = argv[i++];
        if (first_spec == NULL) {
            first_ev = default_first;
            last_ev = default_last;
        } else if (last_spec == NULL) {
            if (fc_resolve_event(shell, first_spec, 1, default_first, &first_ev, &nf) != 0) {
                if (nf) return fc_error_no_command(shell);
                return fc_error_out_of_range(shell);
            }
            last_ev = default_last;
        } else {
            if (fc_resolve_event(shell, first_spec, 1, default_first, &first_ev, &nf) != 0) {
                if (nf) return fc_error_no_command(shell);
                return fc_error_out_of_range(shell);
            }
            if (fc_resolve_event(shell, last_spec, 1, default_last, &last_ev, &nf) != 0) {
                if (nf) return fc_error_no_command(shell);
                return fc_error_out_of_range(shell);
            }
        }
        return fc_print_list(first_ev, last_ev, reverse, show_numbers);
    }

    {
        const char *first_spec = NULL;
        const char *last_spec = NULL;
        int ev;
        char *cmd_text;
        int rc;

        if (i < argc) first_spec = argv[i++];
        if (i < argc) last_spec = argv[i++];

        if (first_spec == NULL) {
            ev = default_last;
        } else {
            if (fc_resolve_event(shell, first_spec, 0, default_last, &first_ev, &nf) != 0) {
                if (nf) return fc_error_no_command(shell);
                return fc_error_out_of_range(shell);
            }
            ev = first_ev;
            if (last_spec != NULL) {
                if (fc_resolve_event(shell, last_spec, 0, default_last, &last_ev, &nf) != 0) {
                    if (nf) return fc_error_no_command(shell);
                    return fc_error_out_of_range(shell);
                }
                ev = last_ev;
            }
        }

        cmd_text = strdup(fc_history_event_command(ev) ? fc_history_event_command(ev) : "");
        if (cmd_text == NULL) return 1;
        if (editor != NULL && editor[0] != '\0' && strcmp(editor, "-") != 0) {
            puts(cmd_text);
        }
        rc = cupid_shell_eval_line(shell, cmd_text, 1);
        free(cmd_text);
        return rc;
    }
}

/* ------------------------------------------------------------------ */
/*  complete                                                          */
/* ------------------------------------------------------------------ */

struct complete_action_def {
    const char *name;
    unsigned flag;
    int short_opt;
};

struct complete_opt_def {
    const char *name;
    unsigned flag;
};

struct complete_print_item {
    size_t index;
    unsigned bucket;
    size_t order;
};

#define COMPLETE_BUCKETS 512U
#define COMPLETE_FNV_OFFSET 2166136261U

/* Matches bash-5.2.21 builtins/complete.def compacts[] order. */
static const struct complete_action_def g_complete_actions[] = {
    {"alias", 1u << 0, 'a'},
    {"arrayvar", 1u << 1, 0},
    {"binding", 1u << 2, 0},
    {"builtin", 1u << 3, 'b'},
    {"command", 1u << 4, 'c'},
    {"directory", 1u << 5, 'd'},
    {"disabled", 1u << 6, 0},
    {"enabled", 1u << 7, 0},
    {"export", 1u << 8, 'e'},
    {"file", 1u << 9, 'f'},
    {"function", 1u << 10, 0},
    {"helptopic", 1u << 11, 0},
    {"hostname", 1u << 12, 0},
    {"group", 1u << 13, 'g'},
    {"job", 1u << 14, 'j'},
    {"keyword", 1u << 15, 'k'},
    {"running", 1u << 16, 0},
    {"service", 1u << 17, 's'},
    {"setopt", 1u << 18, 0},
    {"shopt", 1u << 19, 0},
    {"signal", 1u << 20, 0},
    {"stopped", 1u << 21, 0},
    {"user", 1u << 22, 'u'},
    {"variable", 1u << 23, 'v'},
    {NULL, 0, 0},
};

/* Matches bash-5.2.21 builtins/complete.def compopts[] order. */
static const struct complete_opt_def g_complete_opts[] = {
    {"bashdefault", 1u << 0},
    {"default", 1u << 1},
    {"dirnames", 1u << 2},
    {"filenames", 1u << 3},
    {"noquote", 1u << 4},
    {"nosort", 1u << 5},
    {"nospace", 1u << 6},
    {"plusdirs", 1u << 7},
    {NULL, 0},
};

static unsigned complete_hash_string(const char *s) {
    unsigned i;
    if (s == NULL) return 0;
    for (i = COMPLETE_FNV_OFFSET; *s != '\0'; s++) {
        i += (i << 1) + (i << 4) + (i << 7) + (i << 8) + (i << 24);
        i ^= (unsigned char)*s;
    }
    return i;
}

static int complete_append_bytes(char **buf, size_t *len, size_t *cap, const char *text) {
    size_t need;
    char *next;
    if (text == NULL) return 0;
    need = strlen(text);
    if (*len + need + 1 > *cap) {
        size_t nc = (*cap == 0) ? 64 : *cap * 2;
        while (*len + need + 1 > nc) nc *= 2;
        next = realloc(*buf, nc);
        if (next == NULL) return -1;
        *buf = next;
        *cap = nc;
    }
    memcpy(*buf + *len, text, need);
    *len += need;
    (*buf)[*len] = '\0';
    return 0;
}

static int complete_append_token(char **buf, size_t *len, size_t *cap, const char *tok) {
    if (*len > 0 && complete_append_bytes(buf, len, cap, " ") != 0) return -1;
    return complete_append_bytes(buf, len, cap, tok);
}

static char *complete_quote_single(const char *s) {
    size_t i;
    size_t len = 0;
    char *out;
    if (s == NULL) return strdup("''");
    len += 2;
    for (i = 0; s[i] != '\0'; i++) {
        if (s[i] == '\'') len += 4;
        else len += 1;
    }
    out = calloc(len + 1, 1);
    if (out == NULL) return NULL;
    {
        size_t off = 0;
        out[off++] = '\'';
        for (i = 0; s[i] != '\0'; i++) {
            if (s[i] == '\'') {
                out[off++] = '\'';
                out[off++] = '\\';
                out[off++] = '\'';
                out[off++] = '\'';
            } else {
                out[off++] = s[i];
            }
        }
        out[off++] = '\'';
        out[off] = '\0';
    }
    return out;
}

static int complete_set_field(char **dst, const char *src) {
    char *dup = strdup(src ? src : "");
    if (dup == NULL) return -1;
    free(*dst);
    *dst = dup;
    return 0;
}

static int complete_action_from_name(const char *name, unsigned *flag_out) {
    const struct complete_action_def *def;
    if (flag_out != NULL) *flag_out = 0;
    if (name == NULL) return -1;
    for (def = g_complete_actions; def->name != NULL; def++) {
        if (strcmp(def->name, name) == 0) {
            if (flag_out != NULL) *flag_out = def->flag;
            return 0;
        }
    }
    return -1;
}

static int complete_action_from_short(char ch, unsigned *flag_out) {
    const struct complete_action_def *def;
    if (flag_out != NULL) *flag_out = 0;
    for (def = g_complete_actions; def->name != NULL; def++) {
        if (def->short_opt != 0 && def->short_opt == ch) {
            if (flag_out != NULL) *flag_out = def->flag;
            return 0;
        }
    }
    return -1;
}

static int complete_opt_from_name(const char *name, unsigned *flag_out) {
    const struct complete_opt_def *def;
    if (flag_out != NULL) *flag_out = 0;
    if (name == NULL) return -1;
    for (def = g_complete_opts; def->name != NULL; def++) {
        if (strcmp(def->name, name) == 0) {
            if (flag_out != NULL) *flag_out = def->flag;
            return 0;
        }
    }
    return -1;
}

static int complete_contains_shell_metas(const char *s) {
    size_t i;
    if (s == NULL) return 0;
    for (i = 0; s[i] != '\0'; i++) {
        unsigned char ch = (unsigned char)s[i];
        if (isspace(ch) || strchr("*?[]$`'\"\\|&;<>(){}!", ch) != NULL) return 1;
    }
    return 0;
}

static int complete_append_flag_arg(char **buf, size_t *len, size_t *cap,
                                    const char *flag, const char *value, int quote) {
    char *rendered = NULL;
    int rc;
    if (value == NULL) return 0;
    if (quote) {
        rendered = complete_quote_single(value);
        if (rendered == NULL) return -1;
    }
    rc = complete_append_token(buf, len, cap, flag);
    if (rc == 0) rc = complete_append_token(buf, len, cap, quote ? rendered : value);
    free(rendered);
    return rc;
}

static char *complete_build_spec(unsigned actions, unsigned compopts,
                                 const char *globpat, const char *words,
                                 const char *prefix, const char *suffix,
                                 const char *filterpat, const char *command,
                                 const char *funcname) {
    char *buf = NULL;
    size_t len = 0;
    size_t cap = 0;
    const struct complete_opt_def *co;
    const struct complete_action_def *ca;

    for (co = g_complete_opts; co->name != NULL; co++) {
        if ((compopts & co->flag) == 0) continue;
        if (complete_append_token(&buf, &len, &cap, "-o") != 0 ||
            complete_append_token(&buf, &len, &cap, co->name) != 0) {
            free(buf);
            return NULL;
        }
    }

    for (ca = g_complete_actions; ca->name != NULL; ca++) {
        char flag_buf[4];
        if (ca->short_opt == 0 || (actions & ca->flag) == 0) continue;
        snprintf(flag_buf, sizeof(flag_buf), "-%c", ca->short_opt);
        if (complete_append_token(&buf, &len, &cap, flag_buf) != 0) {
            free(buf);
            return NULL;
        }
    }
    for (ca = g_complete_actions; ca->name != NULL; ca++) {
        if (ca->short_opt != 0 || (actions & ca->flag) == 0) continue;
        if (complete_append_token(&buf, &len, &cap, "-A") != 0 ||
            complete_append_token(&buf, &len, &cap, ca->name) != 0) {
            free(buf);
            return NULL;
        }
    }

    if (complete_append_flag_arg(&buf, &len, &cap, "-G", globpat, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-W", words, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-P", prefix, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-S", suffix, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-X", filterpat, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-C", command, 1) != 0 ||
        complete_append_flag_arg(&buf, &len, &cap, "-F", funcname,
                                 complete_contains_shell_metas(funcname)) != 0) {
        free(buf);
        return NULL;
    }

    if (buf == NULL) {
        buf = strdup("");
    }
    return buf;
}

static long complete_find_index(const struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return -1;
    for (i = 0; i < shell->completions.count; i++) {
        if (strcmp(shell->completions.entries[i].name, name) == 0) {
            return (long)i;
        }
    }
    return -1;
}

static int complete_set_spec(struct cupid_shell *shell, const char *name, const char *spec) {
    long idx;
    char *name_copy;
    char *spec_copy;
    if (shell == NULL || name == NULL || spec == NULL) return -1;
    idx = complete_find_index(shell, name);
    spec_copy = strdup(spec);
    if (spec_copy == NULL) return -1;
    if (idx >= 0) {
        free(shell->completions.entries[(size_t)idx].spec);
        shell->completions.entries[(size_t)idx].spec = spec_copy;
        return 0;
    }
    if (shell->completions.count == shell->completions.capacity) {
        size_t nc = (shell->completions.capacity == 0) ? 32 : shell->completions.capacity * 2;
        struct cupid_completion_spec *next =
            realloc(shell->completions.entries, sizeof(*next) * nc);
        if (next == NULL) {
            free(spec_copy);
            return -1;
        }
        shell->completions.entries = next;
        shell->completions.capacity = nc;
    }
    name_copy = strdup(name);
    if (name_copy == NULL) {
        free(spec_copy);
        return -1;
    }
    shell->completions.entries[shell->completions.count].name = name_copy;
    shell->completions.entries[shell->completions.count].spec = spec_copy;
    shell->completions.entries[shell->completions.count].hash = complete_hash_string(name);
    shell->completions.entries[shell->completions.count].order = shell->completions.next_order++;
    shell->completions.count++;
    return 0;
}

static int complete_unset_spec(struct cupid_shell *shell, const char *name) {
    long idx = complete_find_index(shell, name);
    if (idx < 0) return -1;
    free(shell->completions.entries[(size_t)idx].name);
    free(shell->completions.entries[(size_t)idx].spec);
    if ((size_t)idx + 1 < shell->completions.count) {
        shell->completions.entries[(size_t)idx] =
            shell->completions.entries[shell->completions.count - 1];
    }
    shell->completions.count--;
    return 0;
}

static void complete_clear_specs(struct cupid_shell *shell) {
    size_t i;
    if (shell == NULL) return;
    for (i = 0; i < shell->completions.count; i++) {
        free(shell->completions.entries[i].name);
        free(shell->completions.entries[i].spec);
    }
    shell->completions.count = 0;
    shell->completions.next_order = 0;
}

static int complete_print_spec(struct cupid_shell *shell, const char *name) {
    long idx = complete_find_index(shell, name);
    if (idx < 0) return -1;
    if (shell->completions.entries[(size_t)idx].spec[0] != '\0') {
        printf("complete %s %s\n", shell->completions.entries[(size_t)idx].spec, name);
    } else {
        printf("complete %s\n", name);
    }
    return 0;
}

static int complete_print_item_cmp(const void *a, const void *b) {
    const struct complete_print_item *ia = (const struct complete_print_item *)a;
    const struct complete_print_item *ib = (const struct complete_print_item *)b;
    if (ia->bucket < ib->bucket) return -1;
    if (ia->bucket > ib->bucket) return 1;
    if (ia->order > ib->order) return -1;
    if (ia->order < ib->order) return 1;
    return 0;
}

static void complete_print_all_specs(struct cupid_shell *shell) {
    struct complete_print_item *items;
    size_t i;
    if (shell == NULL || shell->completions.count == 0) return;
    items = calloc(shell->completions.count, sizeof(*items));
    if (items == NULL) return;
    for (i = 0; i < shell->completions.count; i++) {
        items[i].index = i;
        items[i].bucket = shell->completions.entries[i].hash & (COMPLETE_BUCKETS - 1u);
        items[i].order = shell->completions.entries[i].order;
    }
    qsort(items, shell->completions.count, sizeof(*items), complete_print_item_cmp);
    for (i = 0; i < shell->completions.count; i++) {
        struct cupid_completion_spec *entry = &shell->completions.entries[items[i].index];
        if (entry->spec[0] != '\0') printf("complete %s %s\n", entry->spec, entry->name);
        else printf("complete %s\n", entry->name);
    }
    free(items);
}

static int builtin_complete(struct cupid_shell *shell, int argc, char **argv) {
    int mode_print = 0;
    int mode_remove = 0;
    int parsing_opts = 1;
    int i = 1;
    int names_start = 1;
    unsigned actions = 0;
    unsigned compopts = 0;
    char *globpat = NULL;
    char *words = NULL;
    char *prefix = NULL;
    char *suffix = NULL;
    char *filterpat = NULL;
    char *command = NULL;
    char *funcname = NULL;
    char *spec = NULL;
    int status = 0;

    if (shell == NULL) return 1;

    while (i < argc) {
        if (parsing_opts && strcmp(argv[i], "--") == 0) {
            parsing_opts = 0;
            i++;
            names_start = i;
            continue;
        }
        if (!parsing_opts || argv[i][0] != '-' || argv[i][1] == '\0') {
            names_start = i;
            break;
        }
        if (strcmp(argv[i], "-p") == 0) {
            mode_print = 1;
            i++;
            names_start = i;
            continue;
        }
        if (strcmp(argv[i], "-r") == 0) {
            mode_remove = 1;
            i++;
            names_start = i;
            continue;
        }
        if (strcmp(argv[i], "-A") == 0 || strcmp(argv[i], "-o") == 0 ||
            strcmp(argv[i], "-F") == 0 || strcmp(argv[i], "-G") == 0 ||
            strcmp(argv[i], "-W") == 0 || strcmp(argv[i], "-P") == 0 ||
            strcmp(argv[i], "-S") == 0 || strcmp(argv[i], "-X") == 0 ||
            strcmp(argv[i], "-C") == 0) {
            unsigned flag = 0;
            if (i + 1 >= argc) {
                status = 1;
                goto done;
            }
            if (strcmp(argv[i], "-A") == 0) {
                if (complete_action_from_name(argv[i + 1], &flag) != 0) {
                    status = 1;
                    goto done;
                }
                actions |= flag;
            } else if (strcmp(argv[i], "-o") == 0) {
                if (complete_opt_from_name(argv[i + 1], &flag) != 0) {
                    status = 1;
                    goto done;
                }
                compopts |= flag;
            } else if (strcmp(argv[i], "-F") == 0) {
                if (complete_set_field(&funcname, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-G") == 0) {
                if (complete_set_field(&globpat, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-W") == 0) {
                if (complete_set_field(&words, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-P") == 0) {
                if (complete_set_field(&prefix, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-S") == 0) {
                if (complete_set_field(&suffix, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-X") == 0) {
                if (complete_set_field(&filterpat, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else if (strcmp(argv[i], "-C") == 0) {
                if (complete_set_field(&command, argv[i + 1]) != 0) {
                    status = 1;
                    goto done;
                }
            } else {
                status = 1;
                goto done;
            }
            i += 2;
            names_start = i;
            continue;
        } else if (argv[i][0] == '-' && argv[i][1] != '\0') {
            size_t j;
            for (j = 1; argv[i][j] != '\0'; j++) {
                unsigned flag = 0;
                if (complete_action_from_short(argv[i][j], &flag) != 0) {
                    status = 1;
                    goto done;
                }
                actions |= flag;
            }
            i++;
            names_start = i;
            continue;
        } else {
            names_start = i;
            break;
        }
    }

    if (!mode_print && !mode_remove && names_start >= argc && argc == 1) {
        mode_print = 1;
    }

    if (mode_print) {
        if (names_start >= argc) {
            complete_print_all_specs(shell);
        } else {
            for (i = names_start; i < argc; i++) {
                if (complete_print_spec(shell, argv[i]) != 0) {
                    status = 1;
                }
            }
        }
        goto done;
    }

    if (mode_remove) {
        if (names_start >= argc) {
            complete_clear_specs(shell);
            status = 0;
            goto done;
        }
        for (i = names_start; i < argc; i++) {
            if (complete_unset_spec(shell, argv[i]) != 0) {
                cupid_shell_error_prefix(stderr, shell);
                fprintf(stderr, "complete: %s: no completion specification\n", argv[i]);
                status = 1;
            }
        }
        goto done;
    }

    if (names_start >= argc) {
        status = 1;
        goto done;
    }
    spec = complete_build_spec(actions, compopts, globpat, words, prefix, suffix,
                               filterpat, command, funcname);
    if (spec == NULL) {
        status = 1;
        goto done;
    }
    for (i = names_start; i < argc; i++) {
        if (complete_set_spec(shell, argv[i], spec) != 0) {
            status = 1;
            break;
        }
    }

done:
    free(globpat);
    free(words);
    free(prefix);
    free(suffix);
    free(filterpat);
    free(command);
    free(funcname);
    free(spec);
    return status;
}

/* ------------------------------------------------------------------ */
/*  jobs / fg / bg                                                    */
/* ------------------------------------------------------------------ */

static int builtin_jobs(struct cupid_shell *shell) {
    int i;
    for (i = 0; i < shell->job_count; i++) {
        if (shell->jobs[i].pgid == 0) continue;
        printf("[%d] %s  %s\n", shell->jobs[i].job_id,
               shell->jobs[i].stopped ? "Stopped" :
               shell->jobs[i].completed ? "Done" : "Running",
               shell->jobs[i].command ? shell->jobs[i].command : "");
    }
    return 0;
}

static int find_job(struct cupid_shell *shell, const char *spec) {
    int i;
    if (spec == NULL) {
        for (i = shell->job_count - 1; i >= 0; i--) {
            if (shell->jobs[i].pgid != 0 && !shell->jobs[i].completed) return i;
        }
        return -1;
    }
    if (spec[0] == '%') spec++;
    {
        char *end;
        long id = strtol(spec, &end, 10);
        if (*end == '\0') {
            for (i = 0; i < shell->job_count; i++) {
                if (shell->jobs[i].job_id == (int)id) return i;
            }
        }
    }
    return -1;
}

static int builtin_fg(struct cupid_shell *shell, int argc, char **argv) {
    int idx;
    pid_t pgid;
    int status = 0;

    idx = find_job(shell, argc > 1 ? argv[1] : NULL);
    if (idx < 0) {
        fprintf(stderr, "cupid: fg: no current job\n");
        return 1;
    }
    pgid = shell->jobs[idx].pgid;

    tcsetpgrp(STDIN_FILENO, pgid);
    if (shell->jobs[idx].stopped) {
        kill(-pgid, SIGCONT);
        shell->jobs[idx].stopped = 0;
    }

    {
        int st;
        pid_t r;
        while ((r = waitpid(-pgid, &st, WUNTRACED)) > 0) {
            if (WIFSTOPPED(st)) {
                shell->jobs[idx].stopped = 1;
                fprintf(stderr, "\n[%d]+  Stopped  %s\n",
                        shell->jobs[idx].job_id,
                        shell->jobs[idx].command ? shell->jobs[idx].command : "");
                break;
            }
            if (WIFEXITED(st)) {
                status = WEXITSTATUS(st);
                shell->jobs[idx].completed = 1;
                break;
            }
            if (WIFSIGNALED(st)) {
                status = 128 + WTERMSIG(st);
                shell->jobs[idx].completed = 1;
                break;
            }
        }
    }

    tcsetpgrp(STDIN_FILENO, getpgrp());
    return status;
}

static int builtin_bg(struct cupid_shell *shell, int argc, char **argv) {
    int idx;

    idx = find_job(shell, argc > 1 ? argv[1] : NULL);
    if (idx < 0) {
        fprintf(stderr, "cupid: bg: no current job\n");
        return 1;
    }
    if (!shell->jobs[idx].stopped) {
        fprintf(stderr, "cupid: bg: job %d already running\n", shell->jobs[idx].job_id);
        return 0;
    }

    shell->jobs[idx].stopped = 0;
    kill(-shell->jobs[idx].pgid, SIGCONT);
    fprintf(stderr, "[%d]+ %s &\n", shell->jobs[idx].job_id,
            shell->jobs[idx].command ? shell->jobs[idx].command : "");
    return 0;
}

/* ------------------------------------------------------------------ */
/*  Builtin table and dispatch                                        */
/* ------------------------------------------------------------------ */

static const char *g_builtin_names[] = {
    "exit", "cd", "pwd", "export", "unset", "source", ".", "break", "continue",
    "return", "shift", "local", "true", "false", ":", "echo", "printf",
    "test", "[", "read", "eval", "exec", "set", "trap", "wait", "type",
    "command", "builtin", "hash", "shopt", "alias", "unalias",
    "declare", "typeset", "getopts", "kill", "readonly", "let",
    "mapfile", "readarray", "umask", "ulimit", "enable", "times",
    "history", "fc", "complete", "jobs", "fg", "bg", NULL
};

int cupid_is_builtin(const char *name) {
    const char **p;
    for (p = g_builtin_names; *p != NULL; p++) {
        if (strcmp(*p, name) == 0) return 1;
    }
    return 0;
}

const char **cupid_builtin_names(void) {
    return g_builtin_names;
}

int cupid_run_builtin(struct cupid_shell *shell, int argc, char **argv, bool in_child) {
    if (argc == 0 || argv == NULL || argv[0] == NULL) {
        return CUPID_BUILTIN_NOT_FOUND;
    }
    if (shell != NULL &&
        strcmp(argv[0], "enable") != 0 &&
        disabled_builtin_index(shell, argv[0]) >= 0) {
        return CUPID_BUILTIN_NOT_FOUND;
    }
    if (strcmp(argv[0], "exit") == 0) return builtin_exit(shell, argc, argv, in_child);
    if (strcmp(argv[0], "cd") == 0) return builtin_cd(shell, argc, argv);
    if (strcmp(argv[0], "pwd") == 0) return builtin_pwd();
    if (strcmp(argv[0], "export") == 0) return builtin_export(shell, argc, argv);
    if (strcmp(argv[0], "unset") == 0) return builtin_unset(shell, argc, argv);
    if (strcmp(argv[0], "source") == 0) {
        if (shell != NULL && shell->mode == CUPID_MODE_POSIX) {
            return CUPID_BUILTIN_NOT_FOUND;
        }
        return builtin_source(shell, argc, argv);
    }
    if (strcmp(argv[0], ".") == 0) return builtin_source(shell, argc, argv);
    if (strcmp(argv[0], "break") == 0) return builtin_break(shell, argc, argv);
    if (strcmp(argv[0], "continue") == 0) return builtin_continue(shell, argc, argv);
    if (strcmp(argv[0], "return") == 0) return builtin_return(shell, argc, argv);
    if (strcmp(argv[0], "shift") == 0) return builtin_shift(shell, argc, argv);
    if (strcmp(argv[0], "local") == 0) return builtin_local(shell, argc, argv);
    if (strcmp(argv[0], "true") == 0) return builtin_true();
    if (strcmp(argv[0], "false") == 0) return builtin_false();
    if (strcmp(argv[0], ":") == 0) return builtin_true();
    if (strcmp(argv[0], "echo") == 0) return builtin_echo(shell, argc, argv);
    if (strcmp(argv[0], "printf") == 0) return builtin_printf(shell, argc, argv);
    if (strcmp(argv[0], "test") == 0 || strcmp(argv[0], "[") == 0) return builtin_test(shell, argc, argv);
    if (strcmp(argv[0], "read") == 0) return builtin_read(shell, argc, argv);
    if (strcmp(argv[0], "eval") == 0) return builtin_eval(shell, argc, argv);
    if (strcmp(argv[0], "exec") == 0) return builtin_exec(argc, argv);
    if (strcmp(argv[0], "set") == 0) return builtin_set(shell, argc, argv);
    if (strcmp(argv[0], "trap") == 0) return builtin_trap(shell, argc, argv);
    if (strcmp(argv[0], "wait") == 0) return builtin_wait();
    if (strcmp(argv[0], "type") == 0) return builtin_type(shell, argc, argv);
    if (strcmp(argv[0], "command") == 0) return builtin_command(shell, argc, argv, in_child);
    if (strcmp(argv[0], "builtin") == 0) return builtin_builtin(shell, argc, argv, in_child);
    if (strcmp(argv[0], "hash") == 0) return builtin_hash(shell, argc, argv);
    if (strcmp(argv[0], "shopt") == 0) return builtin_shopt(shell, argc, argv);
    if (strcmp(argv[0], "alias") == 0) return builtin_alias(shell, argc, argv);
    if (strcmp(argv[0], "unalias") == 0) return builtin_unalias(shell, argc, argv);
    if (strcmp(argv[0], "declare") == 0 || strcmp(argv[0], "typeset") == 0) return builtin_declare(shell, argc, argv);
    if (strcmp(argv[0], "getopts") == 0) return builtin_getopts(shell, argc, argv);
    if (strcmp(argv[0], "kill") == 0) return builtin_kill(shell, argc, argv);
    if (strcmp(argv[0], "readonly") == 0) return builtin_readonly(shell, argc, argv);
    if (strcmp(argv[0], "let") == 0) return builtin_let(shell, argc, argv);
    if (strcmp(argv[0], "mapfile") == 0 || strcmp(argv[0], "readarray") == 0) return builtin_mapfile(shell, argc, argv);
    if (strcmp(argv[0], "umask") == 0) return builtin_umask(argc, argv);
    if (strcmp(argv[0], "ulimit") == 0) return builtin_ulimit(argc, argv);
    if (strcmp(argv[0], "enable") == 0) return builtin_enable(shell, argc, argv);
    if (strcmp(argv[0], "times") == 0) return builtin_times();
    if (strcmp(argv[0], "history") == 0) return builtin_history(shell, argc, argv);
    if (strcmp(argv[0], "fc") == 0) return builtin_fc(shell, argc, argv);
    if (strcmp(argv[0], "complete") == 0) return builtin_complete(shell, argc, argv);
    if (strcmp(argv[0], "jobs") == 0) return builtin_jobs(shell);
    if (strcmp(argv[0], "fg") == 0) return builtin_fg(shell, argc, argv);
    if (strcmp(argv[0], "bg") == 0) return builtin_bg(shell, argc, argv);
    return CUPID_BUILTIN_NOT_FOUND;
}
