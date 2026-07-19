#include <stddef.h>

typedef struct word_desc {
    char *word;
    int flags;
} WORD_DESC;

typedef struct word_list {
    struct word_list *next;
    WORD_DESC *word;
} WORD_LIST;

typedef int sh_builtin_func_t(WORD_LIST *);

struct builtin {
    char *name;
    sh_builtin_func_t *function;
    int flags;
    char *const *long_doc;
    const char *short_doc;
    char *handle;
};

extern int strmatch(char *pattern, char *string, int flags);

static int fixture_strmatch(WORD_LIST *list) {
    if (list == NULL || list->word == NULL || list->next == NULL ||
        list->next->word == NULL) {
        return 2;
    }
    return strmatch(list->next->word->word, list->word->word,
                    (1 << 0) | (1 << 2) | (1 << 5));
}

static char *fixture_doc[] = {
    "Test the Bash loadable builtin matcher ABI.",
    NULL,
};

struct builtin fixture_strmatch_struct = {
    "fixture_strmatch",
    fixture_strmatch,
    0x01,
    fixture_doc,
    "fixture_strmatch string pattern",
    NULL,
};
