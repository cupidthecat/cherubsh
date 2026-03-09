#include "cupid/vars.h"

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

#include "cupid/arith.h"
#include "cupid/shell.h"

static char g_special_buf[64];
static char g_platform_buf[128];
#define CUPID_NAMEREF_MAX_DEPTH 64
#define CUPID_ASSOC_BUCKETS 128

static const char *platform_hosttype(void) {
    struct utsname uts;
    if (uname(&uts) != 0 || uts.machine[0] == '\0') return "unknown";
    snprintf(g_platform_buf, sizeof(g_platform_buf), "%s", uts.machine);
    return g_platform_buf;
}

static const char *platform_ostype(void) {
    struct utsname uts;
    size_t i;
    size_t len;
    if (uname(&uts) != 0 || uts.sysname[0] == '\0') return "unknown";
    len = strlen(uts.sysname);
    if (len + 5 >= sizeof(g_platform_buf)) len = sizeof(g_platform_buf) - 5;
    for (i = 0; i < len; i++) g_platform_buf[i] = (char)tolower((unsigned char)uts.sysname[i]);
    memcpy(g_platform_buf + len, "-gnu", 5);
    return g_platform_buf;
}

static const char *platform_machtype(void) {
    struct utsname uts;
    const char *ostype;
    if (uname(&uts) != 0 || uts.machine[0] == '\0') return "unknown-unknown";
    ostype = platform_ostype();
    snprintf(g_platform_buf, sizeof(g_platform_buf), "%s-pc-%s", uts.machine, ostype);
    return g_platform_buf;
}

static const char *lookup_special_var(struct cupid_shell *shell, const char *name) {
    if (shell == NULL || name == NULL) return NULL;
    if (strcmp(name, "RANDOM") == 0) {
        snprintf(g_special_buf, sizeof(g_special_buf), "%d", rand() % 32768);
        return g_special_buf;
    }
    if (strcmp(name, "LINENO") == 0) {
        snprintf(g_special_buf, sizeof(g_special_buf), "%d", shell->lineno);
        return g_special_buf;
    }
    if (strcmp(name, "SECONDS") == 0) {
        time_t now = time(NULL);
        long elapsed = (long)(now - shell->start_time);
        snprintf(g_special_buf, sizeof(g_special_buf), "%ld", elapsed);
        return g_special_buf;
    }
    if (strcmp(name, "BASH_VERSION") == 0) {
        return "5.2.21(1)-cupid";
    }
    if (strcmp(name, "BASHPID") == 0) {
        snprintf(g_special_buf, sizeof(g_special_buf), "%ld", (long)getpid());
        return g_special_buf;
    }
    if (strcmp(name, "HOSTTYPE") == 0) {
        return platform_hosttype();
    }
    if (strcmp(name, "OSTYPE") == 0) {
        return platform_ostype();
    }
    if (strcmp(name, "MACHTYPE") == 0) {
        return platform_machtype();
    }
    return NULL;
}

static void vars_error_prefix(struct cupid_shell *shell) {
    cupid_shell_error_prefix(stderr, shell);
}

int cupid_vars_init_defaults(struct cupid_shell *shell) {
    const char *items[6];
    int rc;
    if (shell == NULL) return -1;
    items[0] = "5";
    items[1] = "2";
    items[2] = "21";
    items[3] = "1";
    items[4] = "release";
    items[5] = platform_machtype();
    rc = cupid_array_set_list(shell, "BASH_VERSINFO", (char **)items, 6);
    if (rc != 0) return rc;
    return cupid_vars_set(shell, "IFS", " \t\n");
}

static struct cupid_var *find_var(struct cupid_shell *shell, const char *name) {
    struct cupid_var *best = NULL;
    size_t i;
    for (i = 0; i < shell->vars.count; i++) {
        if (strcmp(shell->vars.entries[i].name, name) == 0) {
            if (best == NULL || shell->vars.entries[i].scope >= best->scope) {
                best = &shell->vars.entries[i];
            }
        }
    }
    return best;
}

static struct cupid_var *find_var_at_scope(struct cupid_shell *shell, const char *name, int scope) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->vars.count; i++) {
        if (shell->vars.entries[i].scope == scope &&
            strcmp(shell->vars.entries[i].name, name) == 0) {
            return &shell->vars.entries[i];
        }
    }
    return NULL;
}

static void remove_var_at_index(struct cupid_shell *shell, size_t idx) {
    struct cupid_var *v;
    if (shell == NULL || idx >= shell->vars.count) return;
    v = &shell->vars.entries[idx];
    free(v->name);
    free(v->value);
    free(v->nameref_target);
    if (idx + 1 < shell->vars.count) {
        shell->vars.entries[idx] = shell->vars.entries[shell->vars.count - 1];
    }
    shell->vars.count--;
}

static struct cupid_var *find_var_follow(struct cupid_shell *shell, const char *name,
                                         const char **final_name_out, int *looped) {
    const char *cur = name;
    int depth;
    if (final_name_out != NULL) *final_name_out = name;
    if (looped != NULL) *looped = 0;
    for (depth = 0; depth < CUPID_NAMEREF_MAX_DEPTH; depth++) {
        struct cupid_var *v = find_var(shell, cur);
        if (v == NULL || v->nameref_target == NULL || v->nameref_target[0] == '\0') {
            if (final_name_out != NULL) *final_name_out = cur;
            return v;
        }
        cur = v->nameref_target;
    }
    if (looped != NULL) *looped = 1;
    if (final_name_out != NULL) *final_name_out = cur;
    return NULL;
}

static int nameref_would_cycle(struct cupid_shell *shell, const char *name, const char *target) {
    const char *cur = target;
    int depth;
    if (name == NULL || target == NULL) return 1;
    if (target[0] == '\0') return 0;
    for (depth = 0; depth < CUPID_NAMEREF_MAX_DEPTH; depth++) {
        struct cupid_var *v;
        if (strcmp(cur, name) == 0) return 1;
        v = find_var(shell, cur);
        if (v == NULL || v->nameref_target == NULL || v->nameref_target[0] == '\0') return 0;
        cur = v->nameref_target;
    }
    return 1;
}

static struct cupid_array *find_array(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return NULL;
    for (i = 0; i < shell->arrays.count; i++) {
        if (strcmp(shell->arrays.entries[i].name, name) == 0) {
            return &shell->arrays.entries[i];
        }
    }
    return NULL;
}

static void free_array_storage(struct cupid_array *a) {
    size_t i;
    if (a == NULL) return;
    for (i = 0; i < a->count; i++) {
        free(a->keys != NULL ? a->keys[i] : NULL);
        free(a->items[i]);
    }
    free(a->keys);
    free(a->items);
    a->keys = NULL;
    a->items = NULL;
    a->count = 0;
}

static struct cupid_array *ensure_array(struct cupid_shell *shell, const char *name) {
    struct cupid_array *a = find_array(shell, name);
    if (a != NULL) return a;
    if (shell == NULL || name == NULL) return NULL;
    if (shell->arrays.count == shell->arrays.capacity) {
        size_t nc = (shell->arrays.capacity == 0) ? 8 : shell->arrays.capacity * 2;
        struct cupid_array *ne = realloc(shell->arrays.entries, sizeof(*ne) * nc);
        if (ne == NULL) return NULL;
        shell->arrays.entries = ne;
        shell->arrays.capacity = nc;
    }
    a = &shell->arrays.entries[shell->arrays.count];
    memset(a, 0, sizeof(*a));
    a->name = strdup(name);
    if (a->name == NULL) return NULL;
    shell->arrays.count++;
    return a;
}

static int parse_unsigned_key(const char *key, size_t *out) {
    size_t i;
    size_t value = 0;
    if (key == NULL || key[0] == '\0') return -1;
    for (i = 0; key[i] != '\0'; i++) {
        unsigned char ch = (unsigned char)key[i];
        if (!isdigit(ch)) return -1;
        value = value * 10u + (size_t)(ch - '0');
    }
    if (out != NULL) *out = value;
    return 0;
}

static char *apply_var_attributes(struct cupid_shell *shell, const struct cupid_var *var, const char *value) {
    char *new_value;

    if (value == NULL) value = "";
    if (var != NULL && var->integer) {
        int arith_err = 0;
        long arith_val = cupid_arith_eval(shell, value, &arith_err);
        char numbuf[64];
        int n;

        if (arith_err) return NULL;
        n = snprintf(numbuf, sizeof(numbuf), "%ld", arith_val);
        if (n < 0) return NULL;
        new_value = strdup(numbuf);
    } else {
        new_value = strdup(value);
    }
    if (new_value == NULL) return NULL;

    if (var != NULL && var->uppercase) {
        size_t i;
        for (i = 0; new_value[i] != '\0'; i++) {
            new_value[i] = (char)toupper((unsigned char)new_value[i]);
        }
    }
    return new_value;
}

static long find_array_item_index(const struct cupid_array *a, const char *key) {
    size_t i;
    if (a == NULL || key == NULL) return -1;
    for (i = 0; i < a->count; i++) {
        if (a->keys != NULL && a->keys[i] != NULL && strcmp(a->keys[i], key) == 0) {
            return (long)i;
        }
    }
    return -1;
}

static unsigned int cupid_hash_string(const char *s) {
    unsigned int i = 2166136261u;
    if (s == NULL) return 0;
    while (*s != '\0') {
        i += (i << 1) + (i << 4) + (i << 7) + (i << 8) + (i << 24);
        i ^= (unsigned char)*s;
        s++;
    }
    return i;
}

static size_t indexed_insert_pos(const struct cupid_array *a, size_t key_index) {
    size_t i;
    if (a == NULL) return 0;
    for (i = 0; i < a->count; i++) {
        size_t existing = 0;
        if (parse_unsigned_key(a->keys[i], &existing) != 0) continue;
        if (existing > key_index) return i;
    }
    return a->count;
}

static size_t associative_insert_pos(const struct cupid_array *a, const char *key) {
    unsigned int target_bucket;
    size_t i;
    if (a == NULL || key == NULL) return 0;
    target_bucket = cupid_hash_string(key) & (CUPID_ASSOC_BUCKETS - 1);
    for (i = 0; i < a->count; i++) {
        unsigned int existing_bucket;
        if (a->keys == NULL || a->keys[i] == NULL) continue;
        existing_bucket = cupid_hash_string(a->keys[i]) & (CUPID_ASSOC_BUCKETS - 1);
        if (existing_bucket >= target_bucket) return i;
    }
    return a->count;
}

static int array_set_entry_mode(struct cupid_array *a, const char *key, const char *value,
                                int assoc_prepend_new) {
    char *key_copy = NULL;
    char *value_copy = NULL;
    long existing;
    if (a == NULL || key == NULL || value == NULL) return -1;

    existing = find_array_item_index(a, key);
    if (existing >= 0) {
        value_copy = strdup(value);
        if (value_copy == NULL) return -1;
        free(a->items[(size_t)existing]);
        a->items[(size_t)existing] = value_copy;
        return 0;
    }

    key_copy = strdup(key);
    value_copy = strdup(value);
    if (key_copy == NULL || value_copy == NULL) {
        free(key_copy);
        free(value_copy);
        return -1;
    }

    {
        char **next_keys = calloc(a->count + 1, sizeof(*next_keys));
        char **next_items = calloc(a->count + 1, sizeof(*next_items));
        size_t insert_at = a->count;
        size_t i;
        if (next_keys == NULL || next_items == NULL) {
            free(next_keys);
            free(next_items);
            free(key_copy);
            free(value_copy);
            return -1;
        }
        if (a->associative) {
            insert_at = assoc_prepend_new ? 0 : associative_insert_pos(a, key);
        } else {
            size_t numeric_key = 0;
            if (parse_unsigned_key(key, &numeric_key) == 0) {
                insert_at = indexed_insert_pos(a, numeric_key);
            }
        }
        for (i = 0; i < insert_at; i++) {
            next_keys[i] = a->keys[i];
            next_items[i] = a->items[i];
        }
        next_keys[insert_at] = key_copy;
        next_items[insert_at] = value_copy;
        for (i = insert_at; i < a->count; i++) {
            next_keys[i + 1] = a->keys[i];
            next_items[i + 1] = a->items[i];
        }
        free(a->keys);
        free(a->items);
        a->keys = next_keys;
        a->items = next_items;
        a->count++;
    }
    return 0;
}

static int array_set_entry(struct cupid_array *a, const char *key, const char *value) {
    return array_set_entry_mode(a, key, value, 0);
}

const char *cupid_vars_get(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    const char *resolved_name = NULL;
    const char *special = NULL;
    int looped = 0;
    if (shell == NULL || name == NULL) {
        return NULL;
    }
    special = lookup_special_var(shell, name);
    if (special != NULL) return special;
    v = find_var_follow(shell, name, &resolved_name, &looped);
    if (looped) return NULL;
    special = lookup_special_var(shell, resolved_name);
    if (special != NULL) return special;
    if (v != NULL) {
        return v->value;
    }
    return getenv(resolved_name != NULL ? resolved_name : name);
}

static int ensure_capacity(struct cupid_var_table *t) {
    if (t->count < t->capacity) {
        return 0;
    }
    {
        size_t new_cap = (t->capacity == 0) ? 16 : t->capacity * 2;
        struct cupid_var *new_entries = realloc(t->entries, sizeof(*new_entries) * new_cap);
        if (new_entries == NULL) {
            return -1;
        }
        t->entries = new_entries;
        t->capacity = new_cap;
    }
    return 0;
}

static int ensure_shell_var_entry(struct cupid_shell *shell, const char *name,
                                  const char *fallback_value, struct cupid_var **out) {
    struct cupid_var *v;

    if (shell == NULL || name == NULL) {
        return -1;
    }

    v = find_var(shell, name);
    if (v != NULL) {
        if (out != NULL) *out = v;
        return 0;
    }

    if (fallback_value == NULL) {
        fallback_value = cupid_vars_get(shell, name);
    }
    if (fallback_value == NULL) {
        fallback_value = "";
    }

    if (ensure_capacity(&shell->vars) != 0) {
        return -1;
    }

    v = &shell->vars.entries[shell->vars.count];
    v->name = strdup(name);
    if (v->name == NULL) {
        return -1;
    }
    v->value = strdup(fallback_value);
    if (v->value == NULL) {
        free(v->name);
        v->name = NULL;
        return -1;
    }
    v->nameref_target = NULL;
    v->exported = 0;
    v->readonly = 0;
    v->integer = 0;
    v->uppercase = 0;
    v->scope = 0;
    shell->vars.count++;

    if (out != NULL) *out = v;
    return 0;
}

static int vars_set_internal(struct cupid_shell *shell, const char *name, const char *value, int depth) {
    struct cupid_var *v;
    char *new_value;
    if (shell == NULL || name == NULL || value == NULL) {
        return -1;
    }
    if (depth > CUPID_NAMEREF_MAX_DEPTH) {
        return -1;
    }
    v = find_var(shell, name);
    if (v != NULL && v->nameref_target != NULL && v->nameref_target[0] != '\0') {
        return vars_set_internal(shell, v->nameref_target, value, depth + 1);
    }

    if (v != NULL) {
        if (v->readonly) {
            vars_error_prefix(shell);
            fprintf(stderr, "%s: readonly variable\n", name);
            return -1;
        }
        new_value = apply_var_attributes(shell, v, value);
        if (new_value == NULL) return -1;
        free(v->value);
        v->value = new_value;
        if (shell->opt_allexport) {
            v->exported = 1;
        }
        if (v->exported || shell->opt_allexport) {
            setenv(name, v->value, 1);
        }
        if (strcmp(name, "POSIXLY_CORRECT") == 0 && value[0] != '\0') {
            shell->mode = CUPID_MODE_POSIX;
        }
        return 0;
    }
    new_value = strdup(value);
    if (new_value == NULL) {
        return -1;
    }
    if (ensure_capacity(&shell->vars) != 0) {
        free(new_value);
        return -1;
    }
    {
        struct cupid_var *entry = &shell->vars.entries[shell->vars.count];
        entry->name = strdup(name);
        if (entry->name == NULL) {
            free(new_value);
            return -1;
        }
        entry->value = new_value;
        entry->nameref_target = NULL;
        entry->exported = shell->opt_allexport ? 1 : 0;
        entry->readonly = 0;
        entry->integer = 0;
        entry->uppercase = 0;
        entry->scope = 0;
        shell->vars.count++;
    }
    if (shell->opt_allexport) {
        setenv(name, new_value, 1);
    }
    if (strcmp(name, "POSIXLY_CORRECT") == 0 && value[0] != '\0') {
        shell->mode = CUPID_MODE_POSIX;
    }
    return 0;
}

int cupid_vars_set(struct cupid_shell *shell, const char *name, const char *value) {
    return vars_set_internal(shell, name, value, 0);
}

static void report_invalid_indexed_array_key(struct cupid_shell *shell, const char *key) {
    char *sanitized;
    size_t i;
    size_t out_len = 0;
    const char *token;

    if (key == NULL) return;
    sanitized = calloc(strlen(key) + 1, 1);
    if (sanitized == NULL) return;
    for (i = 0; key[i] != '\0'; i++) {
        unsigned char ch = (unsigned char)key[i];
        if (ch < 32 && ch != '\t' && ch != '\n' && ch != '\r') continue;
        sanitized[out_len++] = (char)ch;
    }
    sanitized[out_len] = '\0';
    token = sanitized;
    if (isalpha((unsigned char)token[0]) || token[0] == '_') {
        while (isalnum((unsigned char)*token) || *token == '_') token++;
    } else if (isdigit((unsigned char)token[0])) {
        while (isdigit((unsigned char)*token)) token++;
    }
    vars_error_prefix(shell);
    fprintf(stderr, "%s: syntax error: invalid arithmetic operator (error token is \"%s\")\n",
            sanitized[0] != '\0' ? sanitized : key,
            *token != '\0' ? token : (sanitized[0] != '\0' ? sanitized : key));
    free(sanitized);
}

int cupid_vars_export(struct cupid_shell *shell, const char *name, const char *value) {
    struct cupid_var *v;
    const char *resolved = name;
    const char *export_value = value;
    int looped = 0;
    if (shell == NULL || name == NULL) {
        return -1;
    }
    (void)find_var_follow(shell, name, &resolved, &looped);
    if (looped) return -1;

    if (export_value == NULL) {
        export_value = cupid_vars_get(shell, resolved);
    }
    if (export_value == NULL) {
        export_value = "";
    }

    if (ensure_shell_var_entry(shell, resolved, export_value, &v) != 0) {
        return -1;
    }

    if (value != NULL) {
        char *new_value = strdup(export_value);
        if (new_value == NULL) {
            return -1;
        }
        free(v->value);
        v->value = new_value;
    }

    v->exported = 1;
    return setenv(resolved, v->value, 1);
}

static int import_visible_value_for_attr(struct cupid_shell *shell, const char *name,
                                         struct cupid_var **out) {
    const char *resolved = name;
    int looped = 0;

    if (shell == NULL || name == NULL) return -1;
    (void)find_var_follow(shell, name, &resolved, &looped);
    if (looped) return -1;
    if (ensure_shell_var_entry(shell, resolved, cupid_vars_get(shell, resolved), out) != 0) {
        return -1;
    }
    return 0;
}

static int vars_unset_internal(struct cupid_shell *shell, const char *name, int depth) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) {
        return -1;
    }
    if (depth > CUPID_NAMEREF_MAX_DEPTH) {
        return -1;
    }
    v = find_var(shell, name);
    if (v != NULL && v->nameref_target != NULL && v->nameref_target[0] != '\0') {
        return vars_unset_internal(shell, v->nameref_target, depth + 1);
    }
    if (v != NULL && v->readonly) {
        vars_error_prefix(shell);
        fprintf(stderr, "unset: %s: readonly variable\n", name);
        return -1;
    }
    if (v != NULL) {
        size_t idx = (size_t)(v - shell->vars.entries);
        remove_var_at_index(shell, idx);
    }
    (void)cupid_array_unset(shell, name);
    if (strcmp(name, "POSIXLY_CORRECT") == 0) {
        shell->mode = CUPID_MODE_BASH;
    }
    return unsetenv(name);
}

int cupid_vars_unset(struct cupid_shell *shell, const char *name) {
    return vars_unset_internal(shell, name, 0);
}

int cupid_vars_unset_binding(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return -1;
    v = find_var(shell, name);
    if (v == NULL) return 0;
    if (v->readonly) {
        vars_error_prefix(shell);
        fprintf(stderr, "unset: %s: readonly variable\n", name);
        return -1;
    }
    {
        size_t idx = (size_t)(v - shell->vars.entries);
        remove_var_at_index(shell, idx);
    }
    return 0;
}

int cupid_vars_set_local(struct cupid_shell *shell, const char *name, const char *value) {
    size_t i;
    if (shell == NULL || name == NULL || value == NULL) {
        return -1;
    }
    for (i = 0; i < shell->vars.count; i++) {
        if (shell->vars.entries[i].scope == shell->scope_depth &&
            strcmp(shell->vars.entries[i].name, name) == 0) {
            char *new_value = NULL;
            if (shell->vars.entries[i].readonly) {
                vars_error_prefix(shell);
                fprintf(stderr, "%s: readonly variable\n", name);
                free(new_value);
                return -1;
            }
            new_value = apply_var_attributes(shell, &shell->vars.entries[i], value);
            if (new_value == NULL) return -1;
            free(shell->vars.entries[i].value);
            shell->vars.entries[i].value = new_value;
            free(shell->vars.entries[i].nameref_target);
            shell->vars.entries[i].nameref_target = NULL;
            return 0;
        }
    }
    if (ensure_capacity(&shell->vars) != 0) return -1;
    {
        struct cupid_var *entry = &shell->vars.entries[shell->vars.count];
        entry->name = strdup(name);
        if (entry->name == NULL) return -1;
        entry->value = strdup(value);
        if (entry->value == NULL) { free(entry->name); return -1; }
        entry->nameref_target = NULL;
        entry->exported = 0;
        entry->readonly = 0;
        entry->integer = 0;
        entry->uppercase = 0;
        entry->scope = shell->scope_depth;
        shell->vars.count++;
    }
    return 0;
}

int cupid_vars_set_local_nameref(struct cupid_shell *shell, const char *name, const char *target) {
    struct cupid_var *v;
    char *target_dup = NULL;
    char *value_dup = NULL;
    if (shell == NULL || name == NULL || target == NULL) return -1;
    if (nameref_would_cycle(shell, name, target)) {
        vars_error_prefix(shell);
        fprintf(stderr, "declare: %s: nameref cycle\n", name);
        return -1;
    }
    target_dup = strdup(target);
    if (target_dup == NULL) return -1;
    value_dup = strdup(target);
    if (value_dup == NULL) value_dup = strdup("");
    if (value_dup == NULL) {
        free(target_dup);
        return -1;
    }

    v = find_var_at_scope(shell, name, shell->scope_depth);
    if (v != NULL) {
        if (v->readonly) {
            vars_error_prefix(shell);
            fprintf(stderr, "%s: readonly variable\n", name);
            free(target_dup);
            free(value_dup);
            return -1;
        }
        free(v->nameref_target);
        v->nameref_target = target_dup;
        free(v->value);
        v->value = value_dup;
        v->integer = 0;
        v->uppercase = 0;
        return 0;
    }

    if (ensure_capacity(&shell->vars) != 0) {
        free(target_dup);
        free(value_dup);
        return -1;
    }
    {
        struct cupid_var *entry = &shell->vars.entries[shell->vars.count];
        entry->name = strdup(name);
        if (entry->name == NULL) {
            free(target_dup);
            free(value_dup);
            return -1;
        }
        entry->value = value_dup;
        entry->nameref_target = target_dup;
        entry->exported = 0;
        entry->readonly = 0;
        entry->integer = 0;
        entry->uppercase = 0;
        entry->scope = shell->scope_depth;
        shell->vars.count++;
    }
    return 0;
}

void cupid_vars_scope_enter(struct cupid_shell *shell) {
    shell->scope_depth++;
}

void cupid_vars_scope_leave(struct cupid_shell *shell) {
    size_t i = 0;
    while (i < shell->vars.count) {
        if (shell->vars.entries[i].scope == shell->scope_depth) {
            free(shell->vars.entries[i].name);
            free(shell->vars.entries[i].value);
            free(shell->vars.entries[i].nameref_target);
            if (i + 1 < shell->vars.count) {
                shell->vars.entries[i] = shell->vars.entries[shell->vars.count - 1];
            }
            shell->vars.count--;
        } else {
            i++;
        }
    }
    shell->scope_depth--;
}

int cupid_vars_mark_readonly(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return -1;
    if (import_visible_value_for_attr(shell, name, &v) != 0) return -1;
    if (v != NULL) v->readonly = 1;
    return 0;
}

int cupid_vars_is_readonly(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    const char *resolved = name;
    int looped = 0;
    if (shell == NULL || name == NULL) return 0;
    (void)find_var_follow(shell, name, &resolved, &looped);
    if (looped) return 0;
    v = find_var(shell, resolved);
    return (v != NULL && v->readonly) ? 1 : 0;
}

int cupid_vars_set_integer_attr(struct cupid_shell *shell, const char *name, int enabled) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return -1;
    if (import_visible_value_for_attr(shell, name, &v) != 0) return -1;
    if (v == NULL) return -1;
    v->integer = enabled ? 1 : 0;
    return 0;
}

int cupid_vars_is_integer(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    const char *resolved = name;
    int looped = 0;
    if (shell == NULL || name == NULL) return 0;
    (void)find_var_follow(shell, name, &resolved, &looped);
    if (looped) return 0;
    v = find_var(shell, resolved);
    return (v != NULL && v->integer) ? 1 : 0;
}

int cupid_vars_set_upper_attr(struct cupid_shell *shell, const char *name, int enabled) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return -1;
    if (import_visible_value_for_attr(shell, name, &v) != 0) return -1;
    if (v == NULL) return -1;
    v->uppercase = enabled ? 1 : 0;
    if (v->value != NULL && enabled) {
        size_t i;
        for (i = 0; v->value[i] != '\0'; i++) {
            v->value[i] = (char)toupper((unsigned char)v->value[i]);
        }
    }
    return 0;
}

int cupid_vars_is_upper(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    const char *resolved = name;
    int looped = 0;
    if (shell == NULL || name == NULL) return 0;
    (void)find_var_follow(shell, name, &resolved, &looped);
    if (looped) return 0;
    v = find_var(shell, resolved);
    return (v != NULL && v->uppercase) ? 1 : 0;
}

int cupid_vars_set_nameref(struct cupid_shell *shell, const char *name, const char *target) {
    struct cupid_var *v;
    char *target_dup = NULL;
    char *value_dup = NULL;
    if (shell == NULL || name == NULL || target == NULL) return -1;
    if (nameref_would_cycle(shell, name, target)) {
        vars_error_prefix(shell);
        fprintf(stderr, "declare: %s: nameref cycle\n", name);
        return -1;
    }
    target_dup = strdup(target);
    if (target_dup == NULL) return -1;
    value_dup = strdup(target);
    if (value_dup == NULL) value_dup = strdup("");
    if (value_dup == NULL) {
        free(target_dup);
        return -1;
    }
    v = find_var(shell, name);
    if (v != NULL) {
        if (v->readonly) {
            vars_error_prefix(shell);
            fprintf(stderr, "%s: readonly variable\n", name);
            free(target_dup);
            free(value_dup);
            return -1;
        }
        free(v->nameref_target);
        v->nameref_target = target_dup;
        free(v->value);
        v->value = value_dup;
        v->integer = 0;
        v->uppercase = 0;
        return 0;
    }
    if (ensure_capacity(&shell->vars) != 0) {
        free(target_dup);
        free(value_dup);
        return -1;
    }
    {
        struct cupid_var *entry = &shell->vars.entries[shell->vars.count];
        entry->name = strdup(name);
        if (entry->name == NULL) {
            free(target_dup);
            free(value_dup);
            return -1;
        }
        entry->value = value_dup;
        if (entry->value == NULL) {
            free(entry->name);
            free(target_dup);
            free(value_dup);
            return -1;
        }
        entry->nameref_target = target_dup;
        entry->exported = 0;
        entry->readonly = 0;
        entry->integer = 0;
        entry->uppercase = 0;
        entry->scope = 0;
        shell->vars.count++;
    }
    return 0;
}

int cupid_vars_clear_nameref(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return -1;
    v = find_var(shell, name);
    if (v == NULL) {
        return cupid_vars_set(shell, name, "");
    }
    if (v->readonly) {
        vars_error_prefix(shell);
        fprintf(stderr, "%s: readonly variable\n", name);
        return -1;
    }
    if (v->nameref_target != NULL && v->nameref_target[0] != '\0') {
        char *new_value = strdup(v->nameref_target);
        if (new_value == NULL) return -1;
        free(v->value);
        v->value = new_value;
        free(v->nameref_target);
        v->nameref_target = NULL;
    }
    return 0;
}

const char *cupid_vars_nameref_target(struct cupid_shell *shell, const char *name) {
    struct cupid_var *v;
    if (shell == NULL || name == NULL) return NULL;
    v = find_var(shell, name);
    if (v == NULL || v->nameref_target == NULL || v->nameref_target[0] == '\0') return NULL;
    return v->nameref_target;
}

int cupid_array_set_list(struct cupid_shell *shell, const char *name, char **items, size_t count) {
    struct cupid_array *a;
    size_t i;
    if (shell == NULL || name == NULL) return -1;
    a = ensure_array(shell, name);
    if (a == NULL) return -1;
    free_array_storage(a);
    for (i = 0; i < count; i++) {
        char keybuf[32];
        const char *raw = (items != NULL && items[i] != NULL) ? items[i] : "";
        const char *value = raw;
        const char *key = NULL;
        char *parsed_key = NULL;
        if (raw[0] == '[') {
            const char *rb = strchr(raw + 1, ']');
            if (rb != NULL && rb[1] == '=') {
                size_t klen = (size_t)(rb - (raw + 1));
                parsed_key = calloc(klen + 1, 1);
                if (parsed_key == NULL) {
                    free_array_storage(a);
                    return -1;
                }
                memcpy(parsed_key, raw + 1, klen);
                value = rb + 2;
                if (a->associative) {
                    key = parsed_key;
                } else {
                    size_t numeric_index = 0;
                    if (parse_unsigned_key(parsed_key, &numeric_index) != 0) {
                        report_invalid_indexed_array_key(shell, parsed_key);
                        free(parsed_key);
                        free_array_storage(a);
                        return -1;
                    }
                    key = parsed_key;
                }
            }
        }
        if (key == NULL) {
            snprintf(keybuf, sizeof(keybuf), "%zu", i);
            key = keybuf;
        }
        if (array_set_entry(a, key, value) != 0) {
            free(parsed_key);
            free_array_storage(a);
            return -1;
        }
        free(parsed_key);
    }
    return 0;
}

int cupid_array_set_index(struct cupid_shell *shell, const char *name, size_t index, const char *value) {
    char keybuf[32];
    if (shell == NULL || name == NULL || value == NULL) return -1;
    snprintf(keybuf, sizeof(keybuf), "%zu", index);
    return cupid_array_set_key(shell, name, keybuf, value);
}

int cupid_array_set_key(struct cupid_shell *shell, const char *name, const char *key, const char *value) {
    struct cupid_array *a;
    const char *store_value = value;
    char numbuf[64];
    if (shell == NULL || name == NULL || key == NULL || value == NULL) return -1;
    if (cupid_vars_is_integer(shell, name)) {
        int arith_err = 0;
        long arith_val = cupid_arith_eval(shell, value, &arith_err);
        int n;
        if (arith_err) return -1;
        n = snprintf(numbuf, sizeof(numbuf), "%ld", arith_val);
        if (n < 0 || n >= (int)sizeof(numbuf)) return -1;
        store_value = numbuf;
    }
    a = ensure_array(shell, name);
    if (a == NULL) return -1;
    if (!a->associative) {
        size_t idx_check = 0;
        if (parse_unsigned_key(key, &idx_check) != 0) {
            report_invalid_indexed_array_key(shell, key);
            return -1;
        }
    }
    return array_set_entry_mode(a, key, store_value, a->associative ? 1 : 0);
}

const char *cupid_array_get_index(struct cupid_shell *shell, const char *name, size_t index) {
    char keybuf[32];
    snprintf(keybuf, sizeof(keybuf), "%zu", index);
    return cupid_array_get_key(shell, name, keybuf);
}

const char *cupid_array_get_key(struct cupid_shell *shell, const char *name, const char *key) {
    struct cupid_array *a = find_array(shell, name);
    long idx;
    if (a == NULL || key == NULL) return "";
    idx = find_array_item_index(a, key);
    if (idx < 0) return "";
    return a->items[(size_t)idx] != NULL ? a->items[(size_t)idx] : "";
}

size_t cupid_array_length(struct cupid_shell *shell, const char *name) {
    return cupid_array_member_count(shell, name);
}

size_t cupid_array_member_count(struct cupid_shell *shell, const char *name) {
    struct cupid_array *a = find_array(shell, name);
    if (a == NULL) return 0;
    return a->count;
}

const char *cupid_array_member_key(struct cupid_shell *shell, const char *name, size_t ordinal) {
    struct cupid_array *a = find_array(shell, name);
    if (a == NULL || ordinal >= a->count || a->keys == NULL || a->keys[ordinal] == NULL) return "";
    return a->keys[ordinal];
}

const char *cupid_array_member_value(struct cupid_shell *shell, const char *name, size_t ordinal) {
    struct cupid_array *a = find_array(shell, name);
    if (a == NULL || ordinal >= a->count || a->items == NULL || a->items[ordinal] == NULL) return "";
    return a->items[ordinal];
}

int cupid_array_has_index(struct cupid_shell *shell, const char *name, size_t index) {
    char keybuf[32];
    snprintf(keybuf, sizeof(keybuf), "%zu", index);
    return cupid_array_has_key(shell, name, keybuf);
}

int cupid_array_has_key(struct cupid_shell *shell, const char *name, const char *key) {
    struct cupid_array *a = find_array(shell, name);
    if (a == NULL || key == NULL) return 0;
    return find_array_item_index(a, key) >= 0 ? 1 : 0;
}

int cupid_array_exists(struct cupid_shell *shell, const char *name) {
    return find_array(shell, name) != NULL;
}

int cupid_array_is_associative(struct cupid_shell *shell, const char *name) {
    struct cupid_array *a = find_array(shell, name);
    return (a != NULL && a->associative) ? 1 : 0;
}

int cupid_array_set_associative(struct cupid_shell *shell, const char *name, int associative) {
    struct cupid_array *a;
    if (shell == NULL || name == NULL) return -1;
    a = ensure_array(shell, name);
    if (a == NULL) return -1;
    a->associative = associative ? 1 : 0;
    return 0;
}

int cupid_array_unset(struct cupid_shell *shell, const char *name) {
    size_t i;
    if (shell == NULL || name == NULL) return -1;
    for (i = 0; i < shell->arrays.count; i++) {
        if (strcmp(shell->arrays.entries[i].name, name) == 0) {
            free(shell->arrays.entries[i].name);
            free_array_storage(&shell->arrays.entries[i]);
            if (i + 1 < shell->arrays.count) {
                shell->arrays.entries[i] = shell->arrays.entries[shell->arrays.count - 1];
            }
            shell->arrays.count--;
            return 0;
        }
    }
    return -1;
}
