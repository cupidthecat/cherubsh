#include "cupid/brace.h"

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int results_push(char ***arr, size_t *count, char *s) {
    char **next = realloc(*arr, sizeof(char *) * (*count + 1));
    if (next == NULL) { free(s); return -1; }
    *arr = next;
    (*arr)[*count] = s;
    (*count)++;
    return 0;
}

static void results_free(char **arr, size_t count) {
    size_t i;
    for (i = 0; i < count; i++) free(arr[i]);
    free(arr);
}

static char *concat3(const char *a, const char *b, const char *c) {
    size_t la = strlen(a);
    size_t lb = strlen(b);
    size_t lc = strlen(c);
    char *out = calloc(la + lb + lc + 1, 1);
    if (out == NULL) return NULL;
    memcpy(out, a, la);
    memcpy(out + la, b, lb);
    memcpy(out + la + lb, c, lc);
    return out;
}

static int skip_backtick_subst(const char *s, size_t start, size_t *end_out) {
    size_t i;
    if (s == NULL || s[start] != '`' || end_out == NULL) return -1;
    i = start + 1;
    while (s[i] != '\0') {
        if (s[i] == '\\' && s[i + 1] != '\0') {
            i += 2;
            continue;
        }
        if (s[i] == '`') {
            *end_out = i;
            return 0;
        }
        i++;
    }
    return -1;
}

static int skip_dollar_paren_subst(const char *s, size_t start, size_t *end_out) {
    size_t i;
    int depth = 1;
    int in_single = 0;
    int in_double = 0;

    if (s == NULL || s[start] != '$' || s[start + 1] != '(' || end_out == NULL) return -1;
    i = start + 2;
    while (s[i] != '\0') {
        if (!in_single && s[i] == '\\' && s[i + 1] != '\0') {
            i += 2;
            continue;
        }
        if (!in_double && s[i] == '\'') {
            in_single = !in_single;
            i++;
            continue;
        }
        if (!in_single && s[i] == '"') {
            in_double = !in_double;
            i++;
            continue;
        }
        if (in_single || in_double) {
            i++;
            continue;
        }
        if (s[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(s, i, &bt_end) == 0) {
                i = bt_end + 1;
                continue;
            }
        }
        if (s[i] == '$' && s[i + 1] == '(') {
            depth++;
            i += 2;
            continue;
        }
        if (s[i] == '(') {
            depth++;
            i++;
            continue;
        }
        if (s[i] == ')') {
            depth--;
            if (depth == 0) {
                *end_out = i;
                return 0;
            }
            i++;
            continue;
        }
        i++;
    }
    return -1;
}

static int find_matching_brace(const char *s, size_t start, size_t *end) {
    int depth = 1;
    int in_single = 0;
    int in_double = 0;
    size_t i = start;
    while (s[i] != '\0') {
        if (!in_single && s[i] == '\\' && s[i + 1] != '\0') {
            i += 2;
            continue;
        }
        if (!in_double && s[i] == '\'') {
            in_single = !in_single;
            i++;
            continue;
        }
        if (!in_single && s[i] == '"') {
            in_double = !in_double;
            i++;
            continue;
        }
        if (in_single || in_double) {
            i++;
            continue;
        }
        if (s[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(s, i, &bt_end) == 0) {
                i = bt_end + 1;
                continue;
            }
        }
        if (s[i] == '$' && s[i + 1] == '(') {
            size_t cs_end;
            if (skip_dollar_paren_subst(s, i, &cs_end) == 0) {
                i = cs_end + 1;
                continue;
            }
        }
        if (s[i] == '$' && s[i + 1] == '{') {
            size_t pe;
            if (find_matching_brace(s, i + 2, &pe) == 0) {
                i = pe + 1;
                continue;
            }
        }
        if (s[i] == '{') depth++;
        else if (s[i] == '}') {
            depth--;
            if (depth == 0) {
                *end = i;
                return 0;
            }
        }
        i++;
    }
    return -1;
}

static int has_top_level_comma(const char *s, size_t len) {
    int depth = 0;
    int in_single = 0;
    int in_double = 0;
    size_t i;
    for (i = 0; i < len; i++) {
        if (!in_single && s[i] == '\\' && i + 1 < len) {
            i++;
            continue;
        }
        if (!in_double && s[i] == '\'') {
            in_single = !in_single;
            continue;
        }
        if (!in_single && s[i] == '"') {
            in_double = !in_double;
            continue;
        }
        if (in_single || in_double) continue;
        if (s[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(s, i, &bt_end) == 0 && bt_end < len) {
                i = bt_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '(') {
            size_t cs_end;
            if (skip_dollar_paren_subst(s, i, &cs_end) == 0 && cs_end < len) {
                i = cs_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '{') {
            size_t pe;
            if (find_matching_brace(s, i + 2, &pe) == 0 && pe < len) {
                i = pe;
                continue;
            }
        }
        if (s[i] == '{') depth++;
        else if (s[i] == '}') depth--;
        else if (s[i] == ',' && depth == 0) return 1;
    }
    return 0;
}

static int has_dotdot(const char *s, size_t len) {
    int depth = 0;
    int in_single = 0;
    int in_double = 0;
    size_t i;
    for (i = 0; i + 1 < len; i++) {
        if (!in_single && s[i] == '\\' && i + 1 < len) {
            i++;
            continue;
        }
        if (!in_double && s[i] == '\'') {
            in_single = !in_single;
            continue;
        }
        if (!in_single && s[i] == '"') {
            in_double = !in_double;
            continue;
        }
        if (in_single || in_double) continue;
        if (s[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(s, i, &bt_end) == 0 && bt_end < len) {
                i = bt_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '(') {
            size_t cs_end;
            if (skip_dollar_paren_subst(s, i, &cs_end) == 0 && cs_end < len) {
                i = cs_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '{') {
            size_t pe;
            if (find_matching_brace(s, i + 2, &pe) == 0 && pe < len) {
                i = pe;
                continue;
            }
        }
        if (s[i] == '{') {
            depth++;
            continue;
        }
        if (s[i] == '}') {
            depth--;
            continue;
        }
        if (depth == 0 && s[i] == '.' && s[i + 1] == '.') return 1;
    }
    return 0;
}

static int split_top_commas(const char *s, size_t len, char ***out, size_t *out_count) {
    int depth = 0;
    int in_single = 0;
    int in_double = 0;
    size_t start = 0;
    size_t i;
    char *item;

    *out = NULL;
    *out_count = 0;

    for (i = 0; i < len; i++) {
        if (!in_single && s[i] == '\\' && i + 1 < len) {
            i++;
            continue;
        }
        if (!in_double && s[i] == '\'') {
            in_single = !in_single;
            continue;
        }
        if (!in_single && s[i] == '"') {
            in_double = !in_double;
            continue;
        }
        if (in_single || in_double) continue;
        if (s[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(s, i, &bt_end) == 0 && bt_end < len) {
                i = bt_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '(') {
            size_t cs_end;
            if (skip_dollar_paren_subst(s, i, &cs_end) == 0 && cs_end < len) {
                i = cs_end;
                continue;
            }
        }
        if (s[i] == '$' && i + 1 < len && s[i + 1] == '{') {
            size_t pe;
            if (find_matching_brace(s, i + 2, &pe) == 0 && pe < len) {
                i = pe;
                continue;
            }
        }
        if (s[i] == '{') depth++;
        else if (s[i] == '}') depth--;
        else if (s[i] == ',' && depth == 0) {
            item = calloc(i - start + 1, 1);
            if (item == NULL) goto fail;
            if (i > start) memcpy(item, s + start, i - start);
            if (results_push(out, out_count, item) != 0) goto fail;
            start = i + 1;
        }
    }

    item = calloc(len - start + 1, 1);
    if (item == NULL) goto fail;
    if (len > start) memcpy(item, s + start, len - start);
    if (results_push(out, out_count, item) != 0) goto fail;
    return 0;

fail:
    results_free(*out, *out_count);
    *out = NULL;
    *out_count = 0;
    return -1;
}

static int parse_int_val(const char *s, size_t len, long *val, int *width) {
    char *buf;
    char *end;
    int w = 0;

    if (len == 0) return -1;
    buf = calloc(len + 1, 1);
    if (buf == NULL) return -1;
    memcpy(buf, s, len);

    if (buf[0] == '-') {
        if (len > 1 && buf[1] == '0') w = (int)len;
    } else {
        if (buf[0] == '0' && len > 1) w = (int)len;
    }

    *val = strtol(buf, &end, 10);
    if (*end != '\0') { free(buf); return -1; }
    if (*val == 0 && buf[0] == '-') w = 0;
    free(buf);
    if (width) *width = w;
    return 0;
}

static char *dup_sequence_char_word(char ch) {
    char *s;
    if (ch == '\\') {
        s = calloc(1, 1);
        return s;
    }
    int needs_escape = (ch == '$' || ch == '`' || ch == '"' || ch == '\'');
    s = calloc(needs_escape ? 3 : 2, 1);
    if (s == NULL) return NULL;
    if (needs_escape) {
        s[0] = '\\';
        s[1] = ch;
        s[2] = '\0';
    } else {
        s[0] = ch;
        s[1] = '\0';
    }
    return s;
}

static int expand_numeric_seq(long start, long end, long step, int pad_width,
                              char ***out, size_t *out_count) {
    long i;
    *out = NULL;
    *out_count = 0;

    if (step == 0) step = 1;
    if (step < 0) step = -step;

    if (start <= end) {
        for (i = start; i <= end; i += step) {
            char buf[64];
            char *s;
            if (pad_width > 0)
                snprintf(buf, sizeof(buf), "%0*ld", pad_width, i);
            else
                snprintf(buf, sizeof(buf), "%ld", i);
            s = strdup(buf);
            if (s == NULL || results_push(out, out_count, s) != 0) goto fail;
        }
    } else {
        for (i = start; i >= end; i -= step) {
            char buf[64];
            char *s;
            if (pad_width > 0)
                snprintf(buf, sizeof(buf), "%0*ld", pad_width, i);
            else
                snprintf(buf, sizeof(buf), "%ld", i);
            s = strdup(buf);
            if (s == NULL || results_push(out, out_count, s) != 0) goto fail;
        }
    }
    return 0;

fail:
    results_free(*out, *out_count);
    *out = NULL;
    *out_count = 0;
    return -1;
}

static int expand_char_seq(char sc, char ec, long step,
                           char ***out, size_t *out_count) {
    int c;
    *out = NULL;
    *out_count = 0;

    if (step == 0) step = 1;
    if (step < 0) step = -step;

    if (sc <= ec) {
        for (c = (unsigned char)sc; c <= (unsigned char)ec; c += (int)step) {
            char *s;
            s = dup_sequence_char_word((char)c);
            if (s == NULL || results_push(out, out_count, s) != 0) goto fail;
            if ((long)((unsigned char)ec - (unsigned char)c) < step) break;
        }
    } else {
        for (c = (unsigned char)sc; c >= (unsigned char)ec; c -= (int)step) {
            char *s;
            s = dup_sequence_char_word((char)c);
            if (s == NULL || results_push(out, out_count, s) != 0) goto fail;
            if ((long)((unsigned char)c - (unsigned char)ec) < step) break;
        }
    }
    return 0;

fail:
    results_free(*out, *out_count);
    *out = NULL;
    *out_count = 0;
    return -1;
}

static int try_sequence(const char *content, size_t len,
                        char ***out, size_t *out_count) {
    const char *dd1 = NULL;
    const char *dd2 = NULL;
    size_t i;
    size_t seg1_len, seg2_start, seg2_end;

    for (i = 0; i + 1 < len; i++) {
        if (content[i] == '.' && content[i + 1] == '.') {
            if (dd1 == NULL) {
                dd1 = content + i;
                i++;
            } else if (dd2 == NULL) {
                dd2 = content + i;
                i++;
            }
        }
    }
    if (dd1 == NULL) return -1;

    seg1_len = (size_t)(dd1 - content);
    seg2_start = (size_t)(dd1 - content) + 2;

    if (dd2 != NULL) {
        seg2_end = (size_t)(dd2 - content);
    } else {
        seg2_end = len;
    }

    if (seg1_len == 1 && isalpha((unsigned char)content[0]) &&
        (seg2_end - seg2_start) == 1 &&
        isalpha((unsigned char)content[seg2_start])) {
        long step = 1;
        if (dd2 != NULL) {
            size_t step_start = (size_t)(dd2 - content) + 2;
            if (parse_int_val(content + step_start, len - step_start,
                              &step, NULL) != 0)
                return -1;
        }
        return expand_char_seq(content[0], content[seg2_start], step,
                               out, out_count);
    }

    {
        long start_val, end_val, step_val = 1;
        int w1 = 0, w2 = 0, pad = 0;

        if (parse_int_val(content, seg1_len, &start_val, &w1) != 0) return -1;
        if (parse_int_val(content + seg2_start, seg2_end - seg2_start,
                          &end_val, &w2) != 0)
            return -1;

        if (dd2 != NULL) {
            size_t step_start = (size_t)(dd2 - content) + 2;
            if (parse_int_val(content + step_start, len - step_start,
                              &step_val, NULL) != 0)
                return -1;
        }

        pad = (w1 > w2) ? w1 : w2;
        return expand_numeric_seq(start_val, end_val, step_val, pad,
                                  out, out_count);
    }
}

static int brace_expand_recursive(const char *word,
                                  char ***out_words, size_t *out_count) {
    size_t len = strlen(word);
    size_t i;
    int in_single = 0;
    int in_double = 0;

    *out_words = NULL;
    *out_count = 0;

    for (i = 0; i < len; i++) {
        if (!in_single && word[i] == '\\' && i + 1 < len) {
            i++;
            continue;
        }
        if (!in_double && word[i] == '\'') {
            in_single = !in_single;
            continue;
        }
        if (!in_single && word[i] == '"') {
            in_double = !in_double;
            continue;
        }
        if (in_single || in_double) continue;
        if (word[i] == '`') {
            size_t bt_end;
            if (skip_backtick_subst(word, i, &bt_end) == 0 && bt_end < len) {
                i = bt_end;
                continue;
            }
        }
        if (word[i] == '$' && i + 1 < len && word[i + 1] == '(') {
            size_t cs_end;
            if (skip_dollar_paren_subst(word, i, &cs_end) == 0 && cs_end < len) {
                i = cs_end;
                continue;
            }
        }
        if (word[i] == '$' && i + 1 < len && word[i + 1] == '{') {
            size_t skip_end;
            if (find_matching_brace(word, i + 2, &skip_end) == 0)
                i = skip_end;
            continue;
        }
        if (word[i] == '{') {
            size_t end;
            if (find_matching_brace(word, i + 1, &end) == 0) {
                size_t content_len = end - i - 1;
                const char *content = word + i + 1;

                if (has_top_level_comma(content, content_len) ||
                    has_dotdot(content, content_len)) {
                    char *pre;
                    char *post;
                    char **items = NULL;
                    size_t item_count = 0;
                    size_t j;
                    int rc;

                    pre = calloc(i + 1, 1);
                    if (pre == NULL) return -1;
                    if (i > 0) memcpy(pre, word, i);

                    post = strdup(word + end + 1);
                    if (post == NULL) { free(pre); return -1; }

                    if (has_top_level_comma(content, content_len)) {
                        rc = split_top_commas(content, content_len,
                                              &items, &item_count);
                    } else {
                        rc = try_sequence(content, content_len,
                                          &items, &item_count);
                    }

                    if (rc != 0 || item_count == 0) {
                        results_free(items, item_count);
                        free(pre);
                        free(post);
                        continue;
                    }

                    for (j = 0; j < item_count; j++) {
                        char *combined = concat3(pre, items[j], post);
                        char **sub = NULL;
                        size_t sub_count = 0;
                        size_t k;

                        if (combined == NULL) goto inner_fail;

                        if (brace_expand_recursive(combined, &sub,
                                                   &sub_count) != 0) {
                            free(combined);
                            goto inner_fail;
                        }
                        free(combined);

                        for (k = 0; k < sub_count; k++) {
                            if (results_push(out_words, out_count,
                                             sub[k]) != 0) {
                                size_t m;
                                for (m = k + 1; m < sub_count; m++)
                                    free(sub[m]);
                                free(sub);
                                goto inner_fail;
                            }
                        }
                        free(sub);
                        continue;

                    inner_fail:
                        results_free(items, item_count);
                        free(pre);
                        free(post);
                        results_free(*out_words, *out_count);
                        *out_words = NULL;
                        *out_count = 0;
                        return -1;
                    }

                    results_free(items, item_count);
                    free(pre);
                    free(post);
                    return 0;
                }
            }
        }
    }

    {
        char *copy = strdup(word);
        if (copy == NULL) return -1;
        if (results_push(out_words, out_count, copy) != 0) return -1;
    }
    return 0;
}

int cupid_brace_expand(const char *word, char ***out_words, size_t *out_count) {
    if (word == NULL || out_words == NULL || out_count == NULL) return -1;
    return brace_expand_recursive(word, out_words, out_count);
}
