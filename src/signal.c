#include "cupid/signal.h"

#include <signal.h>
#include <unistd.h>

static volatile sig_atomic_t g_prompt_mode = 0;
static volatile sig_atomic_t g_interrupted = 0;

static void on_sigint(int signo) {
    (void)signo;
    g_interrupted = 1;
    if (g_prompt_mode) {
        (void)write(STDOUT_FILENO, "\n", 1);
    }
}

void cupid_signal_setup(void) {
    struct sigaction sa;
    sa.sa_handler = on_sigint;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    sigaction(SIGINT, &sa, NULL);
}

void cupid_signal_set_prompt_mode(int enabled) {
    g_prompt_mode = enabled ? 1 : 0;
}

void cupid_signal_clear_interrupt(void) {
    g_interrupted = 0;
}

int cupid_signal_was_interrupted(void) {
    return g_interrupted ? 1 : 0;
}
