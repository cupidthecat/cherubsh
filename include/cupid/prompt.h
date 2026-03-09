#ifndef CUPID_PROMPT_H
#define CUPID_PROMPT_H

struct cupid_shell;

char *cupid_prompt_expand(struct cupid_shell *shell, const char *ps);

#endif
