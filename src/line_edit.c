#include "cupid/line_edit.h"

#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

#include "cupid/builtins.h"
#include "cupid/history.h"
#include "cupid/shell.h"

/* ------------------------------------------------------------------ */
/*  Terminal state                                                     */
/* ------------------------------------------------------------------ */

static struct termios g_orig_termios;
static int g_have_orig;
static int g_raw_mode;

static void restore_terminal(void) {
    if (g_have_orig) {
        tcsetattr(STDIN_FILENO, TCSAFLUSH, &g_orig_termios);
        g_raw_mode = 0;
    }
}

static int enter_raw_mode(void) {
    struct termios raw;
    if (!g_have_orig) return -1;
    raw = g_orig_termios;
    raw.c_iflag &= ~((tcflag_t)(BRKINT | ICRNL | INPCK | ISTRIP | IXON));
    raw.c_oflag &= ~((tcflag_t)(OPOST));
    raw.c_cflag |= (tcflag_t)CS8;
    raw.c_lflag &= ~((tcflag_t)(ECHO | ICANON | IEXTEN | ISIG));
    raw.c_cc[VMIN] = 1;
    raw.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) != 0) return -1;
    g_raw_mode = 1;
    return 0;
}

static void leave_raw_mode(void) {
    if (g_raw_mode) {
        tcsetattr(STDIN_FILENO, TCSAFLUSH, &g_orig_termios);
        g_raw_mode = 0;
    }
}

/* ------------------------------------------------------------------ */
/*  Output helpers (all use write(), safe in raw mode)                */
/* ------------------------------------------------------------------ */

static void out_raw(const char *s, size_t len) {
    ssize_t r;
    while (len > 0) {
        r = write(STDOUT_FILENO, s, len);
        if (r <= 0) break;
        s += r;
        len -= (size_t)r;
    }
}

static void out_str(const char *s) {
    out_raw(s, strlen(s));
}

/* ------------------------------------------------------------------ */
/*  Prompt visible length (skips \001..\002 regions)                   */
/* ------------------------------------------------------------------ */

static size_t prompt_visible_len(const char *prompt) {
    size_t len = 0;
    int invisible = 0;
    const char *p = prompt;
    while (*p != '\0') {
        if (*p == '\001') { invisible = 1; p++; continue; }
        if (*p == '\002') { invisible = 0; p++; continue; }
        if (!invisible) len++;
        p++;
    }
    return len;
}

/* ------------------------------------------------------------------ */
/*  Line editing state                                                 */
/* ------------------------------------------------------------------ */

struct line_state {
    char *buf;
    size_t len;
    size_t cap;
    size_t pos;
    const char *prompt;
    size_t prompt_vlen;
    struct cupid_shell *shell;
    int history_idx;
    char *saved_line;
};

static void ls_ensure_cap(struct line_state *ls, size_t needed) {
    if (needed + 1 <= ls->cap) return;
    {
        size_t nc = ls->cap;
        char *nb;
        while (nc < needed + 1) nc *= 2;
        nb = realloc(ls->buf, nc);
        if (nb == NULL) return;
        ls->buf = nb;
        ls->cap = nc;
    }
}

/* ------------------------------------------------------------------ */
/*  Refresh line display                                               */
/* ------------------------------------------------------------------ */

static void refresh_line(struct line_state *ls) {
    char seq[64];
    int n;

    out_raw("\r", 1);
    out_str(ls->prompt);
    if (ls->len > 0) out_raw(ls->buf, ls->len);
    out_raw("\033[K", 3);

    if (ls->pos < ls->len) {
        size_t diff = ls->len - ls->pos;
        n = snprintf(seq, sizeof(seq), "\033[%zuD", diff);
        if (n > 0) out_raw(seq, (size_t)n);
    }
}

/* ------------------------------------------------------------------ */
/*  Basic editing operations                                           */
/* ------------------------------------------------------------------ */

static void line_insert_char(struct line_state *ls, char c) {
    ls_ensure_cap(ls, ls->len + 1);
    if (ls->pos < ls->len) {
        memmove(ls->buf + ls->pos + 1, ls->buf + ls->pos, ls->len - ls->pos);
    }
    ls->buf[ls->pos] = c;
    ls->pos++;
    ls->len++;
    ls->buf[ls->len] = '\0';
}

static void line_delete_at_cursor(struct line_state *ls) {
    if (ls->pos >= ls->len) return;
    memmove(ls->buf + ls->pos, ls->buf + ls->pos + 1, ls->len - ls->pos - 1);
    ls->len--;
    ls->buf[ls->len] = '\0';
}

static void line_backspace(struct line_state *ls) {
    if (ls->pos == 0) return;
    ls->pos--;
    line_delete_at_cursor(ls);
}

static void line_kill_to_end(struct line_state *ls) {
    ls->len = ls->pos;
    ls->buf[ls->len] = '\0';
}

static void line_kill_to_start(struct line_state *ls) {
    if (ls->pos == 0) return;
    memmove(ls->buf, ls->buf + ls->pos, ls->len - ls->pos);
    ls->len -= ls->pos;
    ls->pos = 0;
    ls->buf[ls->len] = '\0';
}

static void line_delete_word_backward(struct line_state *ls) {
    size_t old_pos = ls->pos;
    size_t diff;
    while (ls->pos > 0 && ls->buf[ls->pos - 1] == ' ') ls->pos--;
    while (ls->pos > 0 && ls->buf[ls->pos - 1] != ' ') ls->pos--;
    diff = old_pos - ls->pos;
    memmove(ls->buf + ls->pos, ls->buf + old_pos, ls->len - old_pos);
    ls->len -= diff;
    ls->buf[ls->len] = '\0';
}

/* ------------------------------------------------------------------ */
/*  Set buffer contents (for history navigation)                       */
/* ------------------------------------------------------------------ */

static void line_set_buf(struct line_state *ls, const char *s) {
    size_t slen = strlen(s);
    ls_ensure_cap(ls, slen);
    memcpy(ls->buf, s, slen);
    ls->len = slen;
    ls->pos = slen;
    ls->buf[ls->len] = '\0';
}

/* ------------------------------------------------------------------ */
/*  History navigation                                                 */
/* ------------------------------------------------------------------ */

static void history_up(struct line_state *ls) {
    const char *entry;
    int count = cupid_history_count();
    if (count == 0) return;

    if (ls->history_idx == -1) {
        free(ls->saved_line);
        ls->saved_line = strdup(ls->buf);
    }

    if (ls->history_idx + 1 < count) {
        ls->history_idx++;
        entry = cupid_history_get(ls->history_idx);
        if (entry != NULL) line_set_buf(ls, entry);
    }
}

static void history_down(struct line_state *ls) {
    if (ls->history_idx < 0) return;

    ls->history_idx--;
    if (ls->history_idx >= 0) {
        const char *entry = cupid_history_get(ls->history_idx);
        if (entry != NULL) line_set_buf(ls, entry);
    } else {
        if (ls->saved_line != NULL) {
            line_set_buf(ls, ls->saved_line);
        } else {
            ls->len = 0;
            ls->pos = 0;
            ls->buf[0] = '\0';
        }
    }
}

/* ------------------------------------------------------------------ */
/*  Tab completion helpers                                             */
/* ------------------------------------------------------------------ */

static void add_match(char ***matches, size_t *count, const char *s) {
    char **m = realloc(*matches, sizeof(char *) * (*count + 1));
    if (m == NULL) return;
    *matches = m;
    (*matches)[*count] = strdup(s);
    if ((*matches)[*count] == NULL) return;
    (*count)++;
}

static void add_match_owned(char ***matches, size_t *count, char *s) {
    char **m = realloc(*matches, sizeof(char *) * (*count + 1));
    if (m == NULL) { free(s); return; }
    *matches = m;
    (*matches)[*count] = s;
    (*count)++;
}

static int match_exists(char **matches, size_t count, const char *s) {
    size_t i;
    for (i = 0; i < count; i++) {
        if (strcmp(matches[i], s) == 0) return 1;
    }
    return 0;
}

static int cmp_strings(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

static void complete_command(const char *prefix, struct cupid_shell *shell,
                             char ***out, size_t *count) {
    size_t plen = strlen(prefix);
    const char **names;
    const char **bp;

    names = cupid_builtin_names();
    for (bp = names; *bp != NULL; bp++) {
        if (strncmp(*bp, prefix, plen) == 0 && !match_exists(*out, *count, *bp)) {
            add_match(out, count, *bp);
        }
    }

    if (shell != NULL) {
        size_t i;
        for (i = 0; i < shell->funcs.count; i++) {
            const char *fn = shell->funcs.entries[i].name;
            if (strncmp(fn, prefix, plen) == 0 && !match_exists(*out, *count, fn)) {
                add_match(out, count, fn);
            }
        }
    }

    {
        const char *path_env = getenv("PATH");
        const char *pe;
        if (path_env == NULL) return;
        pe = path_env;
        while (*pe != '\0') {
            const char *end = strchr(pe, ':');
            size_t dir_len;
            char dir_path[4096];
            DIR *dir;
            struct dirent *entry;

            if (end == NULL) end = pe + strlen(pe);
            dir_len = (size_t)(end - pe);
            if (dir_len == 0 || dir_len >= sizeof(dir_path)) {
                pe = (*end == ':') ? end + 1 : end;
                continue;
            }

            memcpy(dir_path, pe, dir_len);
            dir_path[dir_len] = '\0';

            dir = opendir(dir_path);
            if (dir != NULL) {
                while ((entry = readdir(dir)) != NULL) {
                    if (strncmp(entry->d_name, prefix, plen) == 0) {
                        char full[4096 + 256];
                        snprintf(full, sizeof(full), "%s/%s", dir_path, entry->d_name);
                        if (access(full, X_OK) == 0 &&
                            !match_exists(*out, *count, entry->d_name)) {
                            add_match(out, count, entry->d_name);
                        }
                    }
                }
                closedir(dir);
            }

            pe = (*end == ':') ? end + 1 : end;
        }
    }
}

static void complete_filename(const char *prefix, char ***out, size_t *count) {
    const char *slash = strrchr(prefix, '/');
    char dir_path[4096];
    const char *partial;
    size_t partial_len;
    DIR *dir;
    struct dirent *entry;

    if (slash != NULL) {
        size_t dl = (size_t)(slash - prefix + 1);
        if (dl >= sizeof(dir_path)) return;
        memcpy(dir_path, prefix, dl);
        dir_path[dl] = '\0';
        partial = slash + 1;
    } else {
        dir_path[0] = '.';
        dir_path[1] = '\0';
        partial = prefix;
    }
    partial_len = strlen(partial);

    dir = opendir(dir_path);
    if (dir == NULL) return;

    while ((entry = readdir(dir)) != NULL) {
        char *match;
        struct stat st;
        char full[4096 + 512];

        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
        if (entry->d_name[0] == '.' && (partial_len == 0 || partial[0] != '.')) continue;
        if (strncmp(entry->d_name, partial, partial_len) != 0) continue;

        if (slash != NULL) {
            size_t dl = (size_t)(slash - prefix + 1);
            size_t nl = strlen(entry->d_name);
            match = calloc(dl + nl + 2, 1);
            if (match == NULL) continue;
            memcpy(match, prefix, dl);
            memcpy(match + dl, entry->d_name, nl);
        } else {
            match = strdup(entry->d_name);
            if (match == NULL) continue;
        }

        snprintf(full, sizeof(full), "%s/%s", dir_path, entry->d_name);
        if (stat(full, &st) == 0 && S_ISDIR(st.st_mode)) {
            size_t ml = strlen(match);
            char *tmp = realloc(match, ml + 2);
            if (tmp == NULL) { free(match); continue; }
            match = tmp;
            match[ml] = '/';
            match[ml + 1] = '\0';
        }

        if (match_exists(*out, *count, match)) { free(match); continue; }
        add_match_owned(out, count, match);
    }
    closedir(dir);
}

/* ------------------------------------------------------------------ */
/*  Tab completion                                                     */
/* ------------------------------------------------------------------ */

static void handle_tab(struct line_state *ls) {
    size_t word_start, prefix_len, i;
    int is_command;
    char *prefix;
    char **matches = NULL;
    size_t match_count = 0;

    word_start = ls->pos;
    while (word_start > 0 && ls->buf[word_start - 1] != ' ' &&
           ls->buf[word_start - 1] != '\t')
        word_start--;

    prefix_len = ls->pos - word_start;
    if (prefix_len == 0) return;

    prefix = calloc(prefix_len + 1, 1);
    if (prefix == NULL) return;
    memcpy(prefix, ls->buf + word_start, prefix_len);

    is_command = 1;
    for (i = 0; i < word_start; i++) {
        if (ls->buf[i] != ' ' && ls->buf[i] != '\t') {
            is_command = 0;
            break;
        }
    }

    if (is_command && strchr(prefix, '/') == NULL) {
        complete_command(prefix, ls->shell, &matches, &match_count);
    }
    if (match_count == 0) {
        complete_filename(prefix, &matches, &match_count);
    }

    if (match_count == 0) {
        free(prefix);
        return;
    }

    if (match_count > 1) {
        qsort(matches, match_count, sizeof(char *), cmp_strings);
    }

    if (match_count == 1) {
        const char *suffix = matches[0] + prefix_len;
        size_t suffix_len = strlen(suffix);
        int is_dir = (strlen(matches[0]) > 0 &&
                      matches[0][strlen(matches[0]) - 1] == '/');

        ls_ensure_cap(ls, ls->len + suffix_len + 2);
        if (ls->pos < ls->len) {
            memmove(ls->buf + ls->pos + suffix_len, ls->buf + ls->pos,
                    ls->len - ls->pos);
        }
        memcpy(ls->buf + ls->pos, suffix, suffix_len);
        ls->pos += suffix_len;
        ls->len += suffix_len;

        if (!is_dir) {
            if (ls->pos < ls->len) {
                memmove(ls->buf + ls->pos + 1, ls->buf + ls->pos,
                        ls->len - ls->pos);
            }
            ls->buf[ls->pos] = ' ';
            ls->pos++;
            ls->len++;
        }
        ls->buf[ls->len] = '\0';
        refresh_line(ls);
    } else {
        size_t common_len = strlen(matches[0]);
        for (i = 1; i < match_count; i++) {
            size_t j;
            size_t ml = strlen(matches[i]);
            if (ml < common_len) common_len = ml;
            for (j = 0; j < common_len; j++) {
                if (matches[i][j] != matches[0][j]) { common_len = j; break; }
            }
        }

        if (common_len > prefix_len) {
            size_t to_insert = common_len - prefix_len;
            ls_ensure_cap(ls, ls->len + to_insert + 1);
            if (ls->pos < ls->len) {
                memmove(ls->buf + ls->pos + to_insert, ls->buf + ls->pos,
                        ls->len - ls->pos);
            }
            memcpy(ls->buf + ls->pos, matches[0] + prefix_len, to_insert);
            ls->pos += to_insert;
            ls->len += to_insert;
            ls->buf[ls->len] = '\0';
        }

        out_raw("\r\n", 2);
        for (i = 0; i < match_count; i++) {
            out_str(matches[i]);
            out_raw("  ", 2);
        }
        out_raw("\r\n", 2);

        out_str(ls->prompt);
        if (ls->len > 0) out_raw(ls->buf, ls->len);
        if (ls->pos < ls->len) {
            char seq[64];
            int n = snprintf(seq, sizeof(seq), "\033[%zuD", ls->len - ls->pos);
            if (n > 0) out_raw(seq, (size_t)n);
        }
    }

    free(prefix);
    for (i = 0; i < match_count; i++) free(matches[i]);
    free(matches);
}

/* ------------------------------------------------------------------ */
/*  Read one character (handles EINTR)                                 */
/* ------------------------------------------------------------------ */

static int read_char(char *c) {
    ssize_t r;
    for (;;) {
        r = read(STDIN_FILENO, c, 1);
        if (r == 1) return 1;
        if (r == 0) return 0;
        if (errno != EINTR) return -1;
    }
}

/* ------------------------------------------------------------------ */
/*  Process escape sequences                                           */
/* ------------------------------------------------------------------ */

static void handle_escape(struct line_state *ls) {
    char seq[3];
    if (read_char(&seq[0]) <= 0) return;

    if (seq[0] == '[') {
        if (read_char(&seq[1]) <= 0) return;

        if (seq[1] >= '0' && seq[1] <= '9') {
            if (read_char(&seq[2]) <= 0) return;
            if (seq[2] == '~') {
                switch (seq[1]) {
                case '3': line_delete_at_cursor(ls); break;
                case '1': ls->pos = 0; break;
                case '4': ls->pos = ls->len; break;
                default: break;
                }
            }
            return;
        }

        switch (seq[1]) {
        case 'A': history_up(ls); break;
        case 'B': history_down(ls); break;
        case 'C': if (ls->pos < ls->len) ls->pos++; break;
        case 'D': if (ls->pos > 0) ls->pos--; break;
        case 'H': ls->pos = 0; break;
        case 'F': ls->pos = ls->len; break;
        default: break;
        }
    } else if (seq[0] == 'O') {
        if (read_char(&seq[1]) <= 0) return;
        switch (seq[1]) {
        case 'H': ls->pos = 0; break;
        case 'F': ls->pos = ls->len; break;
        default: break;
        }
    }
}

/* ------------------------------------------------------------------ */
/*  Public API                                                         */
/* ------------------------------------------------------------------ */

int cupid_line_edit_init(void) {
    if (tcgetattr(STDIN_FILENO, &g_orig_termios) != 0) return -1;
    g_have_orig = 1;
    atexit(restore_terminal);
    return 0;
}

void cupid_line_edit_cleanup(void) {
    restore_terminal();
}

char *cupid_line_read(const char *prompt, struct cupid_shell *shell) {
    struct line_state ls;
    char c;

    memset(&ls, 0, sizeof(ls));
    ls.prompt = prompt != NULL ? prompt : "$ ";
    ls.prompt_vlen = prompt_visible_len(ls.prompt);
    ls.buf = calloc(256, 1);
    if (ls.buf == NULL) return NULL;
    ls.cap = 256;
    ls.shell = shell;
    ls.history_idx = -1;
    ls.saved_line = NULL;

    if (enter_raw_mode() != 0) {
        free(ls.buf);
        return NULL;
    }

    out_str(ls.prompt);

    for (;;) {
        int rc = read_char(&c);
        if (rc <= 0) {
            if (ls.len == 0) {
                leave_raw_mode();
                out_raw("\r\n", 2);
                free(ls.buf);
                free(ls.saved_line);
                return NULL;
            }
            break;
        }

        switch (c) {
        case '\r':
        case '\n':
            out_raw("\r\n", 2);
            leave_raw_mode();
            ls.buf[ls.len] = '\0';
            free(ls.saved_line);
            return ls.buf;

        case 3: /* Ctrl-C */
            out_raw("^C\r\n", 4);
            ls.len = 0;
            ls.pos = 0;
            ls.buf[0] = '\0';
            ls.history_idx = -1;
            free(ls.saved_line);
            ls.saved_line = NULL;
            if (ls.shell != NULL) ls.shell->last_status = 130;
            refresh_line(&ls);
            continue;

        case 4: /* Ctrl-D */
            if (ls.len == 0) {
                leave_raw_mode();
                out_raw("\r\n", 2);
                free(ls.buf);
                free(ls.saved_line);
                return NULL;
            }
            line_delete_at_cursor(&ls);
            break;

        case 8:   /* Ctrl-H */
        case 127: /* Backspace */
            line_backspace(&ls);
            break;

        case 9: /* Tab */
            handle_tab(&ls);
            continue;

        case 1: /* Ctrl-A: Home */
            ls.pos = 0;
            break;

        case 5: /* Ctrl-E: End */
            ls.pos = ls.len;
            break;

        case 11: /* Ctrl-K: kill to end */
            line_kill_to_end(&ls);
            break;

        case 21: /* Ctrl-U: kill to start */
            line_kill_to_start(&ls);
            break;

        case 23: /* Ctrl-W: delete word backward */
            line_delete_word_backward(&ls);
            break;

        case 12: /* Ctrl-L: clear screen */
            out_raw("\033[H\033[2J", 7);
            refresh_line(&ls);
            continue;

        case 27: /* ESC */
            handle_escape(&ls);
            break;

        default:
            if (c >= 32) {
                line_insert_char(&ls, c);
            }
            break;
        }

        refresh_line(&ls);
    }

    leave_raw_mode();
    ls.buf[ls.len] = '\0';
    free(ls.saved_line);
    return ls.buf;
}
