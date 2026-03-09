#ifndef CUPID_BUILTINS_H
#define CUPID_BUILTINS_H

#include <stdbool.h>

#include "cupid/shell.h"

#define CUPID_BUILTIN_NOT_FOUND 1000

int cupid_run_builtin(struct cupid_shell *shell, int argc, char **argv, bool in_child);
int cupid_is_builtin(const char *name);
const char **cupid_builtin_names(void);
char *cupid_find_in_path(const char *name);

#endif
