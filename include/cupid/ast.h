#ifndef CUPID_AST_H
#define CUPID_AST_H

#include <stdbool.h>
#include <stddef.h>

#include "cupid/token.h"

struct cupid_node;
struct cupid_list_ast;
struct cupid_pipeline_ast;

enum cupid_ast_kind {
    AST_LIST = 0,
};

enum cupid_redir_kind {
    CUPID_REDIR_IN = 0,
    CUPID_REDIR_OUT,
    CUPID_REDIR_APPEND,
    CUPID_REDIR_CLOBBER,
    CUPID_REDIR_INOUT,
    CUPID_REDIR_DUP_OUT,
    CUPID_REDIR_DUP_IN,
    CUPID_REDIR_ERR_OUT,
    CUPID_REDIR_ERR_TO_OUT,
    CUPID_REDIR_HEREDOC,
    CUPID_REDIR_HERESTRING,
};

struct cupid_redir {
    enum cupid_redir_kind kind;
    int fd;
    char *fd_var;
    bool has_target;
    struct cupid_word target;
    bool heredoc_quoted;
    bool heredoc_strip_tabs;
    char *heredoc_body;
};

struct cupid_command {
    struct cupid_word *argv;
    size_t argc;
};

struct cupid_cond_node {
    struct cupid_word *words;
    size_t word_count;
};

enum cupid_node_kind {
    NODE_SIMPLE_CMD = 0,
    NODE_IF,
    NODE_FOR,
    NODE_WHILE,
    NODE_UNTIL,
    NODE_CASE,
    NODE_BRACE_GROUP,
    NODE_SUBSHELL,
    NODE_FUNCTION_DEF,
    NODE_COND_EXPR,
    NODE_ARITH_CMD,
    NODE_COPROC,
};

struct cupid_if_node {
    struct cupid_list_ast *condition;
    struct cupid_list_ast *then_body;
    struct cupid_if_node *elif_next;
    struct cupid_list_ast *else_body;
};

struct cupid_for_node {
    char *varname;
    struct cupid_word *words;
    size_t word_count;
    bool has_wordlist;
    bool is_cstyle;
    bool is_select;
    char *c_init;
    char *c_cond;
    char *c_step;
    struct cupid_list_ast *body;
};

struct cupid_arith_cmd_node {
    char *expr;
};

struct cupid_while_node {
    struct cupid_list_ast *condition;
    struct cupid_list_ast *body;
};

struct cupid_case_item {
    struct cupid_word *patterns;
    size_t pattern_count;
    struct cupid_list_ast *body;
    enum {
        CUPID_CASE_BREAK = 0,
        CUPID_CASE_FALLTHRU,
        CUPID_CASE_TEST_NEXT,
    } terminator;
};

struct cupid_case_node {
    struct cupid_word word;
    struct cupid_case_item *items;
    size_t item_count;
};

struct cupid_function_node {
    char *name;
    struct cupid_node *body;
};

struct cupid_coproc_node {
    char *name;
    struct cupid_pipeline_ast *pipeline;
};

struct cupid_node {
    enum cupid_node_kind kind;
    union {
        struct cupid_command simple_cmd;
        struct cupid_if_node if_clause;
        struct cupid_for_node for_clause;
        struct cupid_while_node while_clause;
        struct cupid_case_node case_clause;
        struct cupid_list_ast *brace_group;
        struct cupid_list_ast *subshell;
        struct cupid_function_node func_def;
        struct cupid_cond_node cond_expr;
        struct cupid_arith_cmd_node arith_cmd;
        struct cupid_coproc_node coproc;
    };
    struct cupid_redir *redirs;
    size_t redir_count;
};

struct cupid_pipeline_ast {
    struct cupid_node *commands;
    size_t count;
};

enum cupid_chain_join {
    CUPID_CHAIN_NONE = 0,
    CUPID_CHAIN_SEQ,
    CUPID_CHAIN_AND_IF,
    CUPID_CHAIN_OR_IF,
};

struct cupid_pipeline_item {
    struct cupid_pipeline_ast pipeline;
    enum cupid_chain_join join_from_prev;
    bool negate_status;
    bool timed;
    bool time_posix;
    bool background;
};

struct cupid_list_ast {
    struct cupid_pipeline_item *items;
    size_t count;
};

struct cupid_ast {
    enum cupid_ast_kind kind;
    struct cupid_list_ast list;
};

void cupid_node_free(struct cupid_node *node);
void cupid_list_ast_free(struct cupid_list_ast *list);
void cupid_ast_free(struct cupid_ast *ast);

#endif
