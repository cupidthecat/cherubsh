#ifndef CUPID_EXPAND_H
#define CUPID_EXPAND_H

#include "cupid/token.h"

struct cupid_shell;

#define CUPID_ESC_IFS_SPACE_PLACEHOLDER '\x1d'
#define CUPID_ESC_IFS_TAB_PLACEHOLDER '\x1e'
#define CUPID_ESC_IFS_NEWLINE_PLACEHOLDER '\x1f'
#define CUPID_ESC_BACKTICK_PLACEHOLDER '\x1c'

char *cupid_expand_text(const char *src, enum cupid_quote quote, struct cupid_shell *shell);
char *cupid_expand_word(const struct cupid_word *word, struct cupid_shell *shell);
char *cupid_expand_case_pattern(const struct cupid_word *word, struct cupid_shell *shell);
char *cupid_word_literal(const struct cupid_word *word);
char *cupid_word_source_text(const struct cupid_word *word);
char *cupid_word_dequote_literal(const struct cupid_word *word);
char *cupid_expand_tilde(const char *text, struct cupid_shell *shell);
void cupid_expand_error_reset(void);
int cupid_expand_error_pending(void);
const char *cupid_expand_error_message(void);
int cupid_expand_error_set(const char *message);
void cupid_restore_escaped_ifs_placeholders(char *text);

#endif
