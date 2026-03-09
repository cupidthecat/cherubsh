#ifndef CUPID_HEREDOC_H
#define CUPID_HEREDOC_H

#include <stdbool.h>

struct cupid_shell;

int cupid_make_heredoc_fd(const char *delimiter, const char *body_text,
                          bool quoted, bool strip_tabs, struct cupid_shell *shell);

#endif
