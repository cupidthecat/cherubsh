#ifndef CUPID_LINE_EDIT_H
#define CUPID_LINE_EDIT_H

struct cupid_shell;

int cupid_line_edit_init(void);
void cupid_line_edit_cleanup(void);
char *cupid_line_read(const char *prompt, struct cupid_shell *shell);

#endif
