#include "cupid/exec.h"

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <glob.h>
#include <regex.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "cupid/arith.h"
#include "cupid/brace.h"
#include "cupid/builtins.h"
#include "cupid/expand.h"
#include "cupid/heredoc.h"
#include "cupid/history.h"
#include "cupid/lexer.h"
#include "cupid/shell.h"
#include "cupid/vars.h"

struct runtime_redir {
    enum cupid_redir_kind kind;
    int fd;
    char *fd_var;
    char *target;
    char *target_source;
    char *heredoc_delim;
    char *heredoc_body;
    int heredoc_fd;
    int proc_subst_fd;
    int heredoc_quoted;
    int heredoc_strip_tabs;
};

struct runtime_command {
    char **argv;
    int argc;
    int cmdsub_seen;
    int cmdsub_status;
    struct runtime_redir *redirs;
    size_t redir_count;
};

struct runtime_pipeline {
    struct runtime_command *commands;
    size_t count;
};

struct temp_env_assignment {
    char *name;
    char *old_value;
    int had_old_value;
    char *old_shell_value;
    int had_shell_binding;
    int restore_shell_binding;
};

struct expand_assignment_restore {
    char *name;
    char *old_shell_value;
    int had_shell_binding;
    char *old_env_value;
    int had_env_value;
};

static int command_add_arg(struct runtime_command *cmd, char *arg);
static int parse_subshell_script(const char *arg, char **out_script);
static int execute_list(struct cupid_shell *shell, const struct cupid_list_ast *list);
static int exec_func_call(struct cupid_shell *shell, struct cupid_list_ast *body, struct runtime_command *cmd);
static int exec_compound_node(struct cupid_shell *shell, const struct cupid_node *node);
static int execute_compound_with_redirs(struct cupid_shell *shell, const struct cupid_node *node);
static void runtime_command_free(struct runtime_command *cmd);
static int spawn_proc_subst_fd(const char *text, struct cupid_shell *shell, int *fd_out);
static int maybe_run_script_with_cupid(struct cupid_shell *shell, const struct runtime_command *cmd);
static int exec_runtime_command(struct cupid_shell *shell, struct runtime_command *cmd);
static int add_field_with_glob(struct runtime_command *cmd, const char *field, int allow_glob,
                               struct cupid_shell *shell);
static int add_expanded_word(struct runtime_command *cmd, const struct cupid_word *word,
                             struct cupid_shell *shell, int assignment_tilde_allowed);
static int append_exec_bytes(char **buf, size_t *len, size_t *cap, const char *data, size_t data_len);
static int expand_quoted_positional_parts_word(struct runtime_command *cmd,
                                               const struct cupid_word *word,
                                               struct cupid_shell *shell);
static int expand_general_quoted_positional_splices_word(struct runtime_command *cmd,
                                                         const struct cupid_word *word,
                                                         struct cupid_shell *shell);
static int expand_embedded_quoted_positional_star_at_word(struct runtime_command *cmd,
                                                          const struct cupid_word *word,
                                                          struct cupid_shell *shell);
static int expand_multipart_quoted_positional_star_at_word(struct runtime_command *cmd,
                                                           const struct cupid_word *word,
                                                           struct cupid_shell *shell);
static int shell_has_var_binding(const struct cupid_shell *shell, const char *name);

static int str_ends_with(const char *s, const char *suffix) {
    size_t slen, tlen;
    if (s == NULL || suffix == NULL) return 0;
    slen = strlen(s);
    tlen = strlen(suffix);
    if (tlen > slen) return 0;
    return strcmp(s + slen - tlen, suffix) == 0;
}

static const char *path_basename(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash ? slash + 1 : path;
}

static int script_mode_for_interpreter(const char *name, enum cupid_mode *mode_out) {
    if (name == NULL || mode_out == NULL) return 0;
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

static int shebang_script_mode(const char *buf, enum cupid_mode *mode_out) {
    const char *p = buf;
    const char *tok_start;
    const char *tok_end;
    char tok[128];
    const char *interp_name;
    enum cupid_mode mode;

    if (p[0] != '#' || p[1] != '!') return 0;
    p += 2;
    while (*p == ' ' || *p == '\t') p++;
    if (*p == '\0' || *p == '\n') return 0;

    tok_start = p;
    while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
    tok_end = p;
    if (tok_end <= tok_start) return 0;

    if ((size_t)(tok_end - tok_start) >= sizeof(tok)) return 0;
    memcpy(tok, tok_start, (size_t)(tok_end - tok_start));
    tok[tok_end - tok_start] = '\0';

    interp_name = path_basename(tok);
    if (strcmp(interp_name, "env") == 0) {
        while (*p == ' ' || *p == '\t') p++;
        while (*p == '-') {
            while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
            while (*p == ' ' || *p == '\t') p++;
        }
        tok_start = p;
        while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
        tok_end = p;
        if (tok_end <= tok_start) return 0;
        if ((size_t)(tok_end - tok_start) >= sizeof(tok)) return 0;
        memcpy(tok, tok_start, (size_t)(tok_end - tok_start));
        tok[tok_end - tok_start] = '\0';
        interp_name = path_basename(tok);
    }
    if (!script_mode_for_interpreter(interp_name, &mode)) return 0;
    *mode_out = mode;
    return 1;
}

static int path_is_shell_script(const char *path, enum cupid_mode *mode_out) {
    struct stat st;
    int fd;
    ssize_t n;
    char hdr[256];
    enum cupid_mode mode = *mode_out;
    int has_mode = 0;

    if (path == NULL || mode_out == NULL) return 0;
    if (strchr(path, '/') == NULL) return 0;
    if (access(path, X_OK) != 0) return 0;
    if (stat(path, &st) != 0 || !S_ISREG(st.st_mode)) return 0;
    if (str_ends_with(path, ".sh")) {
        has_mode = 1;
    }

    fd = open(path, O_RDONLY);
    if (fd < 0) {
        if (!has_mode) return 0;
        *mode_out = mode;
        return 1;
    }
    n = read(fd, hdr, sizeof(hdr) - 1);
    close(fd);
    if (n <= 0) {
        if (!has_mode) return 0;
        *mode_out = mode;
        return 1;
    }
    hdr[n] = '\0';
    if (shebang_script_mode(hdr, &mode)) {
        *mode_out = mode;
        return 1;
    }
    if (!has_mode) return 0;
    *mode_out = mode;
    return 1;
}

static int maybe_run_script_with_cupid(struct cupid_shell *shell, const struct runtime_command *cmd) {
    size_t i;
    int rc;
    enum cupid_mode script_mode;
    struct cupid_shell child_shell;

    if (shell == NULL || cmd == NULL || cmd->argc <= 0) return -1;
    script_mode = shell->mode;
    if (!path_is_shell_script(cmd->argv[0], &script_mode)) return -1;

    cupid_shell_init(&child_shell);
    child_shell.mode = script_mode;
    child_shell.shell_pid = shell->shell_pid;
    child_shell.is_interactive = shell->is_interactive;
    child_shell.opt_errexit = shell->opt_errexit;
    child_shell.opt_allexport = shell->opt_allexport;
    child_shell.opt_noglob = shell->opt_noglob;
    child_shell.opt_monitor = shell->opt_monitor;
    child_shell.opt_nounset = shell->opt_nounset;
    child_shell.opt_xtrace = shell->opt_xtrace;
    child_shell.opt_pipefail = shell->opt_pipefail;
    child_shell.opt_expand_aliases = shell->opt_expand_aliases;
    child_shell.opt_extglob = shell->opt_extglob;
    child_shell.opt_nullglob = shell->opt_nullglob;
    child_shell.opt_lastpipe = shell->opt_lastpipe;
    child_shell.opt_histexpand = shell->opt_histexpand;
    child_shell.opt_cmdhist = shell->opt_cmdhist;
    child_shell.opt_sourcepath = shell->opt_sourcepath;
    child_shell.opt_xpg_echo = shell->opt_xpg_echo;
    child_shell.arg0 = strdup(cmd->argv[0]);
    if (child_shell.arg0 == NULL) {
        cupid_shell_destroy(&child_shell);
        return 1;
    }
    if (cmd->argc > 1) {
        child_shell.params.args = calloc((size_t)(cmd->argc - 1), sizeof(char *));
        if (child_shell.params.args == NULL) {
            cupid_shell_destroy(&child_shell);
            return 1;
        }
        for (i = 1; i < (size_t)cmd->argc; i++) {
            child_shell.params.args[i - 1] = strdup(cmd->argv[i]);
            if (child_shell.params.args[i - 1] == NULL) {
                cupid_shell_destroy(&child_shell);
                return 1;
            }
            child_shell.params.count++;
        }
    }

    rc = cupid_shell_eval_file(&child_shell, cmd->argv[0]);
    if (child_shell.should_exit) rc = child_shell.exit_code;
    cupid_shell_run_exit_trap(&child_shell);
    cupid_shell_destroy(&child_shell);
    return rc;
}

static int glob_bracket_has_closing(const char *p) {
    if (p == NULL || *p != '[') return 0;
    p++;
    if (*p == '!' || *p == '^') p++;
    if (*p == ']') p++;
    while (*p != '\0' && *p != '/') {
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (*p == ']') return 1;
        p++;
    }
    return 0;
}

static int has_glob_meta(struct cupid_shell *shell, const char *s) {
    const char *p = s;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (*p == '*' || *p == '?') return 1;
        if (*p == '[') {
            if (glob_bracket_has_closing(p)) return 1;
            p++;
            continue;
        }
        if (shell != NULL && shell->opt_extglob &&
            (*p == '@' || *p == '+' || *p == '!') && p[1] == '(') {
            return 1;
        }
        p++;
    }
    return 0;
}

static size_t trailing_empty_quoted_parts(const struct cupid_word *word) {
    size_t count = 0;

    if (word == NULL) return 0;
    while (count < word->part_count) {
        const struct cupid_word_part *part = &word->parts[word->part_count - count - 1];
        if (part->quote == CUPID_QUOTE_NONE) break;
        if (part->text == NULL || part->text[0] != '\0') break;
        count++;
    }
    return count;
}

static int word_has_only_trailing_empty_quotes(const struct cupid_word *word, size_t *prefix_parts_out) {
    size_t trailing;
    size_t prefix_parts;
    size_t i;

    if (prefix_parts_out == NULL || word == NULL) return 0;
    trailing = trailing_empty_quoted_parts(word);
    if (trailing == 0 || trailing >= word->part_count) return 0;
    prefix_parts = word->part_count - trailing;
    for (i = 0; i < prefix_parts; i++) {
        if (word->parts[i].quote != CUPID_QUOTE_NONE) return 0;
    }
    *prefix_parts_out = prefix_parts;
    return 1;
}

static int pattern_matches(struct cupid_shell *shell, const char *pattern, const char *text) {
    int rc;
    int fm_flags = 0;
#ifdef FNM_EXTMATCH
    if (shell->opt_extglob) fm_flags |= FNM_EXTMATCH;
#endif
    rc = fnmatch(pattern, text, fm_flags);
    if (rc == 0) return 1;

    if (shell->opt_extglob) {
        const char *q = strstr(pattern, "?(");
        if (q != NULL) {
            const char *close = strchr(q + 2, ')');
            if (close != NULL) {
                size_t prefix_len = (size_t)(q - pattern);
                size_t inner_len = (size_t)(close - (q + 2));
                size_t suffix_len = strlen(close + 1);
                char *pat1 = calloc(prefix_len + suffix_len + 1, 1);
                char *pat2 = calloc(prefix_len + inner_len + suffix_len + 1, 1);
                if (pat1 != NULL && pat2 != NULL) {
                    memcpy(pat1, pattern, prefix_len);
                    memcpy(pat1 + prefix_len, close + 1, suffix_len);
                    memcpy(pat2, pattern, prefix_len);
                    memcpy(pat2 + prefix_len, q + 2, inner_len);
                    memcpy(pat2 + prefix_len + inner_len, close + 1, suffix_len);
                    if (fnmatch(pat1, text, 0) == 0 || fnmatch(pat2, text, 0) == 0) {
                        free(pat1);
                        free(pat2);
                        return 1;
                    }
                }
                free(pat1);
                free(pat2);
            }
        }
    }
    return 0;
}

static int add_split_field(char ***fields, size_t *count, const char *start, size_t len) {
    char *field = calloc(len + 1, 1);
    char **next;
    size_t i;
    if (field == NULL) {
        for (i = 0; i < *count; i++) free((*fields)[i]);
        free(*fields);
        *fields = NULL;
        *count = 0;
        return -1;
    }
    if (len > 0) memcpy(field, start, len);
    next = realloc(*fields, sizeof(*next) * (*count + 1));
    if (next == NULL) {
        free(field);
        for (i = 0; i < *count; i++) free((*fields)[i]);
        free(*fields);
        *fields = NULL;
        *count = 0;
        return -1;
    }
    *fields = next;
    (*fields)[*count] = field;
    (*count)++;
    return 0;
}

static void free_split_fields(char **fields, size_t count) {
    size_t i;
    if (fields == NULL) return;
    for (i = 0; i < count; i++) free(fields[i]);
    free(fields);
}

static int ensure_runtime_field(char ***fields, size_t *count) {
    if (*count != 0) return 0;
    return add_split_field(fields, count, "", 0);
}

static int append_runtime_field_text(char ***fields, size_t *count, const char *text) {
    char *next;
    size_t cur_len;
    size_t add_len;

    if (ensure_runtime_field(fields, count) != 0) return -1;
    if (text == NULL) text = "";
    cur_len = strlen((*fields)[*count - 1]);
    add_len = strlen(text);
    next = realloc((*fields)[*count - 1], cur_len + add_len + 1);
    if (next == NULL) {
        free_split_fields(*fields, *count);
        *fields = NULL;
        *count = 0;
        return -1;
    }
    (*fields)[*count - 1] = next;
    memcpy((*fields)[*count - 1] + cur_len, text, add_len);
    (*fields)[*count - 1][cur_len + add_len] = '\0';
    return 0;
}

static int ifs_contains_char(const char *ifs, char c) {
    return (ifs != NULL && strchr(ifs, c) != NULL) ? 1 : 0;
}

static int ifs_is_ws_char(const char *ifs, char c) {
    return ifs_contains_char(ifs, c) && (c == ' ' || c == '\t' || c == '\n');
}

static int ifs_is_nonws_char(const char *ifs, char c) {
    return ifs_contains_char(ifs, c) && !ifs_is_ws_char(ifs, c);
}

static int text_ends_with_ifs_char(struct cupid_shell *shell, const char *text) {
    const char *ifs = cupid_vars_get(shell, "IFS");
    size_t len;

    if (ifs == NULL) ifs = " \t\n";
    if (text == NULL) return 0;
    len = strlen(text);
    if (len == 0) return 0;
    return ifs_contains_char(ifs, text[len - 1]);
}

static int split_ifs_fields(struct cupid_shell *shell, const char *s,
                            char ***out_fields, size_t *out_count) {
    const char *ifs;
    const char *p = s;
    const char *field_start = s;
    char **fields = NULL;
    size_t count = 0;

    ifs = cupid_vars_get(shell, "IFS");
    if (ifs == NULL) ifs = " \t\n";

    if (ifs[0] == '\0') {
        if (s[0] != '\0') {
            if (add_split_field(&fields, &count, s, strlen(s)) != 0) return -1;
        }
        *out_fields = fields;
        *out_count = count;
        return 0;
    }

    while (*p != '\0') {
        if (!ifs_contains_char(ifs, *p)) {
            p++;
            continue;
        }

        if (ifs_is_nonws_char(ifs, *p)) {
            if (add_split_field(&fields, &count, field_start, (size_t)(p - field_start)) != 0) {
                return -1;
            }
            p++;
            while (*p != '\0' && ifs_is_ws_char(ifs, *p)) p++;
            field_start = p;
            continue;
        }

        if (p > field_start) {
            if (add_split_field(&fields, &count, field_start, (size_t)(p - field_start)) != 0) {
                return -1;
            }
        }
        while (*p != '\0' && ifs_is_ws_char(ifs, *p)) p++;
        field_start = p;
    }

    if (p > field_start) {
        if (add_split_field(&fields, &count, field_start, (size_t)(p - field_start)) != 0) {
            return -1;
        }
    }

    *out_fields = fields;
    *out_count = count;
    return 0;
}

static int is_name_start_char(char c) {
    return isalpha((unsigned char)c) || c == '_';
}

static int is_name_char(char c) {
    return isalnum((unsigned char)c) || c == '_';
}

static int split_assignment_word_ext(const char *word, const char **name, size_t *name_len,
                                     const char **value, int *append_out) {
    const char *eq;
    int append = 0;
    const char *p;
    const char *lhs_end;
    if (word == NULL || !is_name_start_char(word[0])) return 0;
    p = word + 1;
    while (*p != '\0' && is_name_char(*p)) p++;
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
        eq = p;
    } else if (*p == '+' && p[1] == '=') {
        append = 1;
        eq = p + 1;
    } else {
        return 0;
    }
    *name = word;
    *name_len = (size_t)(lhs_end - word);
    *value = eq + 1;
    if (append_out != NULL) *append_out = append;
    return 1;
}

static int apply_assignment_value(struct cupid_shell *shell, const char *name,
                                  const char *value, int local_scope, int append) {
    char *merged = NULL;
    int rc;
    if (shell == NULL || name == NULL || value == NULL) return 1;
    if (!append) {
        return local_scope ? cupid_vars_set_local(shell, name, value)
                           : cupid_vars_set(shell, name, value);
    }

    if (cupid_vars_is_integer(shell, name)) {
        const char *old = cupid_vars_get(shell, name);
        const char *base = (old != NULL && old[0] != '\0') ? old : "0";
        size_t need = strlen(base) + strlen(value) + 4;
        merged = calloc(need, 1);
        if (merged == NULL) return 1;
        snprintf(merged, need, "%s+(%s)", base, value);
    } else {
        const char *old = cupid_vars_get(shell, name);
        size_t old_len = (old != NULL) ? strlen(old) : 0;
        size_t need = old_len + strlen(value) + 1;
        merged = calloc(need, 1);
        if (merged == NULL) return 1;
        if (old_len > 0) memcpy(merged, old, old_len);
        memcpy(merged + old_len, value, strlen(value));
    }
    rc = local_scope ? cupid_vars_set_local(shell, name, merged)
                     : cupid_vars_set(shell, name, merged);
    free(merged);
    return rc;
}

static int setenv_assignment_value(const char *name, const char *value, int append) {
    int rc;
    if (!append) return setenv(name, value, 1);
    {
        const char *old = getenv(name);
        size_t old_len = (old != NULL) ? strlen(old) : 0;
        size_t need = old_len + strlen(value) + 1;
        char *merged = calloc(need, 1);
        if (merged == NULL) return -1;
        if (old_len > 0) memcpy(merged, old, old_len);
        memcpy(merged + old_len, value, strlen(value));
        rc = setenv(name, merged, 1);
        free(merged);
    }
    return rc;
}

static int apply_assignment_to_env_var(const char *name, size_t name_len,
                                       const char *value, int append) {
    char *key = calloc(name_len + 1, 1);
    int rc;
    if (key == NULL) return 1;
    memcpy(key, name, name_len);
    rc = setenv_assignment_value(key, value, append);
    free(key);
    return rc == 0 ? 0 : 1;
}

static int apply_assignment_to_shell_var(struct cupid_shell *shell, const char *name,
                                         size_t name_len, const char *value,
                                         int local_scope, int append) {
    char *key = calloc(name_len + 1, 1);
    int rc;
    if (key == NULL) return 1;
    memcpy(key, name, name_len);
    rc = apply_assignment_value(shell, key, value, local_scope, append);
    free(key);
    return rc == 0 ? 0 : 1;
}

static int split_assignment_for_apply(const char *word, const char **name, size_t *name_len,
                                      const char **value, int *append) {
    return split_assignment_word_ext(word, name, name_len, value, append);
}

static int split_assignment_word(const char *word, const char **name, size_t *name_len, const char **value) {
    return split_assignment_word_ext(word, name, name_len, value, NULL);
}

static char *expand_assignment_tilde_segments(const char *word, struct cupid_shell *shell) {
    const char *name = NULL;
    const char *value = NULL;
    size_t name_len = 0;
    int append = 0;
    size_t lhs_len;
    const char *seg_start;
    const char *p;
    char *out = NULL;
    size_t out_len = 0;
    size_t out_cap = 0;

    if (word == NULL) return NULL;
    if (!split_assignment_word_ext(word, &name, &name_len, &value, &append)) {
        return strdup(word);
    }

    lhs_len = name_len + (append ? 2u : 1u);
    if (append_exec_bytes(&out, &out_len, &out_cap, word, lhs_len) != 0) {
        free(out);
        return NULL;
    }

    seg_start = value;
    p = value;
    while (1) {
        if (*p == ':' || *p == '\0') {
            size_t seg_len = (size_t)(p - seg_start);
            char *segment = calloc(seg_len + 1, 1);
            char *expanded_segment;

            if (segment == NULL) {
                free(out);
                return NULL;
            }
            if (seg_len > 0) memcpy(segment, seg_start, seg_len);
            if (segment[0] == '~') {
                expanded_segment = cupid_expand_tilde(segment, shell);
            } else {
                expanded_segment = strdup(segment);
            }
            free(segment);
            if (expanded_segment == NULL) {
                free(out);
                return NULL;
            }
            if (append_exec_bytes(&out, &out_len, &out_cap,
                                  expanded_segment, strlen(expanded_segment)) != 0) {
                free(expanded_segment);
                free(out);
                return NULL;
            }
            free(expanded_segment);

            if (*p == '\0') break;
            if (append_exec_bytes(&out, &out_len, &out_cap, ":", 1) != 0) {
                free(out);
                return NULL;
            }
            p++;
            seg_start = p;
            continue;
        }
        p++;
    }

    if (out == NULL) out = strdup(word);
    return out;
}

static int apply_assignment_word_shell(struct cupid_shell *shell, const char *word, int local_scope) {
    const char *name = NULL;
    const char *value = NULL;
    size_t name_len = 0;
    int append = 0;
    if (!split_assignment_for_apply(word, &name, &name_len, &value, &append)) return 1;
    return apply_assignment_to_shell_var(shell, name, name_len, value, local_scope, append);
}

static int apply_assignment_word_env(struct cupid_shell *shell, const char *word) {
    const char *name = NULL;
    const char *value = NULL;
    size_t name_len = 0;
    int append = 0;
    if (!split_assignment_for_apply(word, &name, &name_len, &value, &append)) return 1;
    if (!append) return apply_assignment_to_env_var(name, name_len, value, 0);
    {
        char *key = calloc(name_len + 1, 1);
        const char *old_visible;
        char *merged = NULL;
        int rc;
        if (key == NULL) return 1;
        memcpy(key, name, name_len);
        old_visible = getenv(key);
        if ((old_visible == NULL || old_visible[0] == '\0') && shell != NULL) {
            old_visible = cupid_vars_get(shell, key);
        }
        if (shell != NULL && cupid_vars_is_integer(shell, key)) {
            const char *base = (old_visible != NULL && old_visible[0] != '\0') ? old_visible : "0";
            size_t need = strlen(base) + strlen(value) + 4;
            int arith_err = 0;
            long arith_val;
            int n;
            char *expr = calloc(need, 1);
            if (expr == NULL) {
                free(key);
                return 1;
            }
            snprintf(expr, need, "%s+(%s)", base, value);
            arith_val = cupid_arith_eval(shell, expr, &arith_err);
            free(expr);
            if (arith_err) {
                free(key);
                return 1;
            }
            merged = calloc(64, 1);
            if (merged == NULL) {
                free(key);
                return 1;
            }
            n = snprintf(merged, 64, "%ld", arith_val);
            if (n < 0 || n >= 64) {
                free(merged);
                free(key);
                return 1;
            }
        } else {
            size_t old_len = (old_visible != NULL) ? strlen(old_visible) : 0;
            size_t need = old_len + strlen(value) + 1;
            merged = calloc(need, 1);
            if (merged == NULL) {
                free(key);
                return 1;
            }
            if (old_len > 0) memcpy(merged, old_visible, old_len);
            memcpy(merged + old_len, value, strlen(value));
        }
        rc = setenv(key, merged, 1);
        free(merged);
        free(key);
        return rc == 0 ? 0 : 1;
    }
}

static int apply_assignment_word_temp_env(struct cupid_shell *shell, const char *word,
                                          struct temp_env_assignment *slot,
                                          int sync_shell_binding) {
    const char *name = NULL;
    const char *value = NULL;
    size_t name_len = 0;
    int append = 0;
    const char *old_value;
    if (slot == NULL) return 1;
    if (!split_assignment_for_apply(word, &name, &name_len, &value, &append)) return 1;
    slot->name = calloc(name_len + 1, 1);
    if (slot->name == NULL) return 1;
    memcpy(slot->name, name, name_len);
    slot->restore_shell_binding = sync_shell_binding ? 1 : 0;
    if (sync_shell_binding) {
        slot->had_shell_binding = shell_has_var_binding(shell, slot->name);
        if (slot->had_shell_binding) {
            const char *old_shell = cupid_vars_get(shell, slot->name);
            slot->old_shell_value = strdup(old_shell == NULL ? "" : old_shell);
            if (slot->old_shell_value == NULL) return 1;
        }
    }
    old_value = getenv(slot->name);
    if (old_value != NULL) {
        slot->old_value = strdup(old_value);
        if (slot->old_value == NULL) return 1;
        slot->had_old_value = 1;
    }
    if (!append) {
        if (setenv_assignment_value(slot->name, value, 0) != 0) return 1;
        if (sync_shell_binding) {
            return cupid_vars_set(shell, slot->name, value) == 0 ? 0 : 1;
        }
        return 0;
    }
    {
        const char *visible = cupid_vars_get(shell, slot->name);
        char *merged = NULL;
        if (shell != NULL && cupid_vars_is_integer(shell, slot->name)) {
            const char *base = (visible != NULL && visible[0] != '\0') ? visible : "0";
            size_t need = strlen(base) + strlen(value) + 4;
            char *expr = calloc(need, 1);
            int arith_err = 0;
            long arith_val;
            int n;
            if (expr == NULL) return 1;
            snprintf(expr, need, "%s+(%s)", base, value);
            arith_val = cupid_arith_eval(shell, expr, &arith_err);
            free(expr);
            if (arith_err) return 1;
            merged = calloc(64, 1);
            if (merged == NULL) return 1;
            n = snprintf(merged, 64, "%ld", arith_val);
            if (n < 0 || n >= 64) {
                free(merged);
                return 1;
            }
        } else {
            size_t old_len = (visible != NULL) ? strlen(visible) : 0;
            size_t need = old_len + strlen(value) + 1;
            merged = calloc(need, 1);
            if (merged == NULL) return 1;
            if (old_len > 0) memcpy(merged, visible, old_len);
            memcpy(merged + old_len, value, strlen(value));
        }
        if (setenv(slot->name, merged, 1) != 0) {
            free(merged);
            return 1;
        }
        if (sync_shell_binding && cupid_vars_set(shell, slot->name, merged) != 0) {
            free(merged);
            return 1;
        }
        free(merged);
    }
    return 0;
}

static int word_is_assignment_candidate(const struct cupid_word *word) {
    char *source;
    const char *name = NULL;
    const char *value = NULL;
    size_t name_len = 0;
    int is_assignment;

    if (word == NULL) return 0;
    source = cupid_word_source_text(word);
    if (source == NULL) return 0;
    is_assignment = split_assignment_word(source, &name, &name_len, &value);
    free(source);
    return is_assignment;
}

static int text_is_all_digits(const char *text) {
    size_t i;

    if (text == NULL || text[0] == '\0') return 0;
    for (i = 0; text[i] != '\0'; i++) {
        if (!isdigit((unsigned char)text[i])) return 0;
    }
    return 1;
}

static const char *exact_param_visible_value(struct cupid_shell *shell, const char *name, int *is_set_out) {
    const char *value = NULL;

    if (is_set_out != NULL) *is_set_out = 0;
    if (shell == NULL || name == NULL) return NULL;

    if (text_is_all_digits(name)) {
        long idx = strtol(name, NULL, 10);
        if (idx == 0) {
            value = shell->arg0 ? shell->arg0 : "";
            if (is_set_out != NULL) *is_set_out = 1;
            return value;
        }
        if (idx > 0 && (size_t)(idx - 1) < shell->params.count) {
            value = shell->params.args[(size_t)(idx - 1)];
            if (is_set_out != NULL) *is_set_out = 1;
            return value;
        }
        return NULL;
    }

    value = cupid_vars_get(shell, name);
    if (value != NULL && is_set_out != NULL) *is_set_out = 1;
    return value;
}

static int word_has_positional_splice(const struct cupid_word *word) {
    char *source;
    int result;

    if (word == NULL) return 0;
    source = cupid_word_source_text(word);
    if (source == NULL) return 0;
    result = strstr(source, "$@") != NULL ||
             strstr(source, "$*") != NULL ||
             strstr(source, "${@") != NULL ||
             strstr(source, "${*") != NULL;
    free(source);
    return result;
}

static int text_has_positional_splice(const char *text) {
    if (text == NULL) return 0;
    return strstr(text, "$@") != NULL ||
           strstr(text, "$*") != NULL ||
           strstr(text, "${@") != NULL ||
           strstr(text, "${*") != NULL;
}

static size_t command_assignment_prefix_count(const struct runtime_command *cmd) {
    size_t i;
    for (i = 0; i < (size_t)cmd->argc; i++) {
        const char *name = NULL;
        const char *value = NULL;
        size_t name_len = 0;
        if (!split_assignment_word(cmd->argv[i], &name, &name_len, &value)) {
            break;
        }
    }
    return i;
}

static int apply_prefix_assignments(struct cupid_shell *shell, const struct runtime_command *cmd,
                                    size_t count, int local_scope) {
    size_t i;
    for (i = 0; i < count; i++) {
        if (apply_assignment_word_shell(shell, cmd->argv[i], local_scope) != 0) return 1;
    }
    return 0;
}

static int apply_prefix_assignments_env(struct cupid_shell *shell,
                                        const struct runtime_command *cmd, size_t count) {
    size_t i;
    for (i = 0; i < count; i++) {
        if (apply_assignment_word_env(shell, cmd->argv[i]) != 0) return 1;
    }
    return 0;
}

static void temp_env_assignments_restore(struct cupid_shell *shell,
                                         struct temp_env_assignment *items, size_t count) {
    size_t i;
    if (items == NULL) return;
    for (i = 0; i < count; i++) {
        if (items[i].name == NULL) continue;
        if (shell != NULL && items[i].restore_shell_binding) {
            if (items[i].had_shell_binding) {
                (void)cupid_vars_set(shell, items[i].name,
                                     items[i].old_shell_value ? items[i].old_shell_value : "");
            } else {
                (void)cupid_vars_unset_binding(shell, items[i].name);
            }
        }
        if (items[i].had_old_value) {
            (void)setenv(items[i].name, items[i].old_value ? items[i].old_value : "", 1);
        } else {
            (void)unsetenv(items[i].name);
        }
        free(items[i].name);
        free(items[i].old_value);
        free(items[i].old_shell_value);
    }
    free(items);
}

static int apply_prefix_assignments_temp_env(struct cupid_shell *shell, const struct runtime_command *cmd,
                                             size_t count, int sync_shell_binding,
                                             struct temp_env_assignment **out_items,
                                             size_t *out_count) {
    struct temp_env_assignment *items;
    size_t i;

    if (out_items == NULL || out_count == NULL) return 1;
    *out_items = NULL;
    *out_count = 0;
    if (count == 0) return 0;

    items = calloc(count, sizeof(*items));
    if (items == NULL) return 1;

    for (i = 0; i < count; i++) {
        if (apply_assignment_word_temp_env(shell, cmd->argv[i], &items[i],
                                           sync_shell_binding) != 0) {
            temp_env_assignments_restore(shell, items, count);
            return 1;
        }
    }

    *out_items = items;
    *out_count = count;
    return 0;
}

static int is_posix_special_builtin_name(const char *name) {
    return strcmp(name, ".") == 0 ||
           strcmp(name, ":") == 0 ||
           strcmp(name, "break") == 0 ||
           strcmp(name, "continue") == 0 ||
           strcmp(name, "eval") == 0 ||
           strcmp(name, "exec") == 0 ||
           strcmp(name, "exit") == 0 ||
           strcmp(name, "export") == 0 ||
           strcmp(name, "readonly") == 0 ||
           strcmp(name, "return") == 0 ||
           strcmp(name, "set") == 0 ||
           strcmp(name, "shift") == 0 ||
           strcmp(name, "trap") == 0 ||
           strcmp(name, "unset") == 0;
}

static int builtin_prefix_assignments_use_shell_scope(const char *name) {
    if (name == NULL) return 0;
    return strcmp(name, "eval") == 0 ||
           strcmp(name, ".") == 0 ||
           strcmp(name, "source") == 0;
}

static char *expand_array_item_fragment(const char *text, struct cupid_shell *shell) {
    struct cupid_tokens toks = {0};
    char *result = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;
    int saw_word = 0;

    if (text == NULL) return strdup("");
    if (cupid_lex(text, &toks) != 0) {
        return cupid_expand_text(text, CUPID_QUOTE_NONE, shell);
    }

    for (i = 0; i < toks.count; i++) {
        char *expanded;
        if (toks.items[i].kind != TOK_WORD) continue;
        if (saw_word && append_exec_bytes(&result, &len, &cap, " ", 1) != 0) {
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        saw_word = 1;
        expanded = cupid_expand_word(&toks.items[i].word, shell);
        if (expanded == NULL) {
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        if (append_exec_bytes(&result, &len, &cap, expanded, strlen(expanded)) != 0) {
            free(expanded);
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
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

static int parse_array_literal(struct cupid_shell *shell, const char *rhs, char ***out_items, size_t *out_count) {
    char *inner;
    char **items = NULL;
    size_t count = 0;
    size_t len;
    struct cupid_tokens toks = {0};
    size_t i;
    if (rhs == NULL) return -1;
    len = strlen(rhs);
    if (len < 2 || rhs[0] != '(' || rhs[len - 1] != ')') return -1;
    inner = calloc(len - 1, 1);
    if (inner == NULL) return -1;
    memcpy(inner, rhs + 1, len - 2);
    if (cupid_lex(inner, &toks) != 0) {
        free(inner);
        return -1;
    }
    free(inner);

    for (i = 0; i < toks.count; i++) {
        char *expanded;
        char *copy;
        char **next;
        if (toks.items[i].kind != TOK_WORD) continue;
        {
            char *source = cupid_word_source_text(&toks.items[i].word);
            char *key_src = NULL;
            char *value_src = NULL;
            int split_rc = (source != NULL) ? split_array_item_source(source, &key_src, &value_src) : 0;
            if (split_rc < 0) {
                free(source);
                size_t j;
                for (j = 0; j < count; j++) free(items[j]);
                free(items);
                cupid_tokens_free(&toks);
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
                    size_t j;
                    for (j = 0; j < count; j++) free(items[j]);
                    free(items);
                    cupid_tokens_free(&toks);
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
                    size_t j;
                    for (j = 0; j < count; j++) free(items[j]);
                    free(items);
                    cupid_tokens_free(&toks);
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
        }
        if (expanded == NULL) {
            size_t j;
            for (j = 0; j < count; j++) free(items[j]);
            free(items);
            cupid_tokens_free(&toks);
            return -1;
        }
        copy = strdup(expanded);
        free(expanded);
        if (copy == NULL) {
            size_t j;
            for (j = 0; j < count; j++) free(items[j]);
            free(items);
            cupid_tokens_free(&toks);
            return -1;
        }
        next = realloc(items, sizeof(*next) * (count + 1));
        if (next == NULL) {
            size_t j;
            free(copy);
            for (j = 0; j < count; j++) free(items[j]);
            free(items);
            cupid_tokens_free(&toks);
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

static char *assignment_word_source_rhs(const struct cupid_pipeline_item *item, size_t arg_index) {
    const struct cupid_node *node;
    const struct cupid_word *word;
    char *source;
    char *eq;

    if (item == NULL || item->pipeline.count != 1) return NULL;
    node = &item->pipeline.commands[0];
    if (node->kind != NODE_SIMPLE_CMD || arg_index >= node->simple_cmd.argc) return NULL;
    word = &node->simple_cmd.argv[arg_index];
    source = cupid_word_source_text(word);
    if (source == NULL) return NULL;
    eq = strchr(source, '=');
    if (eq == NULL) {
        free(source);
        return NULL;
    }
    if (eq[1] != '(') {
        free(source);
        return NULL;
    }
    {
        char *rhs = strdup(eq + 1);
        free(source);
        return rhs;
    }
}

static int parse_subscripted_name(const char *lhs, char **name_out, char **subscript_out,
                                  int *numeric_out, size_t *index_out) {
    const char *lb = strchr(lhs, '[');
    const char *rb;
    char *name;
    char *subscript;
    if (lb == NULL) return 0;
    rb = strchr(lb, ']');
    if (rb == NULL || rb[1] != '\0') return -1;
    if (lb == lhs) return -1;
    name = calloc((size_t)(lb - lhs) + 1, 1);
    subscript = calloc((size_t)(rb - lb), 1);
    if (name == NULL || subscript == NULL) {
        free(name);
        free(subscript);
        return -1;
    }
    memcpy(name, lhs, (size_t)(lb - lhs));
    memcpy(subscript, lb + 1, (size_t)(rb - lb - 1));
    {
        size_t i;
        for (i = 0; name[i] != '\0'; i++) {
            if (i == 0) {
                if (!is_name_start_char(name[i])) { free(name); free(subscript); return -1; }
            } else if (!is_name_char(name[i])) {
                free(name);
                free(subscript);
                return -1;
            }
        }
    }
    if (subscript[0] == '\0') {
        free(name);
        free(subscript);
        return -1;
    }
    if (numeric_out != NULL) *numeric_out = 0;
    if (subscript_out != NULL) *subscript_out = subscript;
    if (name_out != NULL) *name_out = name;
    if (index_out != NULL) *index_out = 0;
    {
        char *end;
        unsigned long idx = strtoul(subscript, &end, 10);
        if (*end == '\0') {
            if (numeric_out != NULL) *numeric_out = 1;
            if (index_out != NULL) *index_out = (size_t)idx;
        }
    }
    return 1;
}

static int parse_array_item_assignment_text(const char *item, char **key_out, char **value_out,
                                            int *append_out) {
    const char *p;
    int mode = 0;
    int append = 0;
    size_t key_len;

    if (key_out != NULL) *key_out = NULL;
    if (value_out != NULL) *value_out = NULL;
    if (append_out != NULL) *append_out = 0;
    if (item == NULL || item[0] != '[' || key_out == NULL || value_out == NULL) return 0;

    p = item + 1;
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
        if (*p == ']') break;
        p++;
    }
    if (*p != ']') return 0;
    if (p[1] == '=') {
        append = 0;
    } else if (p[1] == '+' && p[2] == '=') {
        append = 1;
    } else {
        return 0;
    }

    key_len = (size_t)(p - (item + 1));
    *key_out = calloc(key_len + 1, 1);
    if (*key_out == NULL) return -1;
    memcpy(*key_out, item + 1, key_len);
    *value_out = strdup(append ? p + 3 : p + 2);
    if (*value_out == NULL) {
        free(*key_out);
        *key_out = NULL;
        return -1;
    }
    if (append_out != NULL) *append_out = append;
    return 1;
}

static size_t next_array_append_index(struct cupid_shell *shell, const char *name) {
    size_t count = cupid_array_member_count(shell, name);
    size_t i;
    size_t best = 0;
    int have_best = 0;
    for (i = 0; i < count; i++) {
        const char *key = cupid_array_member_key(shell, name, i);
        size_t j;
        size_t idx = 0;
        if (key == NULL || key[0] == '\0') continue;
        for (j = 0; key[j] != '\0'; j++) {
            if (!isdigit((unsigned char)key[j])) {
                idx = 0;
                break;
            }
            idx = idx * 10u + (size_t)(key[j] - '0');
        }
        if (key[j] != '\0') continue;
        if (!have_best || idx > best) {
            best = idx;
            have_best = 1;
        }
    }
    return have_best ? (best + 1u) : 0u;
}

static int build_append_value_for_name(struct cupid_shell *shell, const char *name,
                                       const char *old_value, const char *rhs,
                                       char **out_value) {
    char *merged;
    if (out_value == NULL || shell == NULL || name == NULL || rhs == NULL) return -1;
    *out_value = NULL;
    if (cupid_vars_is_integer(shell, name)) {
        const char *base = (old_value != NULL && old_value[0] != '\0') ? old_value : "0";
        size_t need = strlen(base) + strlen(rhs) + 4;
        merged = calloc(need, 1);
        if (merged == NULL) return -1;
        snprintf(merged, need, "%s+(%s)", base, rhs);
    } else {
        size_t old_len = (old_value != NULL) ? strlen(old_value) : 0;
        size_t need = old_len + strlen(rhs) + 1;
        merged = calloc(need, 1);
        if (merged == NULL) return -1;
        if (old_len > 0) memcpy(merged, old_value, old_len);
        memcpy(merged + old_len, rhs, strlen(rhs));
    }
    *out_value = merged;
    return 0;
}

static int assign_array_member_with_op(struct cupid_shell *shell, const char *name,
                                       const char *key, const char *value, int append) {
    if (!append) return cupid_array_set_key(shell, name, key, value);
    {
        const char *old = cupid_array_get_key(shell, name, key);
        char *merged = NULL;
        int rc;
        if (build_append_value_for_name(shell, name, old, value, &merged) != 0) return -1;
        rc = cupid_array_set_key(shell, name, key, merged);
        free(merged);
        return rc;
    }
}

static int apply_array_literal_assignment(struct cupid_shell *shell, const char *name,
                                          const char *rhs, int append_rhs) {
    char **items = NULL;
    size_t count = 0;
    size_t i;
    size_t auto_index;
    int was_assoc;

    if (shell == NULL || name == NULL || rhs == NULL) return -1;
    if (parse_array_literal(shell, rhs, &items, &count) != 0) return -1;

    was_assoc = cupid_array_is_associative(shell, name);
    if (!append_rhs) {
        (void)cupid_array_unset(shell, name);
        if (was_assoc && cupid_array_set_associative(shell, name, 1) != 0) {
            for (i = 0; i < count; i++) free(items[i]);
            free(items);
            return -1;
        }
        if (was_assoc) {
            int direct_set_list = 1;
            for (i = 0; i < count; i++) {
                char *item_key = NULL;
                char *item_value = NULL;
                int item_append = 0;
                int parsed = parse_array_item_assignment_text(items[i], &item_key, &item_value, &item_append);
                free(item_key);
                free(item_value);
                if (parsed <= 0 || item_append) {
                    direct_set_list = 0;
                    break;
                }
            }
            if (direct_set_list) {
                int rc = cupid_array_set_list(shell, name, items, count);
                for (i = 0; i < count; i++) free(items[i]);
                free(items);
                return rc;
            }
        }
    }

    auto_index = append_rhs ? next_array_append_index(shell, name) : 0u;
    for (i = 0; i < count; i++) {
        char *item_key = NULL;
        char *item_value = NULL;
        int item_append = 0;
        int parsed = parse_array_item_assignment_text(items[i], &item_key, &item_value, &item_append);
        int rc = 0;

        if (parsed < 0) {
            rc = -1;
        } else if (parsed > 0) {
            rc = assign_array_member_with_op(shell, name, item_key, item_value, item_append ? 1 : 0);
            if (rc == 0) {
                size_t j;
                size_t idx = 0;
                int numeric = (item_key != NULL && item_key[0] != '\0') ? 1 : 0;
                for (j = 0; numeric && item_key[j] != '\0'; j++) {
                    if (!isdigit((unsigned char)item_key[j])) {
                        numeric = 0;
                        break;
                    }
                    idx = idx * 10u + (size_t)(item_key[j] - '0');
                }
                if (numeric && idx >= auto_index) auto_index = idx + 1u;
            }
        } else {
            char keybuf[32];
            int n = snprintf(keybuf, sizeof(keybuf), "%zu", auto_index++);
            if (n < 0 || n >= (int)sizeof(keybuf)) {
                rc = -1;
            } else {
                rc = cupid_array_set_key(shell, name, keybuf, items[i]);
            }
        }
        free(item_key);
        free(item_value);
        if (rc != 0) {
            size_t j;
            for (j = 0; j < count; j++) free(items[j]);
            free(items);
            return -1;
        }
    }

    for (i = 0; i < count; i++) free(items[i]);
    free(items);
    return 0;
}

static int looks_like_array_assignment(const char *s) {
    const char *eq;
    const char *p;
    size_t len;
    if (s == NULL) return 0;
    eq = strchr(s, '=');
    if (eq == NULL || eq == s) return 0;
    if (!is_name_start_char(s[0])) return 0;
    for (p = s + 1; p < eq; p++) {
        if (!is_name_char(*p)) return 0;
    }
    if (eq[1] != '(') return 0;
    len = strlen(eq + 1);
    return len >= 2 && eq[1 + len - 1] == ')';
}

static int try_runtime_alias_expansion(struct cupid_shell *shell, int argc, char **argv, int *status_out) {
    const char *alias_val;
    size_t total = 0;
    int i;
    char *line;
    char *rp;
    int status;
    if (status_out != NULL) *status_out = 0;
    if (shell == NULL || argc <= 0 || argv == NULL || argv[0] == NULL) return 0;
    if (!shell->opt_expand_aliases || shell->alias_expansion_depth >= 32) return 0;
    alias_val = cupid_alias_get(shell, argv[0]);
    if (alias_val == NULL) return 0;

    total = strlen(alias_val);
    for (i = 1; i < argc; i++) total += 1 + strlen(argv[i]);
    line = calloc(total + 1, 1);
    if (line == NULL) return 0;
    memcpy(line, alias_val, strlen(alias_val));
    rp = line + strlen(alias_val);
    for (i = 1; i < argc; i++) {
        *rp++ = ' ';
        memcpy(rp, argv[i], strlen(argv[i]));
        rp += strlen(argv[i]);
    }

    shell->alias_expansion_depth++;
    status = cupid_shell_eval_line(shell, line, 1);
    shell->alias_expansion_depth--;
    free(line);
    if (status_out != NULL) *status_out = status;
    return 1;
}

static int add_field_with_glob(struct runtime_command *cmd, const char *field, int allow_glob,
                               struct cupid_shell *shell) {
    char *normalized;

    normalized = strdup(field == NULL ? "" : field);
    if (normalized == NULL) return -1;
    cupid_restore_escaped_ifs_placeholders(normalized);

    if (allow_glob && !shell->opt_noglob && has_glob_meta(shell, normalized)) {
        glob_t gl;
        size_t i;
        int grc;
        int gflags = shell->opt_nullglob ? 0 : GLOB_NOCHECK;
        memset(&gl, 0, sizeof(gl));
        grc = glob(normalized, gflags, NULL, &gl);
        if (grc != 0) {
            if (shell->opt_nullglob && grc == GLOB_NOMATCH) {
                free(normalized);
                globfree(&gl);
                return 0;
            }
            free(normalized);
            globfree(&gl);
            return -1;
        }
        free(normalized);
        for (i = 0; i < gl.gl_pathc; i++) {
            char *copy = strdup(gl.gl_pathv[i]);
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                globfree(&gl);
                return -1;
            }
        }
        globfree(&gl);
        return 0;
    }
    if (command_add_arg(cmd, normalized) != 0) {
        free(normalized);
        return -1;
    }
    return 0;
}

static int append_exec_bytes(char **buf, size_t *len, size_t *cap, const char *data, size_t data_len) {
    char *next;
    size_t needed;
    if (data_len == 0) return 0;
    needed = *len + data_len + 1;
    if (needed > *cap) {
        size_t next_cap = (*cap == 0) ? 32 : *cap;
        while (next_cap < needed) next_cap *= 2;
        next = realloc(*buf, next_cap);
        if (next == NULL) return -1;
        *buf = next;
        *cap = next_cap;
    }
    memcpy(*buf + *len, data, data_len);
    *len += data_len;
    (*buf)[*len] = '\0';
    return 0;
}

static int scan_param_close(const char *p, enum cupid_quote outer_quote, const char **end_out) {
    int nested = 0;
    int mode = 0;
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
        if (outer_quote != CUPID_QUOTE_DOUBLE && *p == '\'') {
            mode = 1;
            p++;
            continue;
        }
        if (*p == '"') {
            mode = 2;
            p++;
            continue;
        }
        if (p[0] == '$' && p[1] == '{') {
            nested++;
            p += 2;
            continue;
        }
        if (*p == '}') {
            if (nested == 0) {
                *end_out = p;
                return 0;
            }
            nested--;
        }
        p++;
    }
    return -1;
}

static int parse_exact_plus_param_word(const struct cupid_word *word, char **name_out,
                                       int *colon_mode_out, char *op_out, char **fragment_out,
                                       enum cupid_quote *outer_quote_out) {
    char *source = NULL;
    char *text = NULL;
    const char *p;
    const char *name_start;
    const char *name_end;
    const char *close;
    char *name;
    char *fragment;
    size_t name_len;
    size_t frag_len;
    int colon_mode = 0;
    enum cupid_quote outer_quote;

    if (word == NULL || name_out == NULL || colon_mode_out == NULL || op_out == NULL ||
        fragment_out == NULL || outer_quote_out == NULL) {
        return 0;
    }
    if (word->part_count == 1 &&
        (word->parts[0].quote == CUPID_QUOTE_NONE || word->parts[0].quote == CUPID_QUOTE_DOUBLE)) {
        outer_quote = word->parts[0].quote;
        text = word->parts[0].text;
    } else {
        source = cupid_word_source_text(word);
        if (source == NULL) return -1;
        outer_quote = CUPID_QUOTE_NONE;
        text = source;
        if (source[0] == '"' && source[1] != '\0') {
            size_t slen = strlen(source);
            if (slen < 2 || source[slen - 1] != '"') {
                free(source);
                return 0;
            }
            source[slen - 1] = '\0';
            text = source + 1;
            outer_quote = CUPID_QUOTE_DOUBLE;
        }
    }
    if (text[0] != '$' || text[1] != '{') {
        free(source);
        return 0;
    }

    p = text + 2;
    if (!(isalpha((unsigned char)*p) || *p == '_' || isdigit((unsigned char)*p))) {
        free(source);
        return 0;
    }
    name_start = p;
    if (isdigit((unsigned char)*p)) {
        while (isdigit((unsigned char)*p)) p++;
    } else {
        while (isalnum((unsigned char)*p) || *p == '_') p++;
    }
    name_end = p;
    if (*p == ':') {
        colon_mode = 1;
        p++;
    }
    if (*p != '+' && *p != '-' && *p != '=') {
        free(source);
        return 0;
    }
    *op_out = *p;
    p++;
    if (scan_param_close(p, outer_quote, &close) != 0) {
        free(source);
        return 0;
    }
    if (close[1] != '\0') {
        free(source);
        return 0;
    }

    name_len = (size_t)(name_end - name_start);
    frag_len = (size_t)(close - p);
    name = calloc(name_len + 1, 1);
    fragment = calloc(frag_len + 1, 1);
    if (name == NULL || fragment == NULL) {
        free(name);
        free(fragment);
        free(source);
        return -1;
    }
    memcpy(name, name_start, name_len);
    if (frag_len > 0) memcpy(fragment, p, frag_len);
    free(source);

    *name_out = name;
    *colon_mode_out = colon_mode;
    *fragment_out = fragment;
    *outer_quote_out = outer_quote;
    return 1;
}

static int add_expanded_fragment_words(struct runtime_command *cmd, const char *fragment,
                                       struct cupid_shell *shell) {
    struct cupid_tokens toks = {0};
    size_t i;

    if (fragment == NULL || fragment[0] == '\0') return 0;
    if (cupid_lex(fragment, &toks) != 0) return -1;
    for (i = 0; i < toks.count; i++) {
        char *expanded;
        if (toks.items[i].kind != TOK_WORD) continue;
        if (toks.items[i].word.had_quotes && !word_has_positional_splice(&toks.items[i].word)) {
            expanded = cupid_expand_word(&toks.items[i].word, shell);
            if (expanded == NULL) {
                cupid_tokens_free(&toks);
                return -1;
            }
            if (command_add_arg(cmd, expanded) != 0) {
                free(expanded);
                cupid_tokens_free(&toks);
                return -1;
            }
            continue;
        }
        if (add_expanded_word(cmd, &toks.items[i].word, shell, 0) != 0) {
            cupid_tokens_free(&toks);
            return -1;
        }
    }
    cupid_tokens_free(&toks);
    return 0;
}

static int shell_has_var_binding(const struct cupid_shell *shell, const char *name) {
    size_t i;

    if (shell == NULL || name == NULL) return 0;
    for (i = 0; i < shell->vars.count; i++) {
        if (strcmp(shell->vars.entries[i].name, name) == 0) return 1;
    }
    return 0;
}

static void restore_expand_assignments(struct cupid_shell *shell,
                                       struct expand_assignment_restore *items,
                                       size_t count) {
    size_t i;

    if (shell == NULL || items == NULL) return;
    for (i = 0; i < count; i++) {
        if (items[i].name == NULL) continue;
        if (items[i].had_shell_binding) {
            (void)cupid_vars_set(shell, items[i].name,
                                 items[i].old_shell_value ? items[i].old_shell_value : "");
        } else {
            (void)cupid_vars_unset_binding(shell, items[i].name);
        }
        if (items[i].had_env_value) {
            (void)setenv(items[i].name, items[i].old_env_value ? items[i].old_env_value : "", 1);
        } else {
            (void)unsetenv(items[i].name);
        }
        free(items[i].name);
        free(items[i].old_shell_value);
        free(items[i].old_env_value);
    }
    free(items);
}

static int apply_expand_assignment_restore(struct cupid_shell *shell,
                                           struct expand_assignment_restore **items_out,
                                           size_t *count_out,
                                           const char *name,
                                           const char *value) {
    struct expand_assignment_restore *items;
    size_t count;
    size_t i;
    struct expand_assignment_restore *next;
    const char *env_old;

    if (shell == NULL || items_out == NULL || count_out == NULL || name == NULL || value == NULL) {
        return -1;
    }

    items = *items_out;
    count = *count_out;
    for (i = 0; i < count; i++) {
        if (strcmp(items[i].name, name) == 0) break;
    }
    if (i == count) {
        next = realloc(items, sizeof(*next) * (count + 1));
        if (next == NULL) return -1;
        items = next;
        memset(&items[count], 0, sizeof(items[count]));
        items[count].name = strdup(name);
        if (items[count].name == NULL) {
            *items_out = items;
            *count_out = count;
            return -1;
        }
        items[count].had_shell_binding = shell_has_var_binding(shell, name);
        if (items[count].had_shell_binding) {
            const char *old_shell = cupid_vars_get(shell, name);
            items[count].old_shell_value = strdup(old_shell == NULL ? "" : old_shell);
            if (items[count].old_shell_value == NULL) {
                free(items[count].name);
                items[count].name = NULL;
                *items_out = items;
                *count_out = count;
                return -1;
            }
        }
        env_old = getenv(name);
        if (env_old != NULL) {
            items[count].old_env_value = strdup(env_old);
            if (items[count].old_env_value == NULL) {
                free(items[count].name);
                free(items[count].old_shell_value);
                items[count].name = NULL;
                items[count].old_shell_value = NULL;
                *items_out = items;
                *count_out = count;
                return -1;
            }
            items[count].had_env_value = 1;
        }
        count++;
        *items_out = items;
        *count_out = count;
    }

    if (cupid_vars_set(shell, name, value) != 0) return -1;
    if (setenv(name, value, 1) != 0) return -1;
    return 0;
}

static int fragment_is_single_quoted_word(const char *fragment) {
    struct cupid_tokens toks = {0};
    int result = 0;

    if (fragment == NULL) return 0;
    if (cupid_lex(fragment, &toks) != 0) return 0;
    if (toks.count == 1 &&
        toks.items[0].kind == TOK_WORD &&
        toks.items[0].word.had_quotes) {
        result = 1;
    }
    cupid_tokens_free(&toks);
    return result;
}

static char *expand_fragment_outer_double(const char *fragment, struct cupid_shell *shell) {
    struct cupid_tokens toks = {0};
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i, j;
    int saw_word = 0;

    if (fragment == NULL) return strdup("");
    if (cupid_lex(fragment, &toks) != 0) return NULL;

    for (i = 0; i < toks.count; i++) {
        const struct cupid_word *w;
        if (toks.items[i].kind != TOK_WORD) continue;
        w = &toks.items[i].word;
        if (saw_word) {
            if (append_exec_bytes(&out, &len, &cap, " ", 1) != 0) {
                free(out);
                cupid_tokens_free(&toks);
                return NULL;
            }
        }
        saw_word = 1;
        for (j = 0; j < w->part_count; j++) {
            char *expanded;
            if (w->parts[j].quote == CUPID_QUOTE_SINGLE) {
                expanded = cupid_expand_text(w->parts[j].text, CUPID_QUOTE_NONE, shell);
                if (expanded == NULL ||
                    append_exec_bytes(&out, &len, &cap, "'", 1) != 0 ||
                    append_exec_bytes(&out, &len, &cap, expanded, strlen(expanded)) != 0 ||
                    append_exec_bytes(&out, &len, &cap, "'", 1) != 0) {
                    free(expanded);
                    free(out);
                    cupid_tokens_free(&toks);
                    return NULL;
                }
                free(expanded);
                continue;
            }
            expanded = cupid_expand_text(w->parts[j].text, w->parts[j].quote, shell);
            if (expanded == NULL ||
                append_exec_bytes(&out, &len, &cap, expanded, strlen(expanded)) != 0) {
                free(expanded);
                free(out);
                cupid_tokens_free(&toks);
                return NULL;
            }
            free(expanded);
        }
    }

    cupid_tokens_free(&toks);
    if (out == NULL) out = strdup("");
    return out;
}

static int expand_outer_double_fragment_word(struct runtime_command *cmd, const char *fragment,
                                             struct cupid_shell *shell) {
    struct cupid_word_part part = {0};
    struct cupid_word synthetic = {0};
    char *arg;
    int synthetic_rc;

    part.text = (char *)(fragment == NULL ? "" : fragment);
    part.quote = CUPID_QUOTE_DOUBLE;
    synthetic.parts = &part;
    synthetic.part_count = 1;
    synthetic.had_quotes = true;
    synthetic.had_escaped_brace = false;

    synthetic_rc = expand_quoted_positional_parts_word(cmd, &synthetic, shell);
    if (synthetic_rc != 0) return synthetic_rc;
    synthetic_rc = expand_general_quoted_positional_splices_word(cmd, &synthetic, shell);
    if (synthetic_rc != 0) return synthetic_rc;
    synthetic_rc = expand_embedded_quoted_positional_star_at_word(cmd, &synthetic, shell);
    if (synthetic_rc != 0) return synthetic_rc;
    synthetic_rc = expand_multipart_quoted_positional_star_at_word(cmd, &synthetic, shell);
    if (synthetic_rc != 0) return synthetic_rc;

    arg = expand_fragment_outer_double(fragment, shell);
    if (arg == NULL) return -1;
    if (command_add_arg(cmd, arg) != 0) {
        free(arg);
        return -1;
    }
    return 1;
}

static int expand_exact_plus_param_word(struct runtime_command *cmd, const struct cupid_word *word,
                                        struct cupid_shell *shell) {
    char *name = NULL;
    char *fragment = NULL;
    enum cupid_quote outer_quote = CUPID_QUOTE_NONE;
    int colon_mode = 0;
    char op = '\0';
    int parsed;
    const char *cur;
    int is_set;
    int is_non_null;
    int use_existing;
    parsed = parse_exact_plus_param_word(word, &name, &colon_mode, &op, &fragment, &outer_quote);
    if (parsed <= 0) return parsed;

    cur = exact_param_visible_value(shell, name, &is_set);
    is_non_null = is_set && cur[0] != '\0';
    if (op == '+') use_existing = colon_mode ? is_non_null : is_set;
    else use_existing = !(colon_mode ? is_non_null : is_set);
    if (op == '+' && !use_existing) {
        free(name);
        free(fragment);
        return 0;
    }
    if ((op == '-' || op == '=') && !use_existing) {
        free(name);
        free(fragment);
        return 0;
    }
    if ((op == '-' || op == '=') && !text_has_positional_splice(fragment)) {
        free(name);
        free(fragment);
        return 0;
    }
    if (op == '=') {
        char *expanded = cupid_expand_text(fragment, CUPID_QUOTE_NONE, shell);
        const char *stored;
        char **fields = NULL;
        size_t fc = 0;
        size_t fi;
        if (text_is_all_digits(name)) {
            free(name);
            free(fragment);
            return 0;
        }
        if (expanded == NULL) {
            free(name);
            free(fragment);
            return -1;
        }
        cupid_restore_escaped_ifs_placeholders(expanded);
        if (cupid_vars_set(shell, name, expanded) != 0) {
            free(expanded);
            free(name);
            free(fragment);
            return -1;
        }
        stored = cupid_vars_get(shell, name);
        if (stored != NULL) (void)setenv(name, stored, 1);
        free(expanded);

        if (outer_quote == CUPID_QUOTE_DOUBLE) {
            char *copy = strdup(stored == NULL ? "" : stored);
            free(name);
            free(fragment);
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                return -1;
            }
            return 1;
        }

        if (split_ifs_fields(shell, stored == NULL ? "" : stored, &fields, &fc) != 0) {
            free(name);
            free(fragment);
            return -1;
        }
        free(name);
        free(fragment);
        for (fi = 0; fi < fc; fi++) {
            if (add_field_with_glob(cmd, fields[fi], 1, shell) != 0) {
                size_t k;
                for (k = fi; k < fc; k++) free(fields[k]);
                free(fields);
                return -1;
            }
            free(fields[fi]);
        }
        free(fields);
        return 1;
    }

    if (op == '+' && outer_quote == CUPID_QUOTE_DOUBLE) {
        int rc = expand_outer_double_fragment_word(cmd, fragment, shell);
        free(name);
        free(fragment);
        return rc < 0 ? -1 : 1;
    }

    if (op == '+' &&
        text_has_positional_splice(fragment) &&
        fragment[0] == '"' && fragment[1] != '\0' &&
        fragment_is_single_quoted_word(fragment)) {
        size_t flen = strlen(fragment);
        if (flen >= 2 && fragment[flen - 1] == '"') {
            struct cupid_word_part part = {0};
            struct cupid_word synthetic = {0};
            char *inner = calloc(flen - 1, 1);
            char *arg;
            int synthetic_rc;

            if (inner == NULL) {
                free(name);
                free(fragment);
                return -1;
            }
            memcpy(inner, fragment + 1, flen - 2);
            part.text = inner;
            part.quote = CUPID_QUOTE_DOUBLE;
            synthetic.parts = &part;
            synthetic.part_count = 1;
            synthetic.had_quotes = true;
            synthetic.had_escaped_brace = false;

            synthetic_rc = expand_quoted_positional_parts_word(cmd, &synthetic, shell);
            if (synthetic_rc < 0) {
                free(name);
                free(inner);
                free(fragment);
                return -1;
            }
            if (synthetic_rc > 0) {
                free(name);
                free(inner);
                free(fragment);
                return 1;
            }
            synthetic_rc = expand_general_quoted_positional_splices_word(cmd, &synthetic, shell);
            if (synthetic_rc < 0) {
                free(name);
                free(inner);
                free(fragment);
                return -1;
            }
            if (synthetic_rc > 0) {
                free(name);
                free(inner);
                free(fragment);
                return 1;
            }
            synthetic_rc = expand_embedded_quoted_positional_star_at_word(cmd, &synthetic, shell);
            if (synthetic_rc < 0) {
                free(name);
                free(inner);
                free(fragment);
                return -1;
            }
            if (synthetic_rc > 0) {
                free(name);
                free(inner);
                free(fragment);
                return 1;
            }
            synthetic_rc = expand_multipart_quoted_positional_star_at_word(cmd, &synthetic, shell);
            if (synthetic_rc < 0) {
                free(name);
                free(inner);
                free(fragment);
                return -1;
            }
            if (synthetic_rc > 0) {
                free(name);
                free(inner);
                free(fragment);
                return 1;
            }

            arg = cupid_expand_text(inner, CUPID_QUOTE_DOUBLE, shell);
            free(inner);
            free(name);
            free(fragment);
            if (arg == NULL) return -1;
            if (command_add_arg(cmd, arg) != 0) {
                free(arg);
                return -1;
            }
            return 1;
        }
    }

    if (outer_quote == CUPID_QUOTE_DOUBLE && text_has_positional_splice(fragment)) {
        int rc = expand_outer_double_fragment_word(cmd, fragment, shell);
        free(name);
        free(fragment);
        return rc < 0 ? -1 : 1;
    }

    parsed = add_expanded_fragment_words(cmd, fragment, shell);
    free(name);
    free(fragment);
    if (parsed < 0) return -1;
    return 1;
}

static int word_is_subshell_group(const struct cupid_word *word) {
    char *literal;
    char *script = NULL;
    int kind;

    literal = cupid_word_literal(word);
    if (literal == NULL) return -1;
    kind = parse_subshell_script(literal, &script);
    free(literal);
    free(script);
    return kind > 0 ? 1 : 0;
}

static int is_proc_subst(const struct cupid_word *word) {
    if (word->part_count != 1 || word->had_quotes) return 0;
    if (word->parts[0].quote != CUPID_QUOTE_NONE) return 0;
    if (word->parts[0].text[0] == '<' && word->parts[0].text[1] == '(') return 1;
    if (word->parts[0].text[0] == '>' && word->parts[0].text[1] == '(') return 2;
    return 0;
}

static int parse_long_token(const char *s, size_t len, long *out) {
    const char *start = s;
    const char *end = s + len;
    char *tail = NULL;
    long v;

    while (start < end && isspace((unsigned char)*start)) start++;
    while (end > start && isspace((unsigned char)end[-1])) end--;
    if (start == end) return -1;
    v = strtol(start, &tail, 10);
    if (tail == start) return -1;
    while (tail < end && isspace((unsigned char)*tail)) tail++;
    if (tail != end) return -1;
    *out = v;
    return 0;
}

static int parse_positional_slice_spec(const char *spec, long *offset_out,
                                       int *has_length_out, long *length_out) {
    const char *colon;
    size_t spec_len;
    if (spec == NULL || offset_out == NULL || has_length_out == NULL || length_out == NULL) return -1;
    spec_len = strlen(spec);
    colon = strchr(spec, ':');
    if (colon == NULL) {
        if (parse_long_token(spec, spec_len, offset_out) != 0) return -1;
        *has_length_out = 0;
        *length_out = 0;
        return 0;
    }
    if (parse_long_token(spec, (size_t)(colon - spec), offset_out) != 0) return -1;
    *has_length_out = 1;
    if (parse_long_token(colon + 1, spec_len - (size_t)(colon + 1 - spec), length_out) != 0) {
        const char *ls = colon + 1;
        const char *le = spec + spec_len;
        while (ls < le && isspace((unsigned char)*ls)) ls++;
        while (le > ls && isspace((unsigned char)le[-1])) le--;
        if (ls != le) return -1;
        *length_out = 0;
    }
    return 0;
}

static int parse_exact_positional_star_at_word(const struct cupid_word *word, char *sigil_out,
                                               int *has_slice_out, long *offset_out,
                                               int *has_length_out, long *length_out) {
    char *source = NULL;
    const char *text;
    const char *p;
    const char *slice_start;
    const char *slice_end;
    char *slice = NULL;

    if (word == NULL || sigil_out == NULL || has_slice_out == NULL ||
        offset_out == NULL || has_length_out == NULL || length_out == NULL) {
        return 0;
    }
    if (word->part_count != 1 || word->parts[0].quote != CUPID_QUOTE_NONE) return 0;
    source = cupid_word_source_text(word);
    text = source != NULL ? source : word->parts[0].text;
    if (text == NULL || text[0] != '$') {
        free(source);
        return 0;
    }
    if ((strcmp(text, "$@") == 0) || (strcmp(text, "${@}") == 0)) {
        *sigil_out = '@';
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        free(source);
        return 1;
    }
    if ((strcmp(text, "$*") == 0) || (strcmp(text, "${*}") == 0)) {
        *sigil_out = '*';
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        free(source);
        return 1;
    }
    if ((strcmp(text, "@") == 0) || (strcmp(text, "*") == 0)) {
        *sigil_out = text[0];
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        free(source);
        return 1;
    }
    if (text[1] != '{') {
        free(source);
        return 0;
    }
    p = text + 2;
    if (*p != '@' && *p != '*') {
        free(source);
        return 0;
    }
    *sigil_out = *p;
    p++;
    if (*p == '}' && p[1] == '\0') {
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        free(source);
        return 1;
    }
    if (*p != ':') {
        free(source);
        return 0;
    }
    slice_start = p + 1;
    while (*p != '\0' && *p != '}') p++;
    if (*p != '}' || p[1] != '\0') {
        free(source);
        return 0;
    }
    slice_end = p;
    slice = calloc((size_t)(slice_end - slice_start) + 1, 1);
    if (slice == NULL) {
        free(source);
        return -1;
    }
    memcpy(slice, slice_start, (size_t)(slice_end - slice_start));
    if (parse_positional_slice_spec(slice, offset_out, has_length_out, length_out) != 0) {
        free(slice);
        free(source);
        return 0;
    }
    free(slice);
    free(source);
    *has_slice_out = 1;
    return 1;
}

static int parse_quoted_positional_part(const struct cupid_word_part *part, char *sigil_out,
                                        int *has_slice_out, long *offset_out,
                                        int *has_length_out, long *length_out) {
    const char *text;
    const char *p;
    const char *slice_start;
    const char *slice_end;
    char *slice = NULL;

    if (part == NULL || sigil_out == NULL || has_slice_out == NULL ||
        offset_out == NULL || has_length_out == NULL || length_out == NULL) {
        return 0;
    }
    if (part->quote != CUPID_QUOTE_DOUBLE) return 0;
    text = part->text;
    if (text == NULL || text[0] != '$') return 0;

    if ((strcmp(text, "$@") == 0) || (strcmp(text, "${@}") == 0)) {
        *sigil_out = '@';
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        return 1;
    }
    if ((strcmp(text, "$*") == 0) || (strcmp(text, "${*}") == 0)) {
        *sigil_out = '*';
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        return 1;
    }
    if (text[1] != '{') return 0;

    p = text + 2;
    if (*p != '@' && *p != '*') return 0;
    *sigil_out = *p;
    p++;
    if (*p == '}' && p[1] == '\0') {
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        return 1;
    }
    if (*p != ':') return 0;

    slice_start = p + 1;
    while (*p != '\0' && *p != '}') p++;
    if (*p != '}' || p[1] != '\0') return 0;
    slice_end = p;
    slice = calloc((size_t)(slice_end - slice_start) + 1, 1);
    if (slice == NULL) return -1;
    memcpy(slice, slice_start, (size_t)(slice_end - slice_start));
    if (parse_positional_slice_spec(slice, offset_out, has_length_out, length_out) != 0) {
        free(slice);
        return 0;
    }
    free(slice);
    *has_slice_out = 1;
    return 1;
}

static const char *positional_value_at(const struct cupid_shell *shell, long pos_index) {
    if (shell == NULL) return "";
    if (pos_index == 0) return shell->arg0 ? shell->arg0 : "";
    if (pos_index > 0 && (size_t)(pos_index - 1) < shell->params.count) {
        return shell->params.args[(size_t)(pos_index - 1)];
    }
    return "";
}

static int positional_slice_bounds(const struct cupid_shell *shell, int has_slice,
                                   long offset, int has_length, long length,
                                   long *start_out, long *end_out) {
    long max_index = shell ? (long)shell->params.count : 0;
    long start;
    long end;

    if (!has_slice) {
        *start_out = 1;
        *end_out = max_index + 1;
        return 0;
    }

    if (has_length && length < 0) {
        char msg[64];
        snprintf(msg, sizeof(msg), "%ld: substring expression < 0", length);
        (void)cupid_expand_error_set(msg);
        return -1;
    }

    if (offset >= 0) start = offset;
    else start = max_index + 1 + offset;

    if (start < 0 || start > max_index + 1) {
        *start_out = 0;
        *end_out = 0;
        return 0;
    }

    if (has_length) {
        end = start + length;
        if (end < start) end = start;
    } else {
        end = max_index + 1;
    }
    if (end > max_index + 1) end = max_index + 1;

    *start_out = start;
    *end_out = end;
    return 0;
}

static char *join_positional_values(const struct cupid_shell *shell, long start, long end, char sep) {
    long i;
    size_t total = 0;
    char *joined;
    char *rp;

    for (i = start; i < end; i++) {
        if (i > start && sep != '\0') total++;
        total += strlen(positional_value_at(shell, i));
    }
    joined = calloc(total + 1, 1);
    if (joined == NULL) return NULL;
    rp = joined;
    for (i = start; i < end; i++) {
        const char *it = positional_value_at(shell, i);
        size_t ilen = strlen(it);
        if (i > start && sep != '\0') *rp++ = sep;
        memcpy(rp, it, ilen);
        rp += ilen;
    }
    return joined;
}

static int expand_positional_star_at_word(struct runtime_command *cmd, const struct cupid_word *word,
                                          struct cupid_shell *shell) {
    char sigil = '\0';
    int has_slice = 0;
    long offset = 0;
    int has_length = 0;
    long length = 0;
    int parsed;
    long start;
    long end;
    long i;

    parsed = parse_exact_positional_star_at_word(word, &sigil, &has_slice, &offset,
                                                  &has_length, &length);
    if (parsed <= 0) return parsed;
    if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
        return -1;
    }

    if (word->had_quotes && sigil == '@') {
        for (i = start; i < end; i++) {
            char *copy = strdup(positional_value_at(shell, i));
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                return -1;
            }
        }
        return 1;
    }

    if (word->had_quotes && sigil == '*') {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        char *joined;
        if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        joined = join_positional_values(shell, start, end, sep);
        if (joined == NULL) return -1;
        if (command_add_arg(cmd, joined) != 0) {
            free(joined);
            return -1;
        }
        return 1;
    }

    for (i = start; i < end; i++) {
        const char *it = positional_value_at(shell, i);
        char **fields = NULL;
        size_t fc = 0;
        size_t fi;
        if (split_ifs_fields(shell, it, &fields, &fc) != 0) {
            return -1;
        }
        for (fi = 0; fi < fc; fi++) {
            if (add_field_with_glob(cmd, fields[fi], 1, shell) != 0) {
                size_t k;
                for (k = fi; k < fc; k++) free(fields[k]);
                free(fields);
                return -1;
            }
            free(fields[fi]);
        }
        free(fields);
    }
    return 1;
}

static int parse_exact_positional_modified_word(const struct cupid_word *word,
                                                char *sigil_out, char *op_out, int *op_long_out) {
    const char *text;
    const char *p;
    char sigil;
    char op;
    int op_long = 0;

    if (word == NULL || sigil_out == NULL || op_out == NULL || op_long_out == NULL) return 0;
    if (word->part_count != 1) return 0;
    if (word->parts[0].quote != CUPID_QUOTE_NONE &&
        word->parts[0].quote != CUPID_QUOTE_DOUBLE) return 0;

    text = word->parts[0].text;
    if (text == NULL || text[0] != '$' || text[1] != '{') return 0;
    p = text + 2;
    if (*p != '@' && *p != '*') return 0;
    sigil = *p++;

    if (*p == '/' && p[1] == '}' && p[2] == '\0') {
        *sigil_out = sigil;
        *op_out = 'R';
        *op_long_out = 0;
        return 1;
    }

    if (*p == '#' && p[1] == '#' && p[2] == '}' && p[3] == '\0') {
        *sigil_out = sigil;
        *op_out = '#';
        *op_long_out = 1;
        return 1;
    }

    if (*p == '@' && p[1] != '\0' && p[2] == '}' && p[3] == '\0') {
        *sigil_out = sigil;
        *op_out = p[1];
        *op_long_out = 0;
        return 1;
    }

    if (*p != ',' && *p != '^') return 0;
    op = *p++;
    if (*p == op) {
        op_long = 1;
        p++;
    }
    if (*p != '}' || p[1] != '\0') return 0;

    *sigil_out = sigil;
    *op_out = op;
    *op_long_out = op_long;
    return 1;
}

static char *transform_positional_modifier_value(const char *src, char op, int op_long) {
    char *result;
    size_t i;

    if (op == 'Q') {
        const char *text = (src == NULL) ? "" : src;
        size_t len = strlen(text);
        size_t needed = 3;
        char *quoted;
        char *rp;

        for (i = 0; i < len; i++) {
            needed += (text[i] == '\'') ? 4 : 1;
        }
        quoted = calloc(needed, 1);
        if (quoted == NULL) return NULL;
        rp = quoted;
        *rp++ = '\'';
        for (i = 0; i < len; i++) {
            if (text[i] == '\'') {
                memcpy(rp, "'\\''", 4);
                rp += 4;
            } else {
                *rp++ = text[i];
            }
        }
        *rp++ = '\'';
        *rp = '\0';
        return quoted;
    }

    result = strdup(src == NULL ? "" : src);
    if (result == NULL) return NULL;
    if (op == 'R') return result;
    if (op == '#') return result;

    if (op == '^') {
        if (op_long) {
            for (i = 0; result[i] != '\0'; i++) {
                result[i] = (char)toupper((unsigned char)result[i]);
            }
        } else if (result[0] != '\0') {
            result[0] = (char)toupper((unsigned char)result[0]);
        }
        return result;
    }

    if (op == ',') {
        if (op_long) {
            for (i = 0; result[i] != '\0'; i++) {
                result[i] = (char)tolower((unsigned char)result[i]);
            }
        } else if (result[0] != '\0') {
            result[0] = (char)tolower((unsigned char)result[0]);
        }
    }

    return result;
}

static int expand_positional_modified_word(struct runtime_command *cmd, const struct cupid_word *word,
                                           struct cupid_shell *shell) {
    char sigil = '\0';
    char op = '\0';
    int op_long = 0;
    int parsed;
    long start;
    long end;
    long i;

    parsed = parse_exact_positional_modified_word(word, &sigil, &op, &op_long);
    if (parsed <= 0) return parsed;
    if (positional_slice_bounds(shell, 0, 0, 0, 0, &start, &end) != 0) return -1;

    if (!word->had_quotes && (op == '#' || op == 'Q')) {
        for (i = start; i < end; i++) {
            char *copy = transform_positional_modifier_value(positional_value_at(shell, i), op, op_long);
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                return -1;
            }
        }
        return 1;
    }

    if (word->had_quotes && sigil == '@') {
        for (i = start; i < end; i++) {
            char *copy = transform_positional_modifier_value(positional_value_at(shell, i), op, op_long);
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                return -1;
            }
        }
        return 1;
    }

    if (word->had_quotes && sigil == '*') {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        size_t total = 0;
        char *joined;
        char *rp;

        if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        for (i = start; i < end; i++) {
            char *tmp = transform_positional_modifier_value(positional_value_at(shell, i), op, op_long);
            if (tmp == NULL) return -1;
            if (i > start && sep != '\0') total++;
            total += strlen(tmp);
            free(tmp);
        }
        joined = calloc(total + 1, 1);
        if (joined == NULL) return -1;
        rp = joined;
        for (i = start; i < end; i++) {
            char *tmp = transform_positional_modifier_value(positional_value_at(shell, i), op, op_long);
            size_t len;
            if (tmp == NULL) {
                free(joined);
                return -1;
            }
            len = strlen(tmp);
            if (i > start && sep != '\0') *rp++ = sep;
            memcpy(rp, tmp, len);
            rp += len;
            free(tmp);
        }
        if (command_add_arg(cmd, joined) != 0) {
            free(joined);
            return -1;
        }
        return 1;
    }

    for (i = start; i < end; i++) {
        char *tmp = transform_positional_modifier_value(positional_value_at(shell, i), op, op_long);
        char **fields = NULL;
        size_t fc = 0;
        size_t fi;
        if (tmp == NULL) return -1;
        if (split_ifs_fields(shell, tmp, &fields, &fc) != 0) {
            free(tmp);
            return -1;
        }
        free(tmp);
        for (fi = 0; fi < fc; fi++) {
            if (add_field_with_glob(cmd, fields[fi], 1, shell) != 0) {
                size_t k;
                for (k = fi; k < fc; k++) free(fields[k]);
                free(fields);
                return -1;
            }
            free(fields[fi]);
        }
        free(fields);
    }
    return 1;
}

static int expand_quoted_positional_parts_word(struct runtime_command *cmd,
                                               const struct cupid_word *word,
                                               struct cupid_shell *shell) {
    char **fields = NULL;
    size_t field_count = 0;
    int saw_positional = 0;
    int visible = 0;
    size_t i;

    if (word == NULL || !word->had_quotes || word->part_count == 0) return 0;
    if (ensure_runtime_field(&fields, &field_count) != 0) return -1;

    for (i = 0; i < word->part_count; i++) {
        const struct cupid_word_part *part = &word->parts[i];
        char sigil = '\0';
        int has_slice = 0;
        long offset = 0;
        int has_length = 0;
        long length = 0;
        int parsed = parse_quoted_positional_part(part, &sigil, &has_slice, &offset,
                                                  &has_length, &length);
        if (parsed < 0) {
            free_split_fields(fields, field_count);
            return -1;
        }
        if (parsed == 0) {
            char *expanded;
            if (part->quote == CUPID_QUOTE_NONE) {
                free_split_fields(fields, field_count);
                return 0;
            }
            expanded = cupid_expand_text(part->text, part->quote, shell);
            if (expanded == NULL) {
                free_split_fields(fields, field_count);
                return -1;
            }
            if (expanded[0] != '\0' || part->quote != CUPID_QUOTE_NONE) visible = 1;
            if (append_runtime_field_text(&fields, &field_count, expanded) != 0) {
                free(expanded);
                return -1;
            }
            free(expanded);
            continue;
        }

        saw_positional = 1;
        if (sigil == '*') {
            const char *ifs = cupid_vars_get(shell, "IFS");
            char sep = ' ';
            char *joined;
            long start;
            long end;

            if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
                free_split_fields(fields, field_count);
                return -1;
            }
            if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
            if (ifs != NULL && ifs[0] == '\0') sep = '\0';
            joined = join_positional_values(shell, start, end, sep);
            if (joined == NULL) {
                free_split_fields(fields, field_count);
                return -1;
            }
            visible = 1;
            if (append_runtime_field_text(&fields, &field_count, joined) != 0) {
                free(joined);
                return -1;
            }
            free(joined);
            continue;
        }

        {
            long start;
            long end;
            long j;

            if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
                free_split_fields(fields, field_count);
                return -1;
            }
            if (start >= end) continue;

            visible = 1;
            if (append_runtime_field_text(&fields, &field_count, positional_value_at(shell, start)) != 0) {
                return -1;
            }
            for (j = start + 1; j < end; j++) {
                if (add_split_field(&fields, &field_count, positional_value_at(shell, j),
                                    strlen(positional_value_at(shell, j))) != 0) {
                    return -1;
                }
            }
        }
    }

    if (!saw_positional) {
        free_split_fields(fields, field_count);
        return 0;
    }
    if (!visible) {
        free_split_fields(fields, field_count);
        return 1;
    }

    for (i = 0; i < field_count; i++) {
        if (command_add_arg(cmd, fields[i]) != 0) {
            size_t j;
            free(fields[i]);
            for (j = i + 1; j < field_count; j++) free(fields[j]);
            free(fields);
            return -1;
        }
    }
    free(fields);
    return 1;
}

static int parse_embedded_quoted_positional_star_at(const struct cupid_word *word,
                                                    char **prefix_out, char *sigil_out,
                                                    int *has_slice_out, long *offset_out,
                                                    int *has_length_out, long *length_out,
                                                    char **suffix_out) {
    const char *text;
    const char *start = NULL;
    const char *end = NULL;
    const char *p;
    size_t prefix_len;
    size_t suffix_len;
    char *prefix = NULL;
    char *suffix = NULL;
    int found = 0;

    if (word == NULL || prefix_out == NULL || sigil_out == NULL || has_slice_out == NULL ||
        offset_out == NULL || has_length_out == NULL || length_out == NULL || suffix_out == NULL) {
        return 0;
    }
    if (word->part_count != 1 || !word->had_quotes) return 0;
    if (word->parts[0].quote != CUPID_QUOTE_DOUBLE) return 0;

    text = word->parts[0].text;
    if (text == NULL) return 0;
    for (p = text; *p != '\0'; p++) {
        if (*p != '$') continue;
        if (p[1] == '@' || p[1] == '*') {
            if (found) return 0;
            start = p;
            end = p + 2;
            *sigil_out = p[1];
            *has_slice_out = 0;
            *offset_out = 0;
            *has_length_out = 0;
            *length_out = 0;
            found = 1;
            p++;
            continue;
        }
        if (p[1] == '{' && (p[2] == '@' || p[2] == '*')) {
            const char *q = p + 3;
            if (found) return 0;
            start = p;
            *sigil_out = p[2];
            if (*q == '}') {
                end = q + 1;
                *has_slice_out = 0;
                *offset_out = 0;
                *has_length_out = 0;
                *length_out = 0;
            } else {
                const char *slice_start;
                const char *slice_end;
                char *slice;
                if (*q != ':') return 0;
                slice_start = q + 1;
                while (*q != '\0' && *q != '}') q++;
                if (*q != '}') return 0;
                slice_end = q;
                slice = calloc((size_t)(slice_end - slice_start) + 1, 1);
                if (slice == NULL) return -1;
                memcpy(slice, slice_start, (size_t)(slice_end - slice_start));
                if (parse_positional_slice_spec(slice, offset_out, has_length_out, length_out) != 0) {
                    free(slice);
                    return 0;
                }
                free(slice);
                end = q + 1;
                *has_slice_out = 1;
            }
            found = 1;
            p = end - 1;
        }
    }
    if (!found || start == NULL || end == NULL) return 0;

    prefix_len = (size_t)(start - text);
    suffix_len = strlen(end);
    if (prefix_len == 0 && suffix_len == 0) return 0;

    prefix = calloc(prefix_len + 1, 1);
    suffix = calloc(suffix_len + 1, 1);
    if (prefix == NULL || suffix == NULL) {
        free(prefix);
        free(suffix);
        return -1;
    }
    if (prefix_len > 0) memcpy(prefix, text, prefix_len);
    if (suffix_len > 0) memcpy(suffix, end, suffix_len);
    *prefix_out = prefix;
    *suffix_out = suffix;
    return 1;
}

static int parse_quoted_positional_expansion_at(const char *text, char *sigil_out,
                                                int *has_slice_out, long *offset_out,
                                                int *has_length_out, long *length_out,
                                                size_t *consumed_out) {
    const char *p;
    const char *slice_start;
    const char *slice_end;
    char *slice = NULL;

    if (text == NULL || sigil_out == NULL || has_slice_out == NULL ||
        offset_out == NULL || has_length_out == NULL || length_out == NULL ||
        consumed_out == NULL) {
        return 0;
    }
    if (text[0] != '$') return 0;

    if (text[1] == '@' || text[1] == '*') {
        *sigil_out = text[1];
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        *consumed_out = 2;
        return 1;
    }

    if (text[1] != '{' || (text[2] != '@' && text[2] != '*')) return 0;

    *sigil_out = text[2];
    p = text + 3;
    if (*p == '}') {
        *has_slice_out = 0;
        *offset_out = 0;
        *has_length_out = 0;
        *length_out = 0;
        *consumed_out = 4;
        return 1;
    }
    if (*p != ':') return 0;

    slice_start = p + 1;
    while (*p != '\0' && *p != '}') p++;
    if (*p != '}') return 0;
    slice_end = p;
    slice = calloc((size_t)(slice_end - slice_start) + 1, 1);
    if (slice == NULL) return -1;
    memcpy(slice, slice_start, (size_t)(slice_end - slice_start));
    if (parse_positional_slice_spec(slice, offset_out, has_length_out, length_out) != 0) {
        free(slice);
        return 0;
    }
    free(slice);
    *has_slice_out = 1;
    *consumed_out = (size_t)(p - text) + 1;
    return 1;
}

static int append_quoted_literal_segment(char ***fields, size_t *field_count,
                                         const char *text, size_t len,
                                         struct cupid_shell *shell,
                                         int *visible_out) {
    char *segment;
    char *expanded;

    if (len == 0) return 0;
    segment = calloc(len + 1, 1);
    if (segment == NULL) return -1;
    memcpy(segment, text, len);
    expanded = cupid_expand_text(segment, CUPID_QUOTE_DOUBLE, shell);
    free(segment);
    if (expanded == NULL) return -1;
    if (expanded[0] != '\0' && visible_out != NULL) *visible_out = 1;
    if (append_runtime_field_text(fields, field_count, expanded) != 0) {
        free(expanded);
        return -1;
    }
    free(expanded);
    return 0;
}

static int expand_general_quoted_positional_splices_word(struct runtime_command *cmd,
                                                         const struct cupid_word *word,
                                                         struct cupid_shell *shell) {
    const char *text;
    char **fields = NULL;
    size_t field_count = 0;
    size_t cursor = 0;
    size_t i;
    int saw_positional = 0;
    int saw_at = 0;
    int saw_star = 0;
    int visible = 0;

    if (word == NULL || word->part_count != 1 || !word->had_quotes) return 0;
    if (word->parts[0].quote != CUPID_QUOTE_DOUBLE) return 0;
    text = word->parts[0].text;
    if (text == NULL || strchr(text, '$') == NULL) return 0;
    if (ensure_runtime_field(&fields, &field_count) != 0) return -1;

    while (text[cursor] != '\0') {
        size_t pos;
        int found = 0;

        for (pos = cursor; text[pos] != '\0'; pos++) {
            char sigil = '\0';
            int has_slice = 0;
            long offset = 0;
            int has_length = 0;
            long length = 0;
            size_t consumed = 0;
            int parsed;

            if (text[pos] != '$') continue;
            parsed = parse_quoted_positional_expansion_at(text + pos, &sigil, &has_slice, &offset,
                                                          &has_length, &length, &consumed);
            if (parsed < 0) {
                free_split_fields(fields, field_count);
                return -1;
            }
            if (parsed == 0) continue;

            if (append_quoted_literal_segment(&fields, &field_count, text + cursor, pos - cursor,
                                              shell, &visible) != 0) {
                return -1;
            }

            saw_positional = 1;
            if (sigil == '*') {
                const char *ifs = cupid_vars_get(shell, "IFS");
                char sep = ' ';
                char *joined;
                long start;
                long end;

                saw_star = 1;
                if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
                    free_split_fields(fields, field_count);
                    return -1;
                }
                if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
                if (ifs != NULL && ifs[0] == '\0') sep = '\0';
                joined = join_positional_values(shell, start, end, sep);
                if (joined == NULL) {
                    free_split_fields(fields, field_count);
                    return -1;
                }
                if (joined[0] != '\0') visible = 1;
                if (append_runtime_field_text(&fields, &field_count, joined) != 0) {
                    free(joined);
                    return -1;
                }
                free(joined);
            } else {
                long start;
                long end;
                long j;

                saw_at = 1;
                if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
                    free_split_fields(fields, field_count);
                    return -1;
                }
                if (start < end) {
                    visible = 1;
                    if (append_runtime_field_text(&fields, &field_count, positional_value_at(shell, start)) != 0) {
                        return -1;
                    }
                    for (j = start + 1; j < end; j++) {
                        if (add_split_field(&fields, &field_count, positional_value_at(shell, j),
                                            strlen(positional_value_at(shell, j))) != 0) {
                            return -1;
                        }
                    }
                }
            }

            cursor = pos + consumed;
            found = 1;
            break;
        }

        if (!found) break;
    }

    if (append_quoted_literal_segment(&fields, &field_count, text + cursor, strlen(text + cursor),
                                      shell, &visible) != 0) {
        return -1;
    }

    if (!saw_positional) {
        free_split_fields(fields, field_count);
        return 0;
    }

    if (!visible && saw_at && !saw_star) {
        free_split_fields(fields, field_count);
        return 1;
    }

    for (i = 0; i < field_count; i++) {
        if (command_add_arg(cmd, fields[i]) != 0) {
            size_t j;
            free(fields[i]);
            for (j = i + 1; j < field_count; j++) free(fields[j]);
            free(fields);
            return -1;
        }
    }
    free(fields);
    return 1;
}

static int expand_embedded_quoted_positional_star_at_word(struct runtime_command *cmd,
                                                          const struct cupid_word *word,
                                                          struct cupid_shell *shell) {
    char *prefix = NULL;
    char *suffix = NULL;
    char sigil = '\0';
    int has_slice = 0;
    long offset = 0;
    int has_length = 0;
    long length = 0;
    int parsed;
    long start;
    long end;
    long i;

    parsed = parse_embedded_quoted_positional_star_at(word, &prefix, &sigil, &has_slice, &offset,
                                                       &has_length, &length, &suffix);
    if (parsed <= 0) return parsed;
    {
        char *expanded_prefix = cupid_expand_text(prefix, CUPID_QUOTE_DOUBLE, shell);
        char *expanded_suffix = cupid_expand_text(suffix, CUPID_QUOTE_DOUBLE, shell);
        if (expanded_prefix == NULL || expanded_suffix == NULL) {
            free(expanded_prefix);
            free(expanded_suffix);
            free(prefix);
            free(suffix);
            return -1;
        }
        free(prefix);
        free(suffix);
        prefix = expanded_prefix;
        suffix = expanded_suffix;
    }
    if (positional_slice_bounds(shell, has_slice, offset, has_length, length, &start, &end) != 0) {
        free(prefix);
        free(suffix);
        return -1;
    }

    if (sigil == '*') {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        char *joined = NULL;
        size_t plen = strlen(prefix);
        size_t slen = strlen(suffix);
        size_t jlen;
        char *arg;
        if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        joined = join_positional_values(shell, start, end, sep);
        if (joined == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        jlen = strlen(joined);
        arg = calloc(plen + jlen + slen + 1, 1);
        if (arg == NULL) {
            free(joined);
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        if (jlen > 0) memcpy(arg + plen, joined, jlen);
        if (slen > 0) memcpy(arg + plen + jlen, suffix, slen);
        free(joined);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            free(prefix);
            free(suffix);
            return -1;
        }
        free(prefix);
        free(suffix);
        return 1;
    }

    if (start >= end) {
        size_t plen = strlen(prefix);
        size_t slen = strlen(suffix);
        char *arg = calloc(plen + slen + 1, 1);
        if (sigil == '@' && plen == 0 && slen == 0) {
            free(arg);
            free(prefix);
            free(suffix);
            return 1;
        }
        if (arg == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        if (slen > 0) memcpy(arg + plen, suffix, slen);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            free(prefix);
            free(suffix);
            return -1;
        }
        free(prefix);
        free(suffix);
        return 1;
    }

    for (i = start; i < end; i++) {
        const char *it = positional_value_at(shell, i);
        size_t ilen = strlen(it);
        size_t plen = (i == start) ? strlen(prefix) : 0;
        size_t slen = (i + 1 == end) ? strlen(suffix) : 0;
        char *arg = calloc(plen + ilen + slen + 1, 1);
        if (arg == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        memcpy(arg + plen, it, ilen);
        if (slen > 0) memcpy(arg + plen + ilen, suffix, slen);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            free(prefix);
            free(suffix);
            return -1;
        }
    }

    free(prefix);
    free(suffix);
    return 1;
}

static int expand_multipart_quoted_positional_star_at_word(struct runtime_command *cmd,
                                                           const struct cupid_word *word,
                                                           struct cupid_shell *shell) {
    size_t star_idx = (size_t)-1;
    char sigil = '\0';
    char *prefix = NULL;
    char *suffix = NULL;
    size_t i;
    long start;
    long end;

    if (word == NULL || word->part_count < 2 || !word->had_quotes) return 0;

    for (i = 0; i < word->part_count; i++) {
        const struct cupid_word_part *part = &word->parts[i];
        if (part->quote == CUPID_QUOTE_DOUBLE) {
            if ((strcmp(part->text, "$@") == 0) || (strcmp(part->text, "${@}") == 0)) {
                if (star_idx != (size_t)-1) return 0;
                star_idx = i;
                sigil = '@';
                continue;
            }
            if ((strcmp(part->text, "$*") == 0) || (strcmp(part->text, "${*}") == 0)) {
                if (star_idx != (size_t)-1) return 0;
                star_idx = i;
                sigil = '*';
                continue;
            }
        }
    }
    if (star_idx == (size_t)-1) return 0;

    for (i = 0; i < word->part_count; i++) {
        const struct cupid_word_part *part = &word->parts[i];
        char *expanded;
        char **target;
        size_t cur_len;
        size_t add_len;
        char *next;

        if (i == star_idx) continue;
        expanded = cupid_expand_text(part->text, part->quote, shell);
        if (expanded == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        target = (i < star_idx) ? &prefix : &suffix;
        cur_len = (*target != NULL) ? strlen(*target) : 0;
        add_len = strlen(expanded);
        next = realloc(*target, cur_len + add_len + 1);
        if (next == NULL) {
            free(expanded);
            free(prefix);
            free(suffix);
            return -1;
        }
        *target = next;
        if (cur_len == 0) (*target)[0] = '\0';
        memcpy(*target + cur_len, expanded, add_len);
        (*target)[cur_len + add_len] = '\0';
        free(expanded);
    }

    if (positional_slice_bounds(shell, 0, 0, 0, 0, &start, &end) != 0) {
        free(prefix);
        free(suffix);
        return -1;
    }

    if (sigil == '*') {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        char *joined = NULL;
        size_t plen = prefix ? strlen(prefix) : 0;
        size_t jlen = 0;
        size_t slen = suffix ? strlen(suffix) : 0;
        char *arg;
        if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        joined = join_positional_values(shell, start, end, sep);
        jlen = joined ? strlen(joined) : 0;
        if (joined == NULL) joined = strdup("");
        if (joined == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        arg = calloc(plen + jlen + slen + 1, 1);
        if (arg == NULL) {
            free(joined);
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        if (jlen > 0) memcpy(arg + plen, joined, jlen);
        if (slen > 0) memcpy(arg + plen + jlen, suffix, slen);
        free(joined);
        free(prefix);
        free(suffix);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            return -1;
        }
        return 1;
    }

    if (start >= end) {
        size_t plen = prefix ? strlen(prefix) : 0;
        size_t slen = suffix ? strlen(suffix) : 0;
        char *arg = calloc(plen + slen + 1, 1);
        if (arg == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        if (slen > 0) memcpy(arg + plen, suffix, slen);
        free(prefix);
        free(suffix);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            return -1;
        }
        return 1;
    }

    for (i = (size_t)start; (long)i < end; i++) {
        const char *it = positional_value_at(shell, (long)i);
        size_t plen = ((long)i == start && prefix != NULL) ? strlen(prefix) : 0;
        size_t slen = ((long)i + 1 == end && suffix != NULL) ? strlen(suffix) : 0;
        size_t ilen = strlen(it);
        char *arg = calloc(plen + ilen + slen + 1, 1);
        if (arg == NULL) {
            free(prefix);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        memcpy(arg + plen, it, ilen);
        if (slen > 0) memcpy(arg + plen + ilen, suffix, slen);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            free(prefix);
            free(suffix);
            return -1;
        }
    }

    free(prefix);
    free(suffix);
    return 1;
}

static int parse_exact_array_star_at_word(const struct cupid_word *word,
                                          char **name_out, char *sigil_out) {
    const char *text;
    const char *p;
    const char *name_start;
    const char *name_end;
    char sigil;
    char *name;

    if (word == NULL || name_out == NULL || sigil_out == NULL) return 0;
    if (word->part_count != 1 || word->parts[0].quote != CUPID_QUOTE_NONE) return 0;

    text = word->parts[0].text;
    if (text == NULL || text[0] != '$' || text[1] != '{') return 0;
    p = text + 2;
    if (!is_name_start_char(*p)) return 0;
    name_start = p;
    p++;
    while (is_name_char(*p)) p++;
    name_end = p;
    if (*p != '[') return 0;
    p++;
    sigil = *p;
    if (sigil != '@' && sigil != '*') return 0;
    p++;
    if (*p != ']' || p[1] != '}' || p[2] != '\0') return 0;

    name = calloc((size_t)(name_end - name_start) + 1, 1);
    if (name == NULL) return -1;
    memcpy(name, name_start, (size_t)(name_end - name_start));
    *name_out = name;
    *sigil_out = sigil;
    return 1;
}

static int expand_array_star_at_word(struct runtime_command *cmd, const struct cupid_word *word,
                                     struct cupid_shell *shell) {
    char *name = NULL;
    char sigil = '\0';
    int parsed;
    size_t i;
    size_t count;

    parsed = parse_exact_array_star_at_word(word, &name, &sigil);
    if (parsed <= 0) return parsed;

    count = cupid_array_member_count(shell, name);
    if (count == 0) {
        const char *scalar = cupid_vars_get(shell, name);
        if (scalar != NULL) {
            char *copy = strdup(scalar);
            free(name);
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                return -1;
            }
            return 1;
        }
    }
    if (word->had_quotes && sigil == '@') {
        for (i = 0; i < count; i++) {
            char *copy = strdup(cupid_array_member_value(shell, name, i));
            if (copy == NULL || command_add_arg(cmd, copy) != 0) {
                free(copy);
                free(name);
                return -1;
            }
        }
        free(name);
        return 1;
    }

    if (word->had_quotes && sigil == '*') {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        size_t total = 0;
        char *joined;
        char *rp;

        if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        for (i = 0; i < count; i++) total += strlen(cupid_array_member_value(shell, name, i));
        if (sep != '\0' && count > 1) total += count - 1;
        joined = calloc(total + 1, 1);
        if (joined == NULL) {
            free(name);
            return -1;
        }
        rp = joined;
        for (i = 0; i < count; i++) {
            const char *it = cupid_array_member_value(shell, name, i);
            size_t len = strlen(it);
            memcpy(rp, it, len);
            rp += len;
            if (sep != '\0' && i + 1 < count) *rp++ = sep;
        }
        if (command_add_arg(cmd, joined) != 0) {
            free(joined);
            free(name);
            return -1;
        }
        free(name);
        return 1;
    }

    for (i = 0; i < count; i++) {
        const char *it = cupid_array_member_value(shell, name, i);
        char **fields = NULL;
        size_t fi;
        size_t fc = 0;
        if (split_ifs_fields(shell, it, &fields, &fc) != 0) {
            free(name);
            return -1;
        }
        for (fi = 0; fi < fc; fi++) {
            if (add_field_with_glob(cmd, fields[fi], 1, shell) != 0) {
                size_t k;
                for (k = fi; k < fc; k++) free(fields[k]);
                free(fields);
                free(name);
                return -1;
            }
            free(fields[fi]);
        }
        free(fields);
    }
    free(name);
    return 1;
}

static int parse_embedded_quoted_array_at(const struct cupid_word *word,
                                          char **prefix_out, char **name_out, char **suffix_out) {
    const char *text;
    const char *start;
    const char *p;
    const char *name_start;
    const char *name_end;
    char *prefix = NULL;
    char *name = NULL;
    char *suffix = NULL;
    size_t prefix_len;
    size_t name_len;
    size_t suffix_len;

    if (word == NULL || prefix_out == NULL || name_out == NULL || suffix_out == NULL) return 0;
    if (word->part_count != 1 || !word->had_quotes) return 0;
    if (word->parts[0].quote != CUPID_QUOTE_DOUBLE) return 0;
    text = word->parts[0].text;
    if (text == NULL) return 0;

    start = strstr(text, "${");
    if (start == NULL) return 0;
    if (start != strchr(text, '$')) return 0;
    p = start + 2;
    if (!is_name_start_char(*p)) return 0;
    name_start = p;
    p++;
    while (is_name_char(*p)) p++;
    name_end = p;
    if (p[0] != '[' || p[1] != '@' || p[2] != ']' || p[3] != '}') return 0;
    if (strchr(p + 4, '$') != NULL) return 0;

    prefix_len = (size_t)(start - text);
    name_len = (size_t)(name_end - name_start);
    suffix_len = strlen(p + 4);
    if (prefix_len == 0 && suffix_len == 0) return 0;

    prefix = calloc(prefix_len + 1, 1);
    name = calloc(name_len + 1, 1);
    suffix = calloc(suffix_len + 1, 1);
    if (prefix == NULL || name == NULL || suffix == NULL) {
        free(prefix);
        free(name);
        free(suffix);
        return -1;
    }
    if (prefix_len > 0) memcpy(prefix, text, prefix_len);
    memcpy(name, name_start, name_len);
    if (suffix_len > 0) memcpy(suffix, p + 4, suffix_len);

    *prefix_out = prefix;
    *name_out = name;
    *suffix_out = suffix;
    return 1;
}

static int expand_embedded_quoted_array_at_word(struct runtime_command *cmd,
                                                const struct cupid_word *word,
                                                struct cupid_shell *shell) {
    char *prefix = NULL;
    char *name = NULL;
    char *suffix = NULL;
    int parsed;
    size_t count;
    size_t i;

    parsed = parse_embedded_quoted_array_at(word, &prefix, &name, &suffix);
    if (parsed <= 0) return parsed;

    count = cupid_array_member_count(shell, name);
    if (count == 0) {
        const char *scalar = cupid_vars_get(shell, name);
        if (scalar != NULL) {
            size_t len = strlen(prefix) + strlen(scalar) + strlen(suffix);
            char *arg = calloc(len + 1, 1);
            if (arg == NULL) {
                free(prefix);
                free(name);
                free(suffix);
                return -1;
            }
            memcpy(arg, prefix, strlen(prefix));
            memcpy(arg + strlen(prefix), scalar, strlen(scalar));
            memcpy(arg + strlen(prefix) + strlen(scalar), suffix, strlen(suffix));
            if (command_add_arg(cmd, arg) != 0) {
                free(arg);
                free(prefix);
                free(name);
                free(suffix);
                return -1;
            }
            free(prefix);
            free(name);
            free(suffix);
            return 1;
        }
        size_t len = strlen(prefix) + strlen(suffix);
        char *joined = calloc(len + 1, 1);
        if (joined == NULL) {
            free(prefix);
            free(name);
            free(suffix);
            return -1;
        }
        memcpy(joined, prefix, strlen(prefix));
        memcpy(joined + strlen(prefix), suffix, strlen(suffix));
        if (command_add_arg(cmd, joined) != 0) {
            free(joined);
            free(prefix);
            free(name);
            free(suffix);
            return -1;
        }
        free(prefix);
        free(name);
        free(suffix);
        return 1;
    }

    for (i = 0; i < count; i++) {
        const char *it = cupid_array_member_value(shell, name, i);
        size_t plen = (i == 0) ? strlen(prefix) : 0;
        size_t slen = (i + 1 == count) ? strlen(suffix) : 0;
        size_t ilen = strlen(it);
        char *arg = calloc(plen + ilen + slen + 1, 1);
        if (arg == NULL) {
            free(prefix);
            free(name);
            free(suffix);
            return -1;
        }
        if (plen > 0) memcpy(arg, prefix, plen);
        memcpy(arg + plen, it, ilen);
        if (slen > 0) memcpy(arg + plen + ilen, suffix, slen);
        if (command_add_arg(cmd, arg) != 0) {
            free(arg);
            free(prefix);
            free(name);
            free(suffix);
            return -1;
        }
    }
    free(prefix);
    free(name);
    free(suffix);
    return 1;
}

static int expand_proc_subst(struct runtime_command *cmd, const struct cupid_word *word, struct cupid_shell *shell) {
    const char *text = word->parts[0].text;
    int fd = -1;
    char path_buf[64];

    if (spawn_proc_subst_fd(text, shell, &fd) != 0) return -1;
    snprintf(path_buf, sizeof(path_buf), "/dev/fd/%d", fd);

    {
        char *arg = strdup(path_buf);
        if (!arg) {
            close(fd);
            return -1;
        }
        /* Keep fd open so consumer can access /dev/fd/<n>. */
        return command_add_arg(cmd, arg);
    }
}

static int add_expanded_word(struct runtime_command *cmd, const struct cupid_word *word,
                             struct cupid_shell *shell, int assignment_tilde_allowed) {
    int assignment_candidate = word_is_assignment_candidate(word);

    {
        int exact_plus = expand_exact_plus_param_word(cmd, word, shell);
        if (exact_plus < 0) return -1;
        if (exact_plus > 0) return 0;
    }
    if (!assignment_candidate) {
        int quoted_pos = expand_quoted_positional_parts_word(cmd, word, shell);
        if (quoted_pos < 0) return -1;
        if (quoted_pos > 0) return 0;
    }
    if (!assignment_candidate) {
        int pos_mod = expand_positional_modified_word(cmd, word, shell);
        if (pos_mod < 0) return -1;
        if (pos_mod > 0) return 0;
    }
    if (!assignment_candidate) {
        int general_pos = expand_general_quoted_positional_splices_word(cmd, word, shell);
        if (general_pos < 0) return -1;
        if (general_pos > 0) return 0;
    }
    if (!assignment_candidate) {
        int emb_pos = expand_embedded_quoted_positional_star_at_word(cmd, word, shell);
        if (emb_pos < 0) return -1;
        if (emb_pos > 0) return 0;
    }
    if (!assignment_candidate) {
        int multi_pos = expand_multipart_quoted_positional_star_at_word(cmd, word, shell);
        if (multi_pos < 0) return -1;
        if (multi_pos > 0) return 0;
    }
    if (!assignment_candidate) {
        int pos = expand_positional_star_at_word(cmd, word, shell);
        if (pos < 0) return -1;
        if (pos > 0) return 0;
    }
    if (!assignment_candidate) {
        int emb = expand_embedded_quoted_array_at_word(cmd, word, shell);
        if (emb < 0) return -1;
        if (emb > 0) return 0;
    }
    if (!assignment_candidate) {
        int arr = expand_array_star_at_word(cmd, word, shell);
        if (arr < 0) return -1;
        if (arr > 0) return 0;
    }
    {
        int ps = is_proc_subst(word);
        if (ps) {
            if (shell->mode == CUPID_MODE_POSIX) {
                (void)cupid_expand_error_set("syntax error");
                return -1;
            }
            return expand_proc_subst(cmd, word, shell);
        }
    }
    if (shell->mode != CUPID_MODE_POSIX) {
        char *brace_source = cupid_word_source_text(word);
        if (brace_source != NULL && strchr(brace_source, '{') != NULL) {
            char **brace_results = NULL;
            size_t brace_count = 0;
            if (cupid_brace_expand(brace_source, &brace_results, &brace_count) == 0 &&
                brace_count > 0 &&
                (brace_count != 1 || strcmp(brace_results[0], brace_source) != 0)) {
                size_t bi;
                free(brace_source);
                for (bi = 0; bi < brace_count; bi++) {
                    char *with_tilde = cupid_expand_tilde(brace_results[bi], shell);
                    char *expanded = NULL;
                    char **fields = NULL;
                    size_t fi, fc = 0;
                    struct cupid_tokens btoks = {0};
                    if (word->had_quotes &&
                        with_tilde != NULL &&
                        cupid_lex(with_tilde, &btoks) == 0) {
                        size_t ti;
                        size_t word_idx = (size_t)-1;
                        int bad_shape = 0;
                        for (ti = 0; ti < btoks.count; ti++) {
                            if (btoks.items[ti].kind == TOK_NEWLINE) continue;
                            if (btoks.items[ti].kind == TOK_WORD && word_idx == (size_t)-1) {
                                word_idx = ti;
                                continue;
                            }
                            bad_shape = 1;
                            break;
                        }
                        if (!bad_shape && word_idx != (size_t)-1) {
                            expanded = cupid_expand_word(&btoks.items[word_idx].word, shell);
                        }
                    }
                    cupid_tokens_free(&btoks);
                    if (expanded == NULL) {
                        expanded = cupid_expand_text(with_tilde != NULL ? with_tilde : brace_results[bi],
                                                     CUPID_QUOTE_NONE, shell);
                    }
                    free(with_tilde);
                    if (expanded == NULL) {
                        size_t k;
                        for (k = bi; k < brace_count; k++) free(brace_results[k]);
                        free(brace_results);
                        return -1;
                    }
                    if (split_ifs_fields(shell, expanded, &fields, &fc) != 0) {
                        size_t k;
                        free(expanded);
                        for (k = bi; k < brace_count; k++) free(brace_results[k]);
                        free(brace_results);
                        return -1;
                    }
                    if (fc == 0 && brace_results[bi][0] == '\0') {
                        fields = calloc(1, sizeof(*fields));
                        if (fields == NULL) {
                            size_t k;
                            free(expanded);
                            for (k = bi; k < brace_count; k++) free(brace_results[k]);
                            free(brace_results);
                            return -1;
                        }
                        fields[0] = strdup("");
                        if (fields[0] == NULL) {
                            size_t k;
                            free(fields);
                            free(expanded);
                            for (k = bi; k < brace_count; k++) free(brace_results[k]);
                            free(brace_results);
                            return -1;
                        }
                        fc = 1;
                    }
                    free(expanded);
                    for (fi = 0; fi < fc; fi++) {
                        if (add_field_with_glob(cmd, fields[fi], 1, shell) != 0) {
                            size_t j, k;
                            for (j = fi; j < fc; j++) free(fields[j]);
                            free(fields);
                            for (k = bi; k < brace_count; k++) free(brace_results[k]);
                            free(brace_results);
                            return -1;
                        }
                        free(fields[fi]);
                    }
                    free(fields);
                    free(brace_results[bi]);
                }
                free(brace_results);
                return 0;
            }
            {
                size_t k;
                for (k = 0; k < brace_count; k++) free(brace_results[k]);
                free(brace_results);
            }
        }
        free(brace_source);
    }

    {
        size_t prefix_parts = 0;
        if (word_has_only_trailing_empty_quotes(word, &prefix_parts)) {
            struct cupid_word prefix = *word;
            char *expanded;
            char **fields = NULL;
            size_t i;
            size_t count = 0;
            int needs_empty_field;

            prefix.part_count = prefix_parts;
            expanded = cupid_expand_word(&prefix, shell);
            if (expanded == NULL) return -1;
            needs_empty_field = (expanded[0] == '\0') || text_ends_with_ifs_char(shell, expanded);
            if (split_ifs_fields(shell, expanded, &fields, &count) != 0) {
                free(expanded);
                return -1;
            }
            free(expanded);
            for (i = 0; i < count; i++) {
                if (add_field_with_glob(cmd, fields[i], 1, shell) != 0) {
                    size_t j;
                    for (j = i; j < count; j++) free(fields[j]);
                    free(fields);
                    return -1;
                }
                free(fields[i]);
            }
            free(fields);
            if (needs_empty_field) {
                char *empty = strdup("");
                if (empty == NULL || command_add_arg(cmd, empty) != 0) {
                    free(empty);
                    return -1;
                }
            }
            return 0;
        }
    }

    {
        char *expanded = cupid_expand_word(word, shell);
        int is_subshell;
        if (expanded == NULL) return -1;

        is_subshell = word_is_subshell_group(word);
        if (is_subshell < 0) {
            free(expanded);
            return -1;
        }
        if (is_subshell) {
            if (command_add_arg(cmd, expanded) != 0) {
                free(expanded);
                return -1;
            }
            return 0;
        }

        {
            char *source = cupid_word_source_text(word);
            if (source != NULL && looks_like_array_assignment(source)) {
                free(expanded);
                if (command_add_arg(cmd, source) != 0) {
                    free(source);
                    return -1;
                }
                return 0;
            }
            free(source);
        }

        if (!word->had_quotes) {
            if (word_is_assignment_candidate(word)) {
                if (assignment_tilde_allowed) {
                    char *tilde_expanded = expand_assignment_tilde_segments(expanded, shell);
                    if (tilde_expanded == NULL) {
                        free(expanded);
                        return -1;
                    }
                    free(expanded);
                    expanded = tilde_expanded;
                }
                if (command_add_arg(cmd, expanded) != 0) {
                    free(expanded);
                    return -1;
                }
                return 0;
            }
            char **fields = NULL;
            size_t i, count = 0;
            if (split_ifs_fields(shell, expanded, &fields, &count) != 0) {
                free(expanded);
                return -1;
            }
            free(expanded);
            for (i = 0; i < count; i++) {
                if (add_field_with_glob(cmd, fields[i], 1, shell) != 0) {
                    size_t j;
                    for (j = i; j < count; j++) free(fields[j]);
                    free(fields);
                    return -1;
                }
                free(fields[i]);
            }
            free(fields);
            return 0;
        }

        if (add_field_with_glob(cmd, expanded, 0, shell) != 0) {
            free(expanded);
            return -1;
        }
        free(expanded);
        return 0;
    }
}

static int parse_subshell_script(const char *arg, char **out_script) {
    size_t len, start = 1, end;
    char *script;

    if (arg == NULL || out_script == NULL) return 0;
    len = strlen(arg);
    if (len < 2 || arg[0] != '(' || arg[len - 1] != ')') return 0;
    end = len - 1;
    while (start < end && (arg[start] == ' ' || arg[start] == '\t' || arg[start] == '\n')) start++;
    while (end > start && (arg[end - 1] == ' ' || arg[end - 1] == '\t' || arg[end - 1] == '\n')) end--;
    script = calloc((end - start) + 1, 1);
    if (script == NULL) return -1;
    if (end > start) memcpy(script, arg + start, end - start);
    *out_script = script;
    return 1;
}

static int spawn_proc_subst_fd(const char *text, struct cupid_shell *shell, int *fd_out) {
    int direction;
    size_t tlen;
    char *script = NULL;
    int fds[2];
    pid_t pid;

    if (text == NULL || shell == NULL || fd_out == NULL) return -1;
    direction = (text[0] == '<') ? 0 : 1;
    tlen = strlen(text);
    if (tlen < 3 || text[1] != '(' || text[tlen - 1] != ')') return -1;
    script = calloc(tlen - 2, 1);
    if (script == NULL) return -1;
    memcpy(script, text + 2, tlen - 3);

    if (pipe(fds) != 0) {
        free(script);
        return -1;
    }

    pid = fork();
    if (pid < 0) {
        free(script);
        close(fds[0]);
        close(fds[1]);
        return -1;
    }
    if (pid == 0) {
        struct cupid_shell child;
        int rc;
        signal(SIGINT, SIG_DFL);
        if (direction == 0) {
            close(fds[0]);
            if (dup2(fds[1], STDOUT_FILENO) < 0) _exit(1);
            close(fds[1]);
        } else {
            close(fds[1]);
            if (dup2(fds[0], STDIN_FILENO) < 0) _exit(1);
            close(fds[0]);
        }
        cupid_shell_init(&child);
        child.mode = shell->mode;
        child.shell_pid = shell->shell_pid;
        child.last_status = shell->last_status;
        child.is_interactive = shell->is_interactive;
        rc = cupid_shell_eval_line(&child, script, 1);
        cupid_shell_destroy(&child);
        fflush(NULL);
        _exit(rc & 0xff);
    }
    free(script);

    if (direction == 0) {
        close(fds[1]);
        *fd_out = fds[0];
    } else {
        close(fds[0]);
        *fd_out = fds[1];
    }
    return 0;
}

static int status_from_wait_status(int st) {
    if (WIFEXITED(st)) return WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + WTERMSIG(st);
    return 1;
}

static void set_status_array(struct cupid_shell *shell, const char *name,
                             const int *statuses, size_t count) {
    char **items;
    size_t i;
    if (shell == NULL || name == NULL || statuses == NULL || count == 0) return;
    items = calloc(count, sizeof(char *));
    if (items == NULL) return;
    for (i = 0; i < count; i++) {
        char buf[32];
        int n = snprintf(buf, sizeof(buf), "%d", statuses[i]);
        if (n < 0) {
            size_t j;
            for (j = 0; j < i; j++) free(items[j]);
            free(items);
            return;
        }
        items[i] = strdup(buf);
        if (items[i] == NULL) {
            size_t j;
            for (j = 0; j < i; j++) free(items[j]);
            free(items);
            return;
        }
    }
    (void)cupid_array_set_list(shell, name, items, count);
    for (i = 0; i < count; i++) free(items[i]);
    free(items);
}

static void runtime_command_free(struct runtime_command *cmd) {
    size_t j;
    if (cmd == NULL) return;
    for (j = 0; j < (size_t)cmd->argc; j++) free(cmd->argv[j]);
    free(cmd->argv);
    cmd->argv = NULL;
    cmd->argc = 0;
    for (j = 0; j < cmd->redir_count; j++) {
        free(cmd->redirs[j].fd_var);
        free(cmd->redirs[j].target);
        free(cmd->redirs[j].target_source);
        free(cmd->redirs[j].heredoc_delim);
        free(cmd->redirs[j].heredoc_body);
        if (cmd->redirs[j].heredoc_fd >= 0) close(cmd->redirs[j].heredoc_fd);
        if (cmd->redirs[j].proc_subst_fd >= 0) close(cmd->redirs[j].proc_subst_fd);
    }
    free(cmd->redirs);
    cmd->redirs = NULL;
    cmd->redir_count = 0;
}

struct heredoc_ref_list {
    struct cupid_redir **items;
    size_t count;
    size_t capacity;
};

static int heredoc_ref_list_push(struct heredoc_ref_list *list, struct cupid_redir *redir) {
    struct cupid_redir **next;
    size_t nc;
    if (list == NULL || redir == NULL) return -1;
    if (list->count == list->capacity) {
        nc = (list->capacity == 0) ? 8 : list->capacity * 2;
        next = realloc(list->items, sizeof(*next) * nc);
        if (next == NULL) return -1;
        list->items = next;
        list->capacity = nc;
    }
    list->items[list->count++] = redir;
    return 0;
}

static int collect_heredoc_redirs_from_list(struct cupid_list_ast *list,
                                            struct heredoc_ref_list *refs);
static int collect_heredoc_redirs_from_if(struct cupid_if_node *if_node,
                                          struct heredoc_ref_list *refs);

static int collect_heredoc_redirs_from_if(struct cupid_if_node *if_node,
                                          struct heredoc_ref_list *refs) {
    if (if_node == NULL || refs == NULL) return 0;
    if (collect_heredoc_redirs_from_list(if_node->condition, refs) != 0 ||
        collect_heredoc_redirs_from_list(if_node->then_body, refs) != 0) {
        return -1;
    }
    if (collect_heredoc_redirs_from_if(if_node->elif_next, refs) != 0) return -1;
    if (collect_heredoc_redirs_from_list(if_node->else_body, refs) != 0) return -1;
    return 0;
}

static int collect_heredoc_redirs_from_node(struct cupid_node *node,
                                            struct heredoc_ref_list *refs) {
    size_t i;
    if (node == NULL || refs == NULL) return 0;
    for (i = 0; i < node->redir_count; i++) {
        if (node->redirs[i].kind == CUPID_REDIR_HEREDOC) {
            if (heredoc_ref_list_push(refs, &node->redirs[i]) != 0) return -1;
        }
    }
    switch (node->kind) {
        case NODE_IF:
            if (collect_heredoc_redirs_from_if(&node->if_clause, refs) != 0) return -1;
            break;
        case NODE_FOR:
            if (collect_heredoc_redirs_from_list(node->for_clause.body, refs) != 0) return -1;
            break;
        case NODE_WHILE:
        case NODE_UNTIL:
            if (collect_heredoc_redirs_from_list(node->while_clause.condition, refs) != 0 ||
                collect_heredoc_redirs_from_list(node->while_clause.body, refs) != 0) {
                return -1;
            }
            break;
        case NODE_CASE: {
            size_t ci;
            for (ci = 0; ci < node->case_clause.item_count; ci++) {
                if (collect_heredoc_redirs_from_list(node->case_clause.items[ci].body, refs) != 0) return -1;
            }
            break;
        }
        case NODE_BRACE_GROUP:
            if (collect_heredoc_redirs_from_list(node->brace_group, refs) != 0) return -1;
            break;
        case NODE_SUBSHELL:
            if (collect_heredoc_redirs_from_list(node->subshell, refs) != 0) return -1;
            break;
        case NODE_FUNCTION_DEF:
            if (collect_heredoc_redirs_from_node(node->func_def.body, refs) != 0) return -1;
            break;
        case NODE_COPROC: {
            size_t pi;
            if (node->coproc.pipeline == NULL) break;
            for (pi = 0; pi < node->coproc.pipeline->count; pi++) {
                if (collect_heredoc_redirs_from_node(&node->coproc.pipeline->commands[pi], refs) != 0) return -1;
            }
            break;
        }
        default:
            break;
    }
    return 0;
}

static int collect_heredoc_redirs_from_list(struct cupid_list_ast *list,
                                            struct heredoc_ref_list *refs) {
    size_t i;
    if (list == NULL || refs == NULL) return 0;
    for (i = 0; i < list->count; i++) {
        size_t j;
        for (j = 0; j < list->items[i].pipeline.count; j++) {
            if (collect_heredoc_redirs_from_node(&list->items[i].pipeline.commands[j], refs) != 0) return -1;
        }
    }
    return 0;
}

static int capture_next_heredoc_body(const char **cursor_io, const char *delimiter,
                                     int strip_tabs, char **body_out) {
    const char *cursor;
    const char *line_start;
    char *body = NULL;
    size_t body_len = 0;
    size_t body_cap = 0;

    if (cursor_io == NULL || *cursor_io == NULL || delimiter == NULL || body_out == NULL) return -1;
    cursor = *cursor_io;

    while (*cursor != '\0') {
        const char *line_end = cursor;
        while (*line_end != '\0' && *line_end != '\n') line_end++;
        {
            char *line = calloc((size_t)(line_end - cursor) + 1, 1);
            int has_heredoc = 0;
            if (line == NULL) {
                free(body);
                return -1;
            }
            if (line_end > cursor) memcpy(line, cursor, (size_t)(line_end - cursor));
            has_heredoc = (strstr(line, "<<") != NULL) ? 1 : 0;
            free(line);
            if (has_heredoc) {
                cursor = (*line_end == '\n') ? line_end + 1 : line_end;
                break;
            }
        }
        cursor = (*line_end == '\n') ? line_end + 1 : line_end;
    }

    line_start = cursor;
    while (*line_start != '\0') {
        const char *line_end = line_start;
        const char *cmp;
        size_t len;
        while (*line_end != '\0' && *line_end != '\n') line_end++;
        cmp = line_start;
        if (strip_tabs) {
            while (cmp < line_end && *cmp == '\t') cmp++;
        }
        if (strlen(delimiter) == (size_t)(line_end - cmp) &&
            strncmp(cmp, delimiter, (size_t)(line_end - cmp)) == 0) {
            *cursor_io = (*line_end == '\n') ? line_end + 1 : line_end;
            *body_out = (body != NULL) ? body : strdup("");
            return (*body_out == NULL) ? -1 : 0;
        }
        len = (size_t)(line_end - line_start);
        if (body_len + len + 2 > body_cap) {
            size_t nc = (body_cap == 0) ? 128 : body_cap;
            char *next;
            while (body_len + len + 2 > nc) nc *= 2;
            next = realloc(body, nc);
            if (next == NULL) {
                free(body);
                return -1;
            }
            body = next;
            body_cap = nc;
        }
        if (len > 0) memcpy(body + body_len, line_start, len);
        body_len += len;
        body[body_len++] = '\n';
        body[body_len] = '\0';
        line_start = (*line_end == '\n') ? line_end + 1 : line_end;
    }

    free(body);
    return -1;
}

static int capture_heredocs_from_source(struct cupid_list_ast *list, const char *source) {
    struct heredoc_ref_list refs = {0};
    const char *cursor;
    size_t i;
    int rc = 0;

    if (list == NULL || source == NULL) return 0;
    if (collect_heredoc_redirs_from_list(list, &refs) != 0) {
        free(refs.items);
        return -1;
    }
    cursor = source;
    for (i = 0; i < refs.count; i++) {
        char *delimiter = cupid_word_dequote_literal(&refs.items[i]->target);
        char *body = NULL;
        if (delimiter == NULL) {
            rc = -1;
            break;
        }
        if (capture_next_heredoc_body(&cursor, delimiter,
                                      refs.items[i]->heredoc_strip_tabs ? 1 : 0,
                                      &body) != 0) {
            free(delimiter);
            rc = -1;
            break;
        }
        free(refs.items[i]->heredoc_body);
        refs.items[i]->heredoc_body = body;
        free(delimiter);
    }
    free(refs.items);
    return rc;
}

static void runtime_pipeline_free(struct runtime_pipeline *pl) {
    size_t i;
    if (pl == NULL) return;
    for (i = 0; i < pl->count; i++) {
        runtime_command_free(&pl->commands[i]);
    }
    free(pl->commands);
    pl->commands = NULL;
    pl->count = 0;
}

static int command_add_arg(struct runtime_command *cmd, char *arg) {
    if (arg != NULL) cupid_restore_escaped_ifs_placeholders(arg);
    char **next = realloc(cmd->argv, sizeof(*next) * ((size_t)cmd->argc + 2));
    if (next == NULL) return -1;
    cmd->argv = next;
    cmd->argv[cmd->argc] = arg;
    cmd->argc++;
    cmd->argv[cmd->argc] = NULL;
    return 0;
}

static int command_add_redir(struct runtime_command *cmd, struct runtime_redir *redir) {
    struct runtime_redir *next = realloc(cmd->redirs, sizeof(*next) * (cmd->redir_count + 1));
    if (next == NULL) return -1;
    cmd->redirs = next;
    cmd->redirs[cmd->redir_count] = *redir;
    cmd->redir_count++;
    return 0;
}

static int build_runtime_node_redirs(struct runtime_command *dst, const struct cupid_node *node, struct cupid_shell *shell) {
    size_t j;
    for (j = 0; j < node->redir_count; j++) {
        struct runtime_redir redir;
        memset(&redir, 0, sizeof(redir));
        redir.kind = node->redirs[j].kind;
        redir.fd = node->redirs[j].fd;
        if (node->redirs[j].fd_var != NULL) {
            redir.fd_var = strdup(node->redirs[j].fd_var);
            if (redir.fd_var == NULL) return -1;
        }
        redir.heredoc_fd = -1;
        redir.proc_subst_fd = -1;
        redir.heredoc_strip_tabs = node->redirs[j].heredoc_strip_tabs ? 1 : 0;
        if (node->redirs[j].kind == CUPID_REDIR_ERR_TO_OUT) {
            if (command_add_redir(dst, &redir) != 0) return -1;
            continue;
        }
        if (!node->redirs[j].has_target) return -1;
        if (node->redirs[j].kind == CUPID_REDIR_HEREDOC) {
            char *raw_delim = cupid_word_literal(&node->redirs[j].target);
            redir.heredoc_delim = cupid_word_dequote_literal(&node->redirs[j].target);
            if (raw_delim == NULL || redir.heredoc_delim == NULL) {
                free(raw_delim);
                free(redir.fd_var);
                free(redir.heredoc_delim);
                return -1;
            }
            redir.heredoc_quoted = (node->redirs[j].heredoc_quoted ||
                                    strcmp(raw_delim, redir.heredoc_delim) != 0) ? 1 : 0;
            if (node->redirs[j].heredoc_body != NULL) {
                redir.heredoc_body = strdup(node->redirs[j].heredoc_body);
                if (redir.heredoc_body == NULL) {
                    free(raw_delim);
                    free(redir.fd_var);
                    free(redir.heredoc_delim);
                    return -1;
                }
            }
            free(raw_delim);
        } else {
            redir.target_source = cupid_word_source_text(&node->redirs[j].target);
            if (redir.target_source == NULL) {
                free(redir.fd_var);
                return -1;
            }
            if ((redir.target_source[0] == '<' || redir.target_source[0] == '>') &&
                redir.target_source[1] == '(') {
                if (shell->mode == CUPID_MODE_POSIX) {
                    (void)cupid_expand_error_set("syntax error");
                    free(redir.fd_var);
                    free(redir.target_source);
                    return -1;
                }
                redir.target = strdup(redir.target_source);
                if (redir.target == NULL) {
                    free(redir.fd_var);
                    free(redir.target_source);
                    return -1;
                }
                if (spawn_proc_subst_fd(redir.target, shell, &redir.proc_subst_fd) != 0) {
                    free(redir.fd_var);
                    free(redir.target);
                    free(redir.target_source);
                    return -1;
                }
            } else {
                redir.target = cupid_expand_word(&node->redirs[j].target, shell);
                if (redir.target == NULL) {
                    free(redir.fd_var);
                    free(redir.target_source);
                    return -1;
                }
                if ((redir.target[0] == '<' || redir.target[0] == '>') &&
                    redir.target[1] == '(') {
                    if (shell->mode == CUPID_MODE_POSIX) {
                        (void)cupid_expand_error_set("syntax error");
                        free(redir.fd_var);
                        free(redir.target);
                        free(redir.target_source);
                        return -1;
                    }
                    if (spawn_proc_subst_fd(redir.target, shell, &redir.proc_subst_fd) != 0) {
                        free(redir.fd_var);
                        free(redir.target);
                        free(redir.target_source);
                        return -1;
                    }
                }
            }
        }
        if (command_add_redir(dst, &redir) != 0) {
            free(redir.fd_var);
            free(redir.target);
            free(redir.target_source);
            free(redir.heredoc_delim);
            free(redir.heredoc_body);
            return -1;
        }
    }
    return 0;
}

static int build_runtime_pipeline(struct runtime_pipeline *out, const struct cupid_pipeline_ast *pipeline, struct cupid_shell *shell) {
    size_t i;
    cupid_expand_error_reset();
    memset(out, 0, sizeof(*out));
    out->commands = calloc(pipeline->count, sizeof(*out->commands));
    if (out->commands == NULL) return -1;
    out->count = pipeline->count;

    for (i = 0; i < pipeline->count; i++) {
        size_t j;
        const struct cupid_node *node = &pipeline->commands[i];
        struct runtime_command *dst = &out->commands[i];
        struct expand_assignment_restore *expand_restores = NULL;
        size_t expand_restore_count = 0;
        int prefix_only = 1;
        int build_failed = 0;

        if (node->kind != NODE_SIMPLE_CMD) {
            runtime_pipeline_free(out);
            return -1;
        }

        shell->expand_cmdsub_seen = 0;
        shell->expand_cmdsub_status = 0;

        for (j = 0; j < node->simple_cmd.argc; j++) {
            int saw_non_assignment = 0;
            size_t argc_before = (size_t)dst->argc;
            size_t k;
            int word_can_be_assignment = word_is_assignment_candidate(&node->simple_cmd.argv[j]);

            if (prefix_only && !word_can_be_assignment) {
                restore_expand_assignments(shell, expand_restores, expand_restore_count);
                expand_restores = NULL;
                expand_restore_count = 0;
                prefix_only = 0;
            }
            if (add_expanded_word(dst, &node->simple_cmd.argv[j], shell,
                                  shell->mode != CUPID_MODE_POSIX || prefix_only) != 0) {
                build_failed = 1;
                break;
            }
            if (!prefix_only) continue;
            for (k = argc_before; k < (size_t)dst->argc; k++) {
                const char *name = NULL;
                const char *value = NULL;
                size_t name_len = 0;
                char *key;
                if (!split_assignment_word(dst->argv[k], &name, &name_len, &value)) {
                    saw_non_assignment = 1;
                    break;
                }
                if (node->simple_cmd.argc == 1) {
                    continue;
                }
                if (value[0] == '(') {
                    continue;
                }
                key = calloc(name_len + 1, 1);
                if (key == NULL) {
                    build_failed = 1;
                    break;
                }
                memcpy(key, name, name_len);
                if (apply_expand_assignment_restore(shell, &expand_restores, &expand_restore_count,
                                                    key, value) != 0) {
                    free(key);
                    build_failed = 1;
                    break;
                }
                free(key);
            }
            if (build_failed) break;
            if (saw_non_assignment) {
                prefix_only = 0;
            }
        }
        if (!build_failed && build_runtime_node_redirs(dst, node, shell) != 0) {
            build_failed = 1;
        }
        restore_expand_assignments(shell, expand_restores, expand_restore_count);
        if (build_failed) {
            runtime_pipeline_free(out);
            return -1;
        }
        dst->cmdsub_seen = shell->expand_cmdsub_seen;
        dst->cmdsub_status = shell->expand_cmdsub_status;
    }
    return 0;
}

static int prepare_heredocs(struct runtime_pipeline *pl, struct cupid_shell *shell) {
    size_t i;
    for (i = 0; i < pl->count; i++) {
        size_t j;
        struct runtime_command *cmd = &pl->commands[i];
        for (j = 0; j < cmd->redir_count; j++) {
            struct runtime_redir *redir = &cmd->redirs[j];
            if (redir->kind != CUPID_REDIR_HEREDOC) continue;
            redir->heredoc_fd = cupid_make_heredoc_fd(redir->heredoc_delim,
                                                      redir->heredoc_body,
                                                      redir->heredoc_quoted != 0,
                                                      redir->heredoc_strip_tabs != 0,
                                                      shell);
            if (redir->heredoc_fd < 0) return -1;
        }
    }
    return 0;
}

static int parse_dup_redir_target(const char *target, int *src_fd_out, int *close_src_out) {
    size_t len;
    char *end = NULL;
    long srcfd;
    if (src_fd_out == NULL || close_src_out == NULL || target == NULL) return -1;
    *src_fd_out = -1;
    *close_src_out = 0;
    len = strlen(target);
    if (len > 0 && target[len - 1] == '-') {
        char *tmp = calloc(len, 1);
        if (tmp == NULL) return -1;
        memcpy(tmp, target, len - 1);
        srcfd = strtol(tmp, &end, 10);
        if (end == tmp || *end != '\0' || srcfd < 0) {
            free(tmp);
            return -1;
        }
        free(tmp);
        *src_fd_out = (int)srcfd;
        *close_src_out = 1;
        return 0;
    }
    srcfd = strtol(target, &end, 10);
    if (end == target || *end != '\0' || srcfd < 0) return -1;
    *src_fd_out = (int)srcfd;
    return 0;
}

static int parse_varredir_name_ref(const char *name, char **base_out, char **index_out) {
    const char *lb;
    size_t base_len;
    size_t idx_with_rb_len;
    size_t idx_content_len;
    char *base;
    char *idx;
    if (base_out != NULL) *base_out = NULL;
    if (index_out != NULL) *index_out = NULL;
    if (name == NULL || name[0] == '\0') return 0;
    lb = strchr(name, '[');
    if (lb == NULL || name[strlen(name) - 1] != ']') return 0;
    if (strchr(lb + 1, '[') != NULL) return 0;
    base_len = (size_t)(lb - name);
    idx_with_rb_len = strlen(lb + 1);
    if (base_len == 0 || idx_with_rb_len < 2) return 0;
    idx_content_len = idx_with_rb_len - 1;
    base = calloc(base_len + 1, 1);
    idx = calloc(idx_content_len + 1, 1);
    if (base == NULL || idx == NULL) {
        free(base);
        free(idx);
        return -1;
    }
    memcpy(base, name, base_len);
    memcpy(idx, lb + 1, idx_content_len);
    if (idx[0] == '\0') {
        free(base);
        free(idx);
        return 0;
    }
    if (base_out != NULL) *base_out = base;
    else free(base);
    if (index_out != NULL) *index_out = idx;
    else free(idx);
    return 1;
}

static int resolve_varredir_fd(struct cupid_shell *shell, const char *name, int *fd_out) {
    const char *value = NULL;
    char *base = NULL;
    char *index = NULL;
    char *end = NULL;
    long fd;
    int ref_kind;
    if (shell == NULL || name == NULL || fd_out == NULL) return -1;
    ref_kind = parse_varredir_name_ref(name, &base, &index);
    if (ref_kind < 0) return -1;
    if (ref_kind > 0) {
        char *idx_end = NULL;
        long idx_val = strtol(index, &idx_end, 10);
        if (idx_end != index && *idx_end == '\0' && idx_val >= 0) {
            value = cupid_array_get_index(shell, base, (size_t)idx_val);
            if (value != NULL && value[0] == '\0') value = NULL;
        } else {
            value = cupid_array_get_key(shell, base, index);
            if (value != NULL && value[0] == '\0') value = NULL;
        }
    } else {
        value = cupid_vars_get(shell, name);
    }
    if (value == NULL || value[0] == '\0') {
        free(base);
        free(index);
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "%s: ambiguous redirect\n", name);
        return -1;
    }
    fd = strtol(value, &end, 10);
    if (end == value || *end != '\0' || fd < 0) {
        free(base);
        free(index);
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "%s: ambiguous redirect\n", name);
        return -1;
    }
    free(base);
    free(index);
    *fd_out = (int)fd;
    return 0;
}

static int assign_varredir_fd(struct cupid_shell *shell, const char *name, int fd) {
    char buf[32];
    int n;
    char *base = NULL;
    char *index = NULL;
    int ref_kind;
    if (shell == NULL || name == NULL) return -1;
    n = snprintf(buf, sizeof(buf), "%d", fd);
    if (n < 0 || n >= (int)sizeof(buf)) return -1;
    ref_kind = parse_varredir_name_ref(name, &base, &index);
    if (ref_kind < 0) return -1;
    if (ref_kind > 0) {
        char *idx_end = NULL;
        long idx_val = strtol(index, &idx_end, 10);
        int rc;
        if (idx_end != index && *idx_end == '\0' && idx_val >= 0) {
            rc = cupid_array_set_index(shell, base, (size_t)idx_val, buf);
        } else {
            rc = cupid_array_set_key(shell, base, index, buf);
        }
        free(base);
        free(index);
        if (rc != 0) {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "%s: cannot assign fd to variable\n", name);
            return -1;
        }
        return 0;
    }
    if (cupid_vars_set(shell, name, buf) != 0) {
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "%s: cannot assign fd to variable\n", name);
        return -1;
    }
    return 0;
}

static int ensure_fd_at_least_10(int fd) {
    int dupfd;
    if (fd < 0) return -1;
    if (fd >= 10) return fd;
    dupfd = fcntl(fd, F_DUPFD, 10);
    close(fd);
    return dupfd;
}

static int should_close_varredir_now(const struct runtime_command *cmd, const struct cupid_shell *shell) {
    size_t assign_prefix;
    int cmd_argc;
    char **cmd_argv;
    if (shell == NULL || cmd == NULL || !shell->opt_varredir_close) return 0;
    assign_prefix = command_assignment_prefix_count(cmd);
    cmd_argc = cmd->argc - (int)assign_prefix;
    cmd_argv = cmd->argv + assign_prefix;
    if (cmd_argc > 0 && strcmp(cmd_argv[0], "exec") == 0) return 0;
    return 1;
}

static int assign_varredir_fd_runtime(struct cupid_shell *shell, const char *name,
                                      int fd, int close_now) {
    if (assign_varredir_fd(shell, name, fd) != 0) {
        close(fd);
        return -1;
    }
    if (close_now) {
        if (close(fd) != 0 && errno != EBADF) return -1;
    }
    return 0;
}

static const char *redir_display_target(const struct runtime_redir *r) {
    if (r == NULL) return NULL;
    if (r->target_source != NULL && r->target_source[0] != '\0') return r->target_source;
    if (r->target != NULL && r->target[0] != '\0') return r->target;
    return NULL;
}

static int redir_fail_errno(struct cupid_shell *shell, const struct runtime_redir *r,
                            const char *detail) {
    const char *target = redir_display_target(r);
    int err = errno;
    if (detail != NULL && detail[0] != '\0') {
        if (shell != NULL && shell->current_file != NULL) {
            fprintf(stderr, "%s: redirection error: %s: %s\n",
                    shell->current_file, detail, strerror(err));
        } else {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "redirection error: %s: %s\n", detail, strerror(err));
        }
    }
    cupid_shell_error_prefix(stderr, shell);
    if (target != NULL) fprintf(stderr, "%s: %s\n", target, strerror(err));
    else fprintf(stderr, "%s\n", strerror(err));
    errno = err;
    return -1;
}

static int apply_redirections(struct runtime_command *cmd, struct cupid_shell *shell, int close_varredir) {
    size_t i;
    for (i = 0; i < cmd->redir_count; i++) {
        int fd = -1;
        struct runtime_redir *r = &cmd->redirs[i];
        int fd_var_mode = (r->fd_var != NULL && r->fd_var[0] != '\0');
        if (r->proc_subst_fd >= 0) {
            if (fd_var_mode) {
                fd = ensure_fd_at_least_10(r->proc_subst_fd);
                r->proc_subst_fd = -1;
                if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
                if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
                continue;
            }
            if (dup2(r->proc_subst_fd, r->fd) < 0) return redir_fail_errno(shell, r, NULL);
            close(r->proc_subst_fd);
            r->proc_subst_fd = -1;
            continue;
        }
        if (r->kind == CUPID_REDIR_ERR_TO_OUT) {
            if (fd_var_mode) {
                fd = fcntl(STDOUT_FILENO, F_DUPFD, 10);
                if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
                if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
                continue;
            }
            if (dup2(STDOUT_FILENO, r->fd) < 0) return redir_fail_errno(shell, r, NULL);
            continue;
        }
        if (r->kind == CUPID_REDIR_DUP_OUT || r->kind == CUPID_REDIR_DUP_IN) {
            int srcfd;
            int close_src = 0;
            if (r->target == NULL) return -1;
            if (strcmp(r->target, "-") == 0) {
                if (fd_var_mode) {
                    if (resolve_varredir_fd(shell, r->fd_var, &srcfd) != 0) return -1;
                    if (close(srcfd) != 0 && errno != EBADF) return -1;
                    continue;
                }
                if (close(r->fd) != 0) return redir_fail_errno(shell, r, NULL);
                continue;
            }
            if (parse_dup_redir_target(r->target, &srcfd, &close_src) != 0) {
                if (errno == 0) errno = EINVAL;
                return redir_fail_errno(shell, r, NULL);
            }
            if (fd_var_mode) {
                fd = fcntl(srcfd, F_DUPFD, 10);
                if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
                if (close_src) (void)close(srcfd);
                if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
                continue;
            }
            if (dup2(srcfd, r->fd) < 0) return redir_fail_errno(shell, r, NULL);
            if (close_src) {
                if (close(srcfd) != 0) return redir_fail_errno(shell, r, NULL);
                continue;
            }
            continue;
        }
        if (r->kind == CUPID_REDIR_HEREDOC) {
            if (r->heredoc_fd < 0) return -1;
            if (fd_var_mode) {
                fd = ensure_fd_at_least_10(r->heredoc_fd);
                r->heredoc_fd = -1;
                if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
                if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
                continue;
            }
            if (dup2(r->heredoc_fd, r->fd) < 0) return redir_fail_errno(shell, r, NULL);
            continue;
        }
        if (r->kind == CUPID_REDIR_HERESTRING) {
            int hs_fds[2];
            size_t hs_len;
            if (pipe(hs_fds) != 0) return -1;
            hs_len = strlen(r->target);
            if (hs_len > 0) {
                ssize_t nw = write(hs_fds[1], r->target, hs_len);
                (void)nw;
            }
            {
                ssize_t nw = write(hs_fds[1], "\n", 1);
                (void)nw;
            }
            close(hs_fds[1]);
            if (fd_var_mode) {
                fd = ensure_fd_at_least_10(hs_fds[0]);
                if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
                if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
                continue;
            }
            if (dup2(hs_fds[0], r->fd) < 0) { close(hs_fds[0]); return redir_fail_errno(shell, r, NULL); }
            close(hs_fds[0]);
            continue;
        }
        if (r->kind == CUPID_REDIR_IN) fd = open(r->target, O_RDONLY);
        else if (r->kind == CUPID_REDIR_INOUT) fd = open(r->target, O_RDWR | O_CREAT, 0666);
        else if (r->kind == CUPID_REDIR_OUT) fd = open(r->target, O_WRONLY | O_CREAT | O_TRUNC, 0666);
        else if (r->kind == CUPID_REDIR_APPEND) fd = open(r->target, O_WRONLY | O_CREAT | O_APPEND, 0666);
        else if (r->kind == CUPID_REDIR_CLOBBER) fd = open(r->target, O_WRONLY | O_CREAT | O_TRUNC, 0666);
        else if (r->kind == CUPID_REDIR_ERR_OUT) fd = open(r->target, O_WRONLY | O_CREAT | O_TRUNC, 0666);
        if (fd < 0) return redir_fail_errno(shell, r, NULL);
        if (fd_var_mode) {
            fd = ensure_fd_at_least_10(fd);
            if (fd < 0) return redir_fail_errno(shell, r, "cannot duplicate fd");
            if (assign_varredir_fd_runtime(shell, r->fd_var, fd, close_varredir) != 0) return -1;
            continue;
        }
        if (dup2(fd, r->fd) < 0) { close(fd); return redir_fail_errno(shell, r, NULL); }
        close(fd);
    }
    return 0;
}

static void exec_runtime_command_child(struct cupid_shell *shell, struct runtime_command *cmd) {
    size_t assign_prefix = 0;
    int status;
    int cmd_argc;
    char **cmd_argv;

    if (apply_redirections(cmd, shell, should_close_varredir_now(cmd, shell)) != 0) _exit(1);
    assign_prefix = command_assignment_prefix_count(cmd);
    if (assign_prefix > 0) {
        if (apply_prefix_assignments_env(shell, cmd, assign_prefix) != 0) _exit(1);
        if (apply_prefix_assignments(shell, cmd, assign_prefix, 0) != 0) _exit(1);
    }
    cmd_argc = cmd->argc - (int)assign_prefix;
    cmd_argv = cmd->argv + assign_prefix;
    if (cmd_argc == 0) _exit(0);
    if (cmd_argc > 0) {
        char *subscript = NULL;
        int subshell_kind = parse_subshell_script(cmd_argv[0], &subscript);
        if (subshell_kind < 0) _exit(1);
        if (subshell_kind > 0) {
            struct cupid_shell child_shell;
            int rc;
            if (cmd_argc != 1) { free(subscript); _exit(2); }
            cupid_shell_init(&child_shell);
            child_shell.shell_pid = shell->shell_pid;
            child_shell.last_status = shell->last_status;
            rc = cupid_shell_eval_line(&child_shell, subscript, 1);
            free(subscript);
            cupid_shell_run_exit_trap(&child_shell);
            if (child_shell.should_exit) rc = child_shell.exit_code;
            cupid_shell_destroy(&child_shell);
            fflush(NULL);
            _exit(rc & 0xff);
        }
    }
    status = cupid_run_builtin(shell, cmd_argc, cmd_argv, true);
    if (status != CUPID_BUILTIN_NOT_FOUND) {
        fflush(NULL);
        _exit(status);
    }
    {
        struct runtime_command cmd_view = *cmd;
        struct cupid_list_ast *func_body = cupid_func_get(shell, cmd_argv[0]);
        cmd_view.argc = cmd_argc;
        cmd_view.argv = cmd_argv;
        if (func_body != NULL) {
            int rc = exec_func_call(shell, func_body, &cmd_view);
            fflush(NULL);
            _exit(rc & 0xff);
        }
    }
    {
        struct runtime_command cmd_view = *cmd;
        int script_rc;
        cmd_view.argc = cmd_argc;
        cmd_view.argv = cmd_argv;
        script_rc = maybe_run_script_with_cupid(shell, &cmd_view);
        if (script_rc >= 0) {
            fflush(NULL);
            _exit(script_rc & 0xff);
        }
    }
    execvp(cmd_argv[0], cmd_argv);
    {
        int not_found = (errno == ENOENT) ||
                        (errno == EACCES && strchr(cmd_argv[0], '/') == NULL);
        if (not_found) {
            cupid_shell_error_prefix(stderr, shell);
            fprintf(stderr, "%s: command not found\n", cmd_argv[0]);
            _exit(127);
        }
        cupid_shell_error_prefix(stderr, shell);
        fprintf(stderr, "%s: %s\n", cmd_argv[0], strerror(errno));
        _exit(126);
    }
}

static int exec_runtime_command(struct cupid_shell *shell, struct runtime_command *cmd) {
    size_t assign_prefix;
    int cmd_argc;
    char **cmd_argv;
    int status = 1;
    int run_in_shell = 0;
    int subshell_kind = 0;
    char *subscript = NULL;
    struct cupid_list_ast *func_body = NULL;

    if (shell == NULL || cmd == NULL) return 1;

    assign_prefix = command_assignment_prefix_count(cmd);
    cmd_argc = cmd->argc - (int)assign_prefix;
    cmd_argv = cmd->argv + assign_prefix;

    if (cmd_argc == 0) {
        run_in_shell = 1;
    } else if (cupid_is_builtin(cmd_argv[0])) {
        run_in_shell = 1;
    } else if ((func_body = cupid_func_get(shell, cmd_argv[0])) != NULL) {
        run_in_shell = 1;
    } else {
        enum cupid_mode mode = shell->mode;
        subshell_kind = parse_subshell_script(cmd_argv[0], &subscript);
        if (subshell_kind < 0) return 1;
        if (subshell_kind > 0 && cmd_argc == 1) {
            run_in_shell = 1;
        } else if (path_is_shell_script(cmd_argv[0], &mode)) {
            run_in_shell = 1;
        }
    }

    if (!run_in_shell) {
        pid_t pid = fork();
        int st;
        if (pid < 0) return 1;
        if (pid == 0) {
            exec_runtime_command_child(shell, cmd);
        }
        if (waitpid(pid, &st, 0) < 0) return 1;
        return status_from_wait_status(st);
    }

    {
        int saved_fds[3] = {-1, -1, -1};
        int redir_ok = 0;
        int entered_scope = 0;
        int persist_assign = 0;
        int preserve_std_fds = 1;
        struct temp_env_assignment *temp_env = NULL;
        size_t temp_env_count = 0;

        if (cmd_argc > 0 && strcmp(cmd_argv[0], "exec") == 0) {
            preserve_std_fds = 0;
        }
        if (cmd->redir_count > 0) {
            if (preserve_std_fds) {
                saved_fds[0] = dup(STDIN_FILENO);
                saved_fds[1] = dup(STDOUT_FILENO);
                saved_fds[2] = dup(STDERR_FILENO);
                redir_ok = (saved_fds[0] >= 0 && saved_fds[1] >= 0 && saved_fds[2] >= 0 &&
                            apply_redirections(cmd, shell,
                                               should_close_varredir_now(cmd, shell)) == 0) ? 1 : 0;
            } else {
                redir_ok = (apply_redirections(cmd, shell,
                                               should_close_varredir_now(cmd, shell)) == 0) ? 1 : 0;
            }
        } else {
            redir_ok = 1;
        }
        if (!redir_ok) {
            status = 1;
            goto done;
        }

        if (cmd_argc == 0) {
            status = (apply_prefix_assignments(shell, cmd, assign_prefix, 1) == 0) ? 0 : 1;
            goto done;
        }

        if (cupid_is_builtin(cmd_argv[0])) {
            if (assign_prefix > 0) {
                persist_assign = (shell->mode == CUPID_MODE_POSIX &&
                                  is_posix_special_builtin_name(cmd_argv[0])) ? 1 : 0;
                if (!persist_assign &&
                    !builtin_prefix_assignments_use_shell_scope(cmd_argv[0])) {
                    if (apply_prefix_assignments_temp_env(shell, cmd, assign_prefix,
                                                          strcmp(cmd_argv[0], "command") == 0,
                                                          &temp_env, &temp_env_count) != 0) {
                        status = 1;
                    } else if (strcmp(cmd_argv[0], "exec") == 0 &&
                               apply_prefix_assignments_env(shell, cmd, assign_prefix) != 0) {
                        status = 1;
                    } else {
                        status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
                    }
                } else {
                    cupid_vars_scope_enter(shell);
                    entered_scope = 1;
                    if (strcmp(cmd_argv[0], "exec") == 0 &&
                        apply_prefix_assignments_env(shell, cmd, assign_prefix) != 0) {
                        status = 1;
                    } else if (apply_prefix_assignments(shell, cmd, assign_prefix,
                                                        persist_assign ? 0 : 1) != 0) {
                        status = 1;
                    } else {
                        status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
                    }
                }
            } else {
                status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
            }
            temp_env_assignments_restore(shell, temp_env, temp_env_count);
            temp_env = NULL;
            temp_env_count = 0;
            if (entered_scope) {
                cupid_vars_scope_leave(shell);
                entered_scope = 0;
            }
            goto done;
        }

        if (func_body != NULL) {
            struct runtime_command cmd_view = *cmd;
            cmd_view.argc = cmd_argc;
            cmd_view.argv = cmd_argv;
            if (assign_prefix > 0) {
                cupid_vars_scope_enter(shell);
                entered_scope = 1;
                if (apply_prefix_assignments(shell, cmd, assign_prefix, 1) != 0) {
                    status = 1;
                } else {
                    status = exec_func_call(shell, func_body, &cmd_view);
                }
            } else {
                status = exec_func_call(shell, func_body, &cmd_view);
            }
            if (entered_scope) {
                cupid_vars_scope_leave(shell);
                entered_scope = 0;
            }
            goto done;
        }

        if (subshell_kind > 0) {
            struct cupid_shell child_shell;
            int rc;
            if (cmd_argc != 1) {
                status = 2;
                goto done;
            }
            cupid_shell_init(&child_shell);
            child_shell.shell_pid = shell->shell_pid;
            child_shell.last_status = shell->last_status;
            rc = cupid_shell_eval_line(&child_shell, subscript, 1);
            cupid_shell_run_exit_trap(&child_shell);
            if (child_shell.should_exit) rc = child_shell.exit_code;
            cupid_shell_destroy(&child_shell);
            status = rc & 0xff;
            goto done;
        }

        {
            struct runtime_command cmd_view = *cmd;
            int script_rc;
            cmd_view.argc = cmd_argc;
            cmd_view.argv = cmd_argv;
            script_rc = maybe_run_script_with_cupid(shell, &cmd_view);
            if (script_rc >= 0) {
                status = script_rc & 0xff;
            } else {
                status = 127;
            }
        }

done:
        temp_env_assignments_restore(shell, temp_env, temp_env_count);
        if (entered_scope) {
            cupid_vars_scope_leave(shell);
        }
        if (preserve_std_fds && saved_fds[0] >= 0) {
            (void)dup2(saved_fds[0], STDIN_FILENO);
            close(saved_fds[0]);
            (void)dup2(saved_fds[1], STDOUT_FILENO);
            close(saved_fds[1]);
            (void)dup2(saved_fds[2], STDERR_FILENO);
            close(saved_fds[2]);
        }
    }

    free(subscript);
    return status;
}

static int pipeline_has_non_simple(const struct cupid_pipeline_ast *pipeline) {
    size_t i;
    if (pipeline == NULL) return 0;
    for (i = 0; i < pipeline->count; i++) {
        if (pipeline->commands[i].kind != NODE_SIMPLE_CMD) return 1;
    }
    return 0;
}

static int pipeline_use_lastpipe(const struct cupid_shell *shell, size_t count) {
    if (shell == NULL || count < 2) return 0;
    if (!shell->opt_lastpipe) return 0;
    if (shell->is_interactive) return 0;
    if (shell->opt_monitor) return 0;
    return 1;
}

/* sentinel for "stdin was closed before lastpipe rewired it" */
#define SAVED_STDIN_CLOSED (-2)

static int capture_lastpipe_stdin(int *saved_stdin) {
    int fd;
    if (saved_stdin == NULL) return -1;
    errno = 0;
    fd = dup(STDIN_FILENO);
    if (fd >= 0) {
        *saved_stdin = fd;
        return 0;
    }
    if (errno == EBADF) {
        *saved_stdin = SAVED_STDIN_CLOSED;
        return 0;
    }
    return -1;
}

static void restore_lastpipe_stdin(int saved_stdin) {
    if (saved_stdin >= 0) {
        (void)dup2(saved_stdin, STDIN_FILENO);
        close(saved_stdin);
    } else if (saved_stdin == SAVED_STDIN_CLOSED) {
        close(STDIN_FILENO);
    }
}

static int execute_pipeline_node_current(struct cupid_shell *shell, const struct cupid_node *node) {
    if (shell == NULL || node == NULL) return 1;
    if (node->kind == NODE_SIMPLE_CMD) {
        struct cupid_pipeline_ast one = {0};
        struct runtime_pipeline runtime = {0};
        int rc;
        one.commands = (struct cupid_node *)node;
        one.count = 1;
        if (build_runtime_pipeline(&runtime, &one, shell) != 0) return 1;
        if (prepare_heredocs(&runtime, shell) != 0) {
            runtime_pipeline_free(&runtime);
            return 1;
        }
        rc = exec_runtime_command(shell, &runtime.commands[0]);
        runtime_pipeline_free(&runtime);
        return rc;
    }
    if (node->kind == NODE_SUBSHELL && node->redir_count == 0) {
        return execute_list(shell, node->subshell);
    }
    if (node->kind == NODE_BRACE_GROUP && node->redir_count == 0) {
        return execute_list(shell, node->brace_group);
    }
    if (node->redir_count > 0) {
        return execute_compound_with_redirs(shell, node);
    }
    return exec_compound_node(shell, node);
}

static int run_ast_pipeline(struct cupid_shell *shell, const struct cupid_pipeline_ast *pipeline) {
    int *pipefds = NULL;
    pid_t *pids = NULL;
    int *statuses = NULL;
    int status = 0;
    int use_lastpipe;
    int stdin_was_closed = 0;
    struct sigaction old_int, ign_int;
    size_t i;

    memset(&ign_int, 0, sizeof(ign_int));
    ign_int.sa_handler = SIG_IGN;
    sigemptyset(&ign_int.sa_mask);
    sigaction(SIGINT, &ign_int, &old_int);
    use_lastpipe = pipeline_use_lastpipe(shell, pipeline->count);
    if (use_lastpipe) {
        errno = 0;
        if (fcntl(STDIN_FILENO, F_GETFD) < 0 && errno == EBADF) stdin_was_closed = 1;
    }

    if (pipeline->count > 1) {
        pipefds = calloc((pipeline->count - 1) * 2, sizeof(*pipefds));
        if (pipefds == NULL) { sigaction(SIGINT, &old_int, NULL); return 1; }
        for (i = 0; i < pipeline->count - 1; i++) {
            if (pipe(&pipefds[i * 2]) != 0) {
                free(pipefds);
                sigaction(SIGINT, &old_int, NULL);
                return 1;
            }
        }
    }

    pids = calloc(pipeline->count, sizeof(*pids));
    statuses = calloc(pipeline->count, sizeof(*statuses));
    if (pids == NULL || statuses == NULL) {
        free(pipefds);
        free(pids);
        free(statuses);
        sigaction(SIGINT, &old_int, NULL);
        return 1;
    }
    for (i = 0; i < pipeline->count; i++) statuses[i] = 1;

    for (i = 0; i < (use_lastpipe ? pipeline->count - 1 : pipeline->count); i++) {
        pid_t pid = fork();
        if (pid < 0) { status = 1; break; }
        if (pid == 0) {
            size_t k;
            signal(SIGINT, SIG_DFL);
            if (pipeline->count > 1) {
                int keep_stdin = -1;
                int keep_stdout = -1;
                if (i > 0) {
                    if (dup2(pipefds[(i - 1) * 2], STDIN_FILENO) < 0) _exit(1);
                    keep_stdin = STDIN_FILENO;
                }
                if (i + 1 < pipeline->count) {
                    if (dup2(pipefds[i * 2 + 1], STDOUT_FILENO) < 0) _exit(1);
                    keep_stdout = STDOUT_FILENO;
                }
                for (k = 0; k < (pipeline->count - 1) * 2; k++) {
                    if (pipefds[k] == keep_stdin || pipefds[k] == keep_stdout) continue;
                    close(pipefds[k]);
                }
            }

            if (pipeline->commands[i].kind == NODE_SIMPLE_CMD) {
                struct cupid_pipeline_ast one = {0};
                struct runtime_pipeline runtime = {0};
                one.commands = (struct cupid_node *)&pipeline->commands[i];
                one.count = 1;
                if (build_runtime_pipeline(&runtime, &one, shell) != 0) _exit(1);
                if (prepare_heredocs(&runtime, shell) != 0) {
                    runtime_pipeline_free(&runtime);
                    _exit(1);
                }
                exec_runtime_command_child(shell, &runtime.commands[0]);
            }

            {
                int rc;
                const struct cupid_node *node = &pipeline->commands[i];
                if (node->kind == NODE_SUBSHELL && node->redir_count == 0) {
                    rc = execute_list(shell, node->subshell);
                } else if (node->kind == NODE_BRACE_GROUP && node->redir_count == 0) {
                    rc = execute_list(shell, node->brace_group);
                } else if (node->redir_count > 0) {
                    rc = execute_compound_with_redirs(shell, node);
                } else {
                    rc = exec_compound_node(shell, node);
                }
                cupid_shell_run_exit_trap(shell);
                if (shell->should_exit) rc = shell->exit_code;
                fflush(NULL);
                _exit(rc & 0xff);
            }
        }
        pids[i] = pid;
    }

    if (use_lastpipe) {
        int saved_stdin = -1;
        size_t last = pipeline->count - 1;
        int last_read_fd = -1;
        if (pipefds != NULL) {
            last_read_fd = pipefds[(pipeline->count - 2) * 2];
            if (capture_lastpipe_stdin(&saved_stdin) != 0) {
                status = 1;
            } else if (stdin_was_closed && saved_stdin >= 0) {
                close(saved_stdin);
                saved_stdin = SAVED_STDIN_CLOSED;
            }
            if (status == 0 && dup2(last_read_fd, STDIN_FILENO) < 0) {
                status = 1;
            }
            for (i = 0; i < (pipeline->count - 1) * 2; i++) {
                if (pipefds[i] >= 0 && pipefds[i] != STDIN_FILENO) close(pipefds[i]);
            }
        }
        if (status == 0) {
            statuses[last] = execute_pipeline_node_current(shell, &pipeline->commands[last]);
        } else {
            statuses[last] = 1;
        }
        restore_lastpipe_stdin(saved_stdin);
    } else if (pipefds != NULL) {
        for (i = 0; i < (pipeline->count - 1) * 2; i++) close(pipefds[i]);
    }
    for (i = 0; i < pipeline->count; i++) {
        int st = 0;
        if (pids[i] <= 0) continue;
        if (waitpid(pids[i], &st, 0) < 0) continue;
        statuses[i] = status_from_wait_status(st);
    }
    set_status_array(shell, "PIPESTATUS", statuses, pipeline->count);
    if (shell->opt_pipefail && pipeline->count > 1) {
        for (i = pipeline->count; i > 0; i--) {
            if (statuses[i - 1] != 0) {
                status = statuses[i - 1];
                break;
            }
        }
    } else {
        status = statuses[pipeline->count - 1];
    }
    free(pipefds);
    free(pids);
    free(statuses);
    sigaction(SIGINT, &old_int, NULL);
    return status;
}

static int run_pipeline(struct cupid_shell *shell, struct runtime_pipeline *pl) {
    int *pipefds = NULL;
    pid_t *pids = NULL;
    int *statuses = NULL;
    int status = 0;
    int use_lastpipe;
    int stdin_was_closed = 0;
    struct sigaction old_int, ign_int;
    size_t i;

    memset(&ign_int, 0, sizeof(ign_int));
    ign_int.sa_handler = SIG_IGN;
    sigemptyset(&ign_int.sa_mask);
    sigaction(SIGINT, &ign_int, &old_int);
    use_lastpipe = pipeline_use_lastpipe(shell, pl->count);
    if (use_lastpipe) {
        errno = 0;
        if (fcntl(STDIN_FILENO, F_GETFD) < 0 && errno == EBADF) stdin_was_closed = 1;
    }

    if (pl->count > 1) {
        pipefds = calloc((pl->count - 1) * 2, sizeof(*pipefds));
        if (pipefds == NULL) { sigaction(SIGINT, &old_int, NULL); return 1; }
        for (i = 0; i < pl->count - 1; i++) {
            if (pipe(&pipefds[i * 2]) != 0) { free(pipefds); sigaction(SIGINT, &old_int, NULL); return 1; }
        }
    }

    pids = calloc(pl->count, sizeof(*pids));
    statuses = calloc(pl->count, sizeof(*statuses));
    if (pids == NULL || statuses == NULL) {
        free(pipefds);
        free(pids);
        free(statuses);
        sigaction(SIGINT, &old_int, NULL);
        return 1;
    }
    for (i = 0; i < pl->count; i++) statuses[i] = 1;

    for (i = 0; i < (use_lastpipe ? pl->count - 1 : pl->count); i++) {
        pid_t pid = fork();
        if (pid < 0) { status = 1; break; }
        if (pid == 0) {
            size_t k;
            signal(SIGINT, SIG_DFL);
            if (pl->count > 1) {
                int keep_stdin = -1;
                int keep_stdout = -1;
                if (i > 0) {
                    if (dup2(pipefds[(i - 1) * 2], STDIN_FILENO) < 0) _exit(1);
                    keep_stdin = STDIN_FILENO;
                }
                if (i + 1 < pl->count) {
                    if (dup2(pipefds[i * 2 + 1], STDOUT_FILENO) < 0) _exit(1);
                    keep_stdout = STDOUT_FILENO;
                }
                for (k = 0; k < (pl->count - 1) * 2; k++) {
                    if (pipefds[k] == keep_stdin || pipefds[k] == keep_stdout) continue;
                    close(pipefds[k]);
                }
            }
            exec_runtime_command_child(shell, &pl->commands[i]);
        }
        pids[i] = pid;
    }

    if (use_lastpipe) {
        int saved_stdin = -1;
        size_t last = pl->count - 1;
        int last_read_fd = -1;
        if (pipefds != NULL) {
            last_read_fd = pipefds[(pl->count - 2) * 2];
            if (capture_lastpipe_stdin(&saved_stdin) != 0) {
                status = 1;
            } else if (stdin_was_closed && saved_stdin >= 0) {
                close(saved_stdin);
                saved_stdin = SAVED_STDIN_CLOSED;
            }
            if (status == 0 && dup2(last_read_fd, STDIN_FILENO) < 0) {
                status = 1;
            }
            for (i = 0; i < (pl->count - 1) * 2; i++) {
                if (pipefds[i] >= 0 && pipefds[i] != STDIN_FILENO) close(pipefds[i]);
            }
        }
        if (status == 0) {
            statuses[last] = exec_runtime_command(shell, &pl->commands[last]);
        } else {
            statuses[last] = 1;
        }
        restore_lastpipe_stdin(saved_stdin);
    } else if (pipefds != NULL) {
        for (i = 0; i < (pl->count - 1) * 2; i++) close(pipefds[i]);
    }
    for (i = 0; i < pl->count; i++) {
        int st = 0;
        if (pids[i] <= 0) continue;
        if (waitpid(pids[i], &st, 0) < 0) continue;
        statuses[i] = status_from_wait_status(st);
    }
    set_status_array(shell, "PIPESTATUS", statuses, pl->count);
    if (shell->opt_pipefail && pl->count > 1) {
        for (i = pl->count; i > 0; i--) {
            if (statuses[i - 1] != 0) {
                status = statuses[i - 1];
                break;
            }
        }
    } else {
        status = statuses[pl->count - 1];
    }
    free(pipefds);
    free(pids);
    free(statuses);
    sigaction(SIGINT, &old_int, NULL);
    return status;
}

static int cond_eval(struct cupid_shell *shell, char **words, size_t count, size_t *pos);

static char *cond_join_rhs(char **words, size_t start, size_t end) {
    size_t i;
    size_t total = 0;
    size_t off = 0;
    char *out;
    for (i = start; i < end; i++) total += strlen(words[i]);
    out = calloc(total + 1, 1);
    if (out == NULL) return NULL;
    for (i = start; i < end; i++) {
        size_t n = strlen(words[i]);
        memcpy(out + off, words[i], n);
        off += n;
    }
    return out;
}

static int cond_primary(struct cupid_shell *shell, char **words, size_t count, size_t *pos) {
    const char *w;
    if (*pos >= count) return 1;
    w = words[*pos];

    if (strcmp(w, "(") == 0) {
        int r;
        (*pos)++;
        r = cond_eval(shell, words, count, pos);
        if (*pos < count && strcmp(words[*pos], ")") == 0) (*pos)++;
        return r;
    }

    if (strcmp(w, "!") == 0) {
        (*pos)++;
        return cond_primary(shell, words, count, pos) == 0 ? 1 : 0;
    }

    if (w[0] == '-' && w[1] != '\0' && w[2] == '\0') {
        char flag = w[1];
        struct stat st;

        if (flag == 'z' || flag == 'n') {
            const char *arg;
            if (*pos + 1 >= count) return 1;
            arg = words[++(*pos)];
            (*pos)++;
            if (flag == 'z') return (arg[0] == '\0') ? 0 : 1;
            return (arg[0] != '\0') ? 0 : 1;
        }

        if (flag == 'f' || flag == 'd' || flag == 'e' ||
            flag == 'r' || flag == 'w' || flag == 'x' || flag == 's') {
            const char *path;
            if (*pos + 1 >= count) return 1;
            path = words[++(*pos)];
            (*pos)++;
            if (stat(path, &st) != 0) return 1;
            if (flag == 'e') return 0;
            if (flag == 'f') return S_ISREG(st.st_mode) ? 0 : 1;
            if (flag == 'd') return S_ISDIR(st.st_mode) ? 0 : 1;
            if (flag == 's') return (st.st_size > 0) ? 0 : 1;
            if (flag == 'r') return (access(path, R_OK) == 0) ? 0 : 1;
            if (flag == 'w') return (access(path, W_OK) == 0) ? 0 : 1;
            if (flag == 'x') return (access(path, X_OK) == 0) ? 0 : 1;
            return 1;
        }
    }

    if (*pos + 2 <= count) {
        const char *lhs = w;
        const char *op;
        size_t save = *pos;

        if (*pos + 1 < count) {
            op = words[*pos + 1];

            if (strcmp(op, "==") == 0 || strcmp(op, "!=") == 0 ||
                strcmp(op, "=~") == 0 ||
                strcmp(op, "-eq") == 0 || strcmp(op, "-ne") == 0 ||
                strcmp(op, "-lt") == 0 || strcmp(op, "-gt") == 0 ||
                strcmp(op, "-le") == 0 || strcmp(op, "-ge") == 0 ||
                strcmp(op, "<") == 0 || strcmp(op, ">") == 0) {

                const char *rhs;
                char *rhs_join = NULL;
                size_t rhs_end = *pos + 2;
                int paren_depth = 0;
                if (*pos + 2 >= count) {
                    *pos = save;
                    goto string_test;
                }
                while (rhs_end < count) {
                    if (strcmp(words[rhs_end], "(") == 0) {
                        paren_depth++;
                        rhs_end++;
                        continue;
                    }
                    if (strcmp(words[rhs_end], ")") == 0) {
                        if (paren_depth == 0) break;
                        paren_depth--;
                        rhs_end++;
                        continue;
                    }
                    if (paren_depth == 0 &&
                        (strcmp(words[rhs_end], "&&") == 0 || strcmp(words[rhs_end], "||") == 0)) {
                        break;
                    }
                    rhs_end++;
                }
                if (rhs_end == *pos + 2) {
                    *pos = save;
                    goto string_test;
                }
                rhs_join = cond_join_rhs(words, *pos + 2, rhs_end);
                if (rhs_join == NULL) return 1;
                rhs = rhs_join;
                *pos = rhs_end;

                if (strcmp(op, "==") == 0) {
                    int r = pattern_matches(shell, rhs, lhs) ? 0 : 1;
                    free(rhs_join);
                    return r;
                }
                if (strcmp(op, "!=") == 0) {
                    int r = pattern_matches(shell, rhs, lhs) ? 1 : 0;
                    free(rhs_join);
                    return r;
                }
                if (strcmp(op, "=~") == 0) {
                    regex_t re;
                    regmatch_t *match = NULL;
                    int rc;
                    size_t mcount;
                    if (regcomp(&re, rhs, REG_EXTENDED) != 0) { free(rhs_join); return 2; }
                    mcount = (size_t)re.re_nsub + 1;
                    if (mcount == 0) mcount = 1;
                    match = calloc(mcount, sizeof(*match));
                    if (match == NULL) {
                        regfree(&re);
                        free(rhs_join);
                        return 1;
                    }
                    rc = regexec(&re, lhs, mcount, match, 0);
                    if (rc == 0) {
                        char **items = calloc(mcount, sizeof(char *));
                        size_t mi;
                        if (items != NULL) {
                            for (mi = 0; mi < mcount; mi++) {
                                if (match[mi].rm_so >= 0 && match[mi].rm_eo >= match[mi].rm_so) {
                                    size_t lenm = (size_t)(match[mi].rm_eo - match[mi].rm_so);
                                    items[mi] = calloc(lenm + 1, 1);
                                    if (items[mi] != NULL && lenm > 0) {
                                        memcpy(items[mi], lhs + match[mi].rm_so, lenm);
                                    }
                                } else {
                                    items[mi] = strdup("");
                                }
                                if (items[mi] == NULL) {
                                    size_t mj;
                                    for (mj = 0; mj < mi; mj++) free(items[mj]);
                                    free(items);
                                    items = NULL;
                                    break;
                                }
                            }
                            if (items != NULL) {
                                (void)cupid_array_set_list(shell, "BASH_REMATCH", items, mcount);
                                for (mi = 0; mi < mcount; mi++) free(items[mi]);
                                free(items);
                            }
                        }
                    } else {
                        (void)cupid_array_set_list(shell, "BASH_REMATCH", NULL, 0);
                    }
                    free(match);
                    regfree(&re);
                    free(rhs_join);
                    return (rc == 0) ? 0 : 1;
                }
                if (strcmp(op, "<") == 0) {
                    int r = (strcmp(lhs, rhs) < 0) ? 0 : 1;
                    free(rhs_join);
                    return r;
                }
                if (strcmp(op, ">") == 0) {
                    int r = (strcmp(lhs, rhs) > 0) ? 0 : 1;
                    free(rhs_join);
                    return r;
                }
                {
                    long la = strtol(lhs, NULL, 10);
                    long ra = strtol(rhs, NULL, 10);
                    int r = 1;
                    if (strcmp(op, "-eq") == 0) r = (la == ra) ? 0 : 1;
                    else if (strcmp(op, "-ne") == 0) r = (la != ra) ? 0 : 1;
                    else if (strcmp(op, "-lt") == 0) r = (la < ra) ? 0 : 1;
                    else if (strcmp(op, "-gt") == 0) r = (la > ra) ? 0 : 1;
                    else if (strcmp(op, "-le") == 0) r = (la <= ra) ? 0 : 1;
                    else if (strcmp(op, "-ge") == 0) r = (la >= ra) ? 0 : 1;
                    free(rhs_join);
                    return r;
                }
                free(rhs_join);
                return 1;
            }
        }
    }

string_test:
    (*pos)++;
    return (w[0] != '\0') ? 0 : 1;
}

static int cond_and(struct cupid_shell *shell, char **words, size_t count, size_t *pos) {
    int r = cond_primary(shell, words, count, pos);
    while (*pos < count && strcmp(words[*pos], "&&") == 0) {
        (*pos)++;
        if (r != 0) {
            (void)cond_primary(shell, words, count, pos);
        } else {
            r = cond_primary(shell, words, count, pos);
        }
    }
    return r;
}

static int cond_eval(struct cupid_shell *shell, char **words, size_t count, size_t *pos) {
    int r = cond_and(shell, words, count, pos);
    while (*pos < count && strcmp(words[*pos], "||") == 0) {
        (*pos)++;
        if (r == 0) {
            (void)cond_and(shell, words, count, pos);
        } else {
            r = cond_and(shell, words, count, pos);
        }
    }
    return r;
}

static int exec_cond_expr(struct cupid_shell *shell, const struct cupid_cond_node *node) {
    char **words;
    size_t i, pos = 0;
    int result;

    if (node->word_count == 0) return 1;

    words = calloc(node->word_count, sizeof(char *));
    if (!words) return 1;

    for (i = 0; i < node->word_count; i++) {
        words[i] = cupid_expand_word(&node->words[i], shell);
        if (!words[i]) {
            size_t j;
            for (j = 0; j < i; j++) free(words[j]);
            free(words);
            return 1;
        }
    }

    result = cond_eval(shell, words, node->word_count, &pos);

    for (i = 0; i < node->word_count; i++) free(words[i]);
    free(words);
    return result;
}

static int exec_if(struct cupid_shell *shell, const struct cupid_if_node *node) {
    const struct cupid_if_node *cur = node;
    while (cur != NULL) {
        int cond;
        shell->in_condition++;
        cond = execute_list(shell, cur->condition);
        shell->in_condition--;
        if (cond == 0) {
            return execute_list(shell, cur->then_body);
        }
        if (cur->elif_next) {
            cur = cur->elif_next;
            continue;
        }
        if (cur->else_body) {
            return execute_list(shell, cur->else_body);
        }
        break;
    }
    return 0;
}

static int for_run_iteration(struct cupid_shell *shell, const struct cupid_for_node *node,
                             const char *val, int *status) {
    cupid_vars_set(shell, node->varname, val);
    setenv(node->varname, val, 1);
    shell->loop_depth++;
    *status = execute_list(shell, node->body);
    shell->loop_depth--;
    shell->last_status = *status;
    if (shell->break_count > 0) {
        shell->break_count--;
        return 1;
    }
    if (shell->continue_flag) {
        shell->continue_flag--;
        return shell->continue_flag > 0 ? 1 : 2;
    }
    return 0;
}

static int select_items_add(char ***items, size_t *count, const char *s) {
    char **next = realloc(*items, sizeof(**items) * (*count + 1));
    char *copy;
    if (next == NULL) return -1;
    copy = strdup(s ? s : "");
    if (copy == NULL) return -1;
    *items = next;
    (*items)[*count] = copy;
    (*count)++;
    return 0;
}

static void select_items_free(char **items, size_t count) {
    size_t i;
    for (i = 0; i < count; i++) free(items[i]);
    free(items);
}

static int collect_select_items(struct cupid_shell *shell, const struct cupid_for_node *node,
                                char ***out_items, size_t *out_count) {
    char **items = NULL;
    size_t count = 0;
    size_t i;

    if (!node->has_wordlist) {
        for (i = 0; i < shell->params.count; i++) {
            if (select_items_add(&items, &count, shell->params.args[i]) != 0) {
                select_items_free(items, count);
                return -1;
            }
        }
        *out_items = items;
        *out_count = count;
        return 0;
    }

    for (i = 0; i < node->word_count; i++) {
        char *expanded = cupid_expand_word(&node->words[i], shell);
        if (expanded == NULL) {
            select_items_free(items, count);
            return -1;
        }
        if (!node->words[i].had_quotes) {
            char **fields = NULL;
            size_t fc = 0;
            size_t fi;
            if (split_ifs_fields(shell, expanded, &fields, &fc) != 0) {
                free(expanded);
                select_items_free(items, count);
                return -1;
            }
            free(expanded);
            for (fi = 0; fi < fc; fi++) {
                if (!shell->opt_noglob && has_glob_meta(shell, fields[fi])) {
                    glob_t gl;
                    size_t gi;
                    int grc;
                    int gflags = shell->opt_nullglob ? 0 : GLOB_NOCHECK;
                    memset(&gl, 0, sizeof(gl));
                    grc = glob(fields[fi], gflags, NULL, &gl);
                    if (grc != 0) {
                        if (shell->opt_nullglob && grc == GLOB_NOMATCH) {
                            globfree(&gl);
                            free(fields[fi]);
                            continue;
                        }
                        size_t fj;
                        for (fj = fi; fj < fc; fj++) free(fields[fj]);
                        free(fields);
                        select_items_free(items, count);
                        return -1;
                    }
                    for (gi = 0; gi < gl.gl_pathc; gi++) {
                        if (select_items_add(&items, &count, gl.gl_pathv[gi]) != 0) {
                            size_t fj;
                            globfree(&gl);
                            for (fj = fi; fj < fc; fj++) free(fields[fj]);
                            free(fields);
                            select_items_free(items, count);
                            return -1;
                        }
                    }
                    globfree(&gl);
                } else if (select_items_add(&items, &count, fields[fi]) != 0) {
                    size_t fj;
                    for (fj = fi; fj < fc; fj++) free(fields[fj]);
                    free(fields);
                    select_items_free(items, count);
                    return -1;
                }
                free(fields[fi]);
            }
            free(fields);
        } else {
            if (select_items_add(&items, &count, expanded) != 0) {
                free(expanded);
                select_items_free(items, count);
                return -1;
            }
            free(expanded);
        }
    }

    *out_items = items;
    *out_count = count;
    return 0;
}

static int exec_select(struct cupid_shell *shell, const struct cupid_for_node *node) {
    char **items = NULL;
    size_t item_count = 0;
    char *line = NULL;
    size_t line_cap = 0;
    int status = 0;

    if (collect_select_items(shell, node, &items, &item_count) != 0) return 1;

    while (1) {
        const char *ps3 = cupid_vars_get(shell, "PS3");
        size_t i;
        ssize_t nread;
        char *end = NULL;
        long n = 0;
        const char *chosen = "";

        if (ps3 == NULL || ps3[0] == '\0') ps3 = "#? ";
        for (i = 0; i < item_count; i++) {
            fprintf(stderr, "%zu) %s\n", i + 1, items[i]);
        }
        fprintf(stderr, "%s", ps3);
        fflush(stderr);

        nread = getline(&line, &line_cap, stdin);
        if (nread < 0) {
            status = 1;
            break;
        }
        if (nread > 0 && line[nread - 1] == '\n') line[nread - 1] = '\0';
        cupid_vars_set(shell, "REPLY", line);

        if (line[0] != '\0') {
            n = strtol(line, &end, 10);
            if (end != line && *end == '\0' && n >= 1 && (size_t)n <= item_count) {
                chosen = items[(size_t)n - 1];
            }
        }
        cupid_vars_set(shell, node->varname, chosen);
        setenv(node->varname, chosen, 1);

        shell->loop_depth++;
        status = execute_list(shell, node->body);
        shell->loop_depth--;
        shell->last_status = status;
        if (shell->break_count > 0) {
            shell->break_count--;
            break;
        }
        if (shell->continue_flag) {
            shell->continue_flag--;
            if (shell->continue_flag > 0) break;
        }
        if (shell->return_flag || shell->should_exit) {
            break;
        }
    }

    free(line);
    select_items_free(items, item_count);
    return status;
}

static int exec_for(struct cupid_shell *shell, const struct cupid_for_node *node) {
    int status = 0;
    size_t i;

    if (node->is_select) {
        return exec_select(shell, node);
    }

    if (node->is_cstyle) {
        int err = 0;
        if (node->c_init != NULL && node->c_init[0] != '\0') {
            (void)cupid_arith_eval(shell, node->c_init, &err);
            if (err) return 1;
        }
        while (1) {
            long cond = 1;
            if (node->c_cond != NULL && node->c_cond[0] != '\0') {
                cond = cupid_arith_eval(shell, node->c_cond, &err);
                if (err) return 1;
            }
            if (cond == 0) break;

            shell->loop_depth++;
            status = execute_list(shell, node->body);
            shell->loop_depth--;
            shell->last_status = status;
            if (shell->break_count > 0) {
                shell->break_count--;
                break;
            }
            if (shell->continue_flag) {
                shell->continue_flag--;
                if (shell->continue_flag > 0) break;
            }
            if (shell->return_flag || shell->should_exit) {
                break;
            }
            if (node->c_step != NULL && node->c_step[0] != '\0') {
                (void)cupid_arith_eval(shell, node->c_step, &err);
                if (err) return 1;
            }
        }
        return status;
    }

    if (node->has_wordlist) {
        for (i = 0; i < node->word_count; i++) {
            struct runtime_command expanded_words;
            size_t ai;
            memset(&expanded_words, 0, sizeof(expanded_words));
            if (add_expanded_word(&expanded_words, &node->words[i], shell, 0) != 0) {
                runtime_command_free(&expanded_words);
                return 1;
            }
            for (ai = 0; ai < (size_t)expanded_words.argc; ai++) {
                int action = for_run_iteration(shell, node, expanded_words.argv[ai], &status);
                if (action == 1) {
                    runtime_command_free(&expanded_words);
                    return status;
                }
                if (action == 2) continue;
            }
            runtime_command_free(&expanded_words);
        }
    }
    return status;
}

static int exec_coproc(struct cupid_shell *shell, const struct cupid_coproc_node *node) {
    int in_pipe[2] = {-1, -1};
    int out_pipe[2] = {-1, -1};
    pid_t pid;
    char fd0[32], fd1[32], pidbuf[32], pid_name[256];
    const char *name = (node->name != NULL && node->name[0] != '\0') ? node->name : "COPROC";

    if (node->pipeline == NULL) return 1;
    if (pipe(in_pipe) != 0) return 1;
    if (pipe(out_pipe) != 0) {
        close(in_pipe[0]); close(in_pipe[1]);
        return 1;
    }

    pid = fork();
    if (pid < 0) {
        close(in_pipe[0]); close(in_pipe[1]);
        close(out_pipe[0]); close(out_pipe[1]);
        return 1;
    }
    if (pid == 0) {
        int rc = 1;
        signal(SIGINT, SIG_DFL);
        close(in_pipe[1]);
        close(out_pipe[0]);
        if (dup2(in_pipe[0], STDIN_FILENO) < 0) _exit(1);
        if (dup2(out_pipe[1], STDOUT_FILENO) < 0) _exit(1);
        close(in_pipe[0]);
        close(out_pipe[1]);
        if (node->pipeline->count == 1 && node->pipeline->commands[0].kind != NODE_SIMPLE_CMD) {
            const struct cupid_node *compound = &node->pipeline->commands[0];
            if (compound->redir_count > 0) {
                rc = execute_compound_with_redirs(shell, compound);
            } else {
                rc = exec_compound_node(shell, compound);
            }
        } else {
            struct runtime_pipeline pl = {0};
            if (build_runtime_pipeline(&pl, node->pipeline, shell) == 0 &&
                prepare_heredocs(&pl, shell) == 0) {
                rc = run_pipeline(shell, &pl);
            }
            runtime_pipeline_free(&pl);
        }
        cupid_shell_run_exit_trap(shell);
        if (shell->should_exit) rc = shell->exit_code;
        fflush(NULL);
        _exit(rc & 0xff);
    }

    close(in_pipe[0]);
    close(out_pipe[1]);
    snprintf(fd0, sizeof(fd0), "%d", out_pipe[0]);
    snprintf(fd1, sizeof(fd1), "%d", in_pipe[1]);
    if (cupid_array_set_index(shell, name, 0, fd0) != 0 ||
        cupid_array_set_index(shell, name, 1, fd1) != 0) {
        close(out_pipe[0]);
        close(in_pipe[1]);
        return 1;
    }
    snprintf(pidbuf, sizeof(pidbuf), "%ld", (long)pid);
    snprintf(pid_name, sizeof(pid_name), "%s_PID", name);
    cupid_vars_set(shell, pid_name, pidbuf);
    shell->last_bg_pid = pid;
    return 0;
}

static int exec_while(struct cupid_shell *shell, const struct cupid_while_node *node, int is_until) {
    int status = 0;
    while (1) {
        int cond;
        shell->in_condition++;
        cond = execute_list(shell, node->condition);
        shell->in_condition--;
        if (is_until ? (cond == 0) : (cond != 0)) break;
        shell->loop_depth++;
        status = execute_list(shell, node->body);
        shell->loop_depth--;
        shell->last_status = status;
        if (shell->break_count > 0) {
            shell->break_count--;
            break;
        }
        if (shell->continue_flag) {
            shell->continue_flag--;
            if (shell->continue_flag > 0) break;
        }
    }
    return status;
}

static int exec_case(struct cupid_shell *shell, const struct cupid_case_node *node) {
    char *word = cupid_expand_word(&node->word, shell);
    size_t i;
    int status = 0;
    int fallthrough = 0;

    if (word == NULL) return 1;

    for (i = 0; i < node->item_count; i++) {
        size_t j;
        int matched = fallthrough;
        if (!matched) {
            for (j = 0; j < node->items[i].pattern_count; j++) {
                char *pat = cupid_expand_case_pattern(&node->items[i].patterns[j], shell);
                if (pat == NULL) { free(word); return 1; }
                if (pattern_matches(shell, pat, word)) {
                    matched = 1;
                }
                free(pat);
                if (matched) break;
            }
        }
        if (!matched) continue;
        if (node->items[i].body && node->items[i].body->count > 0) {
            status = execute_list(shell, node->items[i].body);
        }
        if (node->items[i].terminator == CUPID_CASE_FALLTHRU) {
            fallthrough = 1;
            continue;
        }
        if (node->items[i].terminator == CUPID_CASE_TEST_NEXT) {
            fallthrough = 0;
            continue;
        }
        free(word);
        return status;
    }
    free(word);
    return 0;
}

static int exec_subshell(struct cupid_shell *shell, const struct cupid_list_ast *list) {
    pid_t pid;
    int st = 0;
    struct sigaction old_int, ign_int;

    memset(&ign_int, 0, sizeof(ign_int));
    ign_int.sa_handler = SIG_IGN;
    sigemptyset(&ign_int.sa_mask);
    sigaction(SIGINT, &ign_int, &old_int);

    pid = fork();
    if (pid < 0) { sigaction(SIGINT, &old_int, NULL); return 1; }
    if (pid == 0) {
        signal(SIGINT, SIG_DFL);
        int rc = execute_list(shell, list);
        cupid_shell_run_exit_trap(shell);
        if (shell->should_exit) rc = shell->exit_code;
        fflush(NULL);
        _exit(rc & 0xff);
    }
    waitpid(pid, &st, 0);
    sigaction(SIGINT, &old_int, NULL);
    if (WIFEXITED(st)) return WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + WTERMSIG(st);
    return 1;
}

static int function_body_to_list(struct cupid_node *body, struct cupid_list_ast **out_list) {
    struct cupid_list_ast *list;
    struct cupid_pipeline_item *items;
    struct cupid_node *commands;

    if (body == NULL || out_list == NULL) return -1;

    list = calloc(1, sizeof(*list));
    if (list == NULL) return -1;
    items = calloc(1, sizeof(*items));
    if (items == NULL) {
        free(list);
        return -1;
    }
    commands = calloc(1, sizeof(*commands));
    if (commands == NULL) {
        free(items);
        free(list);
        return -1;
    }

    commands[0] = *body;
    memset(body, 0, sizeof(*body));
    body->kind = NODE_SIMPLE_CMD;

    items[0].pipeline.commands = commands;
    items[0].pipeline.count = 1;
    items[0].join_from_prev = CUPID_CHAIN_NONE;
    items[0].negate_status = false;
    items[0].timed = false;
    items[0].time_posix = false;
    items[0].background = false;

    list->items = items;
    list->count = 1;
    *out_list = list;
    return 0;
}

static int exec_function_def(struct cupid_shell *shell, const struct cupid_node *node) {
    struct cupid_node *body = node->func_def.body;
    struct cupid_list_ast *list;
    char *source = NULL;
    if (body == NULL) return 1;
    if (function_body_to_list(body, &list) != 0) return 1;
    if (shell != NULL &&
        (shell->current_item_source != NULL || shell->current_command_source != NULL)) {
        source = cupid_extract_first_command_source(
            shell->current_item_source ? shell->current_item_source : shell->current_command_source,
            shell->mode == CUPID_MODE_POSIX ? 1 : 0);
    }
    if (source != NULL && capture_heredocs_from_source(list, source) != 0) {
        /* Keep the function definition even if pretty-print/source heredoc capture
         * fails for the current buffered chunk. */
    }
    cupid_func_set(shell, node->func_def.name, list,
                   source ? source :
                   (shell->current_item_source ? shell->current_item_source :
                    shell->current_command_source));
    free(source);
    return 0;
}

static int exec_func_call(struct cupid_shell *shell, struct cupid_list_ast *body, struct runtime_command *cmd) {
    struct cupid_params old_params = shell->params;
    const char *saved_command_source = shell->current_command_source;
    const char *saved_item_source = shell->current_item_source;
    const char *func_source = NULL;
    int status;
    size_t i;

    memset(&shell->params, 0, sizeof(shell->params));
    if (cmd->argc > 1) {
        shell->params.args = calloc((size_t)(cmd->argc - 1), sizeof(char *));
        if (shell->params.args == NULL) {
            shell->params = old_params;
            return 1;
        }
        for (i = 0; i < (size_t)(cmd->argc - 1); i++) {
            shell->params.args[i] = strdup(cmd->argv[i + 1]);
            if (shell->params.args[i] == NULL) {
                size_t j;
                for (j = 0; j < i; j++) free(shell->params.args[j]);
                free(shell->params.args);
                shell->params = old_params;
                return 1;
            }
            shell->params.count++;
        }
    }

    if (cmd != NULL && cmd->argc > 0) {
        func_source = cupid_func_source_get(shell, cmd->argv[0]);
    }
    if (func_source != NULL) {
        shell->current_command_source = func_source;
        shell->current_item_source = NULL;
    }

    cupid_vars_scope_enter(shell);
    shell->return_flag = 0;
    status = execute_list(shell, body);
    shell->return_flag = 0;
    cupid_vars_scope_leave(shell);

    if (func_source != NULL) {
        shell->current_command_source = saved_command_source;
        shell->current_item_source = saved_item_source;
    }

    for (i = 0; i < shell->params.count; i++) {
        free(shell->params.args[i]);
    }
    free(shell->params.args);
    shell->params = old_params;

    return status;
}

static int exec_compound_node(struct cupid_shell *shell, const struct cupid_node *node) {
    switch (node->kind) {
        case NODE_IF:
            return exec_if(shell, &node->if_clause);
        case NODE_FOR:
            return exec_for(shell, &node->for_clause);
        case NODE_WHILE:
            return exec_while(shell, &node->while_clause, 0);
        case NODE_UNTIL:
            return exec_while(shell, &node->while_clause, 1);
        case NODE_CASE:
            return exec_case(shell, &node->case_clause);
        case NODE_BRACE_GROUP:
            return execute_list(shell, node->brace_group);
        case NODE_SUBSHELL:
            return exec_subshell(shell, node->subshell);
        case NODE_FUNCTION_DEF:
            return exec_function_def(shell, node);
        case NODE_COND_EXPR:
            return exec_cond_expr(shell, &node->cond_expr);
        case NODE_ARITH_CMD: {
            int err = 0;
            long val = cupid_arith_eval(shell, node->arith_cmd.expr ? node->arith_cmd.expr : "0", &err);
            if (err) return 1;
            return (val != 0) ? 0 : 1;
        }
        case NODE_COPROC:
            return exec_coproc(shell, &node->coproc);
        default:
            return 1;
    }
}

static int execute_compound_with_redirs(struct cupid_shell *shell, const struct cupid_node *node) {
    struct runtime_command cmd;
    int saved_fds[3] = {-1, -1, -1};
    int status = 1;
    int redir_ok = 0;
    memset(&cmd, 0, sizeof(cmd));

    if (build_runtime_node_redirs(&cmd, node, shell) != 0) {
        if (cupid_expand_error_pending()) {
            int rc = (strcmp(cupid_expand_error_message(), "syntax error") == 0) ? 2 : 1;
            fprintf(stderr, "cupid: %s\n", cupid_expand_error_message());
            cupid_expand_error_reset();
            runtime_command_free(&cmd);
            return rc;
        }
        runtime_command_free(&cmd);
        return 1;
    }
    {
        size_t j;
        for (j = 0; j < cmd.redir_count; j++) {
            struct runtime_redir *redir = &cmd.redirs[j];
            if (redir->kind != CUPID_REDIR_HEREDOC) continue;
            redir->heredoc_fd = cupid_make_heredoc_fd(redir->heredoc_delim,
                                                      redir->heredoc_body,
                                                      redir->heredoc_quoted != 0,
                                                      redir->heredoc_strip_tabs != 0,
                                                      shell);
            if (redir->heredoc_fd < 0) {
                runtime_command_free(&cmd);
                return 1;
            }
        }
    }

    saved_fds[0] = dup(STDIN_FILENO);
    saved_fds[1] = dup(STDOUT_FILENO);
    saved_fds[2] = dup(STDERR_FILENO);
    redir_ok = (saved_fds[0] >= 0 && saved_fds[1] >= 0 && saved_fds[2] >= 0 &&
                apply_redirections(&cmd, shell,
                                   shell != NULL && shell->opt_varredir_close ? 1 : 0) == 0) ? 1 : 0;
    if (redir_ok) {
        status = exec_compound_node(shell, node);
    }

    if (saved_fds[0] >= 0) {
        dup2(saved_fds[0], STDIN_FILENO);
        close(saved_fds[0]);
    }
    if (saved_fds[1] >= 0) {
        dup2(saved_fds[1], STDOUT_FILENO);
        close(saved_fds[1]);
    }
    if (saved_fds[2] >= 0) {
        dup2(saved_fds[2], STDERR_FILENO);
        close(saved_fds[2]);
    }
    runtime_command_free(&cmd);
    if (!redir_ok) return 1;
    return status;
}

static double seconds_diff_timespec(const struct timespec *start, const struct timespec *end) {
    long sec = end->tv_sec - start->tv_sec;
    long nsec = end->tv_nsec - start->tv_nsec;
    if (nsec < 0) {
        sec--;
        nsec += 1000000000L;
    }
    if (sec < 0) return 0.0;
    return (double)sec + ((double)nsec / 1000000000.0);
}

static double seconds_diff_timeval(const struct timeval *start, const struct timeval *end) {
    long sec = end->tv_sec - start->tv_sec;
    long usec = end->tv_usec - start->tv_usec;
    if (usec < 0) {
        sec--;
        usec += 1000000L;
    }
    if (sec < 0) return 0.0;
    return (double)sec + ((double)usec / 1000000.0);
}

static int finish_timed_status(struct cupid_shell *shell, const struct cupid_pipeline_item *item,
                               int timed_ready, const struct timespec *start_real,
                               const struct rusage *start_usage, int status) {
    struct timespec end_real;
    struct rusage end_usage;
    double real_sec;
    double user_sec;
    double sys_sec;
    const char *time_fmt;

    if (!item->timed || !timed_ready) return status;

    if (clock_gettime(CLOCK_MONOTONIC, &end_real) != 0) return status;
    if (getrusage(RUSAGE_CHILDREN, &end_usage) != 0) return status;

    if (!item->time_posix) {
        time_fmt = cupid_vars_get(shell, "TIMEFORMAT");
        if (time_fmt != NULL && time_fmt[0] == '\0') return status;
    }

    real_sec = seconds_diff_timespec(start_real, &end_real);
    user_sec = seconds_diff_timeval(&start_usage->ru_utime, &end_usage.ru_utime);
    sys_sec = seconds_diff_timeval(&start_usage->ru_stime, &end_usage.ru_stime);

    if (item->time_posix) {
        fprintf(stderr, "real %.2f\n", real_sec);
        fprintf(stderr, "user %.2f\n", user_sec);
        fprintf(stderr, "sys %.2f\n", sys_sec);
        return status;
    }

    {
        long real_min = (long)(real_sec / 60.0);
        long user_min = (long)(user_sec / 60.0);
        long sys_min = (long)(sys_sec / 60.0);
        double real_rest = real_sec - ((double)real_min * 60.0);
        double user_rest = user_sec - ((double)user_min * 60.0);
        double sys_rest = sys_sec - ((double)sys_min * 60.0);
        fprintf(stderr, "\nreal\t%ldm%.3fs\n", real_min, real_rest);
        fprintf(stderr, "user\t%ldm%.3fs\n", user_min, user_rest);
        fprintf(stderr, "sys\t%ldm%.3fs\n", sys_min, sys_rest);
    }
    return status;
}

static int execute_pipeline_item(struct cupid_shell *shell, const struct cupid_pipeline_item *item) {
    struct timespec start_real;
    struct rusage start_usage;
    int timed_ready = 0;
    int status;
#define RETURN_STATUS(code) \
    return finish_timed_status(shell, item, timed_ready, &start_real, &start_usage, (code))

    if (item->timed) {
        if (clock_gettime(CLOCK_MONOTONIC, &start_real) == 0 &&
            getrusage(RUSAGE_CHILDREN, &start_usage) == 0) {
            timed_ready = 1;
        }
    }

    if (item->pipeline.count == 1 && item->pipeline.commands[0].kind != NODE_SIMPLE_CMD) {
        const struct cupid_node *node = &item->pipeline.commands[0];
        int ps_arr[1];
        if (node->redir_count > 0) {
            status = execute_compound_with_redirs(shell, node);
        } else {
            status = exec_compound_node(shell, node);
        }
        if (status == 2 && shell->mode == CUPID_MODE_POSIX && !shell->is_interactive) {
            shell->should_exit = 1;
            shell->exit_code = 2;
        }
        ps_arr[0] = status;
        set_status_array(shell, "PIPESTATUS", ps_arr, 1);
        if (item->negate_status) {
            status = (status == 0) ? 1 : 0;
        }
        RETURN_STATUS(status);
    }

    {
        struct runtime_pipeline pl = {0};

        if (item->pipeline.count > 1 && pipeline_has_non_simple(&item->pipeline)) {
            status = run_ast_pipeline(shell, &item->pipeline);
            if (item->negate_status) {
                status = (status == 0) ? 1 : 0;
            }
            RETURN_STATUS(status);
        }

        if (build_runtime_pipeline(&pl, &item->pipeline, shell) != 0) {
            if (cupid_expand_error_pending()) {
                int rc = (strcmp(cupid_expand_error_message(), "syntax error") == 0) ? 2 : 1;
                fprintf(stderr, "cupid: %s\n", cupid_expand_error_message());
                if (rc == 2 && shell->mode == CUPID_MODE_POSIX && !shell->is_interactive) {
                    shell->should_exit = 1;
                    shell->exit_code = 2;
                }
                RETURN_STATUS(rc);
            }
            RETURN_STATUS(1);
        }
        if (prepare_heredocs(&pl, shell) != 0) {
            runtime_pipeline_free(&pl);
            RETURN_STATUS(1);
        }

        if (pl.count == 1 && pl.commands[0].argc > 0) {
            size_t assign_prefix = command_assignment_prefix_count(&pl.commands[0]);
            int cmd_argc = pl.commands[0].argc - (int)assign_prefix;
            char **cmd_argv = pl.commands[0].argv + assign_prefix;
            if (pl.commands[0].redir_count == 0) {
                int all_assign = 1;
                int posix_array_syntax = 0;
                int aj;
                for (aj = 0; aj < pl.commands[0].argc; aj++) {
                    const char *name = NULL;
                    const char *value = NULL;
                    size_t name_len = 0;
                    int append = 0;
                    int ok = 1;
                    if (!split_assignment_for_apply(pl.commands[0].argv[aj], &name,
                                                    &name_len, &value, &append)) {
                        all_assign = 0;
                        break;
                    }
                    {
                        char *lhs = calloc(name_len + 1, 1);
                        char *name = NULL;
                        char *subscript = NULL;
                        size_t idx_dummy = 0;
                        int pidx;
                        int numeric_subscript = 0;
                        size_t i;
                        if (lhs == NULL) { all_assign = 0; break; }
                        memcpy(lhs, pl.commands[0].argv[aj], name_len);
                        pidx = parse_subscripted_name(lhs, &name, &subscript, &numeric_subscript,
                                                      &idx_dummy);
                        if (pidx < 0) ok = 0;
                        if (shell->mode == CUPID_MODE_POSIX &&
                            (pidx == 1 || (!append && value[0] == '('))) {
                            posix_array_syntax = 1;
                            ok = 0;
                        }
                        if (pidx == 0) {
                            for (i = 1; lhs[i] != '\0'; i++) {
                                if (!is_name_char(lhs[i])) { ok = 0; break; }
                            }
                        }
                        free(lhs);
                        free(name);
                        free(subscript);
                    }
                    if (!ok) {
                        all_assign = 0;
                        break;
                    }
                }
                if (posix_array_syntax) {
                    runtime_pipeline_free(&pl);
                    fprintf(stderr, "cupid: syntax error\n");
                    RETURN_STATUS(2);
                }
                if (all_assign) {
                    int ps_arr[1];
                    int assign_status = pl.commands[0].cmdsub_seen ? pl.commands[0].cmdsub_status : 0;
                    for (aj = 0; aj < pl.commands[0].argc; aj++) {
                        const char *name_text = NULL;
                        const char *value_text = NULL;
                        size_t name_len = 0;
                        int append = 0;
                        char *source_rhs = assignment_word_source_rhs(item, (size_t)aj);
                        const char *array_rhs = NULL;
                        char *lhs = NULL;
                        char *name = NULL;
                        char *subscript = NULL;
                        size_t arr_index = 0;
                        int pidx;
                        int numeric_subscript = 0;
                        if (!split_assignment_for_apply(pl.commands[0].argv[aj], &name_text, &name_len,
                                                        &value_text, &append)) {
                            free(source_rhs);
                            runtime_pipeline_free(&pl);
                            RETURN_STATUS(1);
                        }
                        if (source_rhs != NULL) {
                            array_rhs = source_rhs;
                        } else if (value_text[0] == '(') {
                            array_rhs = value_text;
                        }
                        lhs = calloc(name_len + 1, 1);
                        if (lhs == NULL) {
                            free(source_rhs);
                            runtime_pipeline_free(&pl);
                            RETURN_STATUS(1);
                        }
                        memcpy(lhs, pl.commands[0].argv[aj], name_len);
                        pidx = parse_subscripted_name(lhs, &name, &subscript, &numeric_subscript,
                                                      &arr_index);
                        if (pidx == 1) {
                            int assign_rc;
                            if (append) {
                                if (numeric_subscript) {
                                    char keybuf[32];
                                    int kn = snprintf(keybuf, sizeof(keybuf), "%zu", arr_index);
                                    if (kn < 0 || kn >= (int)sizeof(keybuf)) {
                                        assign_rc = -1;
                                    } else {
                                        assign_rc = assign_array_member_with_op(shell, name, keybuf,
                                                                                value_text, 1);
                                    }
                                } else {
                                    assign_rc = assign_array_member_with_op(shell, name, subscript,
                                                                            value_text, 1);
                                }
                            } else {
                                assign_rc = numeric_subscript
                                    ? cupid_array_set_index(shell, name, arr_index, value_text)
                                    : cupid_array_set_key(shell, name, subscript, value_text);
                            }
                            if (assign_rc != 0) {
                                free(source_rhs);
                                free(name);
                                free(lhs);
                                runtime_pipeline_free(&pl);
                                RETURN_STATUS(1);
                            }
                            free(name);
                        } else if (array_rhs != NULL && array_rhs[0] == '(') {
                            if (apply_array_literal_assignment(shell, lhs, array_rhs, append) != 0) {
                                free(source_rhs);
                                free(lhs);
                                runtime_pipeline_free(&pl);
                                RETURN_STATUS(1);
                            }
                        } else if (cupid_array_exists(shell, lhs)) {
                            int assign_rc;
                            if (append) {
                                assign_rc = assign_array_member_with_op(shell, lhs, "0", value_text, 1);
                            } else {
                                assign_rc = cupid_array_set_index(shell, lhs, 0, value_text);
                            }
                            if (assign_rc != 0) {
                                free(source_rhs);
                                free(lhs);
                                runtime_pipeline_free(&pl);
                                RETURN_STATUS(1);
                            }
                        } else {
                            if (apply_assignment_value(shell, lhs, value_text, 0, append) != 0) {
                                free(source_rhs);
                                free(lhs);
                                runtime_pipeline_free(&pl);
                                RETURN_STATUS(1);
                            }
                        }
                        free(subscript);
                        free(source_rhs);
                        free(lhs);
                    }
                    runtime_pipeline_free(&pl);
                    ps_arr[0] = assign_status;
                    set_status_array(shell, "PIPESTATUS", ps_arr, 1);
                    if (item->negate_status) RETURN_STATUS(assign_status == 0 ? 1 : 0);
                    RETURN_STATUS(assign_status);
                }
                if (assign_prefix == 0) {
                    int alias_status = 0;
                    if (try_runtime_alias_expansion(shell, cmd_argc, cmd_argv, &alias_status)) {
                        int ps_arr[1];
                        runtime_pipeline_free(&pl);
                        ps_arr[0] = alias_status;
                        set_status_array(shell, "PIPESTATUS", ps_arr, 1);
                        if (item->negate_status) RETURN_STATUS(alias_status == 0 ? 1 : 0);
                        RETURN_STATUS(alias_status);
                    }
                }
            }
            if (shell->opt_xtrace) {
                int xi;
                fprintf(stderr, "+");
                for (xi = 0; xi < cmd_argc; xi++) {
                    fprintf(stderr, " %s", cmd_argv[xi]);
                }
                fprintf(stderr, "\n");
            }
            {
                int saved_fds[3] = {-1, -1, -1};
                int redir_ok = 0;
                int entered_scope = 0;
                int persist_assign = 0;
                int preserve_std_fds = 1;
                struct temp_env_assignment *temp_env = NULL;
                size_t temp_env_count = 0;
                if (cmd_argc > 0 && strcmp(cmd_argv[0], "exec") == 0) {
                    preserve_std_fds = 0;
                }
                if (pl.commands[0].redir_count > 0) {
                    if (preserve_std_fds) {
                        saved_fds[0] = dup(STDIN_FILENO);
                        saved_fds[1] = dup(STDOUT_FILENO);
                        saved_fds[2] = dup(STDERR_FILENO);
                        redir_ok = (saved_fds[0] >= 0 && saved_fds[1] >= 0 && saved_fds[2] >= 0 &&
                                    apply_redirections(&pl.commands[0], shell,
                                                       should_close_varredir_now(&pl.commands[0], shell)) == 0) ? 1 : 0;
                    } else {
                        redir_ok = (apply_redirections(&pl.commands[0], shell,
                                                       should_close_varredir_now(&pl.commands[0], shell)) == 0) ? 1 : 0;
                    }
                } else {
                    redir_ok = 1;
                }
                if (redir_ok) {
                    if (assign_prefix > 0 && cmd_argc > 0) {
                        persist_assign = (shell->mode == CUPID_MODE_POSIX &&
                                          is_posix_special_builtin_name(cmd_argv[0])) ? 1 : 0;
                        if (!persist_assign &&
                            !builtin_prefix_assignments_use_shell_scope(cmd_argv[0])) {
                            if (apply_prefix_assignments_temp_env(shell, &pl.commands[0], assign_prefix,
                                                                  strcmp(cmd_argv[0], "command") == 0,
                                                                  &temp_env, &temp_env_count) != 0) {
                                status = 1;
                            } else
                            if (strcmp(cmd_argv[0], "exec") == 0 &&
                                apply_prefix_assignments_env(shell, &pl.commands[0], assign_prefix) != 0) {
                                status = 1;
                            } else {
                                status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
                            }
                        } else {
                            cupid_vars_scope_enter(shell);
                            entered_scope = 1;
                            if (strcmp(cmd_argv[0], "exec") == 0 &&
                                apply_prefix_assignments_env(shell, &pl.commands[0], assign_prefix) != 0) {
                                if (entered_scope) {
                                    cupid_vars_scope_leave(shell);
                                    entered_scope = 0;
                                }
                                status = 1;
                            } else
                            if (apply_prefix_assignments(shell, &pl.commands[0], assign_prefix,
                                                         persist_assign ? 0 : 1) != 0) {
                                if (entered_scope) {
                                    cupid_vars_scope_leave(shell);
                                    entered_scope = 0;
                                }
                                status = 1;
                            } else {
                                status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
                            }
                        }
                    } else {
                        status = cupid_run_builtin(shell, cmd_argc, cmd_argv, false);
                    }
                } else {
                    status = 1;
                }
                temp_env_assignments_restore(shell, temp_env, temp_env_count);
                if (entered_scope) {
                    cupid_vars_scope_leave(shell);
                }
                if (preserve_std_fds && saved_fds[0] >= 0) {
                    dup2(saved_fds[0], STDIN_FILENO); close(saved_fds[0]);
                    dup2(saved_fds[1], STDOUT_FILENO); close(saved_fds[1]);
                    dup2(saved_fds[2], STDERR_FILENO); close(saved_fds[2]);
                }
                if (status != CUPID_BUILTIN_NOT_FOUND) {
                    int ps_arr[1];
                    fflush(NULL);
                    ps_arr[0] = status;
                    set_status_array(shell, "PIPESTATUS", ps_arr, 1);
                    if (item->negate_status) {
                        status = (status == 0) ? 1 : 0;
                    }
                    runtime_pipeline_free(&pl);
                    RETURN_STATUS(status);
                }
            }
            if (pl.commands[0].redir_count == 0) {
                struct cupid_list_ast *func_body = (cmd_argc > 0) ? cupid_func_get(shell, cmd_argv[0]) : NULL;
                if (func_body != NULL) {
                    struct runtime_command cmd_view = pl.commands[0];
                    int entered_scope = 0;
                    int ps_arr[1];
                    cmd_view.argc = cmd_argc;
                    cmd_view.argv = cmd_argv;
                    if (assign_prefix > 0) {
                        cupid_vars_scope_enter(shell);
                        entered_scope = 1;
                        if (apply_prefix_assignments(shell, &pl.commands[0], assign_prefix, 1) != 0) {
                            if (entered_scope) cupid_vars_scope_leave(shell);
                            runtime_pipeline_free(&pl);
                            RETURN_STATUS(1);
                        }
                    }
                    status = exec_func_call(shell, func_body, &cmd_view);
                    ps_arr[0] = status;
                    set_status_array(shell, "PIPESTATUS", ps_arr, 1);
                    if (entered_scope) {
                        cupid_vars_scope_leave(shell);
                    }
                    if (item->negate_status) {
                        status = (status == 0) ? 1 : 0;
                    }
                    runtime_pipeline_free(&pl);
                    RETURN_STATUS(status);
                }
            }
        }

        status = run_pipeline(shell, &pl);
        if (item->negate_status) {
            status = (status == 0) ? 1 : 0;
        }
        runtime_pipeline_free(&pl);
        RETURN_STATUS(status);
    }

#undef RETURN_STATUS
}

static void add_job(struct cupid_shell *shell, pid_t pgid, const char *cmd) {
    if (shell->job_count >= CUPID_MAX_JOBS) return;
    shell->next_job_id++;
    shell->jobs[shell->job_count].pgid = pgid;
    shell->jobs[shell->job_count].job_id = shell->next_job_id;
    shell->jobs[shell->job_count].command = cmd ? strdup(cmd) : NULL;
    shell->jobs[shell->job_count].stopped = 0;
    shell->jobs[shell->job_count].completed = 0;
    shell->jobs[shell->job_count].status = 0;
    shell->job_count++;
}

static void check_background_jobs(struct cupid_shell *shell) {
    int i;
    for (i = 0; i < shell->job_count; i++) {
        if (shell->jobs[i].pgid != 0 && !shell->jobs[i].completed && !shell->jobs[i].stopped) {
            int st;
            pid_t r = waitpid(shell->jobs[i].pgid, &st, WNOHANG);
            if (r > 0) {
                if (WIFEXITED(st) || WIFSIGNALED(st)) {
                    shell->jobs[i].completed = 1;
                    if (WIFEXITED(st)) shell->jobs[i].status = WEXITSTATUS(st);
                    else shell->jobs[i].status = 128 + WTERMSIG(st);
                } else if (WIFSTOPPED(st)) {
                    shell->jobs[i].stopped = 1;
                }
            }
        }
    }
}

static int execute_list(struct cupid_shell *shell, const struct cupid_list_ast *list) {
    int status = shell->last_status;
    size_t i;
    const char *source_cursor;
    const char *saved_item_source;

    if (shell == NULL || list == NULL) return 1;

    check_background_jobs(shell);
    source_cursor = (shell->current_item_source == NULL) ? shell->current_command_source : NULL;
    saved_item_source = shell->current_item_source;

    for (i = 0; i < list->count; i++) {
        const struct cupid_pipeline_item *item = &list->items[i];
        char *item_source = NULL;
        int should_run = 1;
        int suppress_errexit = 0;

        if (source_cursor != NULL) {
            item_source = cupid_extract_next_command_source(
                &source_cursor,
                shell->mode == CUPID_MODE_POSIX ? 1 : 0);
            if (cupid_shell_track_command_source(shell, item_source) != 0) {
                free(item_source);
                shell->current_item_source = saved_item_source;
                return 1;
            }
        }
        shell->current_item_source = item_source;

        if (item->join_from_prev == CUPID_CHAIN_AND_IF && status != 0) should_run = 0;
        if (item->join_from_prev == CUPID_CHAIN_OR_IF && status == 0) should_run = 0;
        if (!should_run) {
            shell->current_item_source = saved_item_source;
            continue;
        }
        if ((shell->opt_histexpand || shell->opt_cmdhist) &&
            item_source != NULL && item_source[0] != '\0') {
            cupid_history_add(item_source);
        }

        if (item->background) {
            pid_t pid = fork();
            if (pid < 0) {
                status = 1;
                shell->last_status = status;
                shell->current_item_source = saved_item_source;
                continue;
            }
            if (pid == 0) {
                signal(SIGINT, SIG_DFL);
                signal(SIGQUIT, SIG_DFL);
                status = execute_pipeline_item(shell, item);
                fflush(NULL);
                _exit(status);
            }
            add_job(shell, pid, "(background)");
            shell->last_bg_pid = pid;
            if (shell->is_interactive) {
                fprintf(stderr, "[%d] %d\n", shell->next_job_id, (int)pid);
            }
            status = 0;
            shell->last_status = status;
            shell->current_item_source = saved_item_source;
            continue;
        }

        if (i + 1 < list->count &&
            (list->items[i + 1].join_from_prev == CUPID_CHAIN_AND_IF ||
             list->items[i + 1].join_from_prev == CUPID_CHAIN_OR_IF)) {
            suppress_errexit = 1;
        }
        if (item->negate_status) suppress_errexit = 1;

        if (suppress_errexit) shell->in_condition++;
        status = execute_pipeline_item(shell, item);
        if (suppress_errexit) shell->in_condition--;
        shell->current_item_source = saved_item_source;
        shell->last_status = status;

        if (cupid_expand_error_pending()) {
            cupid_expand_error_reset();
            break;
        }
        if (shell->break_count > 0 || shell->continue_flag || shell->return_flag) {
            break;
        }
        if (shell->should_exit) break;
        if (shell->opt_errexit && !shell->in_condition && !suppress_errexit && status != 0) {
            shell->should_exit = 1;
            shell->exit_code = status;
            break;
        }
    }
    shell->current_item_source = saved_item_source;
    return status;
}

int cupid_execute_ast(struct cupid_shell *shell, const struct cupid_ast *ast) {
    if (shell == NULL || ast == NULL || ast->kind != AST_LIST) return 1;
    if (shell->current_command_source != NULL &&
        capture_heredocs_from_source((struct cupid_list_ast *)&ast->list,
                                     shell->current_command_source) != 0) {
        return 1;
    }
    return execute_list(shell, &ast->list);
}
