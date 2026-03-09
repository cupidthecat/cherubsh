#ifndef CUPID_SHELL_H
#define CUPID_SHELL_H

#include <stdio.h>
#include <stddef.h>
#include <sys/types.h>
#include <time.h>

struct cupid_list_ast;

enum cupid_mode {
    CUPID_MODE_BASH = 0,
    CUPID_MODE_POSIX = 1,
};

struct cupid_var {
    char *name;
    char *value;
    char *nameref_target;
    int exported;
    int readonly;
    int integer;
    int uppercase;
    int scope;
};

struct cupid_var_table {
    struct cupid_var *entries;
    size_t count;
    size_t capacity;
};

struct cupid_func {
    char *name;
    struct cupid_list_ast *body;
    char *source;
};

struct cupid_func_table {
    struct cupid_func *entries;
    size_t count;
};

struct cupid_alias {
    char *name;
    char *value;
};

struct cupid_alias_table {
    struct cupid_alias *entries;
    size_t count;
    size_t capacity;
};

struct cupid_hash_entry {
    char *name;
    char *path;
    int hits;
};

struct cupid_hash_table {
    struct cupid_hash_entry *entries;
    size_t count;
    size_t capacity;
};

struct cupid_completion_spec {
    char *name;
    char *spec;
    unsigned hash;
    size_t order;
};

struct cupid_completion_table {
    struct cupid_completion_spec *entries;
    size_t count;
    size_t capacity;
    size_t next_order;
};

struct cupid_array {
    char *name;
    char **keys;
    char **items;
    size_t count;
    int associative;
};

struct cupid_array_table {
    struct cupid_array *entries;
    size_t count;
    size_t capacity;
};

struct cupid_params {
    char **args;
    size_t count;
};

#define CUPID_MAX_TRAP_SIGNAL 32
#define CUPID_MAX_JOBS 64

struct cupid_job {
    pid_t pgid;
    int job_id;
    char *command;
    int stopped;
    int completed;
    int status;
};

struct cupid_shell {
    enum cupid_mode mode;
    int is_interactive;
    int is_dash_c;
    int suppress_special_builtin_fatal;
    pid_t shell_pid;
    int last_status;
    int should_exit;
    int exit_code;
    struct cupid_var_table vars;
    struct cupid_func_table funcs;
    struct cupid_params params;
    char *arg0;
    int loop_depth;
    int break_count;
    int continue_flag;
    int return_flag;
    int scope_depth;
    int opt_errexit;
    int opt_allexport;
    int opt_noglob;
    int opt_monitor;
    int opt_nounset;
    int opt_xtrace;
    int opt_pipefail;
    int opt_expand_aliases;
    int alias_expansion_depth;
    int opt_extglob;
    int opt_nullglob;
    int opt_lastpipe;
    int opt_histexpand;
    int opt_cmdhist;
    int opt_sourcepath;
    int opt_xpg_echo;
    int opt_varredir_close;
    int in_condition;
    int expand_cmdsub_seen;
    int expand_cmdsub_status;
    const char *current_file;
    const char *current_command_source;
    const char *current_item_source;
    char *traps[CUPID_MAX_TRAP_SIGNAL + 1];
    time_t start_time;
    int lineno;
    struct cupid_job jobs[CUPID_MAX_JOBS];
    int job_count;
    int next_job_id;
    pid_t last_bg_pid;
    char **tracked_command_sources;
    size_t tracked_command_source_count;
    size_t tracked_command_source_capacity;
    struct cupid_alias_table aliases;
    struct cupid_alias_table disabled_builtins;
    struct cupid_hash_table hashes;
    struct cupid_completion_table completions;
    struct cupid_array_table arrays;
};

void cupid_shell_init(struct cupid_shell *shell);
void cupid_shell_destroy(struct cupid_shell *shell);
int cupid_shell_eval_line(struct cupid_shell *shell, const char *line, int print_errors);
int cupid_shell_eval_text(struct cupid_shell *shell, const char *text, int print_errors);
int cupid_shell_eval_file(struct cupid_shell *shell, const char *path);
char *cupid_extract_first_command_source(const char *text, int posix_mode);
char *cupid_extract_next_command_source(const char **text_io, int posix_mode);
int cupid_shell_track_command_source(struct cupid_shell *shell, char *source);
void cupid_shell_clear_tracked_command_sources(struct cupid_shell *shell);
void cupid_shell_error_prefix(FILE *fp, const struct cupid_shell *shell);

void cupid_func_set(struct cupid_shell *shell, const char *name, struct cupid_list_ast *body,
                    const char *source);
struct cupid_list_ast *cupid_func_get(struct cupid_shell *shell, const char *name);
const char *cupid_func_source_get(struct cupid_shell *shell, const char *name);
void cupid_func_unset(struct cupid_shell *shell, const char *name);
void cupid_shell_run_exit_trap(struct cupid_shell *shell);
const char *cupid_alias_get(struct cupid_shell *shell, const char *name);
int cupid_alias_set(struct cupid_shell *shell, const char *name, const char *value);
int cupid_alias_unset(struct cupid_shell *shell, const char *name);

#endif
