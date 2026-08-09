fn read_logical_command(state: &mut ShellState, exec_state: &mut ExecState) -> ShellResult<String> {
    if state.noexec
        && !state.interactive
        && !state.just_one_command
        && !(state.option("histexpand") && state.option("history"))
    {
        return read_noexec_input(state);
    }

    let mut command = String::new();
    loop {
        let line = if state.interactive && !state.input.is_string() && !state.input.is_stream() {
            match read_interactive_line(state, exec_state, if command.is_empty() { 1 } else { 2 })?
            {
                Some(line) => line,
                None => {
                    state.eof_reached = true;
                    return Ok(command);
                }
            }
        } else {
            match state.input.next_line() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    state.eof_reached = true;
                    return Ok(command);
                }
                Err(_) => return Err(ShellJump::ForceEof),
            }
        };
        state.current_command_line_count = state.current_command_line_count.saturating_add(1);

        if state.verbose_flag {
            use std::io::Write;
            let mut stderr = std::io::stderr();
            let _ = stderr.write_all(line.as_bytes());
            let _ = stderr.flush();
        }

        let in_heredoc_body =
            !command.is_empty() && has_unclosed_heredoc(&expand_aliases_for_parse(&command, state));
        let line = if in_heredoc_body {
            line
        } else if let Some(expanded) = history_expand_physical_line(state, &line)? {
            expanded
        } else {
            return Ok(String::new());
        };

        command.push_str(&line);
        if command.trim().is_empty() {
            return Ok(command);
        }
        if has_trailing_line_continuation(&command) {
            continue;
        }
        let parse_probe = expand_aliases_for_parse(&command, state);
        if has_open_quotes(&parse_probe, state.option("posix"))
            || has_unclosed_command_substitution(&parse_probe)
            || has_unclosed_heredoc(&parse_probe)
            || has_unclosed_compound_assignment(&parse_probe)
        {
            continue;
        }

        let comments_enabled = !state.interactive || state.option("interactive_comments");
        match parse_text(
            &parse_probe,
            state.option("extglob"),
            state.option("posix"),
            comments_enabled,
        ) {
            Ok(_) => return Ok(command),
            Err(err) if err.message == "empty input" => return Ok(command),
            Err(err) if parse_error_wants_more_input(&err, &parse_probe) => continue,
            Err(_) => return Ok(command),
        }
    }
}

fn read_noexec_input(state: &mut ShellState) -> ShellResult<String> {
    let mut input = String::new();
    loop {
        match state.input.next_line() {
            Ok(Some(line)) => {
                state.current_command_line_count =
                    state.current_command_line_count.saturating_add(1);
                if state.verbose_flag {
                    use std::io::Write;
                    let mut stderr = std::io::stderr();
                    let _ = stderr.write_all(line.as_bytes());
                    let _ = stderr.flush();
                }
                input.push_str(&line);
            }
            Ok(None) => {
                state.eof_reached = true;
                return Ok(input);
            }
            Err(_) => return Err(ShellJump::ForceEof),
        }
    }
}

fn history_expand_physical_line(state: &mut ShellState, line: &str) -> ShellResult<Option<String>> {
    if !(state.option("histexpand") && (state.interactive || state.option("history"))) {
        return Ok(Some(line.to_string()));
    }

    let res = histexpand::expand_with_options(line, &state.history_table, state.option("posix"));
    if let Some(err) = res.error {
        eprintln!(
            "{}: line {}: {err}",
            state.shell_name, state.current_command_line_count
        );
        return Ok(None);
    }
    if res.changed
        && !state
            .shopt_options
            .get("histverify")
            .copied()
            .unwrap_or(false)
    {
        eprintln!("{}", res.line.strip_suffix('\n').unwrap_or(&res.line));
    }
    if res.print_only {
        state.last_command_exit_value = 0;
        return Ok(None);
    }
    Ok(Some(res.line))
}

fn read_interactive_line(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    prompt_level: u8,
) -> ShellResult<Option<String>> {
    if state.no_line_editing {
        prompt_again(state, exec_state, prompt_level);
        return match state.input.next_line() {
            Ok(line) => Ok(line),
            Err(_) => Err(ShellJump::ForceEof),
        };
    }

    let key = if prompt_level == 2 { "PS2" } else { "PS1" };
    let raw_prompt = state.get(key).unwrap_or_else(|| {
        if prompt_level == 2 {
            String::from("> ")
        } else {
            String::from("\\s-\\v\\$ ")
        }
    });
    let prompt = expand_prompt_string(state, exec_state, &raw_prompt);
    let keymap = state
        .keymap_get(&state.active_keymap)
        .or_else(|| state.keymap_get("emacs"))
        .unwrap_or_else(|| cherubsh_common::Keymap::new("emacs"));
    let mut editor = state
        .line_editor
        .take()
        .unwrap_or_else(|| LineEditor::new(keymap.clone()));
    editor.keymap = keymap;
    if let Some(command_keymap) = state.keymap_get("vi-command") {
        editor.set_vi_command_keymap(command_keymap);
    }

    let mut history = HistorySnapshot::from_state(state);
    let mut completion = ShellCompleter { state, exec_state };
    let result = if completion.state.input.is_terminal() {
        editor.readline(&prompt, &mut history, &mut completion)
    } else {
        editor.readline_scripted(&prompt, &mut history, &mut completion)
    };
    completion.state.line_editor = Some(editor);

    match result {
        Ok(mut line) => {
            line.push('\n');
            Ok(Some(line))
        }
        Err(EditError::Eof) => Ok(None),
        Err(EditError::Interrupted) => {
            completion.state.last_command_exit_value = 130;
            Err(ShellJump::Discard)
        }
        Err(EditError::Io(_)) => Err(ShellJump::ForceEof),
    }
}
