#ifndef CUPID_BRACE_H
#define CUPID_BRACE_H

#include <stddef.h>

int cupid_brace_expand(const char *word, char ***out_words, size_t *out_count);

#endif
