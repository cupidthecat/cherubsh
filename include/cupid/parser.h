#ifndef CUPID_PARSER_H
#define CUPID_PARSER_H

#include "cupid/ast.h"
#include "cupid/token.h"

struct cupid_parse_opts {
    int posix_mode;
};

int cupid_parse(const struct cupid_tokens *tokens, const struct cupid_parse_opts *opts, struct cupid_ast **out_ast);

#endif
