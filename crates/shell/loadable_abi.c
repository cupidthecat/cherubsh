#define _GNU_SOURCE

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <inttypes.h>
#include <limits.h>
#include <locale.h>
#include <pwd.h>
#include <setjmp.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

typedef struct word_desc {
    char *word;
    int flags;
} WORD_DESC;

typedef struct word_list {
    struct word_list *next;
    WORD_DESC *word;
} WORD_LIST;

typedef struct shell_var {
    char *name;
    char *value;
    char *exportstr;
    void *dynamic_value;
    void *assign_func;
    int attributes;
    int context;
} SHELL_VAR;

typedef struct element_state {
    short type;
    short subtype;
    int64_t index;
    char *key;
    char *value;
} ARRAY_ELTSTATE;

typedef int sh_builtin_func_t(WORD_LIST *);
typedef int sh_load_func_t(const char *);
typedef void sh_unload_func_t(const char *);

struct user_info {
    uid_t uid, euid, saveuid;
    gid_t gid, egid, savegid;
    char *user_name;
    char *shell;
    char *home_dir;
};

extern int cherub_abi_last_status(void);
extern void cherub_abi_set_status(int status);
extern void cherub_abi_force_variable(const char *name, const char *value, int attributes);
extern int cherub_abi_run_source(const char *source, int *requested_exit);
extern char *cherub_abi_next_input_line(void);
extern int cherub_abi_source_complete(const char *source);
extern void cherub_abi_enter_loadable_child(void);
extern char **cherub_abi_export_environment(void);
extern char *cherub_abi_ifs(void);
extern SHELL_VAR *bind_variable(const char *name, const char *value, int flags);
extern char *get_string_value(const char *name);

char *list_optarg;
int list_optflags;
int list_optopt;
int list_opttype;
WORD_LIST *lcurrent;
WORD_LIST *loptend;

char *sh_optarg;
int sh_optind = 1;

int interrupt_state;
int interrupt_immediately;
int terminating_signal;
int no_longjmp_on_fatal_error;
int locale_utf8locale = 1;
char *locale_decpoint = ".";
int shell_compatibility_level = 53;
int interactive_shell;
int unbound_vars_is_error;
int last_command_exit_value;
pid_t last_asynchronous_pid = -1;
pid_t dollar_dollar_pid;
char *this_command_name;
char *bash_badsub_errmsg = "bad array subscript";
char **export_env;
struct user_info current_user;

static _Thread_local sigjmp_buf *active_jump;
static _Thread_local int forked_child;
static _Thread_local const char *active_synopsis;
static _Thread_local const char *const *active_help;
static WORD_LIST *option_head;
static int option_index = 1;

void cherub_loadable_abi_link_anchor(void) {}

static const char *command_name(void) {
    return this_command_name && *this_command_name ? this_command_name : "loadable";
}

static void diagnostic(const char *level, const char *format, va_list args) {
    fprintf(stderr, "%s: ", command_name());
    if (level)
        fprintf(stderr, "%s: ", level);
    vfprintf(stderr, format, args);
    fputc('\n', stderr);
}

void builtin_error(const char *format, ...) {
    va_list args;
    va_start(args, format);
    diagnostic(NULL, format, args);
    va_end(args);
}

void builtin_warning(const char *format, ...) {
    va_list args;
    va_start(args, format);
    diagnostic("warning", format, args);
    va_end(args);
}

void internal_error(const char *format, ...) {
    va_list args;
    va_start(args, format);
    diagnostic(NULL, format, args);
    va_end(args);
}

void file_error(const char *filename) {
    builtin_error("%s: %s", filename, strerror(errno));
}

void builtin_usage(void) {
    fprintf(stderr, "%s: usage: %s\n", command_name(),
            active_synopsis ? active_synopsis : command_name());
}

void builtin_help(void) {
    if (active_synopsis && *active_synopsis)
        printf("%s\n", active_synopsis);
    if (active_help)
        for (const char *const *line = active_help; *line; ++line)
            printf("    %s\n", *line);
}

static void invalid_option(const char *option) {
    builtin_error("%s: invalid option", option ? option : "");
}

void sh_invalidopt(const char *option) { invalid_option(option); }
void sh_invalidid(const char *value) { builtin_error("`%s': not a valid identifier", value); }
void sh_invalidnum(const char *value) { builtin_error("%s: invalid number", value); }
void sh_readonly(const char *value) { builtin_error("%s: readonly variable", value); }
void err_readonly(const char *value) { sh_readonly(value); }
void err_unboundvar(const char *value) { builtin_error("%s: unbound variable", value); }
void sh_erange(const char *value, const char *context) {
    builtin_error("%s: %s: value out of range", value, context ? context : "value");
}

int sh_chkwrite(int status) {
    if (ferror(stdout)) {
        builtin_error("write error: %s", strerror(errno));
        clearerr(stdout);
        return 1;
    }
    return status;
}

void reset_internal_getopt(void) {
    option_head = lcurrent = loptend = NULL;
    option_index = 1;
    list_optarg = NULL;
    list_optflags = 0;
}

static int valid_integer(const char *text, intmax_t *value) {
    if (!text || !*text)
        return 0;
    errno = 0;
    char *end;
    intmax_t parsed = strtoimax(text, &end, 10);
    if (errno || *end)
        return 0;
    if (value)
        *value = parsed;
    return 1;
}

int internal_getopt(WORD_LIST *list, char *options) {
    int plus = options && options[0] == '+';
    if (plus)
        ++options;
    if (!list) {
        loptend = NULL;
        return -1;
    }
    if (list != option_head || !option_head) {
        option_head = lcurrent = list;
        loptend = NULL;
        option_index = 1;
    }
    if (!lcurrent || !lcurrent->word || !lcurrent->word->word) {
        option_head = NULL;
        loptend = lcurrent;
        return -1;
    }
    char *word = lcurrent->word->word;
    if (option_index == 1) {
        if ((word[0] != '-' && (!plus || word[0] != '+')) || !word[1]) {
            option_head = NULL;
            loptend = lcurrent;
            return -1;
        }
        if (strcmp(word, "--help") == 0) {
            option_head = NULL;
            loptend = lcurrent;
            return -99;
        }
        if (strcmp(word, "--") == 0) {
            option_head = NULL;
            loptend = lcurrent->next;
            return -1;
        }
        list_opttype = word[0];
    }

    int option = (unsigned char)word[option_index];
    list_optopt = option;
    char *spec = options ? strchr(options, option) : NULL;
    if (!spec || option == ':') {
        char shown[3] = {(char)list_opttype, (char)option, 0};
        invalid_option(shown);
        if (!word[++option_index]) {
            lcurrent = lcurrent->next;
            option_index = 1;
        }
        list_optarg = NULL;
        return '?';
    }

    if (spec[1] == ':' || spec[1] == ';' || spec[1] == '#') {
        int optional = spec[1] == ';';
        int numeric = spec[1] == '#';
        if (word[option_index + 1]) {
            list_optarg = word + option_index + 1;
            lcurrent = lcurrent->next;
        } else if (lcurrent->next &&
                   (!optional || lcurrent->next->word->word[0] != '-')) {
            lcurrent = lcurrent->next;
            list_optarg = lcurrent->word->word;
            list_optflags = lcurrent->word->flags;
            lcurrent = lcurrent->next;
        } else if (optional) {
            list_optarg = NULL;
            lcurrent = lcurrent->next;
        } else {
            char shown[3] = {(char)list_opttype, (char)option, 0};
            builtin_error("%s: option requires an argument", shown);
            option_index = 1;
            return '?';
        }
        option_index = 1;
        if (numeric && list_optarg && !valid_integer(list_optarg, NULL)) {
            builtin_error("%s: numeric argument required", list_optarg);
            list_optarg = NULL;
            return '?';
        }
    } else {
        list_optarg = NULL;
        list_optflags = 0;
        if (!word[++option_index]) {
            lcurrent = lcurrent->next;
            option_index = 1;
        }
    }
    return option;
}

int no_options(WORD_LIST *list) {
    reset_internal_getopt();
    int option = internal_getopt(list, "");
    if (option == -99)
        builtin_help();
    return option == -1 ? 0 : 2;
}

int cherub_invoke_builtin(sh_builtin_func_t *function, WORD_LIST *list,
                          const char *name, const char *synopsis,
                          const char *const *help) {
    sigjmp_buf jump;
    sigjmp_buf *previous_jump = active_jump;
    const char *previous_name = this_command_name;
    const char *previous_synopsis = active_synopsis;
    const char *const *previous_help = active_help;
    active_jump = &jump;
    this_command_name = (char *)name;
    active_synopsis = synopsis;
    active_help = help;
    reset_internal_getopt();
    last_command_exit_value = cherub_abi_last_status();
    int code = sigsetjmp(jump, 1);
    int status = code == 0 ? function(list) : (last_command_exit_value ?: 1);
    fflush(NULL);
    cherub_abi_set_status(status);
    reset_internal_getopt();
    active_jump = previous_jump;
    this_command_name = (char *)previous_name;
    active_synopsis = previous_synopsis;
    active_help = previous_help;
    return status;
}

int cherub_invoke_load(sh_load_func_t *function, const char *name) {
    sigjmp_buf jump;
    sigjmp_buf *previous = active_jump;
    active_jump = &jump;
    int code = sigsetjmp(jump, 1);
    int status = code == 0 ? function(name) : 0;
    active_jump = previous;
    return status;
}

void cherub_invoke_unload(sh_unload_func_t *function, const char *name) {
    sigjmp_buf jump;
    sigjmp_buf *previous = active_jump;
    active_jump = &jump;
    if (sigsetjmp(jump, 1) == 0)
        function(name);
    active_jump = previous;
}

static void do_jump(int code) {
    if (active_jump)
        siglongjmp(*active_jump, code ? code : 1);
    _exit(last_command_exit_value ? last_command_exit_value : 1);
}

void jump_to_top_level(int code) { do_jump(code); }
void throw_to_top_level(void) { do_jump(2); }
void exit_shell(int status) {
    last_command_exit_value = status;
    cherub_abi_set_status(status);
    if (forked_child) {
        fflush(NULL);
        _exit(status);
    }
    do_jump(3);
}
void termsig_handler(int signal_number) {
    terminating_signal = signal_number;
    last_command_exit_value = 128 + signal_number;
    do_jump(5);
}

void *xmalloc(size_t size) {
    void *value = malloc(size ? size : 1);
    if (!value) {
        fputs("loadable: memory exhausted\n", stderr);
        do_jump(2);
    }
    return value;
}

void *xrealloc(void *pointer, size_t size) {
    void *value = realloc(pointer, size ? size : 1);
    if (!value) {
        fputs("loadable: memory exhausted\n", stderr);
        do_jump(2);
    }
    return value;
}

void xfree(void *pointer) { free(pointer); }

WORD_DESC *make_bare_word(const char *text) {
    WORD_DESC *word = xmalloc(sizeof(*word));
    word->word = strdup(text ? text : "");
    word->flags = 0;
    return word;
}

WORD_DESC *make_word(const char *text) { return make_bare_word(text); }

WORD_LIST *make_word_list(WORD_DESC *word, WORD_LIST *next) {
    WORD_LIST *list = xmalloc(sizeof(*list));
    list->word = word;
    list->next = next;
    return list;
}

void dispose_words(WORD_LIST *list) {
    while (list) {
        WORD_LIST *next = list->next;
        if (list->word) {
            free(list->word->word);
            free(list->word);
        }
        free(list);
        list = next;
    }
}

int list_length(void *head) {
    int length = 0;
    for (WORD_LIST *list = head; list; list = list->next)
        ++length;
    return length;
}

char **make_builtin_argv(WORD_LIST *list, int *count) {
    int length = list_length(list);
    char **argv = xmalloc((size_t)(length + 2) * sizeof(*argv));
    argv[0] = this_command_name ? this_command_name : (char *)"loadable";
    int index = 1;
    for (; list; list = list->next)
        argv[index++] = list->word ? list->word->word : (char *)"";
    argv[index] = NULL;
    if (count)
        *count = length + 1;
    return argv;
}

char *string_list(WORD_LIST *list) {
    size_t size = 1;
    for (WORD_LIST *item = list; item; item = item->next)
        size += strlen(item->word->word) + (item->next != NULL);
    char *text = xmalloc(size);
    text[0] = 0;
    for (WORD_LIST *item = list; item; item = item->next) {
        strcat(text, item->word->word);
        if (item->next)
            strcat(text, " ");
    }
    return text;
}

void print_word_list(WORD_LIST *list, char *separator) {
    const char *join = separator ? separator : " ";
    for (WORD_LIST *item = list; item; item = item->next) {
        fputs(item->word ? item->word->word : "", stdout);
        if (item->next)
            fputs(join, stdout);
    }
}

int all_digits(const char *text) {
    if (!text || !*text)
        return 0;
    for (; *text; ++text)
        if (!isdigit((unsigned char)*text))
            return 0;
    return 1;
}

int valid_identifier(const char *text) {
    if (!text || !(isalpha((unsigned char)*text) || *text == '_'))
        return 0;
    for (++text; *text; ++text)
        if (!(isalnum((unsigned char)*text) || *text == '_'))
            return 0;
    return 1;
}

int valid_number(const char *text, intmax_t *value) { return valid_integer(text, value); }

int read_octal(char *text) {
    if (!text || !*text)
        return -1;
    char *end;
    long value = strtol(text, &end, 8);
    return *end || value < 0 || value > INT_MAX ? -1 : (int)value;
}

char *itos(intmax_t value) {
    char buffer[64];
    snprintf(buffer, sizeof(buffer), "%" PRIdMAX, value);
    return strdup(buffer);
}

char *fmtulong(unsigned long value, int base, char *buffer, size_t size, int flags) {
    (void)flags;
    snprintf(buffer, size, base == 8 ? "%lo" : base == 16 ? "%lx" : "%lu", value);
    return buffer;
}

char *fmtumax(uintmax_t value, int base, char *buffer, size_t size, int flags) {
    (void)flags;
    snprintf(buffer, size, base == 8 ? "%" PRIoMAX : base == 16 ? "%" PRIxMAX : "%" PRIuMAX,
             value);
    return buffer;
}

int uconvert(const char *text, long *seconds, long *microseconds, char **error) {
    if (error)
        *error = NULL;
    if (!text || !*text)
        return 0;
    errno = 0;
    char *end;
    double value = strtod(text, &end);
    if (errno || *end || value < 0) {
        if (error)
            *error = (char *)text;
        return 0;
    }
    *seconds = (long)value;
    *microseconds = (long)((value - (double)*seconds) * 1000000.0);
    return 1;
}

unsigned int fsleep(unsigned int seconds, unsigned int microseconds) {
    struct timespec request = {(time_t)seconds, (long)microseconds * 1000L};
    struct timespec remaining = {0, 0};
    if (nanosleep(&request, &remaining) == 0)
        return 0;
    return (unsigned int)remaining.tv_sec;
}

char *base_pathname(char *path) {
    if (!path)
        return path;
    char *slash = strrchr(path, '/');
    return slash ? slash + 1 : path;
}

int file_isdir(const char *path) {
    struct stat info;
    return path && stat(path, &info) == 0 && S_ISDIR(info.st_mode);
}

char *get_working_directory(const char *context) {
    (void)context;
    return getcwd(NULL, 0);
}

char *make_absolute(const char *path, const char *directory) {
    if (!path)
        return NULL;
    if (*path == '/')
        return strdup(path);
    const char *base = directory && *directory ? directory : ".";
    size_t size = strlen(base) + strlen(path) + 2;
    char *result = xmalloc(size);
    snprintf(result, size, "%s/%s", base, path);
    return result;
}

char *sh_realpath(const char *path, char *resolved) { return realpath(path, resolved); }

char *sh_canonpath(char *path, int flags) {
    (void)flags;
    char *resolved = realpath(path, NULL);
    return resolved ? resolved : strdup(path);
}

char *polite_directory_format(char *path) { return path; }

int parse_symbolic_mode(char *mode, mode_t initial) {
    if (!mode || !*mode)
        return -1;
    mode_t result = initial;
    for (char *clause = mode; *clause;) {
        mode_t who = 0;
        while (*clause && strchr("ugoa", *clause)) {
            if (*clause == 'u' || *clause == 'a') who |= S_IRWXU;
            if (*clause == 'g' || *clause == 'a') who |= S_IRWXG;
            if (*clause == 'o' || *clause == 'a') who |= S_IRWXO;
            ++clause;
        }
        if (!who) who = S_IRWXU | S_IRWXG | S_IRWXO;
        char operation = *clause++;
        if (!strchr("+-=", operation)) return -1;
        mode_t bits = 0;
        while (*clause && *clause != ',') {
            if (*clause == 'r') bits |= S_IRUSR | S_IRGRP | S_IROTH;
            else if (*clause == 'w') bits |= S_IWUSR | S_IWGRP | S_IWOTH;
            else if (*clause == 'x' || (*clause == 'X' && (initial & 0111)))
                bits |= S_IXUSR | S_IXGRP | S_IXOTH;
            else if (*clause == 's') bits |= S_ISUID | S_ISGID;
            else if (*clause == 't') bits |= S_ISVTX;
            else return -1;
            ++clause;
        }
        bits &= who | S_ISUID | S_ISGID | S_ISVTX;
        if (operation == '+') result |= bits;
        else if (operation == '-') result &= ~bits;
        else result = (result & ~who) | bits;
        if (*clause == ',') ++clause;
    }
    return (int)result;
}

int sh_mktmpfd(const char *name, int flags, char **filename) {
    (void)flags;
    const char *base = name && *name ? name : "bash";
    size_t size = strlen(base) + 20;
    char *template = xmalloc(size);
    snprintf(template, size, "/tmp/%s.XXXXXX", base_pathname((char *)base));
    int fd = mkstemp(template);
    if (fd < 0) {
        free(template);
        return -1;
    }
    if (filename) *filename = template; else free(template);
    return fd;
}

char *sh_mktmpdir(const char *name, int flags) {
    (void)flags;
    const char *base = name && *name ? name : "bash";
    size_t size = strlen(base) + 20;
    char *template = xmalloc(size);
    snprintf(template, size, "/tmp/%s.XXXXXX", base_pathname((char *)base));
    if (!mkdtemp(template)) {
        free(template);
        return NULL;
    }
    return template;
}

char **strvec_resize(char **array, size_t elements) {
    return xrealloc(array, elements * sizeof(*array));
}

char *mbschr(const char *text, int character) { return strchr(text, character); }

char *utf8_mbsmbchar(const char *text) {
    if (!text || !*text)
        return NULL;
    unsigned char byte = (unsigned char)*text;
    int length = byte < 0x80 ? 1 : (byte & 0xe0) == 0xc0 ? 2 : (byte & 0xf0) == 0xe0 ? 3 : 4;
    return (char *)text + length;
}

ssize_t zgetline(int fd, char **line, size_t *capacity, int delimiter, int unbuffered) {
    (void)unbuffered;
    if (!line || !capacity) {
        errno = EINVAL;
        return -1;
    }
    if (!*line || !*capacity) {
        *capacity = 128;
        *line = xmalloc(*capacity);
    }
    size_t length = 0;
    char byte;
    while (read(fd, &byte, 1) == 1) {
        if (length + 2 > *capacity) {
            *capacity *= 2;
            *line = xrealloc(*line, *capacity);
        }
        (*line)[length++] = byte;
        if ((unsigned char)byte == (unsigned char)delimiter)
            break;
    }
    if (!length)
        return -1;
    (*line)[length] = 0;
    return (ssize_t)length - 1;
}

void zreset(void) {}

char *ansicstr(const char *text, size_t length, int flags, int *saw_escape, size_t *result_length) {
    (void)flags;
    char *result = xmalloc(length + 1);
    size_t out = 0;
    if (saw_escape) *saw_escape = 0;
    for (size_t i = 0; i < length; ++i) {
        unsigned char value = (unsigned char)text[i];
        if (value == '\\' && i + 1 < length) {
            if (saw_escape) *saw_escape = 1;
            value = (unsigned char)text[++i];
            if (value == 'n') value = '\n';
            else if (value == 'r') value = '\r';
            else if (value == 't') value = '\t';
            else if (value == 'b') value = '\b';
            else if (value == 'a') value = '\a';
            else if (value == 'v') value = '\v';
            else if (value == 'f') value = '\f';
        }
        result[out++] = (char)value;
    }
    result[out] = 0;
    if (result_length) *result_length = out;
    return result;
}

void get_current_user_info(void) {
    current_user.uid = getuid();
    current_user.euid = geteuid();
    current_user.saveuid = current_user.euid;
    current_user.gid = getgid();
    current_user.egid = getegid();
    current_user.savegid = current_user.egid;
    struct passwd *entry = getpwuid(current_user.euid);
    if (entry) {
        current_user.user_name = entry->pw_name;
        current_user.shell = entry->pw_shell;
        current_user.home_dir = entry->pw_dir;
    }
    dollar_dollar_pid = getpid();
}

int *get_group_array(int *count) {
    static gid_t *groups;
    static int allocated;
    int needed = getgroups(0, NULL);
    if (needed > allocated) {
        groups = xrealloc(groups, (size_t)needed * sizeof(*groups));
        allocated = needed;
    }
    int actual = needed > 0 ? getgroups(needed, groups) : 0;
    if (count) *count = actual;
    return (int *)groups;
}

void maybe_make_export_env(void) { export_env = cherub_abi_export_environment(); }
char *getifs(void) { return cherub_abi_ifs(); }

int sh_getopt(int argc, char *const *argv, const char *options) {
    optind = sh_optind;
    opterr = 0;
    int result = getopt(argc, argv, options);
    sh_optind = optind;
    sh_optarg = optarg;
    return result;
}

void set_exit_status(int status) {
    last_command_exit_value = status;
    cherub_abi_set_status(status);
}

void set_ppid(void) {
    char value[32];
    snprintf(value, sizeof(value), "%jd", (intmax_t)getppid());
    cherub_abi_force_variable("PPID", value, 0x0002 | 0x0010);
}

void adjust_shell_level(int change) {
    char *current = get_string_value("SHLVL");
    char *end = NULL;
    long level = current && *current ? strtol(current, &end, 10) : 0;
    if (!current || !*current || !end || *end)
        level = 0;
    level += change;
    if (level < 0)
        level = 0;
    else if (level >= 1000)
        level = 1;
    char value[16];
    snprintf(value, sizeof(value), "%ld", level);
    SHELL_VAR *variable = bind_variable("SHLVL", value, 0);
    if (variable)
        variable->attributes |= 0x0001;
}
void cleanup_the_pipeline(void) {}
void delete_all_jobs(int running_only) { (void)running_only; }
void initialize_job_control(int force) { (void)force; }
void initialize_job_signals(void) {}
void stop_making_children(void) {}
int stop_pipeline(int async, void *deferred) { (void)async; (void)deferred; return 0; }
void top_level_cleanup(void) {}
void flush_eltstate(void *state) {
    ARRAY_ELTSTATE *element = state;
    if (element)
        free(element->key);
}

void init_eltstate(void *state) {
    ARRAY_ELTSTATE *element = state;
    if (!element)
        return;
    element->type = -1;
    element->subtype = 0;
    element->index = INT64_MIN;
    element->key = NULL;
    element->value = NULL;
}
void stupidly_hack_special_variables(const char *name) { (void)name; }
int skipsubscript(const char *text, int start, int flags) {
    (void)flags;
    int depth = 0;
    int quote = 0;
    for (int index = start; text && text[index]; ++index) {
        unsigned char byte = (unsigned char)text[index];
        if (byte == '\\' && quote != '\'') {
            if (text[index + 1])
                ++index;
            continue;
        }
        if (quote) {
            if (byte == quote)
                quote = 0;
            continue;
        }
        if (byte == '\'' || byte == '"') {
            quote = byte;
        } else if (byte == '[') {
            ++depth;
        } else if (byte == ']') {
            if (depth <= 1)
                return index;
            --depth;
        }
    }
    return start;
}

pid_t make_child(char *command, int flags) {
    (void)command;
    (void)flags;
    pid_t pid = fork();
    if (pid == 0) {
        forked_child = 1;
        cherub_abi_enter_loadable_child();
    }
    return pid;
}

int wait_for(pid_t pid, int flags) {
    (void)flags;
    int status;
    if (waitpid(pid, &status, 0) < 0) return 127;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

int reader_loop(void) {
    char *source = NULL;
    size_t length = 0;
    size_t capacity = 0;
    char *line;
    while ((line = cherub_abi_next_input_line()) != NULL) {
        size_t count = strlen(line);
        if (length + count + 1 > capacity) {
            capacity = (length + count + 1) * 2;
            source = xrealloc(source, capacity);
        }
        memcpy(source + length, line, count);
        length += count;
        source[length] = '\0';
        if (!cherub_abi_source_complete(source))
            continue;
        int requested_exit = -1;
        last_command_exit_value = cherub_abi_run_source(source, &requested_exit);
        length = 0;
        if (requested_exit >= 0) {
            last_command_exit_value = requested_exit;
            break;
        }
    }
    if (length > 0) {
        source[length] = '\0';
        int requested_exit = -1;
        last_command_exit_value = cherub_abi_run_source(source, &requested_exit);
        if (requested_exit >= 0)
            last_command_exit_value = requested_exit;
    }
    free(source);
    return last_command_exit_value;
}

void print_var_value(void *variable, int quoted) {
    (void)quoted;
    SHELL_VAR *shell_variable = variable;
    if (shell_variable && shell_variable->value)
        fputs(shell_variable->value, stdout);
}

void print_var_function(void *variable) {
    SHELL_VAR *shell_variable = variable;
    if (shell_variable && shell_variable->value && !(shell_variable->attributes & 0x0008))
        fputs(shell_variable->value, stdout);
}

__attribute__((constructor)) static void initialize_abi(void) {
    get_current_user_info();
    export_env = environ;
}
