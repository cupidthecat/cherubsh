#include "cupid/history.h"

#include <fnmatch.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DEFAULT_HISTSIZE     1000
#define DEFAULT_HISTFILESIZE 2000

static char **g_entries;
static int g_count;
static int g_capacity;

static char *get_history_path(void) {
    const char *histfile = getenv("HISTFILE");
    const char *home = getenv("HOME");
    size_t len;
    char *path;
    if (histfile != NULL && histfile[0] != '\0') {
        return strdup(histfile);
    }
    if (home == NULL) return NULL;
    len = strlen(home) + 20;
    path = malloc(len);
    if (path == NULL) return NULL;
    snprintf(path, len, "%s/.cupid_history", home);
    return path;
}

static int get_histsize(void) {
    const char *s = getenv("HISTSIZE");
    if (s != NULL) {
        long v = strtol(s, NULL, 10);
        if (v > 0) return (int)v;
    }
    return DEFAULT_HISTSIZE;
}

static int get_histfilesize(void) {
    const char *s = getenv("HISTFILESIZE");
    if (s != NULL) {
        long v = strtol(s, NULL, 10);
        if (v > 0) return (int)v;
    }
    return DEFAULT_HISTFILESIZE;
}

static int should_ignore(const char *line) {
    const char *ignore = getenv("HISTIGNORE");
    const char *ctrl = getenv("HISTCONTROL");
    char *copy;
    char *p;
    if (ctrl != NULL) {
        if (strstr(ctrl, "ignorespace") != NULL || strstr(ctrl, "ignoreboth") != NULL) {
            if (line[0] == ' ') return 1;
        }
        if (strstr(ctrl, "ignoredups") != NULL || strstr(ctrl, "ignoreboth") != NULL) {
            if (g_count > 0 && strcmp(g_entries[g_count - 1], line) == 0) return 1;
        }
    }
    if (ignore == NULL || ignore[0] == '\0') return 0;

    copy = strdup(ignore);
    if (copy == NULL) return 0;
    p = copy;
    while (p != NULL && *p != '\0') {
        char *next = strchr(p, ':');
        if (next != NULL) *next = '\0';
        if (strcmp(p, "&") == 0) {
            if (g_count > 0 && strcmp(g_entries[g_count - 1], line) == 0) {
                free(copy);
                return 1;
            }
        } else if (*p != '\0') {
            if (fnmatch(p, line, 0) == 0) {
                free(copy);
                return 1;
            }
        }
        if (next == NULL) break;
        p = next + 1;
    }
    free(copy);
    return 0;
}

static int ensure_capacity(void) {
    if (g_count < g_capacity) return 0;
    {
        int new_cap = g_capacity == 0 ? 256 : g_capacity * 2;
        char **ne = realloc(g_entries, sizeof(char *) * (size_t)new_cap);
        if (ne == NULL) return -1;
        g_entries = ne;
        g_capacity = new_cap;
    }
    return 0;
}

static void trim_to_size(int maxsize) {
    while (g_count > maxsize && g_count > 0) {
        free(g_entries[0]);
        memmove(g_entries, g_entries + 1, sizeof(char *) * (size_t)(g_count - 1));
        g_count--;
    }
}

void cupid_history_init(void) {
    g_entries = NULL;
    g_count = 0;
    g_capacity = 0;
}

void cupid_history_cleanup(void) {
    int i;
    char *path = get_history_path();
    if (path != NULL && g_count > 0) {
        int maxfile = get_histfilesize();
        FILE *f = fopen(path, "w");
        if (f != NULL) {
            int start = 0;
            if (g_count > maxfile) start = g_count - maxfile;
            for (i = start; i < g_count; i++) {
                fprintf(f, "%s\n", g_entries[i]);
            }
            fclose(f);
        }
    }
    free(path);

    for (i = 0; i < g_count; i++) {
        free(g_entries[i]);
    }
    free(g_entries);
    g_entries = NULL;
    g_count = 0;
    g_capacity = 0;
}

void cupid_history_load(void) {
    char *path = get_history_path();
    FILE *f;
    char *line = NULL;
    size_t cap = 0;
    int maxsize;

    if (path == NULL) return;
    f = fopen(path, "r");
    free(path);
    if (f == NULL) return;

    maxsize = get_histsize();

    while (getline(&line, &cap, f) >= 0) {
        size_t len = strlen(line);
        if (len > 0 && line[len - 1] == '\n') line[len - 1] = '\0';
        if (line[0] == '\0') continue;

        if (ensure_capacity() != 0) break;
        g_entries[g_count] = strdup(line);
        if (g_entries[g_count] == NULL) break;
        g_count++;
        trim_to_size(maxsize);
    }

    free(line);
    fclose(f);
}

void cupid_history_add(const char *line) {
    char *path;
    FILE *f;

    if (line == NULL || line[0] == '\0') return;
    if (should_ignore(line)) return;

    if (ensure_capacity() != 0) return;
    g_entries[g_count] = strdup(line);
    if (g_entries[g_count] == NULL) return;
    g_count++;
    trim_to_size(get_histsize());

    path = get_history_path();
    if (path != NULL) {
        f = fopen(path, "a");
        if (f != NULL) {
            fprintf(f, "%s\n", line);
            fclose(f);
        }
        free(path);
    }
}

const char *cupid_history_get(int index) {
    int real_idx;
    if (index < 0 || index >= g_count) return NULL;
    real_idx = g_count - 1 - index;
    return g_entries[real_idx];
}

int cupid_history_count(void) {
    return g_count;
}

void cupid_history_clear(void) {
    int i;
    for (i = 0; i < g_count; i++) {
        free(g_entries[i]);
    }
    g_count = 0;
}
