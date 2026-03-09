#include "cupid/arith.h"

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "cupid/expand.h"
#include "cupid/shell.h"
#include "cupid/vars.h"

struct arith_ctx {
    struct cupid_shell *shell;
    const char *pos;
    int error;
    int evaluate;
};

static void skip_ws(struct arith_ctx *ctx) {
    while (*ctx->pos == ' ' || *ctx->pos == '\t' || *ctx->pos == '\n') {
        ctx->pos++;
    }
}

static int is_ident_start(char c) {
    return isalpha((unsigned char)c) || c == '_';
}

static int is_ident_char(char c) {
    return isalnum((unsigned char)c) || c == '_';
}

static int arith_digit_value(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 36;
    if (c == '@') return 62;
    if (c == '_') return 63;
    return -1;
}

static long get_var(struct arith_ctx *ctx, const char *name, size_t len) {
    char *key = calloc(len + 1, 1);
    const char *val;
    long result;
    if (key == NULL) { ctx->error = 1; return 0; }
    if (!ctx->evaluate || ctx->shell == NULL) {
        free(key);
        return 0;
    }
    memcpy(key, name, len);
    val = cupid_vars_get(ctx->shell, key);
    free(key);
    if (val == NULL) return 0;
    result = strtol(val, NULL, 10);
    return result;
}

static void set_var(struct arith_ctx *ctx, const char *name, size_t len, long value) {
    char *key = calloc(len + 1, 1);
    char buf[32];
    if (!ctx->evaluate || ctx->shell == NULL) return;
    if (key == NULL) { ctx->error = 1; return; }
    memcpy(key, name, len);
    snprintf(buf, sizeof(buf), "%ld", value);
    if (cupid_vars_set(ctx->shell, key, buf) != 0) {
        ctx->error = 1;
    }
    free(key);
}

static long parse_expr(struct arith_ctx *ctx);
static long parse_assign(struct arith_ctx *ctx);
static long parse_ternary(struct arith_ctx *ctx);
static long parse_or(struct arith_ctx *ctx);
static long parse_and(struct arith_ctx *ctx);
static long parse_bitor(struct arith_ctx *ctx);
static long parse_bitxor(struct arith_ctx *ctx);
static long parse_bitand(struct arith_ctx *ctx);
static long parse_eq(struct arith_ctx *ctx);
static long parse_rel(struct arith_ctx *ctx);
static long parse_shift(struct arith_ctx *ctx);
static long parse_add(struct arith_ctx *ctx);
static long parse_mul(struct arith_ctx *ctx);
static long parse_power(struct arith_ctx *ctx);
static long parse_unary(struct arith_ctx *ctx);
static long parse_postfix(struct arith_ctx *ctx);
static long parse_primary(struct arith_ctx *ctx);

static char *extract_command_subst_text(const char **pp) {
    const char *start = *pp;
    const char *p = *pp;
    int depth = 0;
    int mode = 0;
    char *out;
    size_t len;

    if (p[0] != '$' || p[1] != '(') return NULL;
    p += 2;
    depth = 1;

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
        if (*p == '\'') {
            mode = 1;
            p++;
            continue;
        }
        if (*p == '"') {
            mode = 2;
            p++;
            continue;
        }
        if (*p == '\\' && p[1] != '\0') {
            p += 2;
            continue;
        }
        if (p[0] == '$' && p[1] == '(') {
            depth++;
            p += 2;
            continue;
        }
        if (*p == '(') {
            depth++;
            p++;
            continue;
        }
        if (*p == ')') {
            depth--;
            p++;
            if (depth == 0) {
                len = (size_t)(p - start);
                out = calloc(len + 1, 1);
                if (out == NULL) return NULL;
                memcpy(out, start, len);
                *pp = p;
                return out;
            }
            continue;
        }
        p++;
    }
    return NULL;
}

static long parse_expr(struct arith_ctx *ctx) {
    long result = parse_assign(ctx);
    if (ctx->error) return 0;
    while (*ctx->pos == ',') {
        ctx->pos++;
        result = parse_assign(ctx);
        if (ctx->error) return 0;
    }
    return result;
}

static long parse_assign(struct arith_ctx *ctx) {
    const char *save = ctx->pos;
    skip_ws(ctx);

    if (is_ident_start(*ctx->pos)) {
        const char *name = ctx->pos;
        size_t name_len;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);
        skip_ws(ctx);

        if (*ctx->pos == '=' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            set_var(ctx, name, name_len, rhs);
            return rhs;
        }
        if (ctx->pos[0] == '+' && ctx->pos[1] == '=') {
            long rhs, cur;
            ctx->pos += 2;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            cur = get_var(ctx, name, name_len);
            cur += rhs;
            set_var(ctx, name, name_len, cur);
            return cur;
        }
        if (ctx->pos[0] == '-' && ctx->pos[1] == '=') {
            long rhs, cur;
            ctx->pos += 2;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            cur = get_var(ctx, name, name_len);
            cur -= rhs;
            set_var(ctx, name, name_len, cur);
            return cur;
        }
        if (ctx->pos[0] == '*' && ctx->pos[1] == '=') {
            long rhs, cur;
            ctx->pos += 2;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            cur = get_var(ctx, name, name_len);
            cur *= rhs;
            set_var(ctx, name, name_len, cur);
            return cur;
        }
        if (ctx->pos[0] == '/' && ctx->pos[1] == '=') {
            long rhs, cur;
            ctx->pos += 2;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            if (rhs == 0) { ctx->error = 1; return 0; }
            cur = get_var(ctx, name, name_len);
            cur /= rhs;
            set_var(ctx, name, name_len, cur);
            return cur;
        }
        if (ctx->pos[0] == '%' && ctx->pos[1] == '=') {
            long rhs, cur;
            ctx->pos += 2;
            rhs = parse_assign(ctx);
            if (ctx->error) return 0;
            if (rhs == 0) { ctx->error = 1; return 0; }
            cur = get_var(ctx, name, name_len);
            cur %= rhs;
            set_var(ctx, name, name_len, cur);
            return cur;
        }

        ctx->pos = save;
    } else {
        ctx->pos = save;
    }

    return parse_ternary(ctx);
}

static long parse_ternary(struct arith_ctx *ctx) {
    long cond = parse_or(ctx);
    if (ctx->error) return 0;
    skip_ws(ctx);
    if (*ctx->pos == '?') {
        long then_val, else_val;
        ctx->pos++;
        then_val = parse_expr(ctx);
        if (ctx->error) return 0;
        skip_ws(ctx);
        if (*ctx->pos != ':') { ctx->error = 1; return 0; }
        ctx->pos++;
        else_val = parse_ternary(ctx);
        if (ctx->error) return 0;
        return cond ? then_val : else_val;
    }
    return cond;
}

static long parse_or(struct arith_ctx *ctx) {
    long result = parse_and(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '|' && ctx->pos[1] == '|') {
            ctx->pos += 2;
            if (result != 0) {
                struct arith_ctx skip = *ctx;
                skip.evaluate = 0;
                (void)parse_and(&skip);
                if (skip.error) return 0;
                ctx->pos = skip.pos;
                result = 1;
            } else {
                long rhs = parse_and(ctx);
                if (ctx->error) return 0;
                result = (rhs != 0) ? 1 : 0;
            }
        } else {
            break;
        }
    }
    return result;
}

static long parse_and(struct arith_ctx *ctx) {
    long result = parse_bitor(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '&' && ctx->pos[1] == '&') {
            ctx->pos += 2;
            if (result == 0) {
                struct arith_ctx skip = *ctx;
                skip.evaluate = 0;
                (void)parse_bitor(&skip);
                if (skip.error) return 0;
                ctx->pos = skip.pos;
                result = 0;
            } else {
                long rhs = parse_bitor(ctx);
                if (ctx->error) return 0;
                result = (rhs != 0) ? 1 : 0;
            }
        } else {
            break;
        }
    }
    return result;
}

static long parse_bitor(struct arith_ctx *ctx) {
    long result = parse_bitxor(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '|' && ctx->pos[1] != '|' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_bitxor(ctx);
            if (ctx->error) return 0;
            result |= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_bitxor(struct arith_ctx *ctx) {
    long result = parse_bitand(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (*ctx->pos == '^') {
            long rhs;
            ctx->pos++;
            rhs = parse_bitand(ctx);
            if (ctx->error) return 0;
            result ^= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_bitand(struct arith_ctx *ctx) {
    long result = parse_eq(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '&' && ctx->pos[1] != '&' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_eq(ctx);
            if (ctx->error) return 0;
            result &= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_eq(struct arith_ctx *ctx) {
    long result = parse_rel(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '=' && ctx->pos[1] == '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_rel(ctx);
            if (ctx->error) return 0;
            result = (result == rhs) ? 1 : 0;
        } else if (ctx->pos[0] == '!' && ctx->pos[1] == '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_rel(ctx);
            if (ctx->error) return 0;
            result = (result != rhs) ? 1 : 0;
        } else {
            break;
        }
    }
    return result;
}

static long parse_rel(struct arith_ctx *ctx) {
    long result = parse_shift(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '<' && ctx->pos[1] == '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_shift(ctx);
            if (ctx->error) return 0;
            result = (result <= rhs) ? 1 : 0;
        } else if (ctx->pos[0] == '>' && ctx->pos[1] == '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_shift(ctx);
            if (ctx->error) return 0;
            result = (result >= rhs) ? 1 : 0;
        } else if (ctx->pos[0] == '<' && ctx->pos[1] != '<') {
            long rhs;
            ctx->pos++;
            rhs = parse_shift(ctx);
            if (ctx->error) return 0;
            result = (result < rhs) ? 1 : 0;
        } else if (ctx->pos[0] == '>' && ctx->pos[1] != '>') {
            long rhs;
            ctx->pos++;
            rhs = parse_shift(ctx);
            if (ctx->error) return 0;
            result = (result > rhs) ? 1 : 0;
        } else {
            break;
        }
    }
    return result;
}

static long parse_shift(struct arith_ctx *ctx) {
    long result = parse_add(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (ctx->pos[0] == '<' && ctx->pos[1] == '<' && ctx->pos[2] != '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_add(ctx);
            if (ctx->error) return 0;
            result <<= rhs;
        } else if (ctx->pos[0] == '>' && ctx->pos[1] == '>' && ctx->pos[2] != '=') {
            long rhs;
            ctx->pos += 2;
            rhs = parse_add(ctx);
            if (ctx->error) return 0;
            result >>= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_add(struct arith_ctx *ctx) {
    long result = parse_mul(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (*ctx->pos == '+' && ctx->pos[1] != '+' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_mul(ctx);
            if (ctx->error) return 0;
            result += rhs;
        } else if (*ctx->pos == '-' && ctx->pos[1] != '-' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_mul(ctx);
            if (ctx->error) return 0;
            result -= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_mul(struct arith_ctx *ctx) {
    long result = parse_power(ctx);
    if (ctx->error) return 0;
    while (1) {
        skip_ws(ctx);
        if (*ctx->pos == '*' && ctx->pos[1] != '*' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_power(ctx);
            if (ctx->error) return 0;
            result *= rhs;
        } else if (*ctx->pos == '/' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_power(ctx);
            if (ctx->error) return 0;
            if (rhs == 0) {
                if (ctx->evaluate) { ctx->error = 1; return 0; }
                return 0;
            }
            result /= rhs;
        } else if (*ctx->pos == '%' && ctx->pos[1] != '=') {
            long rhs;
            ctx->pos++;
            rhs = parse_power(ctx);
            if (ctx->error) return 0;
            if (rhs == 0) {
                if (ctx->evaluate) { ctx->error = 1; return 0; }
                return 0;
            }
            result %= rhs;
        } else {
            break;
        }
    }
    return result;
}

static long parse_power(struct arith_ctx *ctx) {
    long base = parse_unary(ctx);
    long exp, result, i;
    if (ctx->error) return 0;
    skip_ws(ctx);
    if (ctx->pos[0] == '*' && ctx->pos[1] == '*') {
        ctx->pos += 2;
        exp = parse_power(ctx);
        if (ctx->error) return 0;
        if (exp < 0) return 0;
        result = 1;
        for (i = 0; i < exp; i++) result *= base;
        return result;
    }
    return base;
}

static long parse_unary(struct arith_ctx *ctx) {
    skip_ws(ctx);

    if (ctx->pos[0] == '+' && ctx->pos[1] == '+') {
        const char *name;
        size_t name_len;
        long val;
        ctx->pos += 2;
        skip_ws(ctx);
        if (*ctx->pos == '$') ctx->pos++;
        if (!is_ident_start(*ctx->pos)) { ctx->error = 1; return 0; }
        name = ctx->pos;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);
        val = get_var(ctx, name, name_len) + 1;
        set_var(ctx, name, name_len, val);
        return val;
    }
    if (ctx->pos[0] == '-' && ctx->pos[1] == '-') {
        const char *name;
        size_t name_len;
        long val;
        ctx->pos += 2;
        skip_ws(ctx);
        if (*ctx->pos == '$') ctx->pos++;
        if (!is_ident_start(*ctx->pos)) { ctx->error = 1; return 0; }
        name = ctx->pos;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);
        val = get_var(ctx, name, name_len) - 1;
        set_var(ctx, name, name_len, val);
        return val;
    }
    if (*ctx->pos == '+' && ctx->pos[1] != '+' && ctx->pos[1] != '=') {
        ctx->pos++;
        return parse_unary(ctx);
    }
    if (*ctx->pos == '-' && ctx->pos[1] != '-' && ctx->pos[1] != '=') {
        ctx->pos++;
        return -parse_unary(ctx);
    }
    if (*ctx->pos == '!' && ctx->pos[1] != '=') {
        long v;
        ctx->pos++;
        v = parse_unary(ctx);
        return v ? 0 : 1;
    }
    if (*ctx->pos == '~') {
        ctx->pos++;
        return ~parse_unary(ctx);
    }
    return parse_postfix(ctx);
}

static long parse_postfix(struct arith_ctx *ctx) {
    const char *save;
    skip_ws(ctx);
    save = ctx->pos;

    if (*ctx->pos == '$') {
        if (ctx->pos[1] == '(' && ctx->pos[2] == '(') {
            return parse_primary(ctx);
        }
        ctx->pos++;
    }

    if (is_ident_start(*ctx->pos)) {
        const char *name = ctx->pos;
        size_t name_len;
        long val;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);

        skip_ws(ctx);
        if (ctx->pos[0] == '+' && ctx->pos[1] == '+') {
            val = get_var(ctx, name, name_len);
            set_var(ctx, name, name_len, val + 1);
            ctx->pos += 2;
            return val;
        }
        if (ctx->pos[0] == '-' && ctx->pos[1] == '-') {
            val = get_var(ctx, name, name_len);
            set_var(ctx, name, name_len, val - 1);
            ctx->pos += 2;
            return val;
        }
        return get_var(ctx, name, name_len);
    }

    ctx->pos = save;
    return parse_primary(ctx);
}

static long parse_number(struct arith_ctx *ctx) {
    const char *p = ctx->pos;
    long val = 0;
    const char *base_scan = p;
    int base = 0;

    while (isdigit((unsigned char)*base_scan)) {
        base = base * 10 + (*base_scan - '0');
        base_scan++;
    }
    if (base_scan > p && *base_scan == '#') {
        int saw_digit = 0;
        if (base < 2 || base > 64) { ctx->error = 1; return 0; }
        p = base_scan + 1;
        while (*p != '\0') {
            int dv = arith_digit_value(*p);
            if (dv < 0) break;
            if (dv >= base) { ctx->error = 1; return 0; }
            val = val * base + dv;
            saw_digit = 1;
            p++;
        }
        if (!saw_digit) { ctx->error = 1; return 0; }
        ctx->pos = p;
        return val;
    }

    if (p[0] == '0' && (p[1] == 'x' || p[1] == 'X')) {
        p += 2;
        if (!isxdigit((unsigned char)*p)) { ctx->error = 1; return 0; }
        while (isxdigit((unsigned char)*p)) {
            char c = *p;
            if (c >= '0' && c <= '9') val = val * 16 + (c - '0');
            else if (c >= 'a' && c <= 'f') val = val * 16 + (c - 'a' + 10);
            else val = val * 16 + (c - 'A' + 10);
            p++;
        }
        ctx->pos = p;
        return val;
    }

    if (p[0] == '0' && isdigit((unsigned char)p[1])) {
        p++;
        while (*p >= '0' && *p <= '7') {
            val = val * 8 + (*p - '0');
            p++;
        }
        if (isdigit((unsigned char)*p)) {
            ctx->error = 1;
            return 0;
        }
        ctx->pos = p;
        return val;
    }

    while (isdigit((unsigned char)*p)) {
        val = val * 10 + (*p - '0');
        p++;
    }
    ctx->pos = p;
    return val;
}

static long parse_primary(struct arith_ctx *ctx) {
    skip_ws(ctx);

    if (ctx->pos[0] == '$' && ctx->pos[1] == '(' && ctx->pos[2] == '(') {
        long val;
        ctx->pos += 3;
        val = parse_expr(ctx);
        if (ctx->error) return 0;
        skip_ws(ctx);
        if (ctx->pos[0] != ')' || ctx->pos[1] != ')') { ctx->error = 1; return 0; }
        ctx->pos += 2;
        return val;
    }

    if (ctx->pos[0] == '$' && ctx->pos[1] == '(') {
        const char *next = ctx->pos;
        char *cmdsub = extract_command_subst_text(&next);
        char *expanded;
        char *scan;
        char *end = NULL;
        long parsed = 0;
        if (cmdsub == NULL) { ctx->error = 1; return 0; }
        if (!ctx->evaluate) {
            free(cmdsub);
            ctx->pos = next;
            return 0;
        }
        expanded = cupid_expand_text(cmdsub, CUPID_QUOTE_NONE, ctx->shell);
        free(cmdsub);
        if (expanded == NULL) { ctx->error = 1; return 0; }
        scan = expanded;
        while (*scan != '\0' && isspace((unsigned char)*scan)) scan++;
        if (*scan != '\0') {
            parsed = strtol(scan, &end, 10);
            if (end == scan) parsed = 0;
        }
        free(expanded);
        ctx->pos = next;
        return parsed;
    }

    if (ctx->pos[0] == '$' && is_ident_start(ctx->pos[1])) {
        const char *name;
        size_t name_len;
        ctx->pos++;
        name = ctx->pos;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);
        return get_var(ctx, name, name_len);
    }

    if (*ctx->pos == '(') {
        long val;
        ctx->pos++;
        val = parse_expr(ctx);
        if (ctx->error) return 0;
        skip_ws(ctx);
        if (*ctx->pos != ')') { ctx->error = 1; return 0; }
        ctx->pos++;
        return val;
    }

    if (isdigit((unsigned char)*ctx->pos)) {
        return parse_number(ctx);
    }

    if (is_ident_start(*ctx->pos)) {
        const char *name = ctx->pos;
        size_t name_len;
        while (is_ident_char(*ctx->pos)) ctx->pos++;
        name_len = (size_t)(ctx->pos - name);
        return get_var(ctx, name, name_len);
    }

    ctx->error = 1;
    return 0;
}

long cupid_arith_eval(struct cupid_shell *shell, const char *expr, int *error) {
    struct arith_ctx ctx;
    long result;

    ctx.shell = shell;
    ctx.pos = expr;
    ctx.error = 0;
    ctx.evaluate = 1;

    skip_ws(&ctx);
    if (*ctx.pos == '\0') {
        if (error) *error = 0;
        return 0;
    }

    result = parse_expr(&ctx);

    skip_ws(&ctx);
    if (*ctx.pos != '\0' && !ctx.error) {
        ctx.error = 1;
    }

    if (error) *error = ctx.error;
    return ctx.error ? 0 : result;
}
