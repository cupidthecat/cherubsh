#ifndef CUPID_EXEC_H
#define CUPID_EXEC_H

#include "cupid/ast.h"
#include "cupid/shell.h"

int cupid_execute_ast(struct cupid_shell *shell, const struct cupid_ast *ast);

#endif
