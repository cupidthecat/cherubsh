#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <readline/history.h>

static void print_history(const char *label)
{
    HIST_ENTRY **entries = history_list();
    int index;

    printf("%s=%d,%d", label, history_length, where_history());
    for (index = 0; index < history_length && entries != NULL && entries[index] != NULL; index++)
        printf(",%s", entries[index]->line);
    putchar('\n');
}

int main(void)
{
    HISTORY_STATE *state;
    HISTORY_STATE *saved_state;
    HISTORY_STATE stack_state;
    HISTORY_STATE switch_state;
    HIST_ENTRY *stack_entries[2];
    HIST_ENTRY *switch_entries[2];
    HIST_ENTRY *not_restored;
    char *timestamp;
    char line[32];
    int index;
    int old_limit;

    using_history();
    clear_history();
    add_history("one");
    add_history("two");
    add_history("three");
    history_set_pos(1);

    state = history_get_history_state();
    if (state == NULL)
        return 2;
    printf("snapshot=%d,%d,%d,%d,%s,%s,%s\n",
           state->offset, state->length, state->size, state->flags,
           state->entries[0]->line, state->entries[1]->line,
           state->entries[2]->line);

    add_history("not restored");
    not_restored = history_get(history_base + 3);
    history_set_history_state(state);
    printf("same-backing=%d,%d\n", history_length, history_offset);
    free_history_entry(not_restored);
    free(state);
    print_history("restored");

    stifle_history(2);
    add_history("four");
    print_history("stifled");
    state = history_get_history_state();
    printf("stifled-state=%d,%d,%d\n", state->length, state->size,
           (state->flags & HS_STIFLED) != 0);
    history_set_history_state(state);
    free(state);
    add_history("five");
    state = history_get_history_state();
    printf("stifled-growth=%d,%d\n", state->length, state->size);
    free(state);

    old_limit = unstifle_history();
    printf("unstifle=%d,%d,%d\n", old_limit, history_is_stifled(),
           unstifle_history());
    clear_history();

    timestamp = strdup("#0");
    stack_entries[0] = alloc_history_entry("stacked", timestamp);
    stack_entries[1] = NULL;
    stack_state.entries = stack_entries;
    stack_state.offset = 1;
    stack_state.length = 1;
    stack_state.size = 2;
    stack_state.flags = 0;
    history_set_history_state(&stack_state);
    print_history("stack-state");
    clear_history();

    for (index = 0; index < 33000; index++) {
        snprintf(line, sizeof(line), "entry %d", index);
        add_history(line);
    }
    state = history_get_history_state();
    printf("large-state=%d,%d\n", state->length, state->size);
    free(state);
    clear_history();

    add_history("saved");
    saved_state = history_get_history_state();
    timestamp = strdup("#1");
    switch_entries[0] = alloc_history_entry("alternate", timestamp);
    switch_entries[1] = NULL;
    switch_state.entries = switch_entries;
    switch_state.offset = 1;
    switch_state.length = 1;
    switch_state.size = 2;
    switch_state.flags = 0;
    history_set_history_state(&switch_state);
    print_history("switch-alternate");
    history_set_history_state(saved_state);
    print_history("switch-saved");
    clear_history();
    history_set_history_state(&switch_state);
    print_history("switch-alternate-again");
    clear_history();
    free(saved_state);
    return 0;
}
