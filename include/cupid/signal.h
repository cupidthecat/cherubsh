#ifndef CUPID_SIGNAL_H
#define CUPID_SIGNAL_H

void cupid_signal_setup(void);
void cupid_signal_set_prompt_mode(int enabled);
void cupid_signal_clear_interrupt(void);
int cupid_signal_was_interrupted(void);

#endif
