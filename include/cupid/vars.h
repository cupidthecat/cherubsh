#ifndef CUPID_VARS_H
#define CUPID_VARS_H

#include <stddef.h>

struct cupid_shell;

const char *cupid_vars_get(struct cupid_shell *shell, const char *name);
int cupid_vars_init_defaults(struct cupid_shell *shell);
int cupid_vars_set(struct cupid_shell *shell, const char *name, const char *value);
int cupid_vars_export(struct cupid_shell *shell, const char *name, const char *value);
int cupid_vars_unset(struct cupid_shell *shell, const char *name);
int cupid_vars_unset_binding(struct cupid_shell *shell, const char *name);
int cupid_vars_set_local(struct cupid_shell *shell, const char *name, const char *value);
int cupid_vars_set_local_nameref(struct cupid_shell *shell, const char *name, const char *target);
int cupid_vars_set_nameref(struct cupid_shell *shell, const char *name, const char *target);
int cupid_vars_clear_nameref(struct cupid_shell *shell, const char *name);
const char *cupid_vars_nameref_target(struct cupid_shell *shell, const char *name);
void cupid_vars_scope_enter(struct cupid_shell *shell);
void cupid_vars_scope_leave(struct cupid_shell *shell);
int cupid_vars_mark_readonly(struct cupid_shell *shell, const char *name);
int cupid_vars_is_readonly(struct cupid_shell *shell, const char *name);
int cupid_vars_set_integer_attr(struct cupid_shell *shell, const char *name, int enabled);
int cupid_vars_is_integer(struct cupid_shell *shell, const char *name);
int cupid_vars_set_upper_attr(struct cupid_shell *shell, const char *name, int enabled);
int cupid_vars_is_upper(struct cupid_shell *shell, const char *name);
int cupid_array_set_list(struct cupid_shell *shell, const char *name, char **items, size_t count);
int cupid_array_set_index(struct cupid_shell *shell, const char *name, size_t index, const char *value);
int cupid_array_set_key(struct cupid_shell *shell, const char *name, const char *key, const char *value);
const char *cupid_array_get_index(struct cupid_shell *shell, const char *name, size_t index);
const char *cupid_array_get_key(struct cupid_shell *shell, const char *name, const char *key);
size_t cupid_array_length(struct cupid_shell *shell, const char *name);
size_t cupid_array_member_count(struct cupid_shell *shell, const char *name);
const char *cupid_array_member_key(struct cupid_shell *shell, const char *name, size_t ordinal);
const char *cupid_array_member_value(struct cupid_shell *shell, const char *name, size_t ordinal);
int cupid_array_has_index(struct cupid_shell *shell, const char *name, size_t index);
int cupid_array_has_key(struct cupid_shell *shell, const char *name, const char *key);
int cupid_array_exists(struct cupid_shell *shell, const char *name);
int cupid_array_is_associative(struct cupid_shell *shell, const char *name);
int cupid_array_set_associative(struct cupid_shell *shell, const char *name, int associative);
int cupid_array_unset(struct cupid_shell *shell, const char *name);

#endif
