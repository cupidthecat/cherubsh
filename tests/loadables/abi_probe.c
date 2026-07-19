#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct word_desc {
    char *word;
    int flags;
} WORD_DESC;

typedef struct word_list {
    struct word_list *next;
    WORD_DESC *word;
} WORD_LIST;

typedef int sh_builtin_func_t(WORD_LIST *);

typedef struct shell_var {
    char *name;
    char *value;
    char *exportstr;
    void *dynamic_value;
    void *assign_func;
    int attributes;
    int context;
} SHELL_VAR;

typedef struct array ARRAY;

struct builtin {
    char *name;
    sh_builtin_func_t *function;
    int flags;
    char *const *long_doc;
    const char *short_doc;
    char *handle;
};

extern char *list_optarg;
extern WORD_LIST *loptend;

extern int internal_getopt(WORD_LIST *list, char *options);
extern void reset_internal_getopt(void);
extern void builtin_help(void);
extern SHELL_VAR *bind_variable(const char *name, const char *value, int flags);
extern SHELL_VAR *find_or_make_array_variable(const char *name, int flags);
extern int array_insert(ARRAY *array, long index, char *value);
extern SHELL_VAR *bind_assoc_variable(SHELL_VAR *variable, const char *name,
                                      char *key, const char *value, int flags);

int abi_probe_builtin_load(const char *name)
{
    (void)name;
    bind_variable("PROBE_LOAD", "loaded", 0);
    return 1;
}

void abi_probe_builtin_unload(const char *name)
{
    (void)name;
    bind_variable("PROBE_UNLOAD", "unloaded", 0);
}

static int abi_probe_builtin(WORD_LIST *list)
{
    const char *scalar = "";
    const char *assoc_key = "";
    const char *assoc_value = "";
    long index = 0;
    int option;
    SHELL_VAR *variable;

    reset_internal_getopt();
    while ((option = internal_getopt(list, "s:i:k:v:")) != -1) {
        if (option == -99) {
            builtin_help();
            return 0;
        }
        if (option == '?')
            return 2;
        switch (option) {
        case 's':
            scalar = list_optarg;
            break;
        case 'i':
            index = strtol(list_optarg, NULL, 10);
            break;
        case 'k':
            assoc_key = list_optarg;
            break;
        case 'v':
            assoc_value = list_optarg;
            break;
        }
    }

    bind_variable("PROBE_SCALAR", scalar, 0);
    variable = find_or_make_array_variable("PROBE_INDEXED", 1);
    if (variable == NULL || array_insert((ARRAY *)variable->value, index,
                                         (char *)scalar) != 0)
        return 1;
    variable = find_or_make_array_variable("PROBE_ASSOC", 3);
    if (variable == NULL ||
        bind_assoc_variable(variable, variable->name, strdup(assoc_key),
                            assoc_value, 0) == NULL)
        return 1;

    printf("module:");
    for (; loptend; loptend = loptend->next)
        printf("%s%s", loptend == list ? "" : ",", loptend->word->word);
    printf("\n");
    return 0;
}

static char *abi_probe_doc[] = {
    "Exercise the public Bash loadable-builtin interface.",
    NULL,
};

struct builtin abi_probe_struct = {
    "abi_probe",
    abi_probe_builtin,
    0x01,
    abi_probe_doc,
    "abi_probe [-s value] [-i index] [-k key] [-v value] [argument ...]",
    NULL,
};
