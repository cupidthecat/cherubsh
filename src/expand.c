#include "cupid/expand.h"

#include <ctype.h>
#include <fnmatch.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#include "cupid/arith.h"
#include "cupid/lexer.h"
#include "cupid/shell.h"
#include "cupid/vars.h"

static int append_bytes(char **buf, size_t *len, size_t *cap, const char *data, size_t data_len);
static int append_cstr(char **buf, size_t *len, size_t *cap, const char *data);
static char *g_expand_error = NULL;

static int pattern_bracket_has_closing(const char *p) {
    if (p == NULL || *p != '[') return 0;
    p++;
    if (*p == '!' || *p == '^') p++;
    if (*p == ']') p++;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (*p == ']') return 1;
        p++;
    }
    return 0;
}

static char *normalize_fnmatch_pattern(const char *src) {
    char *out;
    size_t len;
    size_t i;
    size_t j = 0;

    if (src == NULL) return NULL;
    len = strlen(src);
    out = calloc((len * 2) + 1, 1);
    if (out == NULL) return NULL;

    for (i = 0; i < len; i++) {
        if (src[i] == '\\') {
            out[j++] = '\\';
            if (src[i + 1] == '\0') {
                out[j++] = '\\';
                continue;
            }
            out[j++] = src[++i];
            continue;
        }
        if (src[i] == '[' && !pattern_bracket_has_closing(src + i)) {
            out[j++] = '\\';
        }
        out[j++] = src[i];
    }
    out[j] = '\0';
    return out;
}

static int cupid_fnmatch(const char *pat, const char *text) {
    char *normalized;
    int rc;

    normalized = normalize_fnmatch_pattern(pat);
    if (normalized == NULL) return fnmatch(pat, text, 0);
    rc = fnmatch(normalized, text, 0);
    free(normalized);
    return rc;
}

static char escaped_ifs_placeholder(char ch) {
    switch (ch) {
        case ' ': return CUPID_ESC_IFS_SPACE_PLACEHOLDER;
        case '\t': return CUPID_ESC_IFS_TAB_PLACEHOLDER;
        case '\n': return CUPID_ESC_IFS_NEWLINE_PLACEHOLDER;
        default: return '\0';
    }
}

static char *expand_param_word_fragment(const char *text, struct cupid_shell *shell, int *had_quotes_out) {
    struct cupid_tokens toks = {0};
    char *result = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;
    int saw_word = 0;

    if (had_quotes_out != NULL) *had_quotes_out = 0;
    if (text == NULL) return strdup("");

    if (cupid_lex(text, &toks) != 0) {
        return cupid_expand_text(text, CUPID_QUOTE_NONE, shell);
    }

    for (i = 0; i < toks.count; i++) {
        char *expanded;
        if (toks.items[i].kind != TOK_WORD) continue;
        if (saw_word && append_cstr(&result, &len, &cap, " ") != 0) {
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        saw_word = 1;
        if (had_quotes_out != NULL && toks.items[i].word.had_quotes) {
            *had_quotes_out = 1;
        }
        expanded = cupid_expand_word(&toks.items[i].word, shell);
        if (expanded == NULL) {
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        if (append_cstr(&result, &len, &cap, expanded) != 0) {
            free(expanded);
            free(result);
            cupid_tokens_free(&toks);
            return NULL;
        }
        free(expanded);
    }

    cupid_tokens_free(&toks);
    if (result == NULL) result = strdup("");
    return result;
}

static int param_subst_fragment_needs_expansion(const char *text) {
    const char *p = text;

    if (text == NULL) return 0;
    while (*p != '\0') {
        if (*p == '$' || *p == '`' || *p == '\'' || *p == '"') return 1;
        if (*p == '\\' && p[1] == '\n') return 1;
        p++;
    }
    return 0;
}

static char *expand_param_subst_fragment(const char *text, struct cupid_shell *shell) {
    if (text == NULL) return strdup("");
    if (!param_subst_fragment_needs_expansion(text)) return strdup(text);
    return expand_param_word_fragment(text, shell, NULL);
}

void cupid_expand_error_reset(void) {
    free(g_expand_error);
    g_expand_error = NULL;
}

void cupid_restore_escaped_ifs_placeholders(char *text) {
    if (text == NULL) return;
    while (*text != '\0') {
        switch (*text) {
            case CUPID_ESC_IFS_SPACE_PLACEHOLDER:
                *text = ' ';
                break;
            case CUPID_ESC_IFS_TAB_PLACEHOLDER:
                *text = '\t';
                break;
            case CUPID_ESC_IFS_NEWLINE_PLACEHOLDER:
                *text = '\n';
                break;
            default:
                break;
        }
        text++;
    }
}

int cupid_expand_error_pending(void) {
    return g_expand_error != NULL;
}

const char *cupid_expand_error_message(void) {
    if (g_expand_error == NULL) {
        return "";
    }
    return g_expand_error;
}

int cupid_expand_error_set(const char *message) {
    char *copy;
    if (message == NULL) message = "";
    copy = strdup(message);
    if (copy == NULL) return -1;
    cupid_expand_error_reset();
    g_expand_error = copy;
    return 0;
}

static int set_expand_error(const char *name, const char *msg) {
    size_t nlen = strlen(name);
    size_t mlen = strlen(msg);
    char *buf = calloc(nlen + mlen + 3, 1);
    if (buf == NULL) {
        return -1;
    }
    memcpy(buf, name, nlen);
    buf[nlen] = ':';
    buf[nlen + 1] = ' ';
    memcpy(buf + nlen + 2, msg, mlen);
    cupid_expand_error_reset();
    g_expand_error = buf;
    return 0;
}

static int scan_to_brace(const char *p, enum cupid_quote outer_quote, const char **end) {
    int nested = 0;
    int mode = 0;
    while (*p != '\0') {
        if (mode == 1) {
            if (*p == '\'') mode = 0;
            p++;
            continue;
        }
        if (mode == 2) {
            if (*p == '\\' && p[1] != '\0') {
                p += 2;
                continue;
            }
            if (*p == '"') mode = 0;
            p++;
            continue;
        }
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (outer_quote != CUPID_QUOTE_DOUBLE && *p == '\'') {
            mode = 1;
            p++;
            continue;
        }
        if (*p == '"') {
            mode = 2;
            p++;
            continue;
        }
        if (p[0] == '$' && p[1] == '{') {
            nested++;
            p += 2;
            continue;
        }
        if (*p == '}') {
            if (nested == 0) {
                *end = p;
                return 0;
            }
            nested--;
        }
        p++;
    }
    return -1;
}

static int char_is_escaped_by_backslashes(const char *start, const char *pos) {
    size_t backslashes = 0;

    if (start == NULL || pos == NULL || pos <= start) return 0;
    while (pos > start && pos[-1] == '\\') {
        backslashes++;
        pos--;
    }
    return (backslashes % 2) != 0;
}

static int parse_braced_param(const char **pp, enum cupid_quote outer_quote,
                              char **name_out, int *colon_mode_out,
                              char *op_out, int *op_long_out, char **word_out) {
    const char *p = *pp;
    const char *name_start;
    const char *name_end;
    char *name;
    char op = '\0';
    int op_long = 0;
    int colon_mode = 0;
    char *word = NULL;
    int numeric_name = 0;
    int special_name = 0;

    if (p[0] != '$' || p[1] != '{') {
        return -1;
    }
    p += 2;

    /* ${!var} indirect expansion */
    if (*p == '!' && (isalpha((unsigned char)p[1]) || p[1] == '_')) {
        const char *ns = p + 1;
        const char *ne = ns;
        while (isalnum((unsigned char)*ne) || *ne == '_') ne++;
        if (*ne == '}') {
            name = calloc((size_t)(ne - ns) + 1, 1);
            if (name == NULL) return -1;
            memcpy(name, ns, (size_t)(ne - ns));
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'I';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = ne + 1;
            return 0;
        }
        if (*ne == '*' && ne[1] == '}') {
            name = calloc((size_t)(ne - ns) + 1, 1);
            if (name == NULL) return -1;
            memcpy(name, ns, (size_t)(ne - ns));
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'P';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = ne + 2;
            return 0;
        }
    }

    if (*p == '@' || *p == '*') {
        char special = *p;
        p++;
        name = calloc(2, 1);
        if (name == NULL) return -1;
        name[0] = special;
        special_name = 1;
        if (*p == '}') {
            p++;
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'A';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = p;
            return 0;
        }
        if (*p == ':') {
            const char *word_start = p + 1;
            const char *word_end;
            while (*p != '\0' && *p != '}') p++;
            if (*p != '}') return -1;
            word_end = p;
            word = calloc((size_t)(word_end - word_start) + 1, 1);
            if (word == NULL) {
                free(name);
                free(word);
                return -1;
            }
            memcpy(word, word_start, (size_t)(word_end - word_start));
            p++;
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'Q';
            *op_long_out = 0;
            *word_out = word;
            *pp = p;
            return 0;
        }
        goto after_name;
    }
    if (*p == '#') {
        p++;
        if (*p == '}') {
            p++;
            name = calloc(2, 1);
            if (name == NULL) return -1;
            name[0] = '#';
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'C';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = p;
            return 0;
        }
        if ((*p == '#' || *p == '?') && p[1] == '}') {
            name = calloc(2, 1);
            if (name == NULL) return -1;
            name[0] = *p;
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'L';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = p + 2;
            return 0;
        }
        if (*p == '@' || *p == '*') {
            if (p[1] != '}') return -1;
            name = calloc(2, 1);
            if (name == NULL) return -1;
            name[0] = '#';
            *name_out = name;
            *colon_mode_out = 0;
            *op_out = 'C';
            *op_long_out = 0;
            *word_out = NULL;
            *pp = p + 2;
            return 0;
        }
        if (!(isalpha((unsigned char)*p) || *p == '_' || isdigit((unsigned char)*p))) {
            return -1;
        }
        name_start = p;
        if (isdigit((unsigned char)*p)) {
            while (isdigit((unsigned char)*p)) p++;
        } else {
            while (isalnum((unsigned char)*p) || *p == '_') p++;
        }
        if (*p != '}') {
            return -1;
        }
        name_end = p;
        name = calloc((size_t)(name_end - name_start) + 1, 1);
        if (name == NULL) {
            return -1;
        }
        memcpy(name, name_start, (size_t)(name_end - name_start));
        p++;
        *name_out = name;
        *colon_mode_out = 0;
        *op_out = 'L';
        *op_long_out = 0;
        *word_out = NULL;
        *pp = p;
        return 0;
    }
    if (!(isalpha((unsigned char)*p) || *p == '_' || isdigit((unsigned char)*p))) {
        return -1;
    }
    name_start = p;
    if (isdigit((unsigned char)*p)) {
        numeric_name = 1;
        while (isdigit((unsigned char)*p)) p++;
    } else {
        while (isalnum((unsigned char)*p) || *p == '_') p++;
    }
    if (!numeric_name && *p == '[') {
        const char *idx = p + 1;
        while (*idx != '\0' && *idx != ']') idx++;
        if (*idx != ']') {
            return -1;
        }
        p = idx + 1;
    }
    name_end = p;
    name = calloc((size_t)(name_end - name_start) + 1, 1);
    if (name == NULL) {
        return -1;
    }
    memcpy(name, name_start, (size_t)(name_end - name_start));

after_name:
    if (*p == '}') {
        p++;
        *name_out = name;
        *colon_mode_out = colon_mode;
        *op_out = (!special_name && numeric_name) ? 'N' : op;
        *op_long_out = op_long;
        *word_out = NULL;
        *pp = p;
        return 0;
    }

    /* ${var^}, ${var^^}, ${var,}, ${var,,} */
    if (*p == '^' || *p == ',') {
        char case_op = *p;
        p++;
        op_long = 0;
        if (*p == case_op) { op_long = 1; p++; }
        if (*p != '}') { free(name); return -1; }
        p++;
        *name_out = name;
        *colon_mode_out = 0;
        *op_out = case_op;
        *op_long_out = op_long;
        *word_out = NULL;
        *pp = p;
        return 0;
    }

    /* ${var/pat/rep}, ${var//pat/rep}, ${var/#pat/rep}, ${var/%pat/rep} */
    if (*p == '/') {
        const char *brace_end;
        const char *pat_start;
        const char *scan;
        const char *slash2;
        char *pat;
        char *rep;
        size_t pat_len, rep_len;

        p++;
        op = 'R';
        op_long = 0;
        if (*p == '/') { op_long = 1; p++; }
        else if (*p == '#') { op_long = 2; p++; }
        else if (*p == '%') { op_long = 3; p++; }

        if (scan_to_brace(p, outer_quote, &brace_end) != 0) { free(name); return -1; }

        pat_start = p;
        slash2 = NULL;
        for (scan = p; scan < brace_end; scan++) {
            if (*scan == '/' && !char_is_escaped_by_backslashes(p, scan)) {
                slash2 = scan;
                break;
            }
        }
        if (slash2 != NULL) {
            pat_len = (size_t)(slash2 - pat_start);
            rep_len = (size_t)(brace_end - (slash2 + 1));
        } else {
            pat_len = (size_t)(brace_end - pat_start);
            rep_len = 0;
        }

        pat = calloc(pat_len + 1, 1);
        rep = calloc(rep_len + 1, 1);
        if (!pat || !rep) { free(pat); free(rep); free(name); return -1; }
        if (pat_len > 0) memcpy(pat, pat_start, pat_len);
        if (slash2 && rep_len > 0) memcpy(rep, slash2 + 1, rep_len);

        {
            size_t total = pat_len + 1 + rep_len + 1;
            word = calloc(total, 1);
            if (!word) { free(pat); free(rep); free(name); return -1; }
            memcpy(word, pat, pat_len);
            word[pat_len] = '\0';
            memcpy(word + pat_len + 1, rep, rep_len);
        }
        free(pat);
        free(rep);

        *name_out = name;
        *colon_mode_out = (int)pat_len;
        *op_out = op;
        *op_long_out = op_long;
        *word_out = word;
        *pp = brace_end + 1;
        return 0;
    }

    /* ${var:offset} and ${var:offset:length} */
    if (*p == ':') {
        const char *brace_end;
        p++;
        if (*p == '-' || *p == '=' || *p == '+' || *p == '?') {
            colon_mode = 1;
            goto standard_ops;
        }
        op = 'S';
        if (scan_to_brace(p, outer_quote, &brace_end) != 0) { free(name); return -1; }
        {
            size_t wlen = (size_t)(brace_end - p);
            word = calloc(wlen + 1, 1);
            if (!word) { free(name); return -1; }
            memcpy(word, p, wlen);
        }
        *name_out = name;
        *colon_mode_out = 0;
        *op_out = op;
        *op_long_out = 0;
        *word_out = word;
        *pp = brace_end + 1;
        return 0;
    }

standard_ops:
    if (*p == ':') {
        colon_mode = 1;
        p++;
    }
    if (*p == '-' || *p == '=' || *p == '+' || *p == '?' || *p == '%' || *p == '#') {
        const char *word_start;
        const char *word_end;
        op = *p;
        p++;
        if ((op == '%' || op == '#') && *p == op) {
            op_long = 1;
            p++;
        }
        if (colon_mode && (op == '%' || op == '#')) {
            free(name);
            return -1;
        }
        word_start = p;
        if (scan_to_brace(p, outer_quote, &word_end) != 0) {
            free(name);
            return -1;
        }
        word = calloc((size_t)(word_end - word_start) + 1, 1);
        if (word == NULL) {
            free(name);
            return -1;
        }
        memcpy(word, word_start, (size_t)(word_end - word_start));
        p = word_end + 1;
        *name_out = name;
        *colon_mode_out = colon_mode;
        *op_out = op;
        *op_long_out = op_long;
        *word_out = word;
        *pp = p;
        return 0;
    }

    free(name);
    return -1;
}

static char *dup_slice(const char *src, size_t start, size_t end) {
    char *out;
    if (end < start) {
        return NULL;
    }
    out = calloc((end - start) + 1, 1);
    if (out == NULL) {
        return NULL;
    }
    if (end > start) {
        memcpy(out, src + start, end - start);
    }
    return out;
}

static char *remove_prefix_pattern(const char *cur, const char *pat, int longest) {
    size_t len = strlen(cur);
    size_t i;

    if (pat == NULL) {
        return strdup(cur);
    }
    if (!longest) {
        for (i = 0; i <= len; i++) {
            char *pref = dup_slice(cur, 0, i);
            char *res;
            if (pref == NULL) {
                return NULL;
            }
            if (cupid_fnmatch(pat, pref) == 0) {
                free(pref);
                res = strdup(cur + i);
                return res;
            }
            free(pref);
        }
    } else {
        for (i = len + 1; i > 0; i--) {
            size_t cut = i - 1;
            char *pref = dup_slice(cur, 0, cut);
            char *res;
            if (pref == NULL) {
                return NULL;
            }
            if (cupid_fnmatch(pat, pref) == 0) {
                free(pref);
                res = strdup(cur + cut);
                return res;
            }
            free(pref);
        }
    }
    return strdup(cur);
}

static char *remove_suffix_pattern(const char *cur, const char *pat, int longest) {
    size_t len = strlen(cur);
    size_t i;

    if (pat == NULL) {
        return strdup(cur);
    }
    if (!longest) {
        for (i = len + 1; i > 0; i--) {
            size_t cut = i - 1;
            const char *suf = cur + cut;
            if (cupid_fnmatch(pat, suf) == 0) {
                return dup_slice(cur, 0, cut);
            }
        }
    } else {
        for (i = 0; i <= len; i++) {
            const char *suf = cur + i;
            if (cupid_fnmatch(pat, suf) == 0) {
                return dup_slice(cur, 0, i);
            }
        }
    }
    return strdup(cur);
}

static int parse_array_ref_name(const char *name, size_t *base_len,
                                const char **index_start, size_t *index_len) {
    const char *lb;
    const char *rb;
    if (name == NULL) return 0;
    lb = strchr(name, '[');
    if (lb == NULL || lb == name) return 0;
    rb = strrchr(lb + 1, ']');
    if (rb == NULL || rb[1] != '\0') return 0;
    if (base_len != NULL) *base_len = (size_t)(lb - name);
    if (index_start != NULL) *index_start = lb + 1;
    if (index_len != NULL) *index_len = (size_t)(rb - (lb + 1));
    return 1;
}

static int parse_nonneg_index(const char *s, size_t len, size_t *out) {
    size_t i;
    size_t v = 0;
    if (s == NULL || len == 0) return -1;
    for (i = 0; i < len; i++) {
        unsigned char ch = (unsigned char)s[i];
        if (!isdigit(ch)) return -1;
        v = v * 10u + (size_t)(ch - '0');
    }
    if (out != NULL) *out = v;
    return 0;
}

static char *join_array_members(struct cupid_shell *shell, const char *name, int star_mode) {
    size_t count = cupid_array_member_count(shell, name);
    const char *ifs = cupid_vars_get(shell, "IFS");
    char sep = ' ';
    size_t total = 0;
    size_t i;
    char *joined;
    char *rp;
    if (count == 0) return NULL;
    if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
    if (ifs != NULL && ifs[0] == '\0') sep = '\0';
    for (i = 0; i < count; i++) total += strlen(cupid_array_member_value(shell, name, i));
    if ((star_mode ? sep != '\0' : 1) && count > 1) total += count - 1;
    joined = calloc(total + 1, 1);
    if (joined == NULL) return NULL;
    rp = joined;
    for (i = 0; i < count; i++) {
        const char *it = cupid_array_member_value(shell, name, i);
        size_t len = strlen(it);
        memcpy(rp, it, len);
        rp += len;
        if (i + 1 < count) {
            char use_sep = star_mode ? sep : ' ';
            if (use_sep != '\0') *rp++ = use_sep;
        }
    }
    return joined;
}

struct positional_slice_spec {
    long offset;
    int has_length;
    long length;
};

static int is_all_digits(const char *s) {
    size_t i;
    if (s == NULL || s[0] == '\0') return 0;
    for (i = 0; s[i] != '\0'; i++) {
        if (!isdigit((unsigned char)s[i])) return 0;
    }
    return 1;
}

static int parse_long_token(const char *s, size_t len, long *out) {
    const char *start = s;
    const char *end = s + len;
    char *tail = NULL;
    long v;

    while (start < end && isspace((unsigned char)*start)) start++;
    while (end > start && isspace((unsigned char)end[-1])) end--;
    if (start == end) return -1;

    v = strtol(start, &tail, 10);
    if (tail == start) return -1;
    while (tail < end && isspace((unsigned char)*tail)) tail++;
    if (tail != end) return -1;
    *out = v;
    return 0;
}

static int parse_positional_slice_spec(const char *spec, struct positional_slice_spec *out) {
    const char *colon;
    size_t spec_len;
    if (spec == NULL || out == NULL) return -1;
    spec_len = strlen(spec);
    colon = strchr(spec, ':');
    if (colon == NULL) {
        if (parse_long_token(spec, spec_len, &out->offset) != 0) return -1;
        out->has_length = 0;
        out->length = 0;
        return 0;
    }
    if (parse_long_token(spec, (size_t)(colon - spec), &out->offset) != 0) return -1;
    out->has_length = 1;
    if (parse_long_token(colon + 1, spec_len - (size_t)(colon + 1 - spec), &out->length) != 0) {
        /* bash treats empty length as zero */
        const char *ls = colon + 1;
        const char *le = spec + spec_len;
        while (ls < le && isspace((unsigned char)*ls)) ls++;
        while (le > ls && isspace((unsigned char)le[-1])) le--;
        if (ls != le) return -1;
        out->length = 0;
    }
    return 0;
}

static const char *positional_value_at(const struct cupid_shell *shell, long pos_index) {
    if (shell == NULL) return "";
    if (pos_index == 0) return shell->arg0 ? shell->arg0 : "";
    if (pos_index > 0 && (size_t)(pos_index - 1) < shell->params.count) {
        return shell->params.args[(size_t)(pos_index - 1)];
    }
    return "";
}

static char *join_special_params(const struct cupid_shell *shell, char sigil, char sep) {
    size_t pi;
    size_t total = 0;
    char *result;
    char *rp;

    if (shell == NULL) return strdup("");

    for (pi = 0; pi < shell->params.count; pi++) {
        if (pi > 0 && sep != '\0') total++;
        total += strlen(shell->params.args[pi]);
    }

    result = calloc(total + 1, 1);
    if (result == NULL) return NULL;
    rp = result;
    for (pi = 0; pi < shell->params.count; pi++) {
        size_t alen = strlen(shell->params.args[pi]);
        if (pi > 0 && sep != '\0') *rp++ = sep;
        memcpy(rp, shell->params.args[pi], alen);
        rp += alen;
    }
    (void)sigil;
    return result;
}

static void positional_slice_bounds(const struct cupid_shell *shell, const struct positional_slice_spec *slice,
                                    long *start_out, long *end_out) {
    long max_index = shell ? (long)shell->params.count : 0;
    long start;
    long end;

    if (slice->offset >= 0) start = slice->offset;
    else start = max_index + 1 + slice->offset;

    if (start < 0 || start > max_index + 1) {
        *start_out = 0;
        *end_out = 0;
        return;
    }

    if (slice->has_length) {
        end = start + slice->length;
        if (end < start) end = start;
    } else {
        end = max_index + 1;
    }
    if (end > max_index + 1) end = max_index + 1;
    *start_out = start;
    *end_out = end;
}

static char *expand_braced_result(const char *name, int colon_mode, char op, int op_long, const char *word, enum cupid_quote quote, struct cupid_shell *shell) {
    const char *cur = NULL;
    int is_set = 0;
    int set_non_null = (is_set && cur[0] != '\0');
    int use_existing = colon_mode ? set_non_null : is_set;
    size_t base_len = 0;
    const char *index_start = NULL;
    size_t index_len = 0;
    int is_array_ref = parse_array_ref_name(name, &base_len, &index_start, &index_len);
    int is_positional = is_all_digits(name);
    int index_valid = 0;
    size_t index_value = 0;
    char *array_name = NULL;
    char *generated = NULL;
    int whole_array_ref = 0;
    int is_special_star_at = (name != NULL && name[1] == '\0' &&
                              (name[0] == '*' || name[0] == '@'));

    if (is_special_star_at) {
        const char *ifs = cupid_vars_get(shell, "IFS");
        char sep = ' ';
        if (name[0] == '*' && ifs != NULL && ifs[0] != '\0') sep = ifs[0];
        if (name[0] == '*' && ifs != NULL && ifs[0] == '\0') sep = '\0';
        generated = join_special_params(shell, name[0], sep);
        if (generated == NULL) return NULL;
        cur = generated;
        is_set = 1;
    } else if (is_positional) {
        long idx = strtol(name, NULL, 10);
        if (idx == 0) {
            cur = shell->arg0 ? shell->arg0 : "";
            is_set = 1;
        } else if ((size_t)(idx - 1) < shell->params.count) {
            cur = shell->params.args[(size_t)(idx - 1)];
            is_set = 1;
        }
    } else if (is_array_ref) {
        array_name = calloc(base_len + 1, 1);
        if (array_name == NULL) return NULL;
        memcpy(array_name, name, base_len);
        whole_array_ref = (index_len == 1 && (index_start[0] == '@' || index_start[0] == '*'));
        if (whole_array_ref) {
            size_t acount = cupid_array_member_count(shell, array_name);
            index_valid = 1;
            if (acount > 0) {
                generated = join_array_members(shell, array_name, index_start[0] == '*');
                if (generated == NULL) {
                    free(array_name);
                    return NULL;
                }
                cur = generated;
                is_set = 1;
            } else {
                cur = cupid_vars_get(shell, array_name);
                is_set = (cur != NULL);
            }
        } else if (parse_nonneg_index(index_start, index_len, &index_value) == 0) {
            const char *scalar_fallback = NULL;
            index_valid = 1;
            if (cupid_array_has_index(shell, array_name, index_value)) {
                cur = cupid_array_get_index(shell, array_name, index_value);
                is_set = 1;
            } else if (index_value == 0 &&
                       (scalar_fallback = cupid_vars_get(shell, array_name)) != NULL) {
                cur = scalar_fallback;
                is_set = 1;
            }
        } else {
            char *key = calloc(index_len + 1, 1);
            if (key == NULL) {
                free(array_name);
                return NULL;
            }
            memcpy(key, index_start, index_len);
            index_valid = 1;
            if (cupid_array_has_key(shell, array_name, key)) {
                cur = cupid_array_get_key(shell, array_name, key);
                is_set = 1;
            } else if (!cupid_array_exists(shell, array_name) &&
                       (strcmp(key, "@") == 0 || strcmp(key, "*") == 0)) {
                cur = cupid_vars_get(shell, array_name);
                is_set = (cur != NULL);
            }
            free(key);
        }
    } else {
        cur = cupid_vars_get(shell, name);
        is_set = (cur != NULL);
    }

    set_non_null = (is_set && cur[0] != '\0');
    use_existing = colon_mode ? set_non_null : is_set;

    if (op == '\0') {
        char *ret;
        if (cur == NULL && shell->opt_nounset) {
            fprintf(stderr, "cupid: %s: unbound variable\n", name);
            if (!shell->is_interactive) {
                shell->should_exit = 1;
                shell->exit_code = 127;
            }
            free(generated);
            free(array_name);
            return NULL;
        }
        ret = strdup(cur == NULL ? "" : cur);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == 'L') {
        char buf[64];
        char *ret;
        int n;
        if (name != NULL && name[0] == '#' && name[1] == '\0') {
            char count_buf[32];
            snprintf(count_buf, sizeof(count_buf), "%zu", shell != NULL ? shell->params.count : 0u);
            n = snprintf(buf, sizeof(buf), "%zu", strlen(count_buf));
        } else if (name != NULL && name[0] == '?' && name[1] == '\0') {
            char status_buf[32];
            snprintf(status_buf, sizeof(status_buf), "%d", shell != NULL ? shell->last_status : 0);
            n = snprintf(buf, sizeof(buf), "%zu", strlen(status_buf));
        } else {
            n = snprintf(buf, sizeof(buf), "%zu", (size_t)((cur == NULL) ? 0 : strlen(cur)));
        }
        if (n < 0) {
            free(generated);
            free(array_name);
            return NULL;
        }
        ret = strdup(buf);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == '-') {
        char *ret;
        if (use_existing) ret = strdup(cur);
        else ret = expand_param_word_fragment(word == NULL ? "" : word, shell, NULL);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == '=') {
        char *expanded;
        char *ret = NULL;
        if (use_existing) {
            ret = strdup(cur);
            free(generated);
            free(array_name);
            return ret;
        }
        if (is_positional) {
            if (shell->is_dash_c) {
                if (set_expand_error(name, "cannot assign in this way") != 0) {
                    free(generated);
                    free(array_name);
                }
                return NULL;
            }
            fprintf(stderr, "cupid: $%s: cannot assign in this way\n", name);
            free(generated);
            free(array_name);
            return strdup("");
        }
        expanded = expand_param_word_fragment(word == NULL ? "" : word, shell, NULL);
        if (expanded == NULL) {
            free(generated);
            free(array_name);
            return NULL;
        }
        cupid_restore_escaped_ifs_placeholders(expanded);
        if (is_array_ref && index_valid && !whole_array_ref) {
            int assign_rc;
            if (parse_nonneg_index(index_start, index_len, &index_value) == 0) {
                assign_rc = cupid_array_set_index(shell, array_name, index_value, expanded);
            } else {
                char *key = calloc(index_len + 1, 1);
                if (key == NULL) {
                    free(expanded);
                    free(generated);
                    free(array_name);
                    return NULL;
                }
                memcpy(key, index_start, index_len);
                assign_rc = cupid_array_set_key(shell, array_name, key, expanded);
                free(key);
            }
            if (assign_rc != 0) {
                free(expanded);
                free(generated);
                free(array_name);
                return NULL;
            }
            if (parse_nonneg_index(index_start, index_len, &index_value) == 0) {
                const char *stored = cupid_array_get_index(shell, array_name, index_value);
                ret = strdup(stored == NULL ? "" : stored);
            } else {
                char *key = calloc(index_len + 1, 1);
                const char *stored;
                if (key == NULL) {
                    free(expanded);
                    free(generated);
                    free(array_name);
                    return NULL;
                }
                memcpy(key, index_start, index_len);
                stored = cupid_array_get_key(shell, array_name, key);
                ret = strdup(stored == NULL ? "" : stored);
                free(key);
            }
        } else if (cupid_vars_set(shell, name, expanded) != 0) {
            free(expanded);
            free(generated);
            free(array_name);
            return NULL;
        } else {
            const char *stored = cupid_vars_get(shell, name);
            ret = strdup(stored == NULL ? "" : stored);
        }
        if (ret == NULL) {
            free(expanded);
            free(generated);
            free(array_name);
            return NULL;
        }
        if (!is_array_ref) setenv(name, ret, 1);
        free(generated);
        free(array_name);
        free(expanded);
        return ret;
    }
    if (op == '+') {
        char *ret = use_existing
            ? expand_param_word_fragment(word == NULL ? "" : word, shell, NULL)
            : strdup("");
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == '#') {
        char *pat = expand_param_word_fragment(word == NULL ? "" : word, shell, NULL);
        char *ret;
        if (pat == NULL) {
            free(generated);
            free(array_name);
            return NULL;
        }
        ret = remove_prefix_pattern(cur == NULL ? "" : cur, pat, op_long);
        free(pat);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == '%') {
        char *pat = expand_param_word_fragment(word == NULL ? "" : word, shell, NULL);
        char *ret;
        if (pat == NULL) {
            free(generated);
            free(array_name);
            return NULL;
        }
        ret = remove_suffix_pattern(cur == NULL ? "" : cur, pat, op_long);
        free(pat);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == '?') {
        if (use_existing) {
            char *ret = strdup(cur);
            free(generated);
            free(array_name);
            return ret;
        }
        if (set_expand_error(name, (word != NULL && word[0] != '\0') ? word : "parameter null or not set") != 0) {
            free(generated);
            free(array_name);
            return NULL;
        }
        if (!shell->is_interactive) {
            shell->should_exit = 1;
            shell->exit_code = shell->is_dash_c ? 127 : 1;
        }
        free(generated);
        free(array_name);
        return NULL;
    }
    if (op == 'N') {
        long idx = strtol(name, NULL, 10);
        char *ret;
        if (idx == 0) ret = strdup(shell->arg0 ? shell->arg0 : "");
        else if ((size_t)(idx - 1) < shell->params.count) ret = strdup(shell->params.args[(size_t)(idx - 1)]);
        else ret = strdup("");
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == 'C') {
        char cbuf[32];
        char *ret;
        snprintf(cbuf, sizeof(cbuf), "%zu", shell->params.count);
        ret = strdup(cbuf);
        free(generated);
        free(array_name);
        return ret;
    }
    if (op == 'A') {
        char *result = strdup(cur == NULL ? "" : cur);
        if (result == NULL) {
            free(generated);
            free(array_name);
            return NULL;
        }
        free(generated);
        free(array_name);
        return result;
    }
    if (op == 'Q') {
        struct positional_slice_spec slice = {0};
        long start;
        long end;
        long i;
        char sep = ' ';
        char *result = NULL;
        size_t rlen = 0;
        size_t rcap = 0;
        if (parse_positional_slice_spec(word == NULL ? "" : word, &slice) != 0) {
            (void)cupid_expand_error_set("bad substitution");
            free(array_name);
            return NULL;
        }
        if (slice.has_length && slice.length < 0) {
            char msg[64];
            snprintf(msg, sizeof(msg), "%ld: substring expression < 0", slice.length);
            (void)cupid_expand_error_set(msg);
            free(array_name);
            return NULL;
        }
        if (name[0] == '*' && quote == CUPID_QUOTE_DOUBLE) {
            const char *ifs = cupid_vars_get(shell, "IFS");
            if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
            if (ifs != NULL && ifs[0] == '\0') sep = '\0';
        }
        positional_slice_bounds(shell, &slice, &start, &end);
        for (i = start; i < end; i++) {
            const char *it = positional_value_at(shell, i);
            if (i > start && sep != '\0') {
                if (append_bytes(&result, &rlen, &rcap, &sep, 1) != 0) {
                    free(result);
                    free(array_name);
                    return NULL;
                }
            }
            if (append_cstr(&result, &rlen, &rcap, it) != 0) {
                free(result);
                free(array_name);
                return NULL;
            }
        }
        if (result == NULL) result = strdup("");
        free(array_name);
        return result;
    }
    if (op == 'I') {
        const char *indirect_name = cur;
        const char *val;
        if (indirect_name == NULL || indirect_name[0] == '\0') {
            free(array_name);
            return strdup("");
        }
        val = cupid_vars_get(shell, indirect_name);
        {
            char *ret = strdup(val ? val : "");
            free(array_name);
            return ret;
        }
    }
    if (op == 'P') {
        size_t pi, prefix_len = strlen(name);
        char *result = NULL;
        size_t rlen = 0, rcap = 0;
        for (pi = 0; pi < shell->vars.count; pi++) {
            if (strncmp(shell->vars.entries[pi].name, name, prefix_len) == 0) {
                if (rlen > 0) {
                    if (append_bytes(&result, &rlen, &rcap, " ", 1) != 0) {
                        free(result);
                        free(array_name);
                        return NULL;
                    }
                }
                if (append_cstr(&result, &rlen, &rcap, shell->vars.entries[pi].name) != 0) {
                    free(result);
                    free(array_name);
                    return NULL;
                }
            }
        }
        if (result == NULL) {
            free(array_name);
            return strdup("");
        }
        free(array_name);
        return result;
    }
    if (op == '^' || op == ',') {
        const char *src = (cur == NULL) ? "" : cur;
        size_t slen = strlen(src);
        char *result = strdup(src);
        if (result == NULL) {
            free(array_name);
            return NULL;
        }
        if (slen == 0) {
            free(array_name);
            return result;
        }
        if (op == '^') {
            if (op_long) {
                size_t ci;
                for (ci = 0; ci < slen; ci++)
                    result[ci] = (char)toupper((unsigned char)result[ci]);
            } else {
                result[0] = (char)toupper((unsigned char)result[0]);
            }
        } else {
            if (op_long) {
                size_t ci;
                for (ci = 0; ci < slen; ci++)
                    result[ci] = (char)tolower((unsigned char)result[ci]);
            } else {
                result[0] = (char)tolower((unsigned char)result[0]);
            }
        }
        free(array_name);
        return result;
    }
    if (op == 'S') {
        const char *src = (cur == NULL) ? "" : cur;
        size_t slen = strlen(src);
        long offset = 0, length = -1;
        size_t start, end;
        char *result;
        if (word != NULL) {
            char *colon = strchr(word, ':');
            if (colon != NULL) {
                *colon = '\0';
                offset = strtol(word, NULL, 10);
                length = strtol(colon + 1, NULL, 10);
                *colon = ':';
            } else {
                offset = strtol(word, NULL, 10);
            }
        }
        if (offset < 0) {
            offset = (long)slen + offset;
            if (offset < 0) offset = 0;
        }
        start = (size_t)offset;
        if (start > slen) start = slen;
        if (length < 0) {
            end = slen;
        } else {
            end = start + (size_t)length;
            if (end > slen) end = slen;
        }
        result = calloc(end - start + 1, 1);
        if (result == NULL) {
            free(array_name);
            return NULL;
        }
        if (end > start) memcpy(result, src + start, end - start);
        free(array_name);
        return result;
    }
    if (op == 'R') {
        const char *src = (cur == NULL) ? "" : cur;
        size_t pat_len = (size_t)colon_mode;
        const char *pat_raw = (word != NULL) ? word : "";
        const char *rep_raw = (word != NULL) ? word + pat_len + 1 : "";
        char *pat = NULL;
        char *rep = NULL;
        size_t src_len = strlen(src);
        size_t rep_len;
        char *result = NULL;
        size_t rlen = 0, rcap = 0;

        if (pat_len == 0 || pat_raw[0] == '\0') {
            free(array_name);
            return strdup(src);
        }

        {
            char *pat_buf = calloc(pat_len + 1, 1);
            if (pat_buf == NULL) {
                free(array_name);
                return NULL;
            }
            memcpy(pat_buf, pat_raw, pat_len);
            pat = expand_param_subst_fragment(pat_buf, shell);
            free(pat_buf);
        }
        rep = expand_param_subst_fragment(rep_raw, shell);
        if (pat == NULL || rep == NULL) {
            free(pat);
            free(rep);
            free(array_name);
            return NULL;
        }
        rep_len = strlen(rep);
        if (pat[0] == '\0') {
            free(pat);
            free(rep);
            free(array_name);
            return strdup(src);
        }

        if (op_long == 2) {
            /* ${var/#pat/rep} - prefix */
            char *pref = dup_slice(src, 0, pat_len > src_len ? src_len : src_len);
            (void)pref;
            free(pref);
            {
                size_t i;
                for (i = 0; i <= src_len; i++) {
                    char *sl = dup_slice(src, 0, i);
                    if (sl && cupid_fnmatch(pat, sl) == 0) {
                        free(sl);
                        if (append_bytes(&result, &rlen, &rcap, rep, rep_len) != 0) {
                            free(result);
                            free(pat);
                            free(rep);
                            free(array_name);
                            return NULL;
                        }
                        if (append_bytes(&result, &rlen, &rcap, src + i, src_len - i) != 0) {
                            free(result);
                            free(pat);
                            free(rep);
                            free(array_name);
                            return NULL;
                        }
                        free(pat);
                        free(rep);
                        free(array_name);
                        return result ? result : strdup("");
                    }
                    free(sl);
                }
            }
            free(pat);
            free(rep);
            free(array_name);
            return strdup(src);
        }
        if (op_long == 3) {
            /* ${var/%pat/rep} - suffix */
            size_t i;
            for (i = src_len + 1; i > 0; i--) {
                size_t cut = i - 1;
                const char *suf = src + cut;
                if (cupid_fnmatch(pat, suf) == 0) {
                    if (append_bytes(&result, &rlen, &rcap, src, cut) != 0) {
                        free(result);
                        free(pat);
                        free(rep);
                        free(array_name);
                        return NULL;
                    }
                    if (append_bytes(&result, &rlen, &rcap, rep, rep_len) != 0) {
                        free(result);
                        free(pat);
                        free(rep);
                        free(array_name);
                        return NULL;
                    }
                    free(pat);
                    free(rep);
                    free(array_name);
                    return result ? result : strdup("");
                }
            }
            free(pat);
            free(rep);
            free(array_name);
            return strdup(src);
        }

        {
            /* first or all match replacement using fnmatch character-by-character */
            size_t i = 0;
            int replaced = 0;
            while (i < src_len) {
                if (!replaced || op_long == 1) {
                    int found = 0;
                    size_t mend;
                    for (mend = i + 1; mend <= src_len; mend++) {
                        char *sub = dup_slice(src, i, mend);
                        if (sub && cupid_fnmatch(pat, sub) == 0) {
                            free(sub);
                            if (append_bytes(&result, &rlen, &rcap, rep, rep_len) != 0) {
                                free(result);
                                free(pat);
                                free(rep);
                                free(array_name);
                                return NULL;
                            }
                            i = mend;
                            found = 1;
                            replaced = 1;
                            break;
                        }
                        free(sub);
                    }
                    if (found) continue;
                }
                if (append_bytes(&result, &rlen, &rcap, src + i, 1) != 0) {
                    free(result);
                    free(pat);
                    free(rep);
                    free(array_name);
                    return NULL;
                }
                i++;
            }
        }
        if (result == NULL) {
            free(pat);
            free(rep);
            free(array_name);
            return strdup("");
        }
        free(pat);
        free(rep);
        free(array_name);
        return result;
    }
    free(array_name);
    return NULL;
}

static int append_arith_subst_raw(const char **pp, char **buf, size_t *len, size_t *cap) {
    const char *p = *pp;
    int depth = 0;
    if (p[0] != '$' || p[1] != '(' || p[2] != '(') return -1;
    if (append_bytes(buf, len, cap, "$(", 2) != 0) return -1;
    if (append_bytes(buf, len, cap, "(", 1) != 0) return -1;
    p += 3;
    while (*p != '\0') {
        if (*p == '(') {
            if (append_bytes(buf, len, cap, p, 1) != 0) return -1;
            depth++;
            p++;
            continue;
        }
        if (*p == ')') {
            if (depth == 0 && p[1] == ')') {
                if (append_bytes(buf, len, cap, "))", 2) != 0) return -1;
                p += 2;
                *pp = p;
                return 0;
            }
            if (depth > 0) depth--;
            if (append_bytes(buf, len, cap, p, 1) != 0) return -1;
            p++;
            continue;
        }
        if (append_bytes(buf, len, cap, p, 1) != 0) return -1;
        p++;
    }
    return -1;
}

static int extract_command_subst(const char **pp, char **out_cmd, size_t *out_len,
                                 struct cupid_shell *shell) {
    const char *p = *pp;
    char *buf = NULL;
    size_t len = 0;
    size_t cap = 0;
    int depth = 1;
    int extglob_depth = 0;
    int mode = 0;

    if (p[0] != '$' || p[1] != '(') {
        return -1;
    }
    p += 2;
    while (*p != '\0') {
        if (mode == 1) {
            if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
                free(buf);
                return -1;
            }
            if (*p == '\'') {
                mode = 0;
            }
            p++;
            continue;
        }
        if (mode == 2) {
            if (p[0] == '$' && p[1] == '(' && p[2] == '(') {
                if (append_arith_subst_raw(&p, &buf, &len, &cap) != 0) {
                    free(buf);
                    return -1;
                }
                continue;
            }
            if (p[0] == '$' && p[1] == '(') {
                if (append_bytes(&buf, &len, &cap, "$(", 2) != 0) {
                    free(buf);
                    return -1;
                }
                depth++;
                p += 2;
                continue;
            }
            if (*p == '\\' && p[1] != '\0') {
                if (append_bytes(&buf, &len, &cap, p, 2) != 0) {
                    free(buf);
                    return -1;
                }
                p += 2;
                continue;
            }
            if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
                free(buf);
                return -1;
            }
            if (*p == '"') {
                mode = 0;
            }
            p++;
            continue;
        }

        if (p[0] == '$' && p[1] == '(' && p[2] == '(') {
            if (append_arith_subst_raw(&p, &buf, &len, &cap) != 0) {
                free(buf);
                return -1;
            }
            continue;
        }
        if (shell != NULL && shell->opt_extglob) {
            if (extglob_depth > 0) {
                if (*p == '(') {
                    extglob_depth++;
                } else if (*p == ')') {
                    extglob_depth--;
                    if (depth > 0) {
                        /* Extglob parens were counted in depth at open; keep them balanced. */
                        depth--;
                    }
                    if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
                        free(buf);
                        return -1;
                    }
                    p++;
                    continue;
                }
            } else if (*p == '(' && p > (*pp + 2) && strchr("?*+@!", p[-1]) != NULL) {
                extglob_depth = 1;
            }
        }
        if (p[0] == '$' && p[1] == '(') {
            if (append_bytes(&buf, &len, &cap, "$(", 2) != 0) {
                free(buf);
                return -1;
            }
            depth++;
            p += 2;
            continue;
        }
        if (*p == '(') {
            if (append_bytes(&buf, &len, &cap, "(", 1) != 0) {
                free(buf);
                return -1;
            }
            depth++;
            p++;
            continue;
        }
        if (*p == ')') {
            depth--;
            p++;
            if (depth == 0) {
                if (buf == NULL) {
                    buf = calloc(1, 1);
                    if (buf == NULL) {
                        return -1;
                    }
                }
                *out_cmd = buf;
                *out_len = len;
                *pp = p;
                return 0;
            }
            if (append_bytes(&buf, &len, &cap, ")", 1) != 0) {
                free(buf);
                return -1;
            }
            continue;
        }
        if (*p == '\'') {
            if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
                free(buf);
                return -1;
            }
            mode = 1;
            p++;
            continue;
        }
        if (*p == '"') {
            if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
                free(buf);
                return -1;
            }
            mode = 2;
            p++;
            continue;
        }
        if (*p == '\\' && p[1] != '\0') {
            if (append_bytes(&buf, &len, &cap, p, 2) != 0) {
                free(buf);
                return -1;
            }
            p += 2;
            continue;
        }
        if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
            free(buf);
            return -1;
        }
        p++;
    }
    free(buf);
    return -1;
}

static int extract_backtick_subst(const char **pp, char **out_cmd) {
    const char *p = *pp;
    char *buf = NULL;
    size_t len = 0;
    size_t cap = 0;

    if (*p != '`') return -1;
    p++;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            if (p[1] == '\n') {
                p += 2;
                continue;
            }
            if (p[1] == '$' || p[1] == '`' || p[1] == '\\') {
                if (append_bytes(&buf, &len, &cap, p + 1, 1) != 0) {
                    free(buf);
                    return -1;
                }
                p += 2;
                continue;
            }
            if (append_bytes(&buf, &len, &cap, p, 2) != 0) {
                free(buf);
                return -1;
            }
            p += 2;
            continue;
        }
        if (*p == '`') {
            *out_cmd = (buf != NULL) ? buf : strdup("");
            if (*out_cmd == NULL) {
                free(buf);
                return -1;
            }
            *pp = p + 1;
            return 0;
        }
        if (append_bytes(&buf, &len, &cap, p, 1) != 0) {
            free(buf);
            return -1;
        }
        p++;
    }
    free(buf);
    return -1;
}

static char *run_command_subst(const char *script, struct cupid_shell *shell) {
    int fds[2];
    pid_t pid;
    char *out = NULL;
    size_t out_len = 0;
    size_t out_cap = 0;
    int st = 0;
    int saw_nul = 0;

    if (pipe(fds) != 0) {
        return NULL;
    }

    pid = fork();
    if (pid < 0) {
        close(fds[0]);
        close(fds[1]);
        return NULL;
    }

    if (pid == 0) {
        int rc;
        if (dup2(fds[1], STDOUT_FILENO) < 0) {
            _exit(1);
        }
        close(fds[0]);
        close(fds[1]);
        rc = cupid_shell_eval_line(shell, script, 1);
        if (shell->should_exit) {
            rc = shell->exit_code;
        }
        fflush(NULL);
        _exit(rc & 0xff);
    }

    close(fds[1]);
    while (1) {
        char chunk[256];
        ssize_t n = read(fds[0], chunk, sizeof(chunk));
        if (n < 0) {
            close(fds[0]);
            free(out);
            return NULL;
        }
        if (n == 0) {
            break;
        }
        {
            size_t i;
            size_t start = 0;
            for (i = 0; i < (size_t)n; i++) {
                if (chunk[i] == '\0') {
                    saw_nul = 1;
                    if (i > start &&
                        append_bytes(&out, &out_len, &out_cap, chunk + start, i - start) != 0) {
                        close(fds[0]);
                        free(out);
                        return NULL;
                    }
                    start = i + 1;
                }
            }
            if ((size_t)n > start &&
                append_bytes(&out, &out_len, &out_cap, chunk + start, (size_t)n - start) != 0) {
                close(fds[0]);
                free(out);
                return NULL;
            }
        }
    }
    close(fds[0]);
    (void)waitpid(pid, &st, 0);
    if (saw_nul) {
        fprintf(stderr, "%s: warning: command substitution: ignored null byte in input\n",
                (shell != NULL && shell->arg0 != NULL) ? shell->arg0 : "cupid");
    }
    if (shell != NULL) {
        shell->expand_cmdsub_seen = 1;
        shell->expand_cmdsub_status = (WIFEXITED(st) ? WEXITSTATUS(st)
                                                     : (WIFSIGNALED(st) ? (128 + WTERMSIG(st)) : 1));
    }

    if (out == NULL) {
        out = calloc(1, 1);
        if (out == NULL) {
            return NULL;
        }
        return out;
    }
    while (out_len > 0 && out[out_len - 1] == '\n') {
        out_len--;
        out[out_len] = '\0';
    }
    return out;
}

static int extract_arith_expr(const char **pp, char **out_expr) {
    const char *p = *pp;
    const char *start;
    int depth;
    size_t expr_len;
    char *expr;

    if (p[0] != '$' || p[1] != '(' || p[2] != '(') return -1;
    p += 3;
    start = p;
    depth = 0;

    while (*p != '\0') {
        if (*p == '(') {
            depth++;
            p++;
        } else if (*p == ')') {
            if (depth == 0 && p[1] == ')') {
                expr_len = (size_t)(p - start);
                expr = calloc(expr_len + 1, 1);
                if (expr == NULL) return -1;
                if (expr_len > 0) memcpy(expr, start, expr_len);
                *out_expr = expr;
                *pp = p + 2;
                return 0;
            }
            if (depth > 0) depth--;
            p++;
        } else {
            p++;
        }
    }
    return -1;
}

static int extract_oldstyle_arith_expr(const char **pp, char **out_expr) {
    const char *p = *pp;
    const char *start;
    size_t expr_len;
    char *expr;

    if (p[0] != '$' || p[1] != '[') return -1;
    p += 2;
    start = p;
    while (*p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (*p == ']') {
            expr_len = (size_t)(p - start);
            expr = calloc(expr_len + 1, 1);
            if (expr == NULL) return -1;
            if (expr_len > 0) memcpy(expr, start, expr_len);
            *out_expr = expr;
            *pp = p + 1;
            return 0;
        }
        p++;
    }
    return -1;
}

static int append_bytes(char **buf, size_t *len, size_t *cap, const char *data, size_t data_len) {
    char *next;
    size_t needed;
    if (data_len == 0) {
        return 0;
    }
    needed = *len + data_len + 1;
    if (needed > *cap) {
        size_t next_cap = (*cap == 0) ? 32 : *cap;
        while (next_cap < needed) {
            next_cap *= 2;
        }
        next = realloc(*buf, next_cap);
        if (next == NULL) {
            return -1;
        }
        *buf = next;
        *cap = next_cap;
    }
    memcpy(*buf + *len, data, data_len);
    *len += data_len;
    (*buf)[*len] = '\0';
    return 0;
}

static int append_cstr(char **buf, size_t *len, size_t *cap, const char *data) {
    return append_bytes(buf, len, cap, data, strlen(data));
}

static char *duplicate_backslashes(const char *src) {
    size_t len = 0;
    size_t extra = 0;
    char *out;
    size_t i;

    if (src == NULL) return NULL;
    len = strlen(src);
    for (i = 0; i < len; i++) {
        if (src[i] == '\\') extra++;
    }
    out = calloc(len + extra + 1, 1);
    if (out == NULL) return NULL;
    for (i = 0, extra = 0; i < len; i++) {
        if (src[i] == '\\') out[extra++] = '\\';
        out[extra++] = src[i];
    }
    out[extra] = '\0';
    return out;
}

static char *format_declare_quoted_string(const char *value) {
    const unsigned char *p = (const unsigned char *)(value ? value : "");
    int needs_ansi_c = 0;
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;

    while (*p != '\0') {
        if (*p < 32 || *p == 127 || *p >= 128) {
            needs_ansi_c = 1;
            break;
        }
        p++;
    }
    p = (const unsigned char *)(value ? value : "");

    if (needs_ansi_c) {
        if (append_cstr(&out, &len, &cap, "$'") != 0) {
            free(out);
            return NULL;
        }
        while (*p != '\0') {
            char oct[8];
            if (*p == '\\' || *p == '\'') {
                if (append_bytes(&out, &len, &cap, "\\", 1) != 0 ||
                    append_bytes(&out, &len, &cap, (const char *)p, 1) != 0) {
                    free(out);
                    return NULL;
                }
            } else if (*p == '\n') {
                if (append_cstr(&out, &len, &cap, "\\n") != 0) {
                    free(out);
                    return NULL;
                }
            } else if (*p == '\t') {
                if (append_cstr(&out, &len, &cap, "\\t") != 0) {
                    free(out);
                    return NULL;
                }
            } else if (*p < 32 || *p == 127 || *p >= 128) {
                int n = snprintf(oct, sizeof(oct), "\\%03o", (unsigned int)*p);
                if (n < 0 || append_bytes(&out, &len, &cap, oct, (size_t)n) != 0) {
                    free(out);
                    return NULL;
                }
            } else if (append_bytes(&out, &len, &cap, (const char *)p, 1) != 0) {
                free(out);
                return NULL;
            }
            p++;
        }
        if (append_bytes(&out, &len, &cap, "'", 1) != 0) {
            free(out);
            return NULL;
        }
        return out;
    }

    if (append_bytes(&out, &len, &cap, "\"", 1) != 0) {
        free(out);
        return NULL;
    }
    while (*p != '\0') {
        if (*p == '\\' || *p == '"' || *p == '$' || *p == '`') {
            if (append_bytes(&out, &len, &cap, "\\", 1) != 0) {
                free(out);
                return NULL;
            }
        }
        if (append_bytes(&out, &len, &cap, (const char *)p, 1) != 0) {
            free(out);
            return NULL;
        }
        p++;
    }
    if (append_bytes(&out, &len, &cap, "\"", 1) != 0) {
        free(out);
        return NULL;
    }
    return out;
}

static char *format_scalar_assignment_string(const char *name, const char *value) {
    char *quoted;
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;

    quoted = format_declare_quoted_string(value);
    if (quoted == NULL) return NULL;
    if (append_cstr(&out, &len, &cap, name == NULL ? "" : name) != 0 ||
        append_bytes(&out, &len, &cap, "=", 1) != 0 ||
        append_cstr(&out, &len, &cap, quoted) != 0) {
        free(quoted);
        free(out);
        return NULL;
    }
    free(quoted);
    return out;
}

static char *format_array_assignment_string(struct cupid_shell *shell, const char *name) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t count;
    size_t i;
    int associative;

    if (shell == NULL || name == NULL) return NULL;
    associative = cupid_array_is_associative(shell, name);
    count = cupid_array_member_count(shell, name);

    if (append_cstr(&out, &len, &cap, associative ? "declare -A " : "declare -a ") != 0 ||
        append_cstr(&out, &len, &cap, name) != 0 ||
        append_cstr(&out, &len, &cap, "=(") != 0) {
        free(out);
        return NULL;
    }

    for (i = 0; i < count; i++) {
        const char *key = cupid_array_member_key(shell, name, i);
        const char *value = cupid_array_member_value(shell, name, i);
        char *quoted_value = format_declare_quoted_string(value);
        if (quoted_value == NULL) {
            free(out);
            return NULL;
        }
        if (associative) {
            char *quoted_key = format_declare_quoted_string(key);
            if (quoted_key == NULL ||
                append_bytes(&out, &len, &cap, "[", 1) != 0 ||
                append_cstr(&out, &len, &cap, quoted_key) != 0 ||
                append_cstr(&out, &len, &cap, "]=") != 0 ||
                append_cstr(&out, &len, &cap, quoted_value) != 0 ||
                append_bytes(&out, &len, &cap, " ", 1) != 0) {
                free(quoted_key);
                free(quoted_value);
                free(out);
                return NULL;
            }
            free(quoted_key);
        } else {
            if (i > 0 && append_bytes(&out, &len, &cap, " ", 1) != 0) {
                free(quoted_value);
                free(out);
                return NULL;
            }
            if (append_bytes(&out, &len, &cap, "[", 1) != 0 ||
                append_cstr(&out, &len, &cap, key) != 0 ||
                append_cstr(&out, &len, &cap, "]=") != 0 ||
                append_cstr(&out, &len, &cap, quoted_value) != 0) {
                free(quoted_value);
                free(out);
                return NULL;
            }
        }
        free(quoted_value);
    }

    if (append_bytes(&out, &len, &cap, ")", 1) != 0) {
        free(out);
        return NULL;
    }
    return out;
}

static int fnmatch_meta_char(char ch, int extglob) {
    if (ch == '\\' || ch == '*' || ch == '?' || ch == '[' || ch == ']') return 1;
    if (extglob && (ch == '(' || ch == ')' || ch == '|' || ch == '!' || ch == '+' || ch == '@')) return 1;
    return 0;
}

static char *escape_fnmatch_literal(const char *src, int extglob) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    const char *p = src;

    if (src == NULL) return NULL;
    while (*p != '\0') {
        if (fnmatch_meta_char(*p, extglob)) {
            if (append_bytes(&out, &len, &cap, "\\", 1) != 0) {
                free(out);
                return NULL;
            }
        }
        if (append_bytes(&out, &len, &cap, p, 1) != 0) {
            free(out);
            return NULL;
        }
        p++;
    }
    if (out == NULL) out = calloc(1, 1);
    return out;
}

static char *decode_ansi_c_text(const char *src) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    const char *p = src;

    while (p != NULL && *p != '\0') {
        if (*p == '\\' && p[1] != '\0') {
            char ch;
            const char *q = p + 1;
            if (*q >= '0' && *q <= '7') {
                unsigned int value = 0;
                int digits = 0;
                while (digits < 3 && *q >= '0' && *q <= '7') {
                    value = value * 8u + (unsigned int)(*q - '0');
                    digits++;
                    q++;
                }
                ch = (char)(value & 0xff);
                p = q;
            } else {
                switch (*q) {
                    case 'a': ch = '\a'; break;
                    case 'b': ch = '\b'; break;
                    case 'e':
                    case 'E': ch = 27; break;
                    case 'f': ch = '\f'; break;
                    case 'n': ch = '\n'; break;
                    case 'r': ch = '\r'; break;
                    case 't': ch = '\t'; break;
                    case 'v': ch = '\v'; break;
                    case '\\': ch = '\\'; break;
                    case '\'': ch = '\''; break;
                    case '"': ch = '"'; break;
                    default: ch = *q; break;
                }
                p += 2;
            }
            if (append_bytes(&out, &len, &cap, &ch, 1) != 0) {
                free(out);
                return NULL;
            }
            continue;
        }
        if (append_bytes(&out, &len, &cap, p, 1) != 0) {
            free(out);
            return NULL;
        }
        p++;
    }

    if (out == NULL) out = calloc(1, 1);
    return out;
}

char *cupid_expand_text(const char *src, enum cupid_quote quote, struct cupid_shell *shell) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    const char *p;

    if (src == NULL) {
        return NULL;
    }
    if (quote == CUPID_QUOTE_SINGLE) {
        char *copy = strdup(src);
        return copy;
    }
    if (quote == CUPID_QUOTE_ANSI_C) {
        return decode_ansi_c_text(src);
    }

    p = src;
    while (*p != '\0') {
        if (quote == CUPID_QUOTE_DOUBLE && *p == CUPID_ESC_BACKTICK_PLACEHOLDER) {
            if (append_bytes(&out, &len, &cap, "`", 1) != 0) {
                free(out);
                return NULL;
            }
            p++;
            continue;
        }
        if (quote == CUPID_QUOTE_NONE && *p == '\\') {
            if (p[1] == '\n') {
                p += 2;
                continue;
            }
            if (p[1] != '\0') {
                char placeholder = escaped_ifs_placeholder(p[1]);
                const char *append_src = p + 1;
                if (placeholder != '\0') append_src = &placeholder;
                if (append_bytes(&out, &len, &cap, append_src, 1) != 0) {
                    free(out);
                    return NULL;
                }
                p += 2;
                continue;
            }
        }
        if (*p == '`') {
            char *cmd = NULL;
            char *sub_out;
            if (extract_backtick_subst(&p, &cmd) != 0) {
                free(out);
                return NULL;
            }
            sub_out = run_command_subst(cmd, shell);
            free(cmd);
            if (sub_out == NULL) {
                free(out);
                return NULL;
            }
            if (append_cstr(&out, &len, &cap, sub_out) != 0) {
                free(sub_out);
                free(out);
                return NULL;
            }
            free(sub_out);
            continue;
        }
        if (*p == '$') {
            if (p[1] == '$') {
                char pid_buf[64];
                long pid = (shell != NULL && shell->shell_pid > 0)
                    ? (long)shell->shell_pid
                    : (long)getpid();
                int n = snprintf(pid_buf, sizeof(pid_buf), "%ld", pid);
                if (n < 0 || append_bytes(&out, &len, &cap, pid_buf, (size_t)n) != 0) {
                    free(out);
                    return NULL;
                }
                p += 2;
                continue;
            }
            if (p[1] == '(' && p[2] == '(') {
                char *arith_expr = NULL;
                const char *save_p = p;
                if (extract_arith_expr(&p, &arith_expr) == 0) {
                    int arith_err = 0;
                    long arith_val = cupid_arith_eval(shell, arith_expr, &arith_err);
                    free(arith_expr);
                    if (arith_err) {
                        (void)cupid_expand_error_set("arithmetic expansion error");
                        free(out);
                        return NULL;
                    }
                    {
                        char arith_buf[32];
                        int an = snprintf(arith_buf, sizeof(arith_buf), "%ld", arith_val);
                        if (an < 0 || append_bytes(&out, &len, &cap, arith_buf, (size_t)an) != 0) {
                            free(out);
                            return NULL;
                        }
                    }
                    continue;
                }
                p = save_p;
            }
            if (p[1] == '[') {
                char *arith_expr = NULL;
                const char *save_p = p;
                if (extract_oldstyle_arith_expr(&p, &arith_expr) == 0) {
                    int arith_err = 0;
                    long arith_val = cupid_arith_eval(shell, arith_expr, &arith_err);
                    free(arith_expr);
                    if (arith_err) {
                        (void)cupid_expand_error_set("arithmetic expansion error");
                        free(out);
                        return NULL;
                    }
                    {
                        char arith_buf[32];
                        int an = snprintf(arith_buf, sizeof(arith_buf), "%ld", arith_val);
                        if (an < 0 || append_bytes(&out, &len, &cap, arith_buf, (size_t)an) != 0) {
                            free(out);
                            return NULL;
                        }
                    }
                    continue;
                }
                p = save_p;
            }
            if (p[1] == '(') {
                char *cmd = NULL;
                size_t cmd_len = 0;
                char *sub_out;
                if (extract_command_subst(&p, &cmd, &cmd_len, shell) != 0) {
                    free(out);
                    return NULL;
                }
                (void)cmd_len;
                sub_out = run_command_subst(cmd, shell);
                free(cmd);
                if (sub_out == NULL) {
                    free(out);
                    return NULL;
                }
                if (append_cstr(&out, &len, &cap, sub_out) != 0) {
                    free(sub_out);
                    free(out);
                    return NULL;
                }
                free(sub_out);
                continue;
            }
            if (p[1] == '{') {
                const char *q = p + 2;
                if ((q[0] == '*' || q[0] == '@') &&
                    q[1] == '@' && q[2] == 'E' && q[3] == '}') {
                    const char *ifs = cupid_vars_get(shell, "IFS");
                    char sep = ' ';
                    char *joined;
                    if (q[0] == '*' && ifs != NULL && ifs[0] != '\0') sep = ifs[0];
                    if (q[0] == '*' && ifs != NULL && ifs[0] == '\0') sep = '\0';
                    joined = join_special_params(shell, q[0], sep);
                    if (joined == NULL) {
                        free(out);
                        return NULL;
                    }
                    if (append_cstr(&out, &len, &cap, joined) != 0) {
                        free(joined);
                        free(out);
                        return NULL;
                    }
                    free(joined);
                    p = q + 4;
                    continue;
                }
                if (q[0] == '#' && (isalpha((unsigned char)q[1]) || q[1] == '_')) {
                    const char *name_start = q + 1;
                    const char *name_end = name_start;
                    while (isalnum((unsigned char)*name_end) || *name_end == '_') name_end++;
                    if (name_end[0] == '[' &&
                        (name_end[1] == '@' || name_end[1] == '*') &&
                        name_end[2] == ']' && name_end[3] == '}') {
                        size_t nlen = (size_t)(name_end - name_start);
                        char *name = calloc(nlen + 1, 1);
                        char lenbuf[32];
                        int n;
                        if (name == NULL) {
                            free(out);
                            return NULL;
                        }
                        memcpy(name, name_start, nlen);
                        if (cupid_array_exists(shell, name)) {
                            n = snprintf(lenbuf, sizeof(lenbuf), "%zu",
                                         cupid_array_member_count(shell, name));
                        } else {
                            n = snprintf(lenbuf, sizeof(lenbuf), "%d",
                                         cupid_vars_get(shell, name) != NULL ? 1 : 0);
                        }
                        free(name);
                        if (n < 0 || append_bytes(&out, &len, &cap, lenbuf, (size_t)n) != 0) {
                            free(out);
                            return NULL;
                        }
                        p = name_end + 4;
                        continue;
                    }
                }
                if (isalpha((unsigned char)q[0]) || q[0] == '_') {
                    const char *name_start = q;
                    const char *name_end = name_start;
                    while (isalnum((unsigned char)*name_end) || *name_end == '_') name_end++;
                    if (name_end[0] == '@' &&
                        (name_end[1] == 'Q' || name_end[1] == 'P' || name_end[1] == 'A') &&
                        name_end[2] == '}') {
                        size_t nlen = (size_t)(name_end - name_start);
                        char *name = calloc(nlen + 1, 1);
                        char *rendered = NULL;
                        const char *val;
                        if (name == NULL) {
                            free(out);
                            return NULL;
                        }
                        memcpy(name, name_start, nlen);
                        val = cupid_vars_get(shell, name);
                        if (name_end[1] == 'Q') {
                            rendered = format_declare_quoted_string(val == NULL ? "" : val);
                        } else if (name_end[1] == 'P') {
                            rendered = strdup(val == NULL ? "" : val);
                        } else {
                            if (cupid_array_exists(shell, name)) {
                                rendered = format_array_assignment_string(shell, name);
                            } else {
                                rendered = format_scalar_assignment_string(name, val == NULL ? "" : val);
                            }
                        }
                        free(name);
                        if (rendered == NULL) {
                            free(out);
                            return NULL;
                        }
                        if (append_cstr(&out, &len, &cap, rendered) != 0) {
                            free(rendered);
                            free(out);
                            return NULL;
                        }
                        free(rendered);
                        p = name_end + 3;
                        continue;
                    }
                    if (name_end[0] == '@' && name_end[1] == 'E' && name_end[2] == '}') {
                        size_t nlen = (size_t)(name_end - name_start);
                        char *name = calloc(nlen + 1, 1);
                        const char *val;
                        if (name == NULL) {
                            free(out);
                            return NULL;
                        }
                        memcpy(name, name_start, nlen);
                        val = cupid_vars_get(shell, name);
                        free(name);
                        if (val != NULL && append_cstr(&out, &len, &cap, val) != 0) {
                            free(out);
                            return NULL;
                        }
                        p = name_end + 3;
                        continue;
                    }
                    if (name_end[0] == '[') {
                        if ((name_end[1] == '@' || name_end[1] == '*') &&
                            name_end[2] == ']' && name_end[3] == '@' &&
                            (name_end[4] == 'Q' || name_end[4] == 'P' || name_end[4] == 'A') &&
                            name_end[5] == '}') {
                            size_t nlen = (size_t)(name_end - name_start);
                            char *name = calloc(nlen + 1, 1);
                            char *rendered = NULL;
                            char *joined = NULL;
                            if (name == NULL) {
                                free(out);
                                return NULL;
                            }
                            memcpy(name, name_start, nlen);
                            if (name_end[4] == 'A') {
                                if (cupid_array_exists(shell, name)) {
                                    rendered = format_array_assignment_string(shell, name);
                                } else {
                                    const char *scalar = cupid_vars_get(shell, name);
                                    rendered = format_scalar_assignment_string(name, scalar == NULL ? "" : scalar);
                                }
                            } else {
                                joined = join_array_members(shell, name, name_end[1] == '*');
                                if (joined == NULL) joined = strdup("");
                                if (joined == NULL) {
                                    free(name);
                                    free(out);
                                    return NULL;
                                }
                                if (name_end[4] == 'Q') rendered = format_declare_quoted_string(joined);
                                else rendered = strdup(joined);
                                free(joined);
                            }
                            free(name);
                            if (rendered == NULL) {
                                free(out);
                                return NULL;
                            }
                            if (append_cstr(&out, &len, &cap, rendered) != 0) {
                                free(rendered);
                                free(out);
                                return NULL;
                            }
                            free(rendered);
                            p = name_end + 6;
                            continue;
                        }
                        if ((name_end[1] == '@' || name_end[1] == '*') &&
                            name_end[2] == ']' && name_end[3] == '@' &&
                            name_end[4] == 'E' && name_end[5] == '}') {
                            size_t nlen = (size_t)(name_end - name_start);
                            char *name = calloc(nlen + 1, 1);
                            char *joined;
                            if (name == NULL) {
                                free(out);
                                return NULL;
                            }
                            memcpy(name, name_start, nlen);
                            joined = join_array_members(shell, name, name_end[1] == '*');
                            if (joined == NULL) joined = strdup("");
                            free(name);
                            if (joined == NULL) {
                                free(out);
                                return NULL;
                            }
                            if (append_cstr(&out, &len, &cap, joined) != 0) {
                                free(joined);
                                free(out);
                                return NULL;
                            }
                            free(joined);
                            p = name_end + 6;
                            continue;
                        }
                        if ((name_end[1] == '@' || name_end[1] == '*') &&
                            name_end[2] == ']' && name_end[3] == '}') {
                            size_t nlen = (size_t)(name_end - name_start);
                            char *name = calloc(nlen + 1, 1);
                            size_t acount;
                            size_t ai;
                            char sep = ' ';
                            const char *ifs = cupid_vars_get(shell, "IFS");
                            if (name == NULL) {
                                free(out);
                                return NULL;
                            }
                            memcpy(name, name_start, nlen);
                            if (ifs != NULL && ifs[0] != '\0') sep = ifs[0];
                            if (ifs != NULL && ifs[0] == '\0') sep = '\0';
                            acount = cupid_array_member_count(shell, name);
                            if (acount == 0) {
                                const char *scalar = cupid_vars_get(shell, name);
                                if (scalar != NULL &&
                                    append_cstr(&out, &len, &cap, scalar) != 0) {
                                    free(name);
                                    free(out);
                                    return NULL;
                                }
                            }
                            for (ai = 0; ai < acount; ai++) {
                                if (ai > 0) {
                                    char use_sep = (name_end[1] == '*') ? sep : ' ';
                                    if (use_sep != '\0' &&
                                        append_bytes(&out, &len, &cap, &use_sep, 1) != 0) {
                                        free(name);
                                        free(out);
                                        return NULL;
                                    }
                                }
                                if (append_cstr(&out, &len, &cap,
                                                cupid_array_member_value(shell, name, ai)) != 0) {
                                    free(name);
                                    free(out);
                                    return NULL;
                                }
                            }
                            free(name);
                            p = name_end + 4;
                            continue;
                        }
                        const char *idx_start = name_end + 1;
                        const char *idx_end = idx_start;
                        while (isdigit((unsigned char)*idx_end)) idx_end++;
                        if (idx_end > idx_start && idx_end[0] == ']' && idx_end[1] == '}') {
                            size_t nlen = (size_t)(name_end - name_start);
                            size_t ilen = (size_t)(idx_end - idx_start);
                            char *name = calloc(nlen + 1, 1);
                            char *idxs = calloc(ilen + 1, 1);
                            const char *aval;
                            size_t index_value;
                            if (name == NULL || idxs == NULL) {
                                free(name);
                                free(idxs);
                                free(out);
                                return NULL;
                            }
                            memcpy(name, name_start, nlen);
                            memcpy(idxs, idx_start, ilen);
                            index_value = (size_t)strtoul(idxs, NULL, 10);
                            aval = cupid_array_get_index(shell, name, index_value);
                            if (aval[0] == '\0' && index_value == 0) {
                                const char *scalar = cupid_vars_get(shell, name);
                                if (scalar != NULL) aval = scalar;
                            }
                            free(name);
                            free(idxs);
                            if (append_cstr(&out, &len, &cap, aval) != 0) {
                                free(out);
                                return NULL;
                            }
                            p = idx_end + 2;
                            continue;
                        }
                    }
                }

                char *name = NULL;
                char *word = NULL;
                char op = '\0';
                int op_long = 0;
                int colon_mode = 0;
                char *br = NULL;
                if (parse_braced_param(&p, quote, &name, &colon_mode, &op, &op_long, &word) != 0) {
                    free(out);
                    return NULL;
                }
                br = expand_braced_result(name, colon_mode, op, op_long, word, quote, shell);
                if (br == NULL) {
                    free(name);
                    free(word);
                    free(out);
                    return NULL;
                }
                if (append_cstr(&out, &len, &cap, br) != 0) {
                    free(name);
                    free(word);
                    free(br);
                    free(out);
                    return NULL;
                }
                free(name);
                free(word);
                free(br);
                continue;
            }
            if (p[1] == '?') {
                char status_buf[32];
                int n = snprintf(status_buf, sizeof(status_buf), "%d", shell->last_status);
                if (n < 0 || append_bytes(&out, &len, &cap, status_buf, (size_t)n) != 0) {
                    free(out);
                    return NULL;
                }
                p += 2;
                continue;
            }
            if (p[1] == '!') {
                char pid_buf[32];
                int n = 0;
                if (shell->last_bg_pid > 0) {
                    n = snprintf(pid_buf, sizeof(pid_buf), "%ld", (long)shell->last_bg_pid);
                }
                if (n > 0 && append_bytes(&out, &len, &cap, pid_buf, (size_t)n) != 0) {
                    free(out);
                    return NULL;
                }
                p += 2;
                continue;
            }
            if (p[1] >= '0' && p[1] <= '9') {
                int digit = p[1] - '0';
                p += 2;
                if (digit == 0) {
                    const char *val = shell->arg0 ? shell->arg0 : "";
                    if (append_cstr(&out, &len, &cap, val) != 0) {
                        free(out);
                        return NULL;
                    }
                } else {
                    size_t param_idx = (size_t)(digit - 1);
                    if (param_idx < shell->params.count) {
                        if (append_cstr(&out, &len, &cap, shell->params.args[param_idx]) != 0) {
                            free(out);
                            return NULL;
                        }
                    }
                }
                continue;
            }
            if (p[1] == '#') {
                char count_buf[32];
                int sn = snprintf(count_buf, sizeof(count_buf), "%zu", shell->params.count);
                if (sn < 0 || append_bytes(&out, &len, &cap, count_buf, (size_t)sn) != 0) {
                    free(out);
                    return NULL;
                }
                p += 2;
                continue;
            }
            if (p[1] == '@' || p[1] == '*') {
                size_t pi;
                const char *ifs = cupid_vars_get(shell, "IFS");
                char sep = ' ';
                if (p[1] == '*' && ifs != NULL && ifs[0] != '\0') sep = ifs[0];
                if (p[1] == '*' && ifs != NULL && ifs[0] == '\0') sep = '\0';
                for (pi = 0; pi < shell->params.count; pi++) {
                    if (pi > 0) {
                        if (sep != '\0' &&
                            append_bytes(&out, &len, &cap, &sep, 1) != 0) {
                            free(out);
                            return NULL;
                        }
                    }
                    if (append_cstr(&out, &len, &cap, shell->params.args[pi]) != 0) {
                        free(out);
                        return NULL;
                    }
                }
                p += 2;
                continue;
            }

            if (p[1] >= '0' && p[1] <= '9') {
                int digit = p[1] - '0';
                p += 2;
                if (digit == 0) {
                    const char *a0v = shell->arg0 ? shell->arg0 : "";
                    if (append_cstr(&out, &len, &cap, a0v) != 0) { free(out); return NULL; }
                } else {
                    size_t pidx = (size_t)(digit - 1);
                    if (pidx < shell->params.count) {
                        if (append_cstr(&out, &len, &cap, shell->params.args[pidx]) != 0) { free(out); return NULL; }
                    }
                }
                continue;
            }
            if (p[1] == '#') {
                char cbuf[32];
                int cn = snprintf(cbuf, sizeof(cbuf), "%zu", shell->params.count);
                if (cn < 0 || append_bytes(&out, &len, &cap, cbuf, (size_t)cn) != 0) { free(out); return NULL; }
                p += 2;
                continue;
            }
            if (p[1] == '@' || p[1] == '*') {
                size_t pi;
                const char *ifs = cupid_vars_get(shell, "IFS");
                char sep = ' ';
                if (p[1] == '*' && ifs != NULL && ifs[0] != '\0') sep = ifs[0];
                if (p[1] == '*' && ifs != NULL && ifs[0] == '\0') sep = '\0';
                for (pi = 0; pi < shell->params.count; pi++) {
                    if (pi > 0 && sep != '\0' &&
                        append_bytes(&out, &len, &cap, &sep, 1) != 0) { free(out); return NULL; }
                    if (append_cstr(&out, &len, &cap, shell->params.args[pi]) != 0) { free(out); return NULL; }
                }
                p += 2;
                continue;
            }

            if (isalpha((unsigned char)p[1]) || p[1] == '_') {
                const char *start = p + 1;
                const char *end = start;
                char *name;
                const char *val;
                size_t name_len;
                while (isalnum((unsigned char)*end) || *end == '_') {
                    end++;
                }
                name_len = (size_t)(end - start);
                name = calloc(name_len + 1, 1);
                if (name == NULL) {
                    free(out);
                    return NULL;
                }
                memcpy(name, start, name_len);
                val = cupid_vars_get(shell, name);
                if (val == NULL && shell->opt_nounset) {
                    fprintf(stderr, "cupid: %s: unbound variable\n", name);
                    if (!shell->is_interactive) {
                        shell->should_exit = 1;
                        shell->exit_code = 127;
                    }
                    free(name);
                    free(out);
                    return NULL;
                }
                free(name);
                if (val != NULL && append_cstr(&out, &len, &cap, val) != 0) {
                    free(out);
                    return NULL;
                }
                p = end;
                continue;
            }
        }

        if (append_bytes(&out, &len, &cap, p, 1) != 0) {
            free(out);
            return NULL;
        }
        p++;
    }

    if (out == NULL) {
        out = calloc(1, 1);
    }
    return out;
}

char *cupid_expand_tilde(const char *text, struct cupid_shell *shell) {
    const char *p;
    const char *prefix_end;
    size_t prefix_len;
    const char *resolved = NULL;
    size_t resolved_len, rest_len;
    char *result;

    if (text == NULL || text[0] != '~') {
        return strdup(text != NULL ? text : "");
    }

    p = text + 1;
    prefix_end = p;
    while (*prefix_end != '\0' && *prefix_end != '/') prefix_end++;
    prefix_len = (size_t)(prefix_end - p);

    if (prefix_len == 0) {
        resolved = cupid_vars_get(shell, "HOME");
        if (resolved == NULL) resolved = getenv("HOME");
        if (resolved == NULL) return strdup(text);
    } else if (prefix_len == 1 && *p == '+') {
        resolved = cupid_vars_get(shell, "PWD");
        if (resolved == NULL) resolved = getenv("PWD");
        if (resolved == NULL) return strdup(text);
    } else if (prefix_len == 1 && *p == '-') {
        resolved = cupid_vars_get(shell, "OLDPWD");
        if (resolved == NULL) resolved = getenv("OLDPWD");
        if (resolved == NULL) return strdup(text);
    } else {
        char *username = calloc(prefix_len + 1, 1);
        struct passwd *pw;
        if (username == NULL) return strdup(text);
        memcpy(username, p, prefix_len);
        pw = getpwnam(username);
        free(username);
        if (pw == NULL) return strdup(text);
        resolved = pw->pw_dir;
    }

    resolved_len = strlen(resolved);
    rest_len = strlen(prefix_end);
    result = calloc(resolved_len + rest_len + 1, 1);
    if (result == NULL) return strdup(text);
    memcpy(result, resolved, resolved_len);
    memcpy(result + resolved_len, prefix_end, rest_len);
    return result;
}

char *cupid_expand_word(const struct cupid_word *word, struct cupid_shell *shell) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;

    if (word == NULL) {
        return NULL;
    }
    for (i = 0; i < word->part_count; i++) {
        char *expanded;
        if (i == 0 && word->parts[i].quote == CUPID_QUOTE_NONE &&
            word->parts[i].text[0] == '~') {
            char *tilde_done = cupid_expand_tilde(word->parts[i].text, shell);
            if (tilde_done == NULL) { free(out); return NULL; }
            expanded = cupid_expand_text(tilde_done, CUPID_QUOTE_NONE, shell);
            free(tilde_done);
        } else {
            expanded = cupid_expand_text(word->parts[i].text, word->parts[i].quote, shell);
        }
        if (expanded == NULL) {
            free(out);
            return NULL;
        }
        if (append_cstr(&out, &len, &cap, expanded) != 0) {
            free(expanded);
            free(out);
            return NULL;
        }
        free(expanded);
    }

    if (out == NULL) {
        out = calloc(1, 1);
    }
    return out;
}

char *cupid_expand_case_pattern(const struct cupid_word *word, struct cupid_shell *shell) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;

    if (word == NULL) return NULL;
    for (i = 0; i < word->part_count; i++) {
        char *expanded = NULL;
        if (word->parts[i].quote == CUPID_QUOTE_NONE) {
            char *preserved = duplicate_backslashes(word->parts[i].text);
            if (preserved == NULL) {
                free(out);
                return NULL;
            }
            if (i == 0 && preserved[0] == '~') {
                char *tilde_done = cupid_expand_tilde(preserved, shell);
                free(preserved);
                if (tilde_done == NULL) {
                    free(out);
                    return NULL;
                }
                expanded = cupid_expand_text(tilde_done, CUPID_QUOTE_NONE, shell);
                free(tilde_done);
            } else {
                expanded = cupid_expand_text(preserved, CUPID_QUOTE_NONE, shell);
                free(preserved);
            }
        } else {
            char *literal = cupid_expand_text(word->parts[i].text, word->parts[i].quote, shell);
            if (literal != NULL) {
                expanded = escape_fnmatch_literal(literal, shell != NULL && shell->opt_extglob);
            }
            free(literal);
        }
        if (expanded == NULL) {
            free(out);
            return NULL;
        }
        if (append_cstr(&out, &len, &cap, expanded) != 0) {
            free(expanded);
            free(out);
            return NULL;
        }
        free(expanded);
    }
    if (out == NULL) out = calloc(1, 1);
    return out;
}

char *cupid_word_literal(const struct cupid_word *word) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;

    if (word == NULL) {
        return NULL;
    }
    for (i = 0; i < word->part_count; i++) {
        if (append_cstr(&out, &len, &cap, word->parts[i].text) != 0) {
            free(out);
            return NULL;
        }
    }
    if (out == NULL) {
        out = calloc(1, 1);
    }
    return out;
}

char *cupid_word_source_text(const struct cupid_word *word) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;

    if (word == NULL) return NULL;
    for (i = 0; i < word->part_count; i++) {
        const char *text = word->parts[i].text;
        const char *p = text;
        if (word->parts[i].quote == CUPID_QUOTE_NONE) {
            if (append_cstr(&out, &len, &cap, text) != 0) {
                free(out);
                return NULL;
            }
            continue;
        }
        if (word->parts[i].quote == CUPID_QUOTE_ANSI_C) {
            if (append_cstr(&out, &len, &cap, "$'") != 0 ||
                append_cstr(&out, &len, &cap, text) != 0 ||
                append_cstr(&out, &len, &cap, "'") != 0) {
                free(out);
                return NULL;
            }
            continue;
        }
        if (word->parts[i].quote == CUPID_QUOTE_SINGLE) {
            if (append_bytes(&out, &len, &cap, "'", 1) != 0) {
                free(out);
                return NULL;
            }
            while (p != NULL && *p != '\0') {
                if (*p == '\'') {
                    if (append_cstr(&out, &len, &cap, "'\\''") != 0) {
                        free(out);
                        return NULL;
                    }
                } else if (append_bytes(&out, &len, &cap, p, 1) != 0) {
                    free(out);
                    return NULL;
                }
                p++;
            }
            if (append_bytes(&out, &len, &cap, "'", 1) != 0) {
                free(out);
                return NULL;
            }
            continue;
        }

        if (append_bytes(&out, &len, &cap, "\"", 1) != 0) {
            free(out);
            return NULL;
        }
        while (p != NULL && *p != '\0') {
            if (*p == CUPID_ESC_BACKTICK_PLACEHOLDER) {
                if (append_cstr(&out, &len, &cap, "\\`") != 0) {
                    free(out);
                    return NULL;
                }
                p++;
                continue;
            }
            if (*p == '\\' || *p == '"' || *p == '$' || *p == '`') {
                if (append_bytes(&out, &len, &cap, "\\", 1) != 0) {
                    free(out);
                    return NULL;
                }
            }
            if (append_bytes(&out, &len, &cap, p, 1) != 0) {
                free(out);
                return NULL;
            }
            p++;
        }
        if (append_bytes(&out, &len, &cap, "\"", 1) != 0) {
            free(out);
            return NULL;
        }
    }
    if (out == NULL) out = calloc(1, 1);
    return out;
}

char *cupid_word_dequote_literal(const struct cupid_word *word) {
    char *out = NULL;
    size_t len = 0;
    size_t cap = 0;
    size_t i;
    if (word == NULL) return NULL;
    for (i = 0; i < word->part_count; i++) {
        const char *text = word->parts[i].text;
        if (word->parts[i].quote == CUPID_QUOTE_NONE) {
            const char *p = text;
            while (p != NULL && *p != '\0') {
                if (*p == CUPID_ESC_BACKTICK_PLACEHOLDER) {
                    if (append_bytes(&out, &len, &cap, "`", 1) != 0) {
                        free(out);
                        return NULL;
                    }
                    p++;
                    continue;
                }
                if (*p == '\\' && p[1] != '\0') p++;
                if (append_bytes(&out, &len, &cap, p, 1) != 0) {
                    free(out);
                    return NULL;
                }
                p++;
            }
        } else if (word->parts[i].quote == CUPID_QUOTE_ANSI_C) {
            char *expanded = decode_ansi_c_text(text);
            if (expanded == NULL || append_cstr(&out, &len, &cap, expanded) != 0) {
                free(expanded);
                free(out);
                return NULL;
            }
            free(expanded);
        } else {
            const char *p = text;
            while (p != NULL && *p != '\0') {
                char ch = (*p == CUPID_ESC_BACKTICK_PLACEHOLDER) ? '`' : *p;
                if (append_bytes(&out, &len, &cap, &ch, 1) != 0) {
                    free(out);
                    return NULL;
                }
                p++;
            }
        }
    }
    if (out == NULL) out = calloc(1, 1);
    return out;
}
