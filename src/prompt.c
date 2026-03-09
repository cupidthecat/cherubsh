#include "cupid/prompt.h"

#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "cupid/shell.h"

static void buf_append(char **buf, int *len, int *cap, const char *s, int slen) {
    if (*len + slen + 1 > *cap) {
        int nc = *cap;
        char *nb;
        while (nc < *len + slen + 1) nc *= 2;
        nb = realloc(*buf, (size_t)nc);
        if (nb == NULL) return;
        *buf = nb;
        *cap = nc;
    }
    memcpy(*buf + *len, s, (size_t)slen);
    *len += slen;
    (*buf)[*len] = '\0';
}

static void buf_append_str(char **buf, int *len, int *cap, const char *s) {
    buf_append(buf, len, cap, s, (int)strlen(s));
}

static void buf_append_char(char **buf, int *len, int *cap, char c) {
    buf_append(buf, len, cap, &c, 1);
}

static const char *get_username(void) {
    const char *u = getenv("USER");
    if (u != NULL) return u;
    {
        struct passwd *pw = getpwuid(getuid());
        if (pw != NULL) return pw->pw_name;
    }
    return "?";
}

static void get_hostname(char *out, size_t out_sz, int full) {
    if (gethostname(out, out_sz) != 0) {
        out[0] = '?';
        out[1] = '\0';
        return;
    }
    out[out_sz - 1] = '\0';
    if (!full) {
        char *dot = strchr(out, '.');
        if (dot != NULL) *dot = '\0';
    }
}

static void get_cwd_display(char *out, size_t out_sz, int basename_only) {
    char cwd[4096];
    const char *home;
    size_t hlen;

    if (getcwd(cwd, sizeof(cwd)) == NULL) {
        out[0] = '?';
        out[1] = '\0';
        return;
    }

    home = getenv("HOME");
    if (home != NULL) {
        hlen = strlen(home);
        if (strncmp(cwd, home, hlen) == 0 &&
            (cwd[hlen] == '/' || cwd[hlen] == '\0')) {
            char tilde[4096];
            snprintf(tilde, sizeof(tilde), "~%s", cwd + hlen);
            if (basename_only) {
                const char *last = strrchr(tilde, '/');
                snprintf(out, out_sz, "%s", last ? last + 1 : tilde);
            } else {
                snprintf(out, out_sz, "%s", tilde);
            }
            return;
        }
    }

    if (basename_only) {
        const char *last = strrchr(cwd, '/');
        snprintf(out, out_sz, "%s", last ? last + 1 : cwd);
    } else {
        snprintf(out, out_sz, "%s", cwd);
    }
}

char *cupid_prompt_expand(struct cupid_shell *shell, const char *ps) {
    char *buf;
    int len = 0;
    int cap = 256;
    const char *p;

    (void)shell;
    if (ps == NULL) return strdup("$ ");

    buf = malloc((size_t)cap);
    if (buf == NULL) return strdup("$ ");
    buf[0] = '\0';

    p = ps;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            p++;
            switch (*p) {
            case 'u':
                buf_append_str(&buf, &len, &cap, get_username());
                break;
            case 'h': {
                char host[256];
                get_hostname(host, sizeof(host), 0);
                buf_append_str(&buf, &len, &cap, host);
                break;
            }
            case 'H': {
                char host[256];
                get_hostname(host, sizeof(host), 1);
                buf_append_str(&buf, &len, &cap, host);
                break;
            }
            case 'w': {
                char cwd[4096];
                get_cwd_display(cwd, sizeof(cwd), 0);
                buf_append_str(&buf, &len, &cap, cwd);
                break;
            }
            case 'W': {
                char cwd[4096];
                get_cwd_display(cwd, sizeof(cwd), 1);
                buf_append_str(&buf, &len, &cap, cwd);
                break;
            }
            case '$':
                buf_append_char(&buf, &len, &cap, (getuid() == 0) ? '#' : '$');
                break;
            case 'n':
                buf_append_char(&buf, &len, &cap, '\n');
                break;
            case 't': {
                time_t now = time(NULL);
                struct tm *tm = localtime(&now);
                char tbuf[16];
                snprintf(tbuf, sizeof(tbuf), "%02d:%02d:%02d",
                         tm->tm_hour, tm->tm_min, tm->tm_sec);
                buf_append_str(&buf, &len, &cap, tbuf);
                break;
            }
            case 'd': {
                time_t now = time(NULL);
                struct tm *tm = localtime(&now);
                char dbuf[64];
                static const char *days[] = {"Sun","Mon","Tue","Wed","Thu","Fri","Sat"};
                static const char *mons[] = {"Jan","Feb","Mar","Apr","May","Jun",
                                             "Jul","Aug","Sep","Oct","Nov","Dec"};
                snprintf(dbuf, sizeof(dbuf), "%s %s %02d",
                         days[tm->tm_wday], mons[tm->tm_mon], tm->tm_mday);
                buf_append_str(&buf, &len, &cap, dbuf);
                break;
            }
            case 'e':
                buf_append_char(&buf, &len, &cap, '\033');
                break;
            case '[':
                buf_append_char(&buf, &len, &cap, '\001');
                break;
            case ']':
                buf_append_char(&buf, &len, &cap, '\002');
                break;
            case '\\':
                buf_append_char(&buf, &len, &cap, '\\');
                break;
            case 'a':
                buf_append_char(&buf, &len, &cap, '\007');
                break;
            default:
                buf_append_char(&buf, &len, &cap, '\\');
                buf_append_char(&buf, &len, &cap, *p);
                break;
            }
            p++;
        } else {
            buf_append_char(&buf, &len, &cap, *p);
            p++;
        }
    }
    return buf;
}
