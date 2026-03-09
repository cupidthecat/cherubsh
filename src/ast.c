#include "cupid/ast.h"

#include <stdlib.h>

static void command_free(struct cupid_command *cmd) {
    size_t i;
    for (i = 0; i < cmd->argc; i++) {
        cupid_word_free(&cmd->argv[i]);
    }
    free(cmd->argv);
    cmd->argv = NULL;
    cmd->argc = 0;
}

static void redirs_free(struct cupid_redir *redirs, size_t count) {
    size_t i;
    for (i = 0; i < count; i++) {
        free(redirs[i].fd_var);
        if (redirs[i].has_target) {
            cupid_word_free(&redirs[i].target);
        }
        free(redirs[i].heredoc_body);
    }
    free(redirs);
}

static void if_node_free(struct cupid_if_node *node) {
    if (node->condition) {
        cupid_list_ast_free(node->condition);
        free(node->condition);
    }
    if (node->then_body) {
        cupid_list_ast_free(node->then_body);
        free(node->then_body);
    }
    if (node->elif_next) {
        if_node_free(node->elif_next);
        free(node->elif_next);
    }
    if (node->else_body) {
        cupid_list_ast_free(node->else_body);
        free(node->else_body);
    }
}

static void for_node_free(struct cupid_for_node *node) {
    size_t i;
    free(node->varname);
    free(node->c_init);
    free(node->c_cond);
    free(node->c_step);
    for (i = 0; i < node->word_count; i++) {
        cupid_word_free(&node->words[i]);
    }
    free(node->words);
    if (node->body) {
        cupid_list_ast_free(node->body);
        free(node->body);
    }
}

static void while_node_free(struct cupid_while_node *node) {
    if (node->condition) {
        cupid_list_ast_free(node->condition);
        free(node->condition);
    }
    if (node->body) {
        cupid_list_ast_free(node->body);
        free(node->body);
    }
}

static void case_node_free(struct cupid_case_node *node) {
    size_t i, j;
    cupid_word_free(&node->word);
    for (i = 0; i < node->item_count; i++) {
        for (j = 0; j < node->items[i].pattern_count; j++) {
            cupid_word_free(&node->items[i].patterns[j]);
        }
        free(node->items[i].patterns);
        if (node->items[i].body) {
            cupid_list_ast_free(node->items[i].body);
            free(node->items[i].body);
        }
    }
    free(node->items);
}

static void function_node_free(struct cupid_function_node *node) {
    free(node->name);
    if (node->body) {
        cupid_node_free(node->body);
        free(node->body);
    }
}

static void coproc_node_free(struct cupid_coproc_node *node) {
    free(node->name);
    node->name = NULL;
    if (node->pipeline != NULL) {
        size_t i;
        for (i = 0; i < node->pipeline->count; i++) {
            cupid_node_free(&node->pipeline->commands[i]);
        }
        free(node->pipeline->commands);
        free(node->pipeline);
        node->pipeline = NULL;
    }
}

static void cond_node_free(struct cupid_cond_node *node) {
    size_t i;
    for (i = 0; i < node->word_count; i++) {
        cupid_word_free(&node->words[i]);
    }
    free(node->words);
    node->words = NULL;
    node->word_count = 0;
}

static void arith_node_free(struct cupid_arith_cmd_node *node) {
    free(node->expr);
    node->expr = NULL;
}

void cupid_node_free(struct cupid_node *node) {
    if (node == NULL) {
        return;
    }
    switch (node->kind) {
        case NODE_SIMPLE_CMD:
            command_free(&node->simple_cmd);
            break;
        case NODE_IF:
            if_node_free(&node->if_clause);
            break;
        case NODE_FOR:
            for_node_free(&node->for_clause);
            break;
        case NODE_WHILE:
        case NODE_UNTIL:
            while_node_free(&node->while_clause);
            break;
        case NODE_CASE:
            case_node_free(&node->case_clause);
            break;
        case NODE_BRACE_GROUP:
            if (node->brace_group) {
                cupid_list_ast_free(node->brace_group);
                free(node->brace_group);
            }
            break;
        case NODE_SUBSHELL:
            if (node->subshell) {
                cupid_list_ast_free(node->subshell);
                free(node->subshell);
            }
            break;
        case NODE_FUNCTION_DEF:
            function_node_free(&node->func_def);
            break;
        case NODE_COND_EXPR:
            cond_node_free(&node->cond_expr);
            break;
        case NODE_ARITH_CMD:
            arith_node_free(&node->arith_cmd);
            break;
        case NODE_COPROC:
            coproc_node_free(&node->coproc);
            break;
    }
    redirs_free(node->redirs, node->redir_count);
    node->redirs = NULL;
    node->redir_count = 0;
}

static void pipeline_free(struct cupid_pipeline_ast *pl) {
    size_t i;
    for (i = 0; i < pl->count; i++) {
        cupid_node_free(&pl->commands[i]);
    }
    free(pl->commands);
    pl->commands = NULL;
    pl->count = 0;
}

void cupid_list_ast_free(struct cupid_list_ast *list) {
    size_t i;
    if (list == NULL) {
        return;
    }
    for (i = 0; i < list->count; i++) {
        pipeline_free(&list->items[i].pipeline);
    }
    free(list->items);
    list->items = NULL;
    list->count = 0;
}

void cupid_ast_free(struct cupid_ast *ast) {
    if (ast == NULL) {
        return;
    }
    cupid_list_ast_free(&ast->list);
    free(ast);
}
