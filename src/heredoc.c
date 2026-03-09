#include "cupid/heredoc.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "cupid/expand.h"
#include "cupid/shell.h"

static int write_heredoc_line(int fd, const char *line, bool quoted, struct cupid_shell *shell) {
    char *out;
    int should_stop = 0;

    if (!quoted) {
        out = cupid_expand_text(line, CUPID_QUOTE_NONE, shell);
        if (out == NULL) {
            return -1;
        }
    } else {
        out = strdup(line);
        if (out == NULL) {
            return -1;
        }
    }

    if (write(fd, out, strlen(out)) < 0 || write(fd, "\n", 1) < 0) {
        should_stop = 1;
    }
    free(out);
    return should_stop ? -1 : 0;
}

int cupid_make_heredoc_fd(const char *delimiter, const char *body_text,
                          bool quoted, bool strip_tabs, struct cupid_shell *shell) {
    int fds[2];
    char *line = NULL;
    size_t cap = 0;

    if (delimiter == NULL) {
        return -1;
    }
    if (pipe(fds) != 0) {
        return -1;
    }

    if (body_text != NULL) {
        const char *cursor = body_text;
        while (*cursor != '\0') {
            const char *line_end = cursor;
            size_t len;
            char *body;
            while (*line_end != '\0' && *line_end != '\n') line_end++;
            len = (size_t)(line_end - cursor);
            body = calloc(len + 1, 1);
            if (body == NULL) {
                close(fds[0]);
                close(fds[1]);
                return -1;
            }
            if (len > 0) memcpy(body, cursor, len);
            if (strip_tabs) {
                char *trimmed = body;
                while (*trimmed == '\t') trimmed++;
                if (trimmed != body) memmove(body, trimmed, strlen(trimmed) + 1);
            }
            if (write_heredoc_line(fds[1], body, quoted, shell) != 0) {
                free(body);
                close(fds[0]);
                close(fds[1]);
                return -1;
            }
            free(body);
            cursor = line_end;
            if (*cursor == '\n') cursor++;
        }
        close(fds[1]);
        return fds[0];
    }

    while (1) {
        ssize_t nread;
        size_t body_len;
        char *body;

        nread = getline(&line, &cap, stdin);
        if (nread < 0) {
            break;
        }
        body_len = (size_t)nread;
        if (body_len > 0 && line[body_len - 1] == '\n') {
            line[body_len - 1] = '\0';
            body_len--;
        }
        body = line;
        if (strip_tabs) {
            while (*body == '\t') {
                body++;
            }
            body_len = strlen(body);
        }
        if (strcmp(body, delimiter) == 0) {
            break;
        }
        if (write_heredoc_line(fds[1], body, quoted, shell) != 0) {
            close(fds[0]);
            close(fds[1]);
            free(line);
            return -1;
        }
    }

    free(line);
    close(fds[1]);
    return fds[0];
}
