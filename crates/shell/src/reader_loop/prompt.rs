fn execute_prompt_command(state: &mut ShellState, exec_state: &mut ExecState) {
    let raw = state.get("PROMPT_COMMAND").unwrap_or_default();
    if raw.trim().is_empty() {
        return;
    }
    let mut lexer = Lexer::new(&raw);
    lexer.set_extglob_patterns(state.option("extglob"));
    lexer.set_posix_mode(state.option("posix"));
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, &raw);
    match parser.parse() {
        Ok(ast) => {
            let saved = state.last_command_exit_value;
            let _ = execute_with_state(&ast, state, exec_state);
            state.last_command_exit_value = saved;
        }
        Err(err) => {
            eprintln!("cherubsh: PROMPT_COMMAND: {}", err.message);
        }
    }
}

