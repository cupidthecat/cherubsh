#ifndef CUPID_LEXER_H
#define CUPID_LEXER_H

#include "cupid/token.h"

int cupid_lex(const char *input, struct cupid_tokens *out);

#endif
