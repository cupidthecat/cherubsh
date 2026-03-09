#ifndef CUPID_HISTORY_H
#define CUPID_HISTORY_H

void cupid_history_init(void);
void cupid_history_cleanup(void);
void cupid_history_add(const char *line);
const char *cupid_history_get(int index);
int cupid_history_count(void);
void cupid_history_clear(void);
void cupid_history_load(void);

#endif
