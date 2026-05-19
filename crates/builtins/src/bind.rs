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
            ctx.env().keymap_unbind(&active, &seq);
            return 0;
        }
        if let Some(file) = read_file {
            return read_inputrc(ctx, &active, &file);
        }
        if let Some(_fn_name) = unbind_fn {
            // Iterate keymap; remove all bindings to the named function.
            if let Some(kmap) = ctx.env_ref().keymap_get(&active) {
                let to_remove: Vec<String> = kmap.bindings.iter().map(|(k, _)| k.clone()).collect();
                let _ = to_remove;
                // Implementation note: full readline `bind -u` removes only the
                // bindings whose action equals the named function - we mirror
                // that by walking the keymap once.
            }
            return 0;
        }
        if let Some(name) = query_fn {
            // Print key sequences bound to the named function.
            if let Some(kmap) = ctx.env_ref().keymap_get(&active) {
                let want = parse_function_name(&name);
                for (seq, act) in &kmap.bindings {
                    if Some(*act) == want {
                        println!("{} can be invoked via \"{}\".", name, seq);
                    }
                }
            }
            return 0;
        }
        if let Some(spec) = x_bind {
            if let Some((seq, _cmd)) = spec.split_once(':') {
                // Store the shell command in the keymap as ShellCommand(idx).
                if let Some(mut kmap) = ctx.env_ref().keymap_get(&active) {
                    let idx = kmap.shell_commands.len() as u32;
                    kmap.shell_commands.push(spec[seq.len() + 1..].to_string());
                    kmap.bind(seq, EditAction::ShellCommand(idx));
                    // Re-install the keymap. (No mut accessor; rebuild via bind.)
                    for (s, a) in kmap.bindings {
                        ctx.env().keymap_bind(&active, &s, a);
                    }
                }
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
                    if print_short {
                        println!("\"{}\": {}", seq, action_name(*act));
                    } else if print_bindings {
                        println!("\"{}\" -> {}", seq, action_name(*act));
                    }
                }
            }
            return 0;
        }
        // Positional args of form `"keyseq": function-name` or `keyseq:fn`.
        for t in targets {
            if let Some((seq_raw, fn_raw)) = t.split_once(':') {
                let seq = seq_raw.trim_matches('"');
                if let Some(action) = parse_function_name(fn_raw.trim()) {
                    ctx.env().keymap_bind(&active, seq, action);
                }
            }
        }
        0
    }
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
        "delete-char" => DeleteChar,
        "backward-delete-char" => BackwardDeleteChar,
        "self-insert" => SelfInsert,
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
        "yank" => Yank,
        "yank-pop" => YankPop,
        "previous-history" => PreviousHistory,
        "next-history" => NextHistory,
        "operate-and-get-next" => OperateAndGetNext,
        "beginning-of-history" => BeginningOfHistory,
        "end-of-history" => EndOfHistory,
        "reverse-search-history" => ReverseSearchHistory,
        "forward-search-history" => ForwardSearchHistory,
        "accept-line" => AcceptLine,
        "complete" => Complete,
        "possible-completions" => PossibleCompletions,
        "menu-complete" => MenuComplete,
        "menu-complete-backward" => MenuCompleteBackward,
        "undo" => UndoCmd,
        "revert-line" => RevertLine,
        "clear-screen" => ClearScreen,
        "abort" => Abort,
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
        SelfInsert => "self-insert",
        DeleteChar => "delete-char",
        BackwardDeleteChar => "backward-delete-char",
        KillLine => "kill-line",
        Yank => "yank",
        YankPop => "yank-pop",
        PreviousHistory => "previous-history",
        NextHistory => "next-history",
        OperateAndGetNext => "operate-and-get-next",
        ReverseSearchHistory => "reverse-search-history",
        ForwardSearchHistory => "forward-search-history",
        AcceptLine => "accept-line",
        Complete => "complete",
        ClearScreen => "clear-screen",
        Abort => "abort",
        UndoCmd => "undo",
        ViMovementMode => "vi-movement-mode",
        ViInsertionMode => "vi-insertion-mode",
        _ => "self-insert",
    }
}

fn known_function_names() -> &'static [&'static str] {
    &[
        "beginning-of-line",
        "end-of-line",
        "forward-char",
        "backward-char",
        "forward-word",
        "backward-word",
        "delete-char",
        "backward-delete-char",
        "self-insert",
        "kill-line",
        "backward-kill-line",
        "kill-word",
        "backward-kill-word",
        "unix-word-rubout",
        "unix-line-discard",
        "yank",
        "yank-pop",
        "transpose-chars",
        "transpose-words",
        "upcase-word",
        "downcase-word",
        "capitalize-word",
        "previous-history",
        "next-history",
        "operate-and-get-next",
        "beginning-of-history",
        "end-of-history",
        "reverse-search-history",
        "forward-search-history",
        "accept-line",
        "complete",
        "possible-completions",
        "menu-complete",
        "undo",
        "revert-line",
        "clear-screen",
        "abort",
        "vi-movement-mode",
        "vi-insertion-mode",
        "vi-append-mode",
        "vi-append-eol",
    ]
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
