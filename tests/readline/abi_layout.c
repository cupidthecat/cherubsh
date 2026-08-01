#include <stddef.h>
#include <stdio.h>
#include <readline/history.h>
#include <readline/keymaps.h>
#include <readline/readline.h>

#define FIELD(type, field) printf(#type "." #field "=%zu\n", offsetof(type, field))
#define VALUE(name) printf(#name "=%#lx\n", (unsigned long)(name))

int main(void)
{
    printf("HIST_ENTRY=%zu\n", sizeof(HIST_ENTRY));
    FIELD(HIST_ENTRY, line);
    FIELD(HIST_ENTRY, timestamp);
    FIELD(HIST_ENTRY, data);

    printf("HISTORY_STATE=%zu\n", sizeof(HISTORY_STATE));
    FIELD(HISTORY_STATE, entries);
    FIELD(HISTORY_STATE, offset);
    FIELD(HISTORY_STATE, length);
    FIELD(HISTORY_STATE, size);
    FIELD(HISTORY_STATE, flags);

    printf("UNDO_LIST=%zu\n", sizeof(UNDO_LIST));
    FIELD(UNDO_LIST, next);
    FIELD(UNDO_LIST, start);
    FIELD(UNDO_LIST, end);
    FIELD(UNDO_LIST, text);
    FIELD(UNDO_LIST, what);

    printf("FUNMAP=%zu\n", sizeof(FUNMAP));
    FIELD(FUNMAP, name);
    FIELD(FUNMAP, function);

    printf("KEYMAP_ENTRY=%zu\n", sizeof(KEYMAP_ENTRY));
    FIELD(KEYMAP_ENTRY, type);
    FIELD(KEYMAP_ENTRY, function);

    printf("readline_state=%zu\n", sizeof(struct readline_state));
    FIELD(struct readline_state, point);
    FIELD(struct readline_state, buffer);
    FIELD(struct readline_state, rlstate);
    FIELD(struct readline_state, kmap);
    FIELD(struct readline_state, inf);
    FIELD(struct readline_state, outf);
    FIELD(struct readline_state, entryfunc);
    FIELD(struct readline_state, attemptfunc);
    FIELD(struct readline_state, reserved);

    VALUE(RL_READLINE_VERSION);
    VALUE(RL_VERSION_MAJOR);
    VALUE(RL_VERSION_MINOR);
    VALUE(KEYMAP_SIZE);
    VALUE(ANYOTHERKEY);
    VALUE(ISFUNC);
    VALUE(ISKMAP);
    VALUE(ISMACR);
    VALUE(UNDO_DELETE);
    VALUE(UNDO_INSERT);
    VALUE(UNDO_BEGIN);
    VALUE(UNDO_END);
    VALUE(HS_STIFLED);
    VALUE(READERR);
    VALUE(RL_PROMPT_START_IGNORE);
    VALUE(RL_PROMPT_END_IGNORE);
    VALUE(NO_MATCH);
    VALUE(SINGLE_MATCH);
    VALUE(MULT_MATCH);
    VALUE(RL_STATE_NONE);
    VALUE(RL_STATE_INITIALIZING);
    VALUE(RL_STATE_INITIALIZED);
    VALUE(RL_STATE_TERMPREPPED);
    VALUE(RL_STATE_READCMD);
    VALUE(RL_STATE_METANEXT);
    VALUE(RL_STATE_DISPATCHING);
    VALUE(RL_STATE_MOREINPUT);
    VALUE(RL_STATE_ISEARCH);
    VALUE(RL_STATE_NSEARCH);
    VALUE(RL_STATE_SEARCH);
    VALUE(RL_STATE_NUMERICARG);
    VALUE(RL_STATE_MACROINPUT);
    VALUE(RL_STATE_MACRODEF);
    VALUE(RL_STATE_OVERWRITE);
    VALUE(RL_STATE_COMPLETING);
    VALUE(RL_STATE_SIGHANDLER);
    VALUE(RL_STATE_UNDOING);
    VALUE(RL_STATE_INPUTPENDING);
    VALUE(RL_STATE_TTYCSAVED);
    VALUE(RL_STATE_CALLBACK);
    VALUE(RL_STATE_VIMOTION);
    VALUE(RL_STATE_MULTIKEY);
    VALUE(RL_STATE_VICMDONCE);
    VALUE(RL_STATE_CHARSEARCH);
    VALUE(RL_STATE_REDISPLAYING);
    VALUE(RL_STATE_DONE);
    VALUE(RL_STATE_TIMEOUT);
    VALUE(RL_STATE_EOF);
    VALUE(RL_STATE_READSTR);
    return 0;
}
