#include "cupid/shell.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "cupid/ast.h"
#include "cupid/expand.h"
#include "cupid/exec.h"
#include "cupid/lexer.h"
#include "cupid/parser.h"
#include "cupid/vars.h"

static int buffer_has_heredoc(const char *buf);
static char *strip_heredoc_bodies(const char *text);
static int maybe_print_bash_like_arith_for_parse_error(const struct cupid_shell *shell, const char *line);
static const char *cupid_base_name(const char *path);
static int find_item_line_in_file(const char *path, const char *item_source, int hint_line);
static char *normalize_source_for_match(const char *text, size_t len);

static const char *cupid_base_name(const char *path) {
    const char *slash;
    if (path == NULL || *path == '\0') return "cupid";
    slash = strrchr(path, '/');
    return (slash != NULL && slash[1] != '\0') ? slash + 1 : path;
}

static int extract_for_arith_inner(const char *line, const char **inner_start, size_t *inner_len) {
    const char *p = line;
    int depth = 0;
    char quote = '\0';

    if (line == NULL || inner_start == NULL || inner_len == NULL) return -1;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (!(p[0] == 'f' && p[1] == 'o' && p[2] == 'r' &&
          (p[3] == ' ' || p[3] == '\t' || p[3] == '\n' || p[3] == '\r'))) {
        return -1;
    }
    p += 3;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (p[0] != '(' || p[1] != '(') return -1;
    p += 2;
    *inner_start = p;

    while (*p != '\0') {
        char ch = *p;
        if (quote != '\0') {
            if (ch == '\\' && p[1] != '\0') {
                p += 2;
                continue;
            }
            if (ch == quote) quote = '\0';
            p++;
            continue;
        }
        if (ch == '\'' || ch == '"') {
            quote = ch;
            p++;
            continue;
        }
        if (ch == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (ch == '(') {
            depth++;
            p++;
            continue;
        }
        if (ch == ')') {
            if (depth == 0 && p[1] == ')') {
                *inner_len = (size_t)(p - *inner_start);
                return 0;
            }
            if (depth > 0) depth--;
            p++;
            continue;
        }
        p++;
    }
    return -1;
}

static int count_top_level_semicolons(const char *text, size_t len) {
    size_t i;
    int depth = 0;
    int count = 0;
    char quote = '\0';

    for (i = 0; i < len; i++) {
        char ch = text[i];
        if (quote != '\0') {
            if (ch == '\\' && i + 1 < len) {
                i++;
                continue;
            }
            if (ch == quote) quote = '\0';
            continue;
        }
        if (ch == '\'' || ch == '"') {
            quote = ch;
            continue;
        }
        if (ch == '\\' && i + 1 < len) {
            i++;
            continue;
        }
        if (ch == '(') {
            depth++;
            continue;
        }
        if (ch == ')') {
            if (depth > 0) depth--;
            continue;
        }
        if (ch == ';' && depth == 0) count++;
    }
    return count;
}

static int maybe_print_bash_like_arith_for_parse_error(const struct cupid_shell *shell, const char *line) {
    const char *inner = NULL;
    size_t inner_len = 0;
    int semicolons;
    const char *prog = cupid_base_name(shell != NULL ? shell->arg0 : NULL);

    if (shell == NULL || !shell->is_dash_c || line == NULL) return 0;
    if (extract_for_arith_inner(line, &inner, &inner_len) != 0) return 0;
    semicolons = count_top_level_semicolons(inner, inner_len);

    if (semicolons == 1) {
        fprintf(stderr, "%s: -c: line 1: syntax error: arithmetic expression required\n", prog);
        fprintf(stderr, "%s: -c: line 1: syntax error: `((%.*s))'\n", prog, (int)inner_len, inner);
        return 1;
    }
    if (semicolons > 2) {
        fprintf(stderr, "%s: -c: line 1: syntax error: `;' unexpected\n", prog);
        fprintf(stderr, "%s: -c: line 1: syntax error: `((%.*s))'\n", prog, (int)inner_len, inner);
        return 1;
    }
    return 0;
}

static int find_item_line_in_file(const char *path, const char *item_source, int hint_line) {
    FILE *fp;
    char *line = NULL;
    size_t cap = 0;
    ssize_t nread;
    int lineno = 0;
    int best_any = 0;
    int best_hint = 0;
    const char *start = item_source;
    const char *end = NULL;
    const char *line_start = NULL;
    const char *line_end = NULL;
    const char *p;
    char *needle = NULL;
    char *needle_norm = NULL;
    size_t needle_len;

    if (path == NULL || item_source == NULL) return 0;
    while (*start == ' ' || *start == '\t' || *start == '\n' || *start == '\r') start++;
    p = start;
    while (*p != '\0') {
        const char *ls = p;
        const char *le;
        size_t tlen;
        while (*p != '\0' && *p != '\n' && *p != '\r') p++;
        le = p;
        while (ls < le && (*ls == ' ' || *ls == '\t')) ls++;
        while (le > ls && (le[-1] == ' ' || le[-1] == '\t' || le[-1] == ';')) le--;
        tlen = (size_t)(le - ls);
        if (tlen > 0) {
            int keep = 1;
            if (tlen == 1 && (ls[0] == '{' || ls[0] == '}')) keep = 0;
            if (keep && tlen >= 2 && le[-1] == ')' && le[-2] == '(') keep = 0;
            if (keep) {
                line_start = ls;
                line_end = le;
                break;
            }
        }
        while (*p == '\n' || *p == '\r') p++;
    }
    if (line_start == NULL) {
        line_start = start;
        line_end = start + strlen(start);
        while (line_end > line_start &&
               (line_end[-1] == ' ' || line_end[-1] == '\t' ||
                line_end[-1] == '\n' || line_end[-1] == '\r' || line_end[-1] == ';')) {
            line_end--;
        }
    }
    end = line_end;
    start = line_start;
    if (end <= start) return 0;
    needle_len = (size_t)(end - start);
    needle = calloc(needle_len + 1, 1);
    if (needle == NULL) return 0;
    memcpy(needle, start, needle_len);
    needle_norm = normalize_source_for_match(needle, needle_len);

    fp = fopen(path, "r");
    if (fp == NULL) {
        free(needle);
        free(needle_norm);
        return 0;
    }
    while ((nread = getline(&line, &cap, fp)) >= 0) {
        int matched = 0;
        (void)nread;
        lineno++;
        if (strstr(line, needle) != NULL) {
            matched = 1;
        } else if (needle_norm != NULL && needle_norm[0] != '\0') {
            char *line_norm = normalize_source_for_match(line, strlen(line));
            if (line_norm != NULL) {
                if (strstr(line_norm, needle_norm) != NULL) matched = 1;
                free(line_norm);
            }
        }
        if (matched) {
            if (best_any == 0) best_any = lineno;
            if (hint_line > 0 && lineno <= hint_line && lineno >= best_hint) {
                best_hint = lineno;
            }
        }
    }
    free(line);
    fclose(fp);
    free(needle);
    free(needle_norm);
    return best_hint > 0 ? best_hint : best_any;
}

static char *normalize_source_for_match(const char *text, size_t len) {
    char *out;
    size_t i;
    size_t j = 0;

    if (text == NULL) return NULL;
    out = calloc(len + 1, 1);
    if (out == NULL) return NULL;

    for (i = 0; i < len; i++) {
        char ch = text[i];
        if (ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' || ch == ';') continue;
        out[j++] = ch;
    }
    out[j] = '\0';
    return out;
}

void cupid_shell_init(struct cupid_shell *shell) {
    static int seeded = 0;
    memset(shell, 0, sizeof(*shell));
    shell->mode = CUPID_MODE_BASH;
    shell->is_interactive = 0;
    shell->shell_pid = getpid();
    shell->opt_sourcepath = 1;
    shell->opt_xpg_echo = 0;
    shell->start_time = time(NULL);
    shell->lineno = 1;
    if (!seeded) {
        srand((unsigned int)time(NULL) ^ (unsigned int)getpid());
        seeded = 1;
    }
    (void)cupid_vars_init_defaults(shell);
    {
        const char *posixly = getenv("POSIXLY_CORRECT");
        if (posixly != NULL && posixly[0] != '\0') {
            shell->mode = CUPID_MODE_POSIX;
        }
    }
}

void cupid_shell_error_prefix(FILE *fp, const struct cupid_shell *shell) {
    if (fp == NULL) return;
    if (shell != NULL) {
        if (shell->current_file != NULL && shell->lineno > 0) {
            const char *source_hint = shell->current_item_source;
            if (source_hint == NULL) source_hint = shell->current_command_source;
            if (source_hint != NULL) {
                int src_line = find_item_line_in_file(shell->current_file,
                                                      source_hint,
                                                      shell->lineno);
                if (src_line > 0) {
                    fprintf(fp, "%s: line %d: ", shell->current_file, src_line);
                    return;
                }
            }
            fprintf(fp, "%s: line %d: ", shell->current_file, shell->lineno);
            return;
        }
        if (shell->is_dash_c && shell->arg0 != NULL && shell->lineno > 0) {
            fprintf(fp, "%s: line %d: ", shell->arg0, shell->lineno);
            return;
        }
        if (shell->arg0 != NULL) {
            fprintf(fp, "%s: ", shell->arg0);
            return;
        }
    }
    fputs("cupid: ", fp);
}

void cupid_shell_destroy(struct cupid_shell *shell) {
    size_t i;
    if (shell == NULL) {
        return;
    }
    for (i = 0; i < shell->vars.count; i++) {
        free(shell->vars.entries[i].name);
        free(shell->vars.entries[i].value);
        free(shell->vars.entries[i].nameref_target);
    }
    free(shell->vars.entries);
    shell->vars.entries = NULL;
    shell->vars.count = 0;
    shell->vars.capacity = 0;

    for (i = 0; i < shell->funcs.count; i++) {
        free(shell->funcs.entries[i].name);
        free(shell->funcs.entries[i].source);
        if (shell->funcs.entries[i].body) {
            cupid_list_ast_free(shell->funcs.entries[i].body);
            free(shell->funcs.entries[i].body);
        }
    }
    free(shell->funcs.entries);
    shell->funcs.entries = NULL;
    shell->funcs.count = 0;

    for (i = 0; i < shell->hashes.count; i++) {
        free(shell->hashes.entries[i].name);
        free(shell->hashes.entries[i].path);
    }
    free(shell->hashes.entries);
    shell->hashes.entries = NULL;
    shell->hashes.count = 0;
    shell->hashes.capacity = 0;

    for (i = 0; i < shell->completions.count; i++) {
        free(shell->completions.entries[i].name);
        free(shell->completions.entries[i].spec);
    }
    free(shell->completions.entries);
    shell->completions.entries = NULL;
    shell->completions.count = 0;
    shell->completions.capacity = 0;

    for (i = 0; i < shell->params.count; i++) {
        free(shell->params.args[i]);
    }
    free(shell->params.args);
    shell->params.args = NULL;
    shell->params.count = 0;

    free(shell->arg0);
    shell->arg0 = NULL;

    {
        int ti;
        for (ti = 0; ti <= CUPID_MAX_TRAP_SIGNAL; ti++) {
            free(shell->traps[ti]);
            shell->traps[ti] = NULL;
        }
    }

    {
        int ji;
        for (ji = 0; ji < shell->job_count; ji++) {
            free(shell->jobs[ji].command);
            shell->jobs[ji].command = NULL;
        }
        shell->job_count = 0;
    }

    for (i = 0; i < shell->aliases.count; i++) {
        free(shell->aliases.entries[i].name);
        free(shell->aliases.entries[i].value);
    }
    free(shell->aliases.entries);
    shell->aliases.entries = NULL;
    shell->aliases.count = 0;
    shell->aliases.capacity = 0;

    for (i = 0; i < shell->disabled_builtins.count; i++) {
        free(shell->disabled_builtins.entries[i].name);
        free(shell->disabled_builtins.entries[i].value);
    }
    free(shell->disabled_builtins.entries);
    shell->disabled_builtins.entries = NULL;
    shell->disabled_builtins.count = 0;
    shell->disabled_builtins.capacity = 0;

    for (i = 0; i < shell->arrays.count; i++) {
        size_t j;
        free(shell->arrays.entries[i].name);
        for (j = 0; j < shell->arrays.entries[i].count; j++) {
            free(shell->arrays.entries[i].keys[j]);
            free(shell->arrays.entries[i].items[j]);
        }
        free(shell->arrays.entries[i].keys);
        free(shell->arrays.entries[i].items);
    }
    free(shell->arrays.entries);
    shell->arrays.entries = NULL;
    shell->arrays.count = 0;
    shell->arrays.capacity = 0;

    cupid_shell_clear_tracked_command_sources(shell);
}

void cupid_func_set(struct cupid_shell *shell, const char *name, struct cupid_list_ast *body,
                    const char *source) {
    size_t i;
    char *source_copy = NULL;
    if (source != NULL) {
        source_copy = strdup(source);
        if (source_copy == NULL) return;
    }
    for (i = 0; i < shell->funcs.count; i++) {
        if (strcmp(shell->funcs.entries[i].name, name) == 0) {
            if (shell->funcs.entries[i].body) {
                cupid_list_ast_free(shell->funcs.entries[i].body);
                free(shell->funcs.entries[i].body);
            }
            free(shell->funcs.entries[i].source);
            shell->funcs.entries[i].body = body;
            shell->funcs.entries[i].source = source_copy;
            return;
        }
    }
    {
        struct cupid_func *entries = realloc(shell->funcs.entries,
            sizeof(*entries) * (shell->funcs.count + 1));
        if (entries == NULL) {
            free(source_copy);
            return;
        }
        shell->funcs.entries = entries;
        shell->funcs.entries[shell->funcs.count].name = strdup(name);
        if (shell->funcs.entries[shell->funcs.count].name == NULL) {
            free(source_copy);
            return;
        }
        shell->funcs.entries[shell->funcs.count].body = body;
        shell->funcs.entries[shell->funcs.count].source = source_copy;
        shell->funcs.count++;
    }
}

struct cupid_list_ast *cupid_func_get(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->funcs.count; i++) {
        if (strcmp(shell->funcs.entries[i].name, name) == 0) {
            return shell->funcs.entries[i].body;
        }
    }
    return NULL;
}

const char *cupid_func_source_get(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->funcs.count; i++) {
        if (strcmp(shell->funcs.entries[i].name, name) == 0) {
            return shell->funcs.entries[i].source;
        }
    }
    return NULL;
}

void cupid_func_unset(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return;
    for (i = 0; i < shell->funcs.count; i++) {
        if (strcmp(shell->funcs.entries[i].name, name) == 0) {
            free(shell->funcs.entries[i].name);
            free(shell->funcs.entries[i].source);
            if (shell->funcs.entries[i].body) {
                cupid_list_ast_free(shell->funcs.entries[i].body);
                free(shell->funcs.entries[i].body);
            }
            if (i + 1 < shell->funcs.count) {
                shell->funcs.entries[i] = shell->funcs.entries[shell->funcs.count - 1];
            }
            shell->funcs.count--;
            return;
        }
    }
}

void cupid_shell_run_exit_trap(struct cupid_shell *shell) {
    if (shell == NULL) return;
    if (shell->traps[0] != NULL && shell->traps[0][0] != '\0') {
        char *handler = strdup(shell->traps[0]);
        if (handler != NULL) {
            free(shell->traps[0]);
            shell->traps[0] = NULL;
            cupid_shell_eval_line(shell, handler, 1);
            free(handler);
        }
    }
}

int cupid_shell_eval_line(struct cupid_shell *shell, const char *line, int print_errors) {
    struct cupid_tokens toks = {0};
    struct cupid_ast *ast = NULL;
    struct cupid_parse_opts popts;
    int status;
    char *parse_line = NULL;
    const char *lex_input = line;

    if (shell == NULL || line == NULL) {
        return 1;
    }
    if (buffer_has_heredoc(line)) {
        parse_line = strip_heredoc_bodies(line);
        if (parse_line != NULL) lex_input = parse_line;
    }
    if (cupid_lex(lex_input, &toks) != 0) {
        if (print_errors) {
            fprintf(stderr, "cupid: syntax error\n");
        }
        free(parse_line);
        shell->last_status = 2;
        return 2;
    }
    popts.posix_mode = (shell->mode == CUPID_MODE_POSIX) ? 1 : 0;
    if (cupid_parse(&toks, &popts, &ast) != 0) {
        if (print_errors) {
            if (!maybe_print_bash_like_arith_for_parse_error(shell, line)) {
                fprintf(stderr, "cupid: syntax error\n");
            }
        }
        cupid_tokens_free(&toks);
        free(parse_line);
        shell->last_status = 2;
        return 2;
    }

    shell->current_command_source = line;
    status = cupid_execute_ast(shell, ast);
    shell->current_command_source = NULL;
    shell->current_item_source = NULL;
    shell->last_status = status;
    cupid_ast_free(ast);
    cupid_tokens_free(&toks);
    free(parse_line);
    return status;
}

int cupid_shell_track_command_source(struct cupid_shell *shell, char *source) {
    char **next;
    size_t nc;
    if (shell == NULL || source == NULL) return 0;
    if (shell->tracked_command_source_count == shell->tracked_command_source_capacity) {
        nc = (shell->tracked_command_source_capacity == 0) ? 8 :
            shell->tracked_command_source_capacity * 2;
        next = realloc(shell->tracked_command_sources, sizeof(*next) * nc);
        if (next == NULL) return -1;
        shell->tracked_command_sources = next;
        shell->tracked_command_source_capacity = nc;
    }
    shell->tracked_command_sources[shell->tracked_command_source_count++] = source;
    return 0;
}

void cupid_shell_clear_tracked_command_sources(struct cupid_shell *shell) {
    size_t i;
    if (shell == NULL) return;
    for (i = 0; i < shell->tracked_command_source_count; i++) {
        free(shell->tracked_command_sources[i]);
    }
    free(shell->tracked_command_sources);
    shell->tracked_command_sources = NULL;
    shell->tracked_command_source_count = 0;
    shell->tracked_command_source_capacity = 0;
}

int cupid_shell_eval_text(struct cupid_shell *shell, const char *text, int print_errors) {
    char tmp[] = "/tmp/cupid-eval-XXXXXX";
    size_t len;
    int fd;
    FILE *f;
    int status;

    if (shell == NULL || text == NULL) {
        return 1;
    }
    if (!buffer_has_heredoc(text)) {
        return cupid_shell_eval_line(shell, text, print_errors);
    }

    fd = mkstemp(tmp);
    if (fd < 0) {
        return cupid_shell_eval_line(shell, text, print_errors);
    }
    f = fdopen(fd, "w");
    if (f == NULL) {
        close(fd);
        unlink(tmp);
        return cupid_shell_eval_line(shell, text, print_errors);
    }
    len = strlen(text);
    if ((len > 0 && fwrite(text, 1, len, f) != len) ||
        (len == 0 && ferror(f))) {
        fclose(f);
        unlink(tmp);
        return cupid_shell_eval_line(shell, text, print_errors);
    }
    if (len == 0 || text[len - 1] != '\n') {
        fputc('\n', f);
    }
    if (fclose(f) != 0) {
        unlink(tmp);
        return cupid_shell_eval_line(shell, text, print_errors);
    }
    status = cupid_shell_eval_file(shell, tmp);
    unlink(tmp);
    return status;
}

static const char *path_base_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash ? slash + 1 : path;
}

static int shebang_requests_posix_line(const char *line) {
    const char *p = line;
    const char *start;
    const char *end;
    char tok[128];
    const char *name;
    if (line == NULL || p[0] != '#' || p[1] != '!') return 0;
    p += 2;
    while (*p == ' ' || *p == '\t') p++;
    start = p;
    while (*p != '\0' && *p != '\n' && *p != ' ' && *p != '\t') p++;
    end = p;
    if (end <= start || (size_t)(end - start) >= sizeof(tok)) return 0;
    memcpy(tok, start, (size_t)(end - start));
    tok[end - start] = '\0';
    name = path_base_name(tok);
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
        name = path_base_name(tok);
    }
    return strcmp(name, "sh") == 0 || strcmp(name, "dash") == 0;
}

static int run_posix_script_fallback(struct cupid_shell *shell, const char *path) {
    pid_t pid;
    int st = 0;
    char **argv;
    size_t i;
    if (shell == NULL || path == NULL) return 1;
    argv = calloc(shell->params.count + 3, sizeof(char *));
    if (argv == NULL) return 1;
    argv[0] = "sh";
    argv[1] = (char *)path;
    for (i = 0; i < shell->params.count; i++) argv[i + 2] = shell->params.args[i];
    argv[shell->params.count + 2] = NULL;
    pid = fork();
    if (pid < 0) {
        free(argv);
        return 1;
    }
    if (pid == 0) {
        execvp("sh", argv);
        _exit(127);
    }
    free(argv);
    if (waitpid(pid, &st, 0) < 0) return 1;
    if (WIFEXITED(st)) return WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + WTERMSIG(st);
    return 1;
}

static int buffer_has_heredoc(const char *buf) {
    struct cupid_tokens toks = {0};
    size_t i;
    int found = 0;
    if (buf == NULL) return 0;
    if (cupid_lex(buf, &toks) != 0) return 0;
    for (i = 0; i < toks.count; i++) {
        if (toks.items[i].kind == TOK_HEREDOC || toks.items[i].kind == TOK_HEREDOC_STRIP) {
            found = 1;
            break;
        }
    }
    cupid_tokens_free(&toks);
    return found;
}

static int line_is_ignorable(const char *line);

static int buffer_parses_as_command(const char *buf, int posix_mode) {
    struct cupid_tokens toks = {0};
    struct cupid_ast *ast = NULL;
    struct cupid_parse_opts popts;
    int ok = 0;
    char *parse_buf = NULL;
    const char *lex_input = buf;

    if (buf == NULL || buf[0] == '\0') return 0;
    if (buffer_has_heredoc(buf)) {
        parse_buf = strip_heredoc_bodies(buf);
        if (parse_buf != NULL) lex_input = parse_buf;
    }
    if (cupid_lex(lex_input, &toks) != 0) {
        free(parse_buf);
        return 0;
    }
    if (toks.count == 0) {
        cupid_tokens_free(&toks);
        free(parse_buf);
        return 0;
    }
    popts.posix_mode = posix_mode ? 1 : 0;
    if (cupid_parse(&toks, &popts, &ast) == 0) {
        ok = 1;
    }
    cupid_ast_free(ast);
    cupid_tokens_free(&toks);
    free(parse_buf);
    return ok;
}

char *cupid_extract_next_command_source(const char **text_io, int posix_mode) {
    const char *p;
    char *chunk = NULL;
    size_t chunk_len = 0;
    size_t chunk_cap = 0;

    if (text_io == NULL || *text_io == NULL) return NULL;
    p = *text_io;

    while (p != NULL && *p != '\0') {
        const char *end = strchr(p, '\n');
        size_t len = end ? (size_t)(end - p + 1) : strlen(p);
        size_t needed;
        char *next;

        if (chunk_len == 0) {
            char *line = calloc(len + 1, 1);
            int ignorable = 0;
            if (line == NULL) {
                free(chunk);
                return NULL;
            }
            memcpy(line, p, len);
            ignorable = line_is_ignorable(line);
            free(line);
            if (ignorable) {
                p += len;
                continue;
            }
        }

        needed = chunk_len + len + 1;
        if (needed > chunk_cap) {
            size_t nc = (chunk_cap == 0) ? 256 : chunk_cap;
            while (nc < needed) nc *= 2;
            next = realloc(chunk, nc);
            if (next == NULL) {
                free(chunk);
                return NULL;
            }
            chunk = next;
            chunk_cap = nc;
        }
        memcpy(chunk + chunk_len, p, len);
        chunk_len += len;
        chunk[chunk_len] = '\0';
        p += len;

        if (buffer_parses_as_command(chunk, posix_mode)) {
            *text_io = p;
            return chunk;
        }
    }

    *text_io = p;
    return chunk;
}

char *cupid_extract_first_command_source(const char *text, int posix_mode) {
    return cupid_extract_next_command_source(&text, posix_mode);
}

static int line_is_ignorable(const char *line) {
    struct cupid_tokens toks = {0};
    size_t i;
    int ignorable = 1;

    if (line == NULL) return 1;
    if (cupid_lex(line, &toks) != 0) return 0;
    for (i = 0; i < toks.count; i++) {
        if (toks.items[i].kind != TOK_NEWLINE) {
            ignorable = 0;
            break;
        }
    }
    cupid_tokens_free(&toks);
    return ignorable;
}

struct pending_heredoc {
    char delimiter[128];
    int strip_tabs;
};

static int append_text_chunk(char **buf, size_t *len, size_t *cap, const char *text, size_t text_len) {
    char *next;
    size_t needed;
    if (buf == NULL || len == NULL || cap == NULL || text == NULL) return -1;
    needed = *len + text_len + 1;
    if (needed > *cap) {
        size_t nc = (*cap == 0) ? 256 : *cap;
        while (nc < needed) nc *= 2;
        next = realloc(*buf, nc);
        if (next == NULL) return -1;
        *buf = next;
        *cap = nc;
    }
    if (text_len > 0) memcpy(*buf + *len, text, text_len);
    *len += text_len;
    (*buf)[*len] = '\0';
    return 0;
}

static int parse_heredoc_delim_from_line(const char *line, char *delim, size_t delim_size,
                                         int *strip_tabs_out) {
    struct cupid_tokens toks = {0};
    size_t i;
    if (delim == NULL || delim_size == 0 || line == NULL) return 0;
    if (strip_tabs_out != NULL) *strip_tabs_out = 0;
    if (cupid_lex(line, &toks) != 0) return 0;
    for (i = 0; i + 1 < toks.count; i++) {
        if (toks.items[i].kind == TOK_HEREDOC || toks.items[i].kind == TOK_HEREDOC_STRIP) {
            char *dequoted = cupid_word_dequote_literal(&toks.items[i + 1].word);
            size_t len;
            if (dequoted == NULL) {
                cupid_tokens_free(&toks);
                return 0;
            }
            len = strlen(dequoted);
            if (len == 0 || len + 1 > delim_size) {
                free(dequoted);
                cupid_tokens_free(&toks);
                return 0;
            }
            memcpy(delim, dequoted, len + 1);
            if (strip_tabs_out != NULL) {
                *strip_tabs_out = (toks.items[i].kind == TOK_HEREDOC_STRIP) ? 1 : 0;
            }
            free(dequoted);
            cupid_tokens_free(&toks);
            return 1;
        }
    }
    cupid_tokens_free(&toks);
    return 0;
}

static int collect_pending_heredocs(const char *buf, struct pending_heredoc **out_items,
                                    size_t *out_count) {
    const char *cursor = buf;
    struct pending_heredoc *items = NULL;
    size_t count = 0;
    while (cursor != NULL && *cursor != '\0') {
        const char *line_end = strchr(cursor, '\n');
        size_t len = line_end ? (size_t)(line_end - cursor) : strlen(cursor);
        char *line = calloc(len + 1, 1);
        struct pending_heredoc *next;
        char delim[128];
        int strip_tabs = 0;
        if (line == NULL) goto fail;
        if (len > 0) memcpy(line, cursor, len);
        if (parse_heredoc_delim_from_line(line, delim, sizeof(delim), &strip_tabs)) {
            next = realloc(items, sizeof(*next) * (count + 1));
            if (next == NULL) {
                free(line);
                goto fail;
            }
            items = next;
            memset(&items[count], 0, sizeof(items[count]));
            memcpy(items[count].delimiter, delim, sizeof(delim));
            items[count].strip_tabs = strip_tabs;
            count++;
        }
        free(line);
        if (line_end == NULL) break;
        cursor = line_end + 1;
    }
    *out_items = items;
    *out_count = count;
    return 0;
fail:
    free(items);
    return -1;
}

static int append_pending_heredoc_bodies(FILE *input, char **buf, size_t *buf_len, size_t *buf_cap) {
    struct pending_heredoc *items = NULL;
    size_t count = 0;
    size_t i;
    char *line = NULL;
    size_t line_cap = 0;

    if (input == NULL || buf == NULL || *buf == NULL || buf_len == NULL || buf_cap == NULL) return -1;
    if (collect_pending_heredocs(*buf, &items, &count) != 0) return -1;
    for (i = 0; i < count; i++) {
        while (getline(&line, &line_cap, input) >= 0) {
            char *cmp;
            size_t raw_len = strlen(line);
            if (append_text_chunk(buf, buf_len, buf_cap, line, raw_len) != 0) {
                free(items);
                free(line);
                return -1;
            }
            cmp = line;
            if (raw_len > 0 && cmp[raw_len - 1] == '\n') cmp[--raw_len] = '\0';
            if (items[i].strip_tabs) {
                while (*cmp == '\t') cmp++;
            }
            if (strcmp(cmp, items[i].delimiter) == 0) {
                break;
            }
        }
    }
    free(items);
    free(line);
    return 0;
}

static char *strip_heredoc_bodies(const char *text) {
    struct pending_heredoc *queue = NULL;
    size_t qcount = 0;
    size_t qcap = 0;
    const char *cursor = text;
    char *out = NULL;
    size_t out_len = 0;
    size_t out_cap = 0;

    if (text == NULL) return NULL;
    while (*cursor != '\0') {
        const char *line_end = cursor;
        size_t line_len;
        char *line;

        while (*line_end != '\0' && *line_end != '\n') line_end++;
        line_len = (size_t)(line_end - cursor);
        line = calloc(line_len + 1, 1);
        if (line == NULL) {
            free(queue);
            free(out);
            return NULL;
        }
        if (line_len > 0) memcpy(line, cursor, line_len);

        if (qcount > 0) {
            char *cmp = line;
            if (queue[0].strip_tabs) {
                while (*cmp == '\t') cmp++;
            }
            if (strcmp(cmp, queue[0].delimiter) == 0) {
                if (qcount > 1) {
                    memmove(queue, queue + 1, sizeof(*queue) * (qcount - 1));
                }
                qcount--;
            }
            free(line);
        } else {
            char delim[128];
            int strip_tabs = 0;
            if (append_text_chunk(&out, &out_len, &out_cap, cursor, line_len) != 0 ||
                (*line_end == '\n' && append_text_chunk(&out, &out_len, &out_cap, "\n", 1) != 0)) {
                free(line);
                free(queue);
                free(out);
                return NULL;
            }
            if (parse_heredoc_delim_from_line(line, delim, sizeof(delim), &strip_tabs)) {
                struct pending_heredoc *next;
                if (qcount == qcap) {
                    size_t nc = (qcap == 0) ? 4 : qcap * 2;
                    next = realloc(queue, sizeof(*next) * nc);
                    if (next == NULL) {
                        free(line);
                        free(queue);
                        free(out);
                        return NULL;
                    }
                    queue = next;
                    qcap = nc;
                }
                memset(&queue[qcount], 0, sizeof(queue[qcount]));
                memcpy(queue[qcount].delimiter, delim, sizeof(delim));
                queue[qcount].strip_tabs = strip_tabs;
                qcount++;
            }
            free(line);
        }

        cursor = line_end;
        if (*cursor == '\n') cursor++;
    }

    free(queue);
    if (out == NULL) out = calloc(1, 1);
    return out;
}

static int eval_stream_for_heredoc(struct cupid_shell *shell, FILE *f, int posix_shebang) {
    char *line = NULL;
    size_t cap = 0;
    char *buf = NULL;
    size_t buf_len = 0;
    size_t buf_cap = 0;
    int status = 0;
    int first_line = 1;
    int saved_stdin = -1;
    int current_line = 0;
    int command_line = 1;
    const char *old_file = shell ? shell->current_file : NULL;
    int old_lineno = shell ? shell->lineno : 1;

    if (f == NULL) return 127;

    saved_stdin = dup(STDIN_FILENO);
    if (saved_stdin < 0) {
        fclose(f);
        return 1;
    }
    if (dup2(fileno(f), STDIN_FILENO) < 0) {
        close(saved_stdin);
        fclose(f);
        return 1;
    }
    clearerr(stdin);

    while (getline(&line, &cap, stdin) >= 0) {
        size_t len = strlen(line);
        size_t needed;
        char *next;
        current_line++;

        if (first_line && line[0] == '#' && line[1] == '!') {
            first_line = 0;
            continue;
        }
        first_line = 0;

        if (buf_len == 0 && line_is_ignorable(line)) continue;
        if (buf_len == 0) command_line = current_line;

        needed = buf_len + len + 1;
        if (needed > buf_cap) {
            size_t nc = (buf_cap == 0) ? 256 : buf_cap;
            while (nc < needed) nc *= 2;
            next = realloc(buf, nc);
            if (next == NULL) {
                free(buf);
                free(line);
                close(saved_stdin);
                return 1;
            }
            buf = next;
            buf_cap = nc;
        }
        memcpy(buf + buf_len, line, len);
        buf_len += len;
        buf[buf_len] = '\0';

        if (buffer_has_heredoc(buf)) {
            if (append_pending_heredoc_bodies(stdin, &buf, &buf_len, &buf_cap) != 0) {
                free(buf);
                free(line);
                close(saved_stdin);
                return 1;
            }
        }
        if (!buffer_parses_as_command(buf, posix_shebang)) {
            continue;
        }
        if (shell != NULL) {
            shell->lineno = command_line;
        }
        status = cupid_shell_eval_line(shell, buf, posix_shebang ? 0 : 1);
        if (shell != NULL && shell->return_flag) {
            break;
        }
        if (shell->should_exit) {
            status = shell->exit_code;
            break;
        }
        buf_len = 0;
        if (buf != NULL) buf[0] = '\0';
    }

    free(line);
    if (!shell->should_exit && buf_len > 0) {
        if (shell != NULL) {
            shell->lineno = command_line;
        }
        status = cupid_shell_eval_line(shell, buf, posix_shebang ? 0 : 1);
    }
    free(buf);
    if (shell != NULL) {
        shell->current_file = old_file;
        shell->lineno = old_lineno;
    }
    if (dup2(saved_stdin, STDIN_FILENO) < 0) status = 1;
    close(saved_stdin);
    clearerr(stdin);
    return status;
}

static int eval_buffered_commands(struct cupid_shell *shell, const char *buf, int posix_shebang,
                                  const char *path) {
    const char *p = buf;
    char *chunk = NULL;
    size_t chunk_len = 0;
    size_t chunk_cap = 0;
    int line_no = 1;
    int command_line = 1;
    int status = 0;
    const char *old_file = shell ? shell->current_file : NULL;
    int old_lineno = shell ? shell->lineno : 1;

    if (shell != NULL) shell->current_file = path;

    while (p != NULL && *p != '\0') {
        const char *end = strchr(p, '\n');
        size_t len = end ? (size_t)(end - p + 1) : strlen(p);
        size_t needed;
        char *next;

        if (chunk_len == 0) {
            char *line = calloc(len + 1, 1);
            int ignorable = 0;
            if (line == NULL) {
                if (shell != NULL) {
                    shell->current_file = old_file;
                    shell->lineno = old_lineno;
                }
                free(chunk);
                return 1;
            }
            memcpy(line, p, len);
            ignorable = line_is_ignorable(line);
            free(line);
            if (ignorable) {
                p += len;
                if (end != NULL) line_no++;
                continue;
            }
        }
        if (chunk_len == 0) command_line = line_no;

        needed = chunk_len + len + 1;
        if (needed > chunk_cap) {
            size_t nc = (chunk_cap == 0) ? 256 : chunk_cap;
            while (nc < needed) nc *= 2;
            next = realloc(chunk, nc);
            if (next == NULL) {
                free(chunk);
                if (shell != NULL) {
                    shell->current_file = old_file;
                    shell->lineno = old_lineno;
                }
                return 1;
            }
            chunk = next;
            chunk_cap = nc;
        }
        memcpy(chunk + chunk_len, p, len);
        chunk_len += len;
        chunk[chunk_len] = '\0';

        if (end != NULL) line_no++;
        p += len;

        if (!buffer_parses_as_command(chunk, posix_shebang)) {
            continue;
        }

        if (shell != NULL) shell->lineno = command_line;
        status = cupid_shell_eval_line(shell, chunk, posix_shebang ? 0 : 1);
        if (shell != NULL && shell->return_flag) break;
        if (status == 2 && posix_shebang) break;
        if (shell != NULL && shell->should_exit) {
            status = shell->exit_code;
            break;
        }
        chunk_len = 0;
        chunk[0] = '\0';
    }

    if (status == 0 && chunk_len > 0) {
        if (shell != NULL) shell->lineno = command_line;
        status = cupid_shell_eval_line(shell, chunk, posix_shebang ? 0 : 1);
    }

    free(chunk);
    if (shell != NULL) {
        shell->current_file = old_file;
        shell->lineno = old_lineno;
    }
    return status;
}

static int eval_file_stream_for_heredoc(struct cupid_shell *shell, const char *path, int posix_shebang) {
    FILE *f;
    int status;

    f = fopen(path, "r");
    if (f == NULL) {
        fprintf(stderr, "cupid: %s: No such file or directory\n", path);
        return 127;
    }
    status = eval_stream_for_heredoc(shell, f, posix_shebang);
    fclose(f);
    return status;
}

int cupid_shell_eval_file(struct cupid_shell *shell, const char *path) {
    FILE *f;
    struct stat st;
    char *line = NULL;
    size_t line_cap = 0;
    char *buf = NULL;
    size_t buf_len = 0;
    size_t buf_cap = 0;
    int status = 0;
    int first_line = 1;
    int posix_shebang = 0;

    if (shell == NULL || path == NULL) {
        return 1;
    }
    f = fopen(path, "r");
    if (f == NULL) {
        fprintf(stderr, "cupid: %s: No such file or directory\n", path);
        return 127;
    }
    if (fstat(fileno(f), &st) == 0 && !S_ISREG(st.st_mode)) {
        status = eval_stream_for_heredoc(shell, f, 0);
        fclose(f);
        return status;
    }

    while (getline(&line, &line_cap, f) >= 0) {
        size_t len = strlen(line);
        if (first_line && len >= 2 && line[0] == '#' && line[1] == '!') {
            posix_shebang = shebang_requests_posix_line(line) ? 1 : 0;
            first_line = 0;
            continue;
        }
        first_line = 0;

        {
            size_t needed = buf_len + len + 1;
            if (needed > buf_cap) {
                size_t nc = (buf_cap == 0) ? 256 : buf_cap;
                char *nb;
                while (nc < needed) nc *= 2;
                nb = realloc(buf, nc);
                if (nb == NULL) {
                    status = 1;
                    break;
                }
                buf = nb;
                buf_cap = nc;
            }
            memcpy(buf + buf_len, line, len);
            buf_len += len;
            buf[buf_len] = '\0';
        }
        if (status != 0) break;

    }

    free(line);
    fclose(f);
    if (status != 0) {
        free(buf);
        return status;
    }
    if (buf == NULL) {
        return 0;
    }
    if (posix_shebang) {
        free(buf);
        return run_posix_script_fallback(shell, path);
    }
    if (buffer_has_heredoc(buf)) {
        const char *old_file = shell->current_file;
        int old_lineno = shell->lineno;
        shell->current_file = path;
        shell->lineno = 1;
        status = eval_file_stream_for_heredoc(shell, path, posix_shebang);
        if (status == 2 && posix_shebang) {
            status = run_posix_script_fallback(shell, path);
        }
        shell->current_file = old_file;
        shell->lineno = old_lineno;
        free(buf);
        return status;
    }
    status = eval_buffered_commands(shell, buf, posix_shebang, path);
    if (status == 2 && posix_shebang) {
        status = run_posix_script_fallback(shell, path);
    }
    free(buf);
    return status;
}

const char *cupid_alias_get(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->aliases.count; i++) {
        if (strcmp(shell->aliases.entries[i].name, name) == 0) {
            return shell->aliases.entries[i].value;
        }
    }
    return NULL;
}

int cupid_alias_set(struct cupid_shell *shell, const char *name, const char *value) {
    size_t i;
    char *vdup;
    if (shell == NULL || name == NULL || value == NULL) return -1;
    vdup = strdup(value);
    if (vdup == NULL) return -1;
    for (i = 0; i < shell->aliases.count; i++) {
        if (strcmp(shell->aliases.entries[i].name, name) == 0) {
            free(shell->aliases.entries[i].value);
            shell->aliases.entries[i].value = vdup;
            return 0;
        }
    }
    if (shell->aliases.count == shell->aliases.capacity) {
        size_t nc = (shell->aliases.capacity == 0) ? 8 : shell->aliases.capacity * 2;
        struct cupid_alias *ne = realloc(shell->aliases.entries, sizeof(*ne) * nc);
        if (ne == NULL) {
            free(vdup);
            return -1;
        }
        shell->aliases.entries = ne;
        shell->aliases.capacity = nc;
    }
    shell->aliases.entries[shell->aliases.count].name = strdup(name);
    if (shell->aliases.entries[shell->aliases.count].name == NULL) {
        free(vdup);
        return -1;
    }
    shell->aliases.entries[shell->aliases.count].value = vdup;
    shell->aliases.count++;
    return 0;
}

int cupid_alias_unset(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return -1;
    for (i = 0; i < shell->aliases.count; i++) {
        if (strcmp(shell->aliases.entries[i].name, name) == 0) {
            free(shell->aliases.entries[i].name);
            free(shell->aliases.entries[i].value);
            if (i + 1 < shell->aliases.count) {
                shell->aliases.entries[i] = shell->aliases.entries[shell->aliases.count - 1];
            }
            shell->aliases.count--;
            return 0;
        }
    }
    return -1;
}
