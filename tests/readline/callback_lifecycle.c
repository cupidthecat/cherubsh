#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <readline/readline.h>

static int callback_count;
static int callback_state;
static int callback_point;
static int callback_end;
static int null_count;
static int eof_found;
static int eof_state;
static int remove_inside;
static int redisplay_count;
static int prep_count;
static int deprep_count;
static char received[64];

static void line_callback(char *line)
{
    callback_count++;
    callback_state = RL_ISSTATE(RL_STATE_CALLBACK) != 0;
    callback_point = rl_point;
    callback_end = rl_end;
    if (line != NULL) {
        snprintf(received, sizeof(received), "%s", line);
    } else {
        null_count++;
        eof_found = rl_eof_found;
        eof_state = RL_ISSTATE(RL_STATE_EOF) != 0;
    }
    free(line);
    if (remove_inside)
        rl_callback_handler_remove();
}

static void prep_terminal(int meta_flag)
{
    prep_count += meta_flag == 0 || meta_flag == 1;
}

static void deprep_terminal(void)
{
    deprep_count++;
}

static void redisplay(void)
{
    redisplay_count++;
}

int main(void)
{
    int input_pipe[2];
    FILE *input;
    FILE *output = tmpfile();
    int calls;

    if (pipe(input_pipe) != 0 || output == NULL)
        return 2;
    input = fdopen(input_pipe[0], "r");
    if (input == NULL)
        return 3;
    if (write(input_pipe[1], "a", 1) != 1)
        return 4;

    rl_initialize();
    rl_catch_signals = 0;
    rl_instream = input;
    rl_outstream = output;
    rl_prep_term_function = prep_terminal;
    rl_deprep_term_function = deprep_terminal;
    rl_redisplay_function = redisplay;
    remove_inside = 1;
    rl_callback_handler_install("callback> ", line_callback);
    printf("installed=%d\n", RL_ISSTATE(RL_STATE_CALLBACK) != 0);
    rl_callback_read_char();
    printf("partial=%d,redisplay=%d\n", callback_count, redisplay_count);
    if (write(input_pipe[1], "c\002b\n", 4) != 4)
        return 5;
    close(input_pipe[1]);

    for (calls = 0; calls < 32 && callback_count == 0; calls++)
        rl_callback_read_char();

    printf("callback=%d,%d,%d,%d,%s\n", callback_count, callback_state,
           callback_point, callback_end, received);
    printf("self-remove=%d,%d,hooks=%d,%d\n",
           RL_ISSTATE(RL_STATE_CALLBACK) == 0, rl_line_buffer[0] == '\0',
           prep_count, deprep_count);
    printf("redisplays=%d\n", redisplay_count);

    remove_inside = 0;
    rl_callback_handler_install("eof> ", line_callback);
    rl_callback_read_char();
    printf("eof=%d,%d,%d,%d\n", null_count, eof_found, eof_state,
           callback_count);
    rl_callback_handler_remove();
    printf("removed=%d,hooks=%d,%d\n",
           RL_ISSTATE(RL_STATE_CALLBACK) == 0, prep_count, deprep_count);
    fclose(input);
    fclose(output);
    return callback_count == 2 && null_count == 1 ? 0 : 3;
}
