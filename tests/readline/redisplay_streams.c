#include <stdio.h>
#include <stdlib.h>
#include <readline/readline.h>

static int redisplay_count;
static int redisplay_state;

static void redisplay_hook(void)
{
    redisplay_count++;
    redisplay_state = RL_ISSTATE(RL_STATE_REDISPLAYING) != 0;
}

int main(void)
{
    FILE *input = tmpfile();
    FILE *output = tmpfile();
    char *line;
    long output_size;

    if (input == NULL || output == NULL)
        return 2;
    fputs("stream line\n", input);
    rewind(input);

    rl_initialize();
    rl_catch_signals = 0;
    rl_instream = input;
    rl_outstream = output;
    line = readline("stream> ");
    fflush(output);
    output_size = ftell(output);
    printf("stream=%s,%ld\n", line, output_size);
    free(line);

    rl_redisplay_function = redisplay_hook;
    printf("forced=%d\n", rl_forced_update_display());
    printf("redisplay=%d,%d\n", redisplay_count, redisplay_state);

    fclose(input);
    fclose(output);
    return 0;
}
