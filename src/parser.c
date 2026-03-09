#include "cupid/parser.h"

#include <ctype.h>
#include <stdlib.h>
#include <string.h>

static int g_posix_mode = 0;
static int parse_node(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node);

static int word_clone(struct cupid_word *dst, const struct cupid_word *src) {
    size_t i;
    memset(dst, 0, sizeof(*dst));
    dst->had_quotes = src->had_quotes;
    dst->had_escaped_brace = src->had_escaped_brace;
    dst->had_escaped_chars = src->had_escaped_chars;
    for (i = 0; i < src->part_count; i++) {
        struct cupid_word_part *parts;
        size_t len = strlen(src->parts[i].text);
        char *copy = calloc(len + 1, 1);
        if (copy == NULL) {
            cupid_word_free(dst);
            return -1;
        }
        memcpy(copy, src->parts[i].text, len);
        parts = realloc(dst->parts, sizeof(*parts) * (dst->part_count + 1));
        if (parts == NULL) {
            free(copy);
            cupid_word_free(dst);
            return -1;
        }
        dst->parts = parts;
        dst->parts[dst->part_count].text = copy;
        dst->parts[dst->part_count].quote = src->parts[i].quote;
        dst->part_count++;
    }
    return 0;
}

static int command_push_argv(struct cupid_command *cmd, const struct cupid_word *word) {
    struct cupid_word *argv = realloc(cmd->argv, sizeof(*argv) * (cmd->argc + 1));
    if (argv == NULL) {
        return -1;
    }
    cmd->argv = argv;
    memset(&cmd->argv[cmd->argc], 0, sizeof(cmd->argv[cmd->argc]));
    if (word_clone(&cmd->argv[cmd->argc], word) != 0) {
        return -1;
    }
    cmd->argc++;
    return 0;
}

static enum cupid_redir_kind map_redir(enum cupid_token_kind kind) {
    switch (kind) {
        case TOK_REDIR_IN:     return CUPID_REDIR_IN;
        case TOK_REDIR_OUT:    return CUPID_REDIR_OUT;
        case TOK_REDIR_APPEND: return CUPID_REDIR_APPEND;
        case TOK_REDIR_CLOBBER: return CUPID_REDIR_CLOBBER;
        case TOK_REDIR_INOUT: return CUPID_REDIR_INOUT;
        case TOK_REDIR_DUP_OUT: return CUPID_REDIR_DUP_OUT;
        case TOK_REDIR_DUP_IN: return CUPID_REDIR_DUP_IN;
        case TOK_REDIR_ERR_OUT: return CUPID_REDIR_ERR_OUT;
        case TOK_REDIR_ERR_TO_OUT: return CUPID_REDIR_ERR_TO_OUT;
        case TOK_HEREDOC:      return CUPID_REDIR_HEREDOC;
        case TOK_HEREDOC_STRIP: return CUPID_REDIR_HEREDOC;
        case TOK_HERESTRING:   return CUPID_REDIR_HERESTRING;
        default:               return CUPID_REDIR_OUT;
    }
}

static int token_is_redir(enum cupid_token_kind kind) {
    return kind == TOK_REDIR_IN || kind == TOK_REDIR_OUT || kind == TOK_REDIR_APPEND ||
           kind == TOK_REDIR_CLOBBER || kind == TOK_REDIR_INOUT ||
           kind == TOK_REDIR_DUP_OUT || kind == TOK_REDIR_DUP_IN ||
           kind == TOK_REDIR_ERR_OUT || kind == TOK_REDIR_ERR_TO_OUT ||
           kind == TOK_HEREDOC || kind == TOK_HEREDOC_STRIP || kind == TOK_HERESTRING;
}

static int token_is_separator(enum cupid_token_kind kind) {
    return kind == TOK_AND_IF || kind == TOK_OR_IF || kind == TOK_SEMI ||
           kind == TOK_NEWLINE || kind == TOK_AMP;
}

static int token_is_case_terminator(enum cupid_token_kind kind) {
    return kind == TOK_DSEMI || kind == TOK_CASE_FALLTHROUGH || kind == TOK_CASE_TESTNEXT;
}

static const char *word_text(const struct cupid_token *tok) {
    if (tok->kind != TOK_WORD || tok->word.part_count != 1 ||
        tok->word.had_quotes || tok->word.parts[0].quote != CUPID_QUOTE_NONE) {
        return NULL;
    }
    return tok->word.parts[0].text;
}

static int parse_varredir_word(const struct cupid_token *tok, char **name_out) {
    const char *t = word_text(tok);
    size_t len;
    char *name;
    if (name_out != NULL) *name_out = NULL;
    if (t == NULL) return 0;
    len = strlen(t);
    if (len < 3 || t[0] != '{' || t[len - 1] != '}') return 0;
    name = calloc(len - 1, 1);
    if (name == NULL) return -1;
    memcpy(name, t + 1, len - 2);
    if (name[0] == '\0') {
        free(name);
        return 0;
    }
    if (name_out != NULL) *name_out = name;
    else free(name);
    return 1;
}

static int is_keyword(const struct cupid_token *tok, const char *kw) {
    const char *t = word_text(tok);
    return t != NULL && strcmp(t, kw) == 0;
}

static int token_is_reserved_bang(const struct cupid_token *tok) {
    return is_keyword(tok, "!");
}

static int word_is_arith_expr(const struct cupid_token *tok, const char **inner_start, size_t *inner_len) {
    const char *t = word_text(tok);
    size_t len;
    if (t == NULL) return 0;
    len = strlen(t);
    if (len < 4) return 0;
    if (t[0] != '(' || t[1] != '(' || t[len - 2] != ')' || t[len - 1] != ')') return 0;
    if (inner_start != NULL) *inner_start = t + 2;
    if (inner_len != NULL) *inner_len = len - 4;
    return 1;
}

static int tokens_start_arith(const struct cupid_tokens *tokens, size_t idx) {
    return idx + 1 < tokens->count &&
           tokens->items[idx].kind == TOK_LPAREN &&
           tokens->items[idx + 1].kind == TOK_LPAREN;
}

static int append_piece(char **buf, size_t *len, size_t *cap, const char *text) {
    size_t tlen;
    char *next;
    if (text == NULL) return -1;
    tlen = strlen(text);
    if (*len + tlen + 2 > *cap) {
        size_t nc = (*cap == 0) ? 32 : *cap;
        while (*len + tlen + 2 > nc) nc *= 2;
        next = realloc(*buf, nc);
        if (next == NULL) return -1;
        *buf = next;
        *cap = nc;
    }
    if (*len > 0) {
        (*buf)[(*len)++] = ' ';
    }
    memcpy(*buf + *len, text, tlen);
    *len += tlen;
    (*buf)[*len] = '\0';
    return 0;
}

static char *flatten_word(const struct cupid_word *word) {
    size_t i;
    size_t total = 0;
    char *out;
    size_t off = 0;
    for (i = 0; i < word->part_count; i++) total += strlen(word->parts[i].text);
    out = calloc(total + 1, 1);
    if (out == NULL) return NULL;
    for (i = 0; i < word->part_count; i++) {
        size_t n = strlen(word->parts[i].text);
        memcpy(out + off, word->parts[i].text, n);
        off += n;
    }
    return out;
}

static int parse_arith_expr_tokens(const struct cupid_tokens *tokens, size_t *idx, char **expr_out) {
    size_t i = *idx;
    int depth = 0;
    char *buf = NULL;
    size_t len = 0, cap = 0;

    if (!tokens_start_arith(tokens, i)) return -1;
    i += 2;

    while (i < tokens->count) {
        const struct cupid_token *tok = &tokens->items[i];
        if (tok->kind == TOK_LPAREN) {
            depth++;
            if (append_piece(&buf, &len, &cap, "(") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_RPAREN) {
            if (depth == 0) {
                if (i + 1 < tokens->count && tokens->items[i + 1].kind == TOK_RPAREN) {
                    i += 2;
                    *idx = i;
                    *expr_out = (buf != NULL) ? buf : strdup("");
                    if (*expr_out == NULL) goto fail;
                    return 0;
                }
                goto fail;
            }
            depth--;
            if (append_piece(&buf, &len, &cap, ")") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_WORD) {
            char *w = flatten_word(&tok->word);
            if (w == NULL) goto fail;
            if (append_piece(&buf, &len, &cap, w) != 0) {
                free(w);
                goto fail;
            }
            free(w);
            i++;
            continue;
        }
        if (tok->kind == TOK_SEMI) {
            if (append_piece(&buf, &len, &cap, ";") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_DSEMI) {
            if (append_piece(&buf, &len, &cap, ";;") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_IN) {
            if (i + 1 < tokens->count &&
                tokens->items[i + 1].kind == TOK_WORD &&
                is_keyword(&tokens->items[i + 1], "=")) {
                if (append_piece(&buf, &len, &cap, "<=") != 0) goto fail;
                i += 2;
                continue;
            }
            if (append_piece(&buf, &len, &cap, "<") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_OUT) {
            if (i + 1 < tokens->count &&
                tokens->items[i + 1].kind == TOK_WORD &&
                is_keyword(&tokens->items[i + 1], "=")) {
                if (append_piece(&buf, &len, &cap, ">=") != 0) goto fail;
                i += 2;
                continue;
            }
            if (append_piece(&buf, &len, &cap, ">") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_APPEND) {
            if (append_piece(&buf, &len, &cap, ">>") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_CLOBBER) {
            if (append_piece(&buf, &len, &cap, ">|") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_INOUT) {
            if (append_piece(&buf, &len, &cap, "<>") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_DUP_OUT) {
            if (append_piece(&buf, &len, &cap, ">&") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_DUP_IN) {
            if (append_piece(&buf, &len, &cap, "<&") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_ERR_OUT) {
            if (append_piece(&buf, &len, &cap, "&>") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_REDIR_ERR_TO_OUT) {
            if (append_piece(&buf, &len, &cap, ">&1") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_HERESTRING) {
            if (append_piece(&buf, &len, &cap, "<<<") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_AND_IF) {
            if (append_piece(&buf, &len, &cap, "&&") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_OR_IF) {
            if (append_piece(&buf, &len, &cap, "||") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_PIPE) {
            if (append_piece(&buf, &len, &cap, "|") != 0) goto fail;
            i++;
            continue;
        }
        if (tok->kind == TOK_AMP) {
            if (append_piece(&buf, &len, &cap, "&") != 0) goto fail;
            i++;
            continue;
        }
        goto fail;
    }

fail:
    free(buf);
    return -1;
}

static char *trim_dup_slice(const char *start, size_t len) {
    size_t i = 0;
    size_t j = len;
    char *out;
    while (i < len && isspace((unsigned char)start[i])) i++;
    while (j > i && isspace((unsigned char)start[j - 1])) j--;
    out = calloc((j - i) + 1, 1);
    if (out == NULL) return NULL;
    if (j > i) memcpy(out, start + i, j - i);
    return out;
}

static int parse_cstyle_for_fields(const char *inner, size_t inner_len, struct cupid_for_node *for_node) {
    size_t i;
    size_t first = (size_t)-1;
    size_t second = (size_t)-1;
    int depth = 0;
    char quote = '\0';
    for (i = 0; i < inner_len; i++) {
        char ch = inner[i];
        if (quote != '\0') {
            if (ch == '\\' && i + 1 < inner_len) {
                i++;
                continue;
            }
            if (ch == quote) quote = '\0';
            continue;
        }
        if (ch == '\'' || ch == '"') {
            quote = ch;
            continue;
        }
        if (ch == '\\' && i + 1 < inner_len) {
            i++;
            continue;
        }
        if (ch == '(') {
            depth++;
            continue;
        }
        if (ch == ')') {
            if (depth > 0) depth--;
            continue;
        }
        if (ch == ';' && depth == 0) {
            if (first == (size_t)-1) {
                first = i;
            } else if (second == (size_t)-1) {
                second = i;
            } else {
                return -1;
            }
        }
    }
    if (first == (size_t)-1 || second == (size_t)-1) return -1;

    for_node->c_init = trim_dup_slice(inner, first);
    for_node->c_cond = trim_dup_slice(inner + first + 1, second - first - 1);
    for_node->c_step = trim_dup_slice(inner + second + 1, inner_len - second - 1);
    if (for_node->c_init == NULL || for_node->c_cond == NULL || for_node->c_step == NULL) return -1;
    for_node->is_cstyle = true;
    return 0;
}

static int default_redir_fd(enum cupid_redir_kind kind) {
    switch (kind) {
        case CUPID_REDIR_IN:
        case CUPID_REDIR_HEREDOC:
        case CUPID_REDIR_HERESTRING:
        case CUPID_REDIR_DUP_IN:
        case CUPID_REDIR_INOUT:
            return 0;
        case CUPID_REDIR_ERR_OUT:
        case CUPID_REDIR_ERR_TO_OUT:
            return 2;
        case CUPID_REDIR_OUT:
        case CUPID_REDIR_APPEND:
        case CUPID_REDIR_CLOBBER:
        case CUPID_REDIR_DUP_OUT:
        default:
            return 1;
    }
}

static int node_push_redir(struct cupid_node *node, enum cupid_redir_kind kind, int fd,
                           const char *fd_var, const struct cupid_word *target,
                           int heredoc_strip_tabs) {
    struct cupid_redir *redirs = realloc(node->redirs, sizeof(*redirs) * (node->redir_count + 1));
    if (redirs == NULL) {
        return -1;
    }
    node->redirs = redirs;
    memset(&node->redirs[node->redir_count], 0, sizeof(node->redirs[node->redir_count]));
    node->redirs[node->redir_count].kind = kind;
    node->redirs[node->redir_count].fd = fd;
    if (fd_var != NULL) {
        node->redirs[node->redir_count].fd_var = strdup(fd_var);
        if (node->redirs[node->redir_count].fd_var == NULL) return -1;
    }
    node->redirs[node->redir_count].heredoc_strip_tabs = heredoc_strip_tabs ? true : false;
    if (target != NULL) {
        node->redirs[node->redir_count].has_target = true;
        node->redirs[node->redir_count].heredoc_quoted =
            target->had_quotes || target->had_escaped_chars;
        if (word_clone(&node->redirs[node->redir_count].target, target) != 0) {
            return -1;
        }
    }
    node->redir_count++;
    return 0;
}

static void skip_newlines(const struct cupid_tokens *tokens, size_t *idx) {
    while (*idx < tokens->count && tokens->items[*idx].kind == TOK_NEWLINE) {
        (*idx)++;
    }
}

static int parse_node(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node);
static int parse_list(const struct cupid_tokens *tokens, size_t *idx, struct cupid_list_ast *list);
static int parse_pipeline(const struct cupid_tokens *tokens, size_t *idx, struct cupid_pipeline_ast *pl, int *negate_status);

static int tokens_look_like_arith_command(const struct cupid_tokens *tokens, size_t idx) {
    size_t i = idx;
    int depth = 0;

    if (!tokens_start_arith(tokens, idx)) return 0;
    i += 2;

    while (i < tokens->count) {
        enum cupid_token_kind kind = tokens->items[i].kind;

        if (kind == TOK_SEMI || kind == TOK_NEWLINE || token_is_case_terminator(kind)) {
            return 0;
        }
        if (kind == TOK_LPAREN) {
            depth++;
            i++;
            continue;
        }
        if (kind == TOK_RPAREN) {
            if (depth == 0) {
                if (i + 1 < tokens->count && tokens->items[i + 1].kind == TOK_RPAREN) {
                    return 1;
                }
                return 0;
            }
            depth--;
            i++;
            continue;
        }
        i++;
    }

    return 0;
}

static int parse_trailing_redirs(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    while (*idx < tokens->count) {
        const struct cupid_token *rtok = &tokens->items[*idx];
        if (rtok->kind == TOK_WORD &&
            *idx + 1 < tokens->count &&
            token_is_redir(tokens->items[*idx + 1].kind)) {
            char *fd_var = NULL;
            int vword = parse_varredir_word(rtok, &fd_var);
            if (vword < 0) return -1;
            if (vword > 0) {
                const struct cupid_token *redir_tok = &tokens->items[*idx + 1];
                enum cupid_redir_kind rk = map_redir(redir_tok->kind);
                int fd = (redir_tok->redir_fd >= 0) ? redir_tok->redir_fd : default_redir_fd(rk);
                int strip_tabs = (redir_tok->kind == TOK_HEREDOC_STRIP) ? 1 : 0;
                if (g_posix_mode && rk == CUPID_REDIR_HERESTRING) {
                    free(fd_var);
                    return -1;
                }
                (*idx) += 2;
                if (rk == CUPID_REDIR_ERR_TO_OUT) {
                    if (node_push_redir(node, rk, fd, fd_var, NULL, 0) != 0) {
                        free(fd_var);
                        return -1;
                    }
                    free(fd_var);
                    continue;
                }
                if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) {
                    free(fd_var);
                    return -1;
                }
                if (node_push_redir(node, rk, fd, fd_var, &tokens->items[*idx].word, strip_tabs) != 0) {
                    free(fd_var);
                    return -1;
                }
                free(fd_var);
                (*idx)++;
                continue;
            }
        }

        if (!token_is_redir(rtok->kind)) break;
        {
            enum cupid_redir_kind rk = map_redir(rtok->kind);
            int fd = (rtok->redir_fd >= 0) ? rtok->redir_fd : default_redir_fd(rk);
            int strip_tabs = (rtok->kind == TOK_HEREDOC_STRIP) ? 1 : 0;
            if (g_posix_mode && rk == CUPID_REDIR_HERESTRING) return -1;
            (*idx)++;
            if (rk == CUPID_REDIR_ERR_TO_OUT) {
                if (node_push_redir(node, rk, fd, NULL, NULL, 0) != 0) return -1;
                continue;
            }
            if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
            if (node_push_redir(node, rk, fd, NULL, &tokens->items[*idx].word, strip_tabs) != 0) return -1;
            (*idx)++;
        }
    }
    return 0;
}

static int parse_if(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    struct cupid_if_node *cur;
    (*idx)++;
    node->kind = NODE_IF;
    memset(&node->if_clause, 0, sizeof(node->if_clause));

    node->if_clause.condition = calloc(1, sizeof(struct cupid_list_ast));
    node->if_clause.then_body = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->if_clause.condition || !node->if_clause.then_body) return -1;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->if_clause.condition) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "then")) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->if_clause.then_body) != 0) return -1;

    cur = &node->if_clause;
    while (*idx < tokens->count && is_keyword(&tokens->items[*idx], "elif")) {
        (*idx)++;
        cur->elif_next = calloc(1, sizeof(struct cupid_if_node));
        if (!cur->elif_next) return -1;
        cur = cur->elif_next;
        cur->condition = calloc(1, sizeof(struct cupid_list_ast));
        cur->then_body = calloc(1, sizeof(struct cupid_list_ast));
        if (!cur->condition || !cur->then_body) return -1;

        skip_newlines(tokens, idx);
        if (parse_list(tokens, idx, cur->condition) != 0) return -1;
        skip_newlines(tokens, idx);
        if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "then")) return -1;
        (*idx)++;
        skip_newlines(tokens, idx);
        if (parse_list(tokens, idx, cur->then_body) != 0) return -1;
    }

    if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "else")) {
        (*idx)++;
        skip_newlines(tokens, idx);
        cur->else_body = calloc(1, sizeof(struct cupid_list_ast));
        if (!cur->else_body) return -1;
        if (parse_list(tokens, idx, cur->else_body) != 0) return -1;
    }

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "fi")) return -1;
    (*idx)++;

    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_for(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    const char *varname;
    (*idx)++;
    node->kind = NODE_FOR;
    memset(&node->for_clause, 0, sizeof(node->for_clause));

    if (tokens_start_arith(tokens, *idx)) {
        char *expr = NULL;
        if (parse_arith_expr_tokens(tokens, idx, &expr) != 0) return -1;
        if (parse_cstyle_for_fields(expr, strlen(expr), &node->for_clause) != 0) {
            free(expr);
            return -1;
        }
        free(expr);
        skip_newlines(tokens, idx);
        if (*idx < tokens->count &&
            (tokens->items[*idx].kind == TOK_SEMI || tokens->items[*idx].kind == TOK_NEWLINE)) {
            (*idx)++;
        }
        skip_newlines(tokens, idx);
        node->for_clause.body = calloc(1, sizeof(struct cupid_list_ast));
        if (!node->for_clause.body) return -1;
        if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "do")) {
            (*idx)++;
            skip_newlines(tokens, idx);
            if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
            skip_newlines(tokens, idx);
            if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "done")) return -1;
            (*idx)++;
        } else if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "{")) {
            (*idx)++;
            skip_newlines(tokens, idx);
            if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
            skip_newlines(tokens, idx);
            if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "}")) return -1;
            (*idx)++;
        } else {
            return -1;
        }
        return parse_trailing_redirs(tokens, idx, node);
    }

    if (*idx < tokens->count && tokens->items[*idx].kind == TOK_WORD) {
        const char *inner = NULL;
        size_t inner_len = 0;
        if (word_is_arith_expr(&tokens->items[*idx], &inner, &inner_len)) {
            (*idx)++;
            if (parse_cstyle_for_fields(inner, inner_len, &node->for_clause) != 0) return -1;
            skip_newlines(tokens, idx);
            if (*idx < tokens->count &&
                (tokens->items[*idx].kind == TOK_SEMI || tokens->items[*idx].kind == TOK_NEWLINE)) {
                (*idx)++;
            }
            skip_newlines(tokens, idx);
            node->for_clause.body = calloc(1, sizeof(struct cupid_list_ast));
            if (!node->for_clause.body) return -1;
            if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "do")) {
                (*idx)++;
                skip_newlines(tokens, idx);
                if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
                skip_newlines(tokens, idx);
                if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "done")) return -1;
                (*idx)++;
            } else if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "{")) {
                (*idx)++;
                skip_newlines(tokens, idx);
                if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
                skip_newlines(tokens, idx);
                if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "}")) return -1;
                (*idx)++;
            } else {
                return -1;
            }
            return parse_trailing_redirs(tokens, idx, node);
        }
    }

    if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
    varname = word_text(&tokens->items[*idx]);
    if (!varname) return -1;
    node->for_clause.varname = strdup(varname);
    if (!node->for_clause.varname) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);

    if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "in")) {
        (*idx)++;
        node->for_clause.has_wordlist = true;
        while (*idx < tokens->count && tokens->items[*idx].kind == TOK_WORD &&
               !is_keyword(&tokens->items[*idx], "do")) {
            struct cupid_word *words = realloc(node->for_clause.words,
                sizeof(*words) * (node->for_clause.word_count + 1));
            if (!words) return -1;
            node->for_clause.words = words;
            memset(&node->for_clause.words[node->for_clause.word_count], 0, sizeof(struct cupid_word));
            if (word_clone(&node->for_clause.words[node->for_clause.word_count],
                           &tokens->items[*idx].word) != 0) return -1;
            node->for_clause.word_count++;
            (*idx)++;
        }
        if (*idx < tokens->count && (tokens->items[*idx].kind == TOK_SEMI ||
                                      tokens->items[*idx].kind == TOK_NEWLINE)) {
            (*idx)++;
        }
    }

    skip_newlines(tokens, idx);
    node->for_clause.body = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->for_clause.body) return -1;
    if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "do")) {
        (*idx)++;
        skip_newlines(tokens, idx);
        if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
        skip_newlines(tokens, idx);
        if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "done")) return -1;
        (*idx)++;
    } else if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "{")) {
        (*idx)++;
        skip_newlines(tokens, idx);
        if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;
        skip_newlines(tokens, idx);
        if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "}")) return -1;
        (*idx)++;
    } else {
        return -1;
    }

    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_select(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    const char *varname;

    (*idx)++;
    node->kind = NODE_FOR;
    memset(&node->for_clause, 0, sizeof(node->for_clause));
    node->for_clause.is_select = true;

    if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
    varname = word_text(&tokens->items[*idx]);
    if (!varname) return -1;
    node->for_clause.varname = strdup(varname);
    if (!node->for_clause.varname) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);
    if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "in")) {
        (*idx)++;
        node->for_clause.has_wordlist = true;
        while (*idx < tokens->count && tokens->items[*idx].kind == TOK_WORD &&
               !is_keyword(&tokens->items[*idx], "do")) {
            struct cupid_word *words = realloc(node->for_clause.words,
                sizeof(*words) * (node->for_clause.word_count + 1));
            if (!words) return -1;
            node->for_clause.words = words;
            memset(&node->for_clause.words[node->for_clause.word_count], 0, sizeof(struct cupid_word));
            if (word_clone(&node->for_clause.words[node->for_clause.word_count],
                           &tokens->items[*idx].word) != 0) return -1;
            node->for_clause.word_count++;
            (*idx)++;
        }
        if (*idx < tokens->count && (tokens->items[*idx].kind == TOK_SEMI ||
                                      tokens->items[*idx].kind == TOK_NEWLINE)) {
            (*idx)++;
        }
    }

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "do")) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);
    node->for_clause.body = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->for_clause.body) return -1;
    if (parse_list(tokens, idx, node->for_clause.body) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "done")) return -1;
    (*idx)++;

    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_coproc(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    int negate_status = 0;

    (*idx)++;
    node->kind = NODE_COPROC;
    memset(&node->coproc, 0, sizeof(node->coproc));
    node->coproc.name = strdup("COPROC");
    if (node->coproc.name == NULL) return -1;

    if (*idx < tokens->count && tokens->items[*idx].kind == TOK_WORD &&
        *idx + 1 < tokens->count &&
        (is_keyword(&tokens->items[*idx + 1], "{") || tokens->items[*idx + 1].kind == TOK_LPAREN)) {
        const char *nm = word_text(&tokens->items[*idx]);
        char *new_name;
        if (nm != NULL) {
            new_name = strdup(nm);
            if (new_name == NULL) return -1;
            free(node->coproc.name);
            node->coproc.name = new_name;
            (*idx)++;
        }
    }

    node->coproc.pipeline = calloc(1, sizeof(struct cupid_pipeline_ast));
    if (node->coproc.pipeline == NULL) return -1;
    if (parse_pipeline(tokens, idx, node->coproc.pipeline, &negate_status) != 0) return -1;
    if (negate_status) return -1;
    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_while_until(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node, int is_until) {
    (*idx)++;
    node->kind = is_until ? NODE_UNTIL : NODE_WHILE;
    memset(&node->while_clause, 0, sizeof(node->while_clause));

    node->while_clause.condition = calloc(1, sizeof(struct cupid_list_ast));
    node->while_clause.body = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->while_clause.condition || !node->while_clause.body) return -1;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->while_clause.condition) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "do")) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->while_clause.body) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "done")) return -1;
    (*idx)++;

    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_case(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    (*idx)++;
    node->kind = NODE_CASE;
    memset(&node->case_clause, 0, sizeof(node->case_clause));

    if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
    if (word_clone(&node->case_clause.word, &tokens->items[*idx].word) != 0) return -1;
    (*idx)++;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "in")) return -1;
    (*idx)++;
    skip_newlines(tokens, idx);

    while (*idx < tokens->count && !is_keyword(&tokens->items[*idx], "esac")) {
        struct cupid_case_item *items;
        struct cupid_case_item item;
        memset(&item, 0, sizeof(item));

        if (*idx < tokens->count && tokens->items[*idx].kind == TOK_LPAREN) {
            (*idx)++;
        }

        while (*idx < tokens->count && tokens->items[*idx].kind == TOK_WORD) {
            struct cupid_word *pats = realloc(item.patterns, sizeof(*pats) * (item.pattern_count + 1));
            if (!pats) {
                size_t k;
                for (k = 0; k < item.pattern_count; k++) cupid_word_free(&item.patterns[k]);
                free(item.patterns);
                return -1;
            }
            item.patterns = pats;
            memset(&item.patterns[item.pattern_count], 0, sizeof(struct cupid_word));
            if (word_clone(&item.patterns[item.pattern_count], &tokens->items[*idx].word) != 0) return -1;
            item.pattern_count++;
            (*idx)++;
            if (*idx < tokens->count && tokens->items[*idx].kind == TOK_PIPE) {
                (*idx)++;
            } else {
                break;
            }
        }

        if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_RPAREN) {
            size_t k;
            for (k = 0; k < item.pattern_count; k++) cupid_word_free(&item.patterns[k]);
            free(item.patterns);
            return -1;
        }
        (*idx)++;

        skip_newlines(tokens, idx);
        item.body = calloc(1, sizeof(struct cupid_list_ast));
        if (!item.body) return -1;

        if (*idx < tokens->count && !token_is_case_terminator(tokens->items[*idx].kind) &&
            !is_keyword(&tokens->items[*idx], "esac")) {
            if (parse_list(tokens, idx, item.body) != 0) return -1;
        }

        if (*idx < tokens->count && token_is_case_terminator(tokens->items[*idx].kind)) {
            if (tokens->items[*idx].kind == TOK_CASE_FALLTHROUGH) {
                item.terminator = CUPID_CASE_FALLTHRU;
            } else if (tokens->items[*idx].kind == TOK_CASE_TESTNEXT) {
                item.terminator = CUPID_CASE_TEST_NEXT;
            } else {
                item.terminator = CUPID_CASE_BREAK;
            }
            (*idx)++;
        }
        skip_newlines(tokens, idx);

        items = realloc(node->case_clause.items, sizeof(*items) * (node->case_clause.item_count + 1));
        if (!items) return -1;
        node->case_clause.items = items;
        node->case_clause.items[node->case_clause.item_count] = item;
        node->case_clause.item_count++;
    }

      if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "esac")) return -1;
      (*idx)++;

      return parse_trailing_redirs(tokens, idx, node);
  }

static int cond_push_literal(struct cupid_cond_node *cond, const char *text) {
    struct cupid_word *words = realloc(cond->words,
        sizeof(*words) * (cond->word_count + 1));
    struct cupid_word_part *part;
    if (!words) return -1;
    cond->words = words;
    memset(&cond->words[cond->word_count], 0, sizeof(struct cupid_word));
    part = calloc(1, sizeof(*part));
    if (!part) return -1;
    part->text = strdup(text);
    if (!part->text) { free(part); return -1; }
    part->quote = CUPID_QUOTE_NONE;
    cond->words[cond->word_count].parts = part;
    cond->words[cond->word_count].part_count = 1;
    cond->words[cond->word_count].had_quotes = false;
    cond->words[cond->word_count].had_escaped_brace = false;
    cond->word_count++;
    return 0;
}

static int parse_cond_expr(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    (*idx)++;
    node->kind = NODE_COND_EXPR;
    memset(&node->cond_expr, 0, sizeof(node->cond_expr));

    while (*idx < tokens->count) {
        struct cupid_word *words;
        enum cupid_token_kind tk = tokens->items[*idx].kind;

        if (is_keyword(&tokens->items[*idx], "]]")) {
            (*idx)++;
            return parse_trailing_redirs(tokens, idx, node);
        }

        if (tk == TOK_AND_IF) {
            if (cond_push_literal(&node->cond_expr, "&&") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_OR_IF) {
            if (cond_push_literal(&node->cond_expr, "||") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_REDIR_IN) {
            if (cond_push_literal(&node->cond_expr, "<") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_REDIR_OUT) {
            if (cond_push_literal(&node->cond_expr, ">") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_LPAREN) {
            if (cond_push_literal(&node->cond_expr, "(") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_RPAREN) {
            if (cond_push_literal(&node->cond_expr, ")") != 0) return -1;
            (*idx)++;
            continue;
        }
        if (tk == TOK_PIPE) {
            if (cond_push_literal(&node->cond_expr, "|") != 0) return -1;
            (*idx)++;
            continue;
        }

        if (tk != TOK_WORD) return -1;
        words = realloc(node->cond_expr.words,
            sizeof(*words) * (node->cond_expr.word_count + 1));
        if (!words) return -1;
        node->cond_expr.words = words;
        memset(&node->cond_expr.words[node->cond_expr.word_count], 0, sizeof(struct cupid_word));
        if (word_clone(&node->cond_expr.words[node->cond_expr.word_count],
                       &tokens->items[*idx].word) != 0) return -1;
        node->cond_expr.word_count++;
        (*idx)++;
    }
    return -1;
}

static int parse_brace_group(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    (*idx)++;
    node->kind = NODE_BRACE_GROUP;
    node->brace_group = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->brace_group) return -1;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->brace_group) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || !is_keyword(&tokens->items[*idx], "}")) return -1;
    (*idx)++;

    return parse_trailing_redirs(tokens, idx, node);
}

static int parse_subshell(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    (*idx)++;
    node->kind = NODE_SUBSHELL;
    node->subshell = calloc(1, sizeof(struct cupid_list_ast));
    if (!node->subshell) return -1;

    skip_newlines(tokens, idx);
    if (parse_list(tokens, idx, node->subshell) != 0) return -1;

    skip_newlines(tokens, idx);
    if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_RPAREN) return -1;
    (*idx)++;

    return parse_trailing_redirs(tokens, idx, node);
}

static int is_compound_start(const struct cupid_tokens *tokens, size_t idx) {
    if (idx >= tokens->count) return 0;
    if (tokens->items[idx].kind == TOK_LPAREN) return 1;
    if (tokens->items[idx].kind != TOK_WORD) return 0;
    return is_keyword(&tokens->items[idx], "if") ||
           is_keyword(&tokens->items[idx], "for") ||
           is_keyword(&tokens->items[idx], "select") ||
           is_keyword(&tokens->items[idx], "while") ||
           is_keyword(&tokens->items[idx], "until") ||
           is_keyword(&tokens->items[idx], "case") ||
           is_keyword(&tokens->items[idx], "coproc") ||
           is_keyword(&tokens->items[idx], "{") ||
           is_keyword(&tokens->items[idx], "[[") ||
           is_keyword(&tokens->items[idx], "function");
}

static int is_terminator_word(const struct cupid_tokens *tokens, size_t idx) {
    if (idx >= tokens->count) return 0;
    if (tokens->items[idx].kind != TOK_WORD) return 0;
    return is_keyword(&tokens->items[idx], "then") ||
           is_keyword(&tokens->items[idx], "elif") ||
           is_keyword(&tokens->items[idx], "else") ||
           is_keyword(&tokens->items[idx], "fi") ||
           is_keyword(&tokens->items[idx], "do") ||
           is_keyword(&tokens->items[idx], "done") ||
           is_keyword(&tokens->items[idx], "esac") ||
           is_keyword(&tokens->items[idx], "]]") ||
           is_keyword(&tokens->items[idx], "}");
}

static int parse_simple_command(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    node->kind = NODE_SIMPLE_CMD;
    memset(&node->simple_cmd, 0, sizeof(node->simple_cmd));

    while (*idx < tokens->count &&
           tokens->items[*idx].kind != TOK_PIPE &&
           !token_is_separator(tokens->items[*idx].kind) &&
           tokens->items[*idx].kind != TOK_RPAREN &&
           !token_is_case_terminator(tokens->items[*idx].kind)) {
        const struct cupid_token *tok = &tokens->items[*idx];

        if (node->simple_cmd.argc == 0 && is_terminator_word(tokens, *idx)) break;

        if (tok->kind == TOK_WORD) {
            if (*idx + 1 < tokens->count && token_is_redir(tokens->items[*idx + 1].kind)) {
                char *fd_var = NULL;
                int vword = parse_varredir_word(tok, &fd_var);
                if (vword < 0) return -1;
                if (vword > 0) {
                    const struct cupid_token *rtok = &tokens->items[*idx + 1];
                    enum cupid_redir_kind rk = map_redir(rtok->kind);
                    int fd = (rtok->redir_fd >= 0) ? rtok->redir_fd : default_redir_fd(rk);
                    int strip_tabs = (rtok->kind == TOK_HEREDOC_STRIP) ? 1 : 0;
                    if (g_posix_mode && rk == CUPID_REDIR_HERESTRING) {
                        free(fd_var);
                        return -1;
                    }
                    (*idx) += 2;
                    if (rk == CUPID_REDIR_ERR_TO_OUT) {
                        if (node_push_redir(node, rk, fd, fd_var, NULL, 0) != 0) {
                            free(fd_var);
                            return -1;
                        }
                        free(fd_var);
                        continue;
                    }
                    if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) {
                        free(fd_var);
                        return -1;
                    }
                    if (node_push_redir(node, rk, fd, fd_var, &tokens->items[*idx].word, strip_tabs) != 0) {
                        free(fd_var);
                        return -1;
                    }
                    free(fd_var);
                    (*idx)++;
                    continue;
                }
            }
            if (command_push_argv(&node->simple_cmd, &tok->word) != 0) {
                return -1;
            }
            (*idx)++;
            continue;
        }
        if (token_is_redir(tok->kind)) {
            enum cupid_redir_kind rk = map_redir(tok->kind);
            int fd = (tok->redir_fd >= 0) ? tok->redir_fd : default_redir_fd(rk);
            int strip_tabs = (tok->kind == TOK_HEREDOC_STRIP) ? 1 : 0;
            if (g_posix_mode && rk == CUPID_REDIR_HERESTRING) return -1;
            (*idx)++;
            if (rk == CUPID_REDIR_ERR_TO_OUT) {
                if (node_push_redir(node, rk, fd, NULL, NULL, 0) != 0) return -1;
                continue;
            }
            if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
            if (node_push_redir(node, rk, fd, NULL, &tokens->items[*idx].word, strip_tabs) != 0) return -1;
            (*idx)++;
            continue;
        }
        return -1;
    }
    if (node->simple_cmd.argc == 0 && node->redir_count == 0) {
        return -1;
    }
    return 0;
}

static int parse_function_def(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    const char *name;

    if (g_posix_mode && is_keyword(&tokens->items[*idx], "function")) {
        return -1;
    }

    node->kind = NODE_FUNCTION_DEF;
    memset(&node->func_def, 0, sizeof(node->func_def));

    if (is_keyword(&tokens->items[*idx], "function")) {
        (*idx)++;
        if (*idx >= tokens->count || tokens->items[*idx].kind != TOK_WORD) return -1;
    }

    name = word_text(&tokens->items[*idx]);
    if (name == NULL) return -1;
    node->func_def.name = strdup(name);
    if (node->func_def.name == NULL) return -1;
    (*idx)++;

    if (*idx + 1 < tokens->count &&
        tokens->items[*idx].kind == TOK_LPAREN &&
        tokens->items[*idx + 1].kind == TOK_RPAREN) {
        (*idx) += 2;
    }

    skip_newlines(tokens, idx);

    node->func_def.body = calloc(1, sizeof(struct cupid_node));
    if (node->func_def.body == NULL) {
        free(node->func_def.name);
        node->func_def.name = NULL;
        return -1;
    }
    memset(node->func_def.body, 0, sizeof(struct cupid_node));

    if (parse_node(tokens, idx, node->func_def.body) != 0 ||
        node->func_def.body->kind == NODE_SIMPLE_CMD) {
        free(node->func_def.body);
        node->func_def.body = NULL;
        free(node->func_def.name);
        node->func_def.name = NULL;
        return -1;
    }

    if (node->func_def.body->redir_count > 0) {
        node->redirs = node->func_def.body->redirs;
        node->redir_count = node->func_def.body->redir_count;
        node->func_def.body->redirs = NULL;
        node->func_def.body->redir_count = 0;
    }

    return 0;
}

static int parse_node(const struct cupid_tokens *tokens, size_t *idx, struct cupid_node *node) {
    if (*idx >= tokens->count) return -1;

    if (tokens_look_like_arith_command(tokens, *idx)) {
        char *expr = NULL;
        if (parse_arith_expr_tokens(tokens, idx, &expr) != 0) return -1;
        node->kind = NODE_ARITH_CMD;
        node->arith_cmd.expr = expr;
        return parse_trailing_redirs(tokens, idx, node);
    }

    if (*idx < tokens->count && word_is_arith_expr(&tokens->items[*idx], NULL, NULL)) {
        const char *inner = NULL;
        size_t inner_len = 0;
        char *expr;
        if (!word_is_arith_expr(&tokens->items[*idx], &inner, &inner_len)) return -1;
        expr = trim_dup_slice(inner, inner_len);
        if (expr == NULL) return -1;
        node->kind = NODE_ARITH_CMD;
        node->arith_cmd.expr = expr;
        (*idx)++;
        return parse_trailing_redirs(tokens, idx, node);
    }

    if (g_posix_mode &&
        (is_keyword(&tokens->items[*idx], "[[") ||
         is_keyword(&tokens->items[*idx], "select") ||
         is_keyword(&tokens->items[*idx], "coproc"))) {
        return parse_simple_command(tokens, idx, node);
    }

    if (tokens->items[*idx].kind == TOK_WORD &&
        !is_terminator_word(tokens, *idx) &&
        *idx + 2 < tokens->count &&
        tokens->items[*idx + 1].kind == TOK_LPAREN &&
        tokens->items[*idx + 2].kind == TOK_RPAREN) {
        return parse_function_def(tokens, idx, node);
    }

    if (is_compound_start(tokens, *idx)) {
        if (tokens->items[*idx].kind == TOK_LPAREN) {
            return parse_subshell(tokens, idx, node);
        }
        if (g_posix_mode && is_keyword(&tokens->items[*idx], "function")) return -1;
        if (is_keyword(&tokens->items[*idx], "function")) return parse_function_def(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "[[")) return parse_cond_expr(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "if")) return parse_if(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "for")) return parse_for(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "select")) return parse_select(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "while")) return parse_while_until(tokens, idx, node, 0);
        if (is_keyword(&tokens->items[*idx], "until")) return parse_while_until(tokens, idx, node, 1);
        if (is_keyword(&tokens->items[*idx], "case")) return parse_case(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "coproc")) return parse_coproc(tokens, idx, node);
        if (is_keyword(&tokens->items[*idx], "{")) return parse_brace_group(tokens, idx, node);
    }

    return parse_simple_command(tokens, idx, node);
}

static int set_word_literal(struct cupid_word *word, const char *text) {
    memset(word, 0, sizeof(*word));
    word->parts = calloc(1, sizeof(*word->parts));
    if (word->parts == NULL) return -1;
    word->parts[0].text = strdup(text);
    if (word->parts[0].text == NULL) {
        free(word->parts);
        word->parts = NULL;
        return -1;
    }
    word->parts[0].quote = CUPID_QUOTE_NONE;
    word->part_count = 1;
    word->had_quotes = false;
    word->had_escaped_brace = false;
    return 0;
}

static int pipeline_set_null_command(struct cupid_pipeline_ast *pl) {
    struct cupid_node *commands = calloc(1, sizeof(*commands));
    if (commands == NULL) return -1;
    memset(commands, 0, sizeof(*commands));
    commands[0].kind = NODE_SIMPLE_CMD;
    commands[0].simple_cmd.argv = calloc(1, sizeof(*commands[0].simple_cmd.argv));
    if (commands[0].simple_cmd.argv == NULL) {
        free(commands);
        return -1;
    }
    if (set_word_literal(&commands[0].simple_cmd.argv[0], ":") != 0) {
        free(commands[0].simple_cmd.argv);
        free(commands);
        return -1;
    }
    commands[0].simple_cmd.argc = 1;
    pl->commands = commands;
    pl->count = 1;
    return 0;
}

static int parse_pipeline(const struct cupid_tokens *tokens, size_t *idx, struct cupid_pipeline_ast *pl, int *negate_status) {
    if (*idx < tokens->count && token_is_reserved_bang(&tokens->items[*idx])) {
        *negate_status = 1;
        (*idx)++;
    } else {
        *negate_status = 0;
    }
    skip_newlines(tokens, idx);
    while (*idx < tokens->count && !token_is_separator(tokens->items[*idx].kind) &&
           tokens->items[*idx].kind != TOK_RPAREN &&
           !token_is_case_terminator(tokens->items[*idx].kind) &&
           !is_terminator_word(tokens, *idx)) {
        struct cupid_node node;
        struct cupid_node *commands;

        memset(&node, 0, sizeof(node));

        if (tokens->items[*idx].kind == TOK_PIPE) {
            return -1;
        }

        if (parse_node(tokens, idx, &node) != 0) {
            cupid_node_free(&node);
            return -1;
        }
        commands = realloc(pl->commands, sizeof(*commands) * (pl->count + 1));
        if (commands == NULL) {
            cupid_node_free(&node);
            return -1;
        }
        pl->commands = commands;
        pl->commands[pl->count] = node;
        pl->count++;

        if (*idx < tokens->count && tokens->items[*idx].kind == TOK_PIPE) {
            (*idx)++;
            skip_newlines(tokens, idx);
            if (*idx >= tokens->count || token_is_separator(tokens->items[*idx].kind) ||
                tokens->items[*idx].kind == TOK_PIPE) {
                return -1;
            }
        }
    }

    if (pl->count == 0) {
        return -1;
    }
    return 0;
}

static int parse_list(const struct cupid_tokens *tokens, size_t *idx, struct cupid_list_ast *list) {
    enum cupid_chain_join join = CUPID_CHAIN_NONE;

    skip_newlines(tokens, idx);

    while (*idx < tokens->count &&
           tokens->items[*idx].kind != TOK_RPAREN &&
           !token_is_case_terminator(tokens->items[*idx].kind) &&
           !is_terminator_word(tokens, *idx)) {

        struct cupid_pipeline_ast pl = {0};
        struct cupid_pipeline_item *items;
        int negate_status = 0;
        int timed = 0;
        int time_posix = 0;
        int timed_null = 0;

        if (token_is_separator(tokens->items[*idx].kind) ||
            tokens->items[*idx].kind == TOK_PIPE) {
            break;
        }

        if (!g_posix_mode && is_keyword(&tokens->items[*idx], "time")) {
            timed = 1;
            (*idx)++;
            if (*idx < tokens->count && is_keyword(&tokens->items[*idx], "-p")) {
                time_posix = 1;
                (*idx)++;
            }
            skip_newlines(tokens, idx);
            if (*idx >= tokens->count ||
                token_is_separator(tokens->items[*idx].kind) ||
                tokens->items[*idx].kind == TOK_RPAREN ||
                token_is_case_terminator(tokens->items[*idx].kind) ||
                is_terminator_word(tokens, *idx)) {
                timed_null = 1;
            } else if (tokens->items[*idx].kind == TOK_PIPE) {
                return -1;
            }
        }

        if (timed_null) {
            if (pipeline_set_null_command(&pl) != 0) {
                return -1;
            }
        } else if (parse_pipeline(tokens, idx, &pl, &negate_status) != 0) {
            size_t k;
            for (k = 0; k < pl.count; k++) cupid_node_free(&pl.commands[k]);
            free(pl.commands);
            return -1;
        }

        items = realloc(list->items, sizeof(*items) * (list->count + 1));
        if (items == NULL) {
            size_t k;
            for (k = 0; k < pl.count; k++) cupid_node_free(&pl.commands[k]);
            free(pl.commands);
            return -1;
        }
        list->items = items;
        list->items[list->count].pipeline = pl;
        list->items[list->count].join_from_prev = join;
        list->items[list->count].negate_status = negate_status ? true : false;
        list->items[list->count].timed = timed ? true : false;
        list->items[list->count].time_posix = time_posix ? true : false;
        list->items[list->count].background = false;
        list->count++;

        if (*idx < tokens->count && token_is_separator(tokens->items[*idx].kind)) {
            enum cupid_token_kind sep = tokens->items[*idx].kind;
            if (sep == TOK_AMP) {
                list->items[list->count - 1].background = true;
                join = CUPID_CHAIN_SEQ;
            } else if (sep == TOK_AND_IF) {
                join = CUPID_CHAIN_AND_IF;
            } else if (sep == TOK_OR_IF) {
                join = CUPID_CHAIN_OR_IF;
            } else {
                join = CUPID_CHAIN_SEQ;
            }
            (*idx)++;
            skip_newlines(tokens, idx);

            if (*idx >= tokens->count ||
                is_terminator_word(tokens, *idx) ||
                tokens->items[*idx].kind == TOK_RPAREN ||
                token_is_case_terminator(tokens->items[*idx].kind)) {
                if (sep == TOK_SEMI || sep == TOK_NEWLINE || sep == TOK_AMP) {
                    break;
                }
                return -1;
            }
        } else {
            break;
        }
    }

    if (list->count == 0) {
        return -1;
    }
    return 0;
}

int cupid_parse(const struct cupid_tokens *tokens, const struct cupid_parse_opts *opts, struct cupid_ast **out_ast) {
    struct cupid_ast *ast;
    size_t idx = 0;

    if (tokens == NULL || out_ast == NULL || tokens->count == 0) {
        return -1;
    }
    g_posix_mode = (opts != NULL && opts->posix_mode) ? 1 : 0;

    skip_newlines(tokens, &idx);
    if (idx >= tokens->count) {
        return -1;
    }

    ast = calloc(1, sizeof(*ast));
    if (ast == NULL) {
        return -1;
    }
    ast->kind = AST_LIST;

    if (parse_list(tokens, &idx, &ast->list) != 0) {
        cupid_ast_free(ast);
        return -1;
    }

    skip_newlines(tokens, &idx);
    if (idx != tokens->count) {
        cupid_ast_free(ast);
        return -1;
    }

    *out_ast = ast;
    return 0;
}
