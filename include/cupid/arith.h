#ifndef CUPID_ARITH_H
#define CUPID_ARITH_H

struct cupid_shell;

long cupid_arith_eval(struct cupid_shell *shell, const char *expr, int *error);

#endif
