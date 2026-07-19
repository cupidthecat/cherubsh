//! `bind` builtin - readline-equivalent keymap manipulation.

use cherubsh_common::keymap::EditAction;

use crate::{Builtin, BuiltinCtx};

pub struct Bind;
pub static BIND: Bind = Bind;
impl Builtin for Bind {
    fn name(&self) -> &'static str {
        "bind"
    }
    fn synopsis(&self) -> &'static str {
        "bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut keymap_name: Option<String> = None;
        let mut list_fns = false;
        let mut print_bindings = false;
        let mut print_short = false;
        let mut print_macros = false;
        let mut print_macros_short = false;
        let mut print_vars = false;
        let mut print_vars_short = false;
        let mut print_xbinds = false;
        let mut remove_seq: Option<String> = None;
        let mut unbind_fn: Option<String> = None;
        let mut read_file: Option<String> = None;
        let mut query_fn: Option<String> = None;
        let mut x_bind: Option<String> = None;
        let mut targets: Vec<String> = Vec::new();
        let mut i = 0;
        while i < ctx.args.len() {
            let a = ctx.args[i].clone();
            match a.as_str() {
                "-l" => list_fns = true,
                "-p" => print_bindings = true,
                "-P" => print_short = true,
                "-s" => print_macros = true,
                "-S" => print_macros_short = true,
                "-v" => print_vars = true,
                "-V" => print_vars_short = true,
                "-X" => print_xbinds = true,
                "-m" => {
                    i += 1;
                    keymap_name = ctx.args.get(i).cloned();
                }
                "-r" => {
                    i += 1;
                    remove_seq = ctx.args.get(i).cloned();
                }
                "-u" => {
                    i += 1;
                    unbind_fn = ctx.args.get(i).cloned();
                }
                "-f" => {
                    i += 1;
                    read_file = ctx.args.get(i).cloned();
                }
                "-q" => {
                    i += 1;
                    query_fn = ctx.args.get(i).cloned();
                }
                "-x" => {
                    i += 1;
                    x_bind = ctx.args.get(i).cloned();
                }
                _ if a.starts_with('-') => {
                    eprintln!("cherubsh: bind: invalid option {a}");
                    return 2;
                }
                _ => targets.push(a),
            }
            i += 1;
        }
        let active = keymap_name.unwrap_or_else(|| ctx.env_ref().keymap_active().to_string());

        if list_fns {
            for name in known_function_names() {
                println!("{}", name);
            }
            return 0;
        }
        if let Some(seq) = remove_seq {
            let seq = trim_keyseq_quotes(&seq);
            ctx.env().keymap_unbind(&active, &seq);
            return 0;
        }
        if let Some(file) = read_file {
            return read_inputrc(ctx, &active, &file);
        }
        if let Some(_fn_name) = unbind_fn {
            // Iterate keymap; remove all bindings to the named function.
            if let Some(kmap) = ctx.env_ref().keymap_get(&active) {
                let want = parse_function_name(_fn_name.as_str());
                let to_remove: Vec<String> = kmap
                    .bindings
                    .iter()
                    .filter_map(|(seq, action)| (Some(*action) == want).then_some(seq.clone()))
                    .collect();
                for seq in to_remove {
                    ctx.env().keymap_unbind(&active, &seq);
                }
            }
            return 0;
        }
        if let Some(name) = query_fn {
            // Print key sequences bound to the named function.
            let mut found = false;
            if let Some(kmap) = ctx.env_ref().keymap_get(&active) {
                let want = parse_function_name(&name);
                for (seq, act) in &kmap.bindings {
                    if Some(*act) == want {
                        println!("{} can be invoked via \"{}\".", name, seq);
                        found = true;
                    }
                }
            }
            if found {
                return 0;
            }
            println!("{name} is not bound to any keys.");
            return 1;
        }
        if let Some(spec) = x_bind {
            if let Some((seq, cmd)) = spec.split_once(':') {
                let seq = trim_keyseq_quotes(seq);
                ctx.env().keymap_bind_shell_command(&active, &seq, cmd);
            }
            return 0;
        }
        if print_bindings
            || print_short
            || print_macros
            || print_macros_short
            || print_vars
            || print_vars_short
            || print_xbinds
        {
            if let Some(kmap) = ctx.env_ref().keymap_get(&active) {
                for (seq, act) in &kmap.bindings {
                    match *act {
                        EditAction::ShellCommand(idx) => {
                            let Some(command) = kmap.shell_commands.get(idx as usize) else {
                                continue;
                            };
                            if print_xbinds {
                                println!("\"{seq}\" \"{command}\"");
                            }
                        }
                        EditAction::Macro(idx) => {
                            let Some(text) = kmap.macros.get(idx as usize) else {
                                continue;
                            };
                            if print_macros_short {
                                println!("{seq} outputs {text}");
                            } else if print_macros {
                                println!("\"{seq}\": \"{text}\"");
                            }
                        }
                        _ => {
                            if print_short {
                                println!("{} can be found on \"{}\".", action_name(*act), seq);
                            } else if print_bindings {
                                println!("\"{}\": {}", seq, action_name(*act));
                            }
                        }
                    }
                }
            }
            return 0;
        }
        // Positional args of form `"keyseq": function-name` or `keyseq:fn`.
        for t in targets {
            if let Some((seq_raw, fn_raw)) = t.split_once(':') {
                let seq = seq_raw.trim_matches('"');
                let rhs = fn_raw.trim();
                if let Some(action) = parse_function_name(rhs) {
                    ctx.env().keymap_bind(&active, seq, action);
                } else if let Some(text) = quoted_macro(rhs) {
                    ctx.env().keymap_bind_macro(&active, seq, text);
                }
            }
        }
        0
    }
}

fn quoted_macro(value: &str) -> Option<&str> {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
}

fn trim_keyseq_quotes(value: &str) -> String {
    value.trim_matches('"').to_string()
}

fn parse_function_name(name: &str) -> Option<EditAction> {
    use EditAction::*;
    Some(match name {
        "beginning-of-line" => BeginningOfLine,
        "end-of-line" => EndOfLine,
        "forward-char" => ForwardChar,
        "backward-char" => BackwardChar,
        "forward-word" => ForwardWord,
        "backward-word" => BackwardWord,
        "next-screen-line" => NextScreenLine,
        "previous-screen-line" => PreviousScreenLine,
        "delete-char" => DeleteChar,
        "delete-char-or-list" => DeleteCharOrList,
        "backward-delete-char" => BackwardDeleteChar,
        "self-insert" => SelfInsert,
        "quoted-insert" => QuotedInsert,
        "tab-insert" => TabInsert,
        "transpose-chars" => TransposeChars,
        "transpose-words" => TransposeWords,
        "upcase-word" => UpcaseWord,
        "downcase-word" => DowncaseWord,
        "capitalize-word" => CapitalizeWord,
        "kill-line" => KillLine,
        "backward-kill-line" => BackwardKillLine,
        "kill-word" => KillWord,
        "backward-kill-word" => BackwardKillWord,
        "unix-word-rubout" => UnixWordRubout,
        "unix-line-discard" => UnixLineDiscard,
        "kill-region" => KillRegion,
        "yank" => Yank,
        "yank-pop" => YankPop,
        "yank-last-arg" | "insert-last-argument" => YankLastArg,
        "yank-nth-arg" => YankNthArg,
        "previous-history" => PreviousHistory,
        "next-history" => NextHistory,
        "operate-and-get-next" => OperateAndGetNext,
        "beginning-of-history" => BeginningOfHistory,
        "end-of-history" => EndOfHistory,
        "reverse-search-history" => ReverseSearchHistory,
        "forward-search-history" => ForwardSearchHistory,
        "non-incremental-reverse-search-history" => NonIncrementalReverseSearchHistory,
        "non-incremental-forward-search-history" => NonIncrementalForwardSearchHistory,
        "history-search-forward" | "history-substring-search-forward" => HistorySearchForward,
        "history-search-backward" | "history-substring-search-backward" => HistorySearchBackward,
        "accept-line" => AcceptLine,
        "newline" => NewLine,
        "complete" => Complete,
        "possible-completions" => PossibleCompletions,
        "insert-completions" => InsertCompletions,
        "menu-complete" => MenuComplete,
        "menu-complete-backward" => MenuCompleteBackward,
        "undo" => UndoCmd,
        "revert-line" => RevertLine,
        "clear-screen" => ClearScreen,
        "redraw-current-line" => Redraw,
        "abort" => Abort,
        "tilde-expand" => Tilde,
        "vi-eof-maybe" => Quit,
        "vi-movement-mode" => ViMovementMode,
        "vi-insertion-mode" => ViInsertionMode,
        "vi-append-mode" => ViAppendMode,
        "vi-append-eol" => ViAppendEol,
        _ => return None,
    })
}

fn action_name(action: EditAction) -> &'static str {
    use EditAction::*;
    match action {
        BeginningOfLine => "beginning-of-line",
        EndOfLine => "end-of-line",
        ForwardChar => "forward-char",
        BackwardChar => "backward-char",
        ForwardWord => "forward-word",
        BackwardWord => "backward-word",
        NextScreenLine => "next-screen-line",
        PreviousScreenLine => "previous-screen-line",
        SelfInsert => "self-insert",
        DeleteChar => "delete-char",
        DeleteCharOrList => "delete-char-or-list",
        BackwardDeleteChar => "backward-delete-char",
        QuotedInsert => "quoted-insert",
        TabInsert => "tab-insert",
        TransposeChars => "transpose-chars",
        TransposeWords => "transpose-words",
        UpcaseWord => "upcase-word",
        DowncaseWord => "downcase-word",
        CapitalizeWord => "capitalize-word",
        KillLine => "kill-line",
        BackwardKillLine => "backward-kill-line",
        KillWord => "kill-word",
        BackwardKillWord => "backward-kill-word",
        UnixWordRubout => "unix-word-rubout",
        UnixLineDiscard => "unix-line-discard",
        KillRegion => "kill-region",
        Yank => "yank",
        YankPop => "yank-pop",
        YankLastArg => "yank-last-arg",
        YankNthArg => "yank-nth-arg",
        PreviousHistory => "previous-history",
        NextHistory => "next-history",
        OperateAndGetNext => "operate-and-get-next",
        BeginningOfHistory => "beginning-of-history",
        EndOfHistory => "end-of-history",
        ReverseSearchHistory => "reverse-search-history",
        ForwardSearchHistory => "forward-search-history",
        NonIncrementalReverseSearchHistory => "non-incremental-reverse-search-history",
        NonIncrementalForwardSearchHistory => "non-incremental-forward-search-history",
        HistorySearchForward => "history-search-forward",
        HistorySearchBackward => "history-search-backward",
        AcceptLine => "accept-line",
        NewLine => "newline",
        Complete => "complete",
        PossibleCompletions => "possible-completions",
        InsertCompletions => "insert-completions",
        MenuComplete => "menu-complete",
        MenuCompleteBackward => "menu-complete-backward",
        ClearScreen => "clear-screen",
        Redraw => "redraw-current-line",
        Abort => "abort",
        UndoCmd => "undo",
        RevertLine => "revert-line",
        Tilde => "tilde-expand",
        Quit => "vi-eof-maybe",
        ViMovementMode => "vi-movement-mode",
        ViInsertionMode => "vi-insertion-mode",
        ViAppendMode => "vi-append-mode",
        ViAppendEol => "vi-append-eol",
        Noop => "do-lowercase-version",
        ShellCommand(_) => "shell-command",
        Macro(_) => "keyboard-macro",
    }
}

pub fn known_function_names() -> &'static [&'static str] {
    &[
        "abort",
        "accept-line",
        "alias-expand-line",
        "arrow-key-prefix",
        "backward-byte",
        "backward-char",
        "backward-delete-char",
        "backward-kill-line",
        "backward-kill-word",
        "backward-word",
        "bash-vi-complete",
        "beginning-of-history",
        "beginning-of-line",
        "bracketed-paste-begin",
        "call-last-kbd-macro",
        "capitalize-word",
        "character-search",
        "character-search-backward",
        "clear-display",
        "clear-screen",
        "complete",
        "complete-command",
        "complete-filename",
        "complete-hostname",
        "complete-into-braces",
        "complete-username",
        "complete-variable",
        "copy-backward-word",
        "copy-forward-word",
        "copy-region-as-kill",
        "dabbrev-expand",
        "delete-char",
        "delete-char-or-list",
        "delete-horizontal-space",
        "digit-argument",
        "display-shell-version",
        "do-lowercase-version",
        "downcase-word",
        "dump-functions",
        "dump-macros",
        "dump-variables",
        "dynamic-complete-history",
        "edit-and-execute-command",
        "emacs-editing-mode",
        "end-kbd-macro",
        "end-of-history",
        "end-of-line",
        "exchange-point-and-mark",
        "execute-named-command",
        "export-completions",
        "fetch-history",
        "forward-backward-delete-char",
        "forward-byte",
        "forward-char",
        "forward-search-history",
        "forward-word",
        "glob-complete-word",
        "glob-expand-word",
        "glob-list-expansions",
        "history-and-alias-expand-line",
        "history-expand-line",
        "history-search-backward",
        "history-search-forward",
        "history-substring-search-backward",
        "history-substring-search-forward",
        "insert-comment",
        "insert-completions",
        "insert-last-argument",
        "kill-line",
        "kill-region",
        "kill-whole-line",
        "kill-word",
        "magic-space",
        "menu-complete",
        "menu-complete-backward",
        "next-history",
        "next-screen-line",
        "non-incremental-forward-search-history",
        "non-incremental-forward-search-history-again",
        "non-incremental-reverse-search-history",
        "non-incremental-reverse-search-history-again",
        "old-menu-complete",
        "operate-and-get-next",
        "overwrite-mode",
        "possible-command-completions",
        "possible-completions",
        "possible-filename-completions",
        "possible-hostname-completions",
        "possible-username-completions",
        "possible-variable-completions",
        "previous-history",
        "previous-screen-line",
        "print-last-kbd-macro",
        "quoted-insert",
        "re-read-init-file",
        "redraw-current-line",
        "reverse-search-history",
        "revert-line",
        "self-insert",
        "set-mark",
        "shell-backward-kill-word",
        "shell-backward-word",
        "shell-expand-line",
        "shell-forward-word",
        "shell-kill-word",
        "shell-transpose-words",
        "skip-csi-sequence",
        "spell-correct-word",
        "start-kbd-macro",
        "tab-insert",
        "tilde-expand",
        "transpose-chars",
        "transpose-words",
        "tty-status",
        "undo",
        "universal-argument",
        "unix-filename-rubout",
        "unix-line-discard",
        "unix-word-rubout",
        "upcase-word",
        "vi-append-eol",
        "vi-append-mode",
        "vi-arg-digit",
        "vi-bWord",
        "vi-back-to-indent",
        "vi-backward-bigword",
        "vi-backward-word",
        "vi-bword",
        "vi-change-case",
        "vi-change-char",
        "vi-change-to",
        "vi-char-search",
        "vi-column",
        "vi-complete",
        "vi-delete",
        "vi-delete-to",
        "vi-eWord",
        "vi-edit-and-execute-command",
        "vi-editing-mode",
        "vi-end-bigword",
        "vi-end-word",
        "vi-eof-maybe",
        "vi-eword",
        "vi-fWord",
        "vi-fetch-history",
        "vi-first-print",
        "vi-forward-bigword",
        "vi-forward-word",
        "vi-fword",
        "vi-goto-mark",
        "vi-insert-beg",
        "vi-insertion-mode",
        "vi-match",
        "vi-movement-mode",
        "vi-next-word",
        "vi-overstrike",
        "vi-overstrike-delete",
        "vi-prev-word",
        "vi-put",
        "vi-redo",
        "vi-replace",
        "vi-rubout",
        "vi-search",
        "vi-search-again",
        "vi-set-mark",
        "vi-subst",
        "vi-tilde-expand",
        "vi-undo",
        "vi-unix-word-rubout",
        "vi-yank-arg",
        "vi-yank-pop",
        "vi-yank-to",
        "yank",
        "yank-last-arg",
        "yank-nth-arg",
        "yank-pop",
    ]
}

pub fn bash_line_function(name: &str) -> bool {
    [
        "alias-expand-line",
        "bash-vi-complete",
        "complete-command",
        "complete-filename",
        "complete-hostname",
        "complete-into-braces",
        "complete-username",
        "complete-variable",
        "dabbrev-expand",
        "display-shell-version",
        "dynamic-complete-history",
        "edit-and-execute-command",
        "glob-complete-word",
        "glob-expand-word",
        "glob-list-expansions",
        "history-and-alias-expand-line",
        "history-expand-line",
        "insert-last-argument",
        "magic-space",
        "possible-command-completions",
        "possible-filename-completions",
        "possible-hostname-completions",
        "possible-username-completions",
        "possible-variable-completions",
        "shell-backward-kill-word",
        "shell-backward-word",
        "shell-expand-line",
        "shell-forward-word",
        "shell-kill-word",
        "shell-transpose-words",
        "spell-correct-word",
        "vi-edit-and-execute-command",
    ]
    .contains(&name)
}

fn read_inputrc(ctx: &mut BuiltinCtx<'_>, keymap: &str, path: &str) -> i32 {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cherubsh: bind: {path}: {e}");
            return 1;
        }
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((seq, fn_name)) = line.split_once(':') {
            let seq = seq.trim().trim_matches('"');
            if let Some(act) = parse_function_name(fn_name.trim()) {
                ctx.env().keymap_bind(keymap, seq, act);
            }
        }
    }
    0
}
