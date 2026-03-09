#ifndef CUPID_TOKEN_H
#define CUPID_TOKEN_H

#include <stdbool.h>
#include <stddef.h>

enum cupid_quote {
    CUPID_QUOTE_NONE = 0,
    CUPID_QUOTE_SINGLE = 1,
    CUPID_QUOTE_DOUBLE = 2,
    CUPID_QUOTE_ANSI_C = 3,
};

struct cupid_word_part {
    char *text;
    enum cupid_quote quote;
};

struct cupid_word {
    struct cupid_word_part *parts;
    size_t part_count;
    bool had_quotes;
    bool had_escaped_brace;
    bool had_escaped_chars;
};

enum cupid_token_kind {
    TOK_WORD = 0,
    TOK_PIPE,
    TOK_SEMI,
    TOK_AND_IF,
    TOK_OR_IF,
    TOK_REDIR_IN,
    TOK_REDIR_OUT,
    TOK_REDIR_APPEND,
    TOK_REDIR_CLOBBER,
    TOK_REDIR_INOUT,
    TOK_REDIR_DUP_OUT,
    TOK_REDIR_DUP_IN,
    TOK_REDIR_ERR_OUT,
    TOK_REDIR_ERR_TO_OUT,
    TOK_HEREDOC,
    TOK_HEREDOC_STRIP,
    TOK_HERESTRING,
    TOK_NEWLINE,
    TOK_LPAREN,
    TOK_RPAREN,
    TOK_DSEMI,
    TOK_CASE_FALLTHROUGH,
    TOK_CASE_TESTNEXT,
    TOK_AMP,
};

struct cupid_token {
    enum cupid_token_kind kind;
    int redir_fd;
    bool heredoc_strip_tabs;
    struct cupid_word word;
};

struct cupid_tokens {
    struct cupid_token *items;
    size_t count;
};

void cupid_word_free(struct cupid_word *word);
void cupid_tokens_free(struct cupid_tokens *tokens);

#endif
