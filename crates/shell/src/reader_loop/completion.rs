struct ShellCompleter<'a> {
    state: &'a mut ShellState,
    exec_state: &'a mut ExecState,
}

impl CompletionProvider for ShellCompleter<'_> {
    fn complete(&mut self, line: &str, point: usize) -> Completion {
        let word_breaks = self
            .state
            .get("COMP_WORDBREAKS")
            .unwrap_or_else(|| " \t\n\"'@><=;|&(:".to_string());
        let req = completion_request(line, point, &word_breaks);
        completion::complete(self.state, self.exec_state, &req)
    }

    fn run_shell_command(
        &mut self,
        command: &str,
        line: &str,
        point: usize,
    ) -> Option<(String, usize)> {
        let saved_line = self.state.get("READLINE_LINE");
        let saved_point = self.state.get("READLINE_POINT");
        self.state.set("READLINE_LINE", line.to_string());
        self.state.set("READLINE_POINT", point.to_string());
        if let Err(error) = self.exec_state.execute_source(command, self.state) {
            eprintln!("{}: {error}", self.state.shell_name);
        }
        let edited_line = self
            .state
            .get("READLINE_LINE")
            .unwrap_or_else(|| line.to_string());
        let edited_point = self
            .state
            .get("READLINE_POINT")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(point)
            .min(edited_line.len());
        restore_readline_variable(self.state, "READLINE_LINE", saved_line);
        restore_readline_variable(self.state, "READLINE_POINT", saved_point);
        Some((edited_line, edited_point))
    }
}

fn restore_readline_variable(state: &mut ShellState, name: &str, value: Option<String>) {
    if let Some(value) = value {
        state.set(name, value);
    } else {
        state.unset(name);
    }
}

fn completion_request<'a>(line: &'a str, point: usize, word_breaks: &str) -> CompRequest<'a> {
    let point = clamp_char_boundary(line, point.min(line.len()));
    let command_start = completion_command_start(line, point);
    let command_end = completion_command_end(line, point);
    let segment = &line[command_start..command_end];
    let relative_point = point.saturating_sub(command_start);
    let tokens = completion_tokens(segment, word_breaks);
    let current_info = current_completion_word(segment, relative_point, word_breaks);
    let current = current_info.text;
    let replace_start = command_start + current_info.start;

    let mut words: Vec<String> = tokens.iter().map(|token| token.text.clone()).collect();
    let after_whitespace = segment[..relative_point]
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace);
    let cword = if current.is_empty() && after_whitespace {
        words.len()
    } else {
        tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| token.start <= relative_point)
            .map(|(index, _)| index)
            .next_back()
            .unwrap_or(0)
    };
    if words.is_empty() {
        words.push(String::new());
    }
    let command = words
        .iter()
        .find(|word| !is_completion_delimiter(word, word_breaks))
        .cloned()
        .unwrap_or_default();
    let previous = cword
        .checked_sub(1)
        .and_then(|index| words.get(index))
        .cloned()
        .unwrap_or_default();
    CompRequest {
        line,
        point,
        words,
        cword,
        command,
        current,
        previous,
        replace_start,
        quote: current_info.quote,
    }
}

#[derive(Clone, Debug)]
struct CompletionToken {
    text: String,
    start: usize,
}

#[derive(Clone, Debug)]
struct CurrentCompletionWord {
    text: String,
    start: usize,
    quote: CompletionQuote,
}

fn clamp_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn completion_command_start(line: &str, point: usize) -> usize {
    let mut start = 0usize;
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for (index, ch) in line[..point].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
        } else if ch == '"' && !single {
            double = !double;
        } else if !single && !double && matches!(ch, ';' | '|' | '&' | '\n' | '(' | ')') {
            start = index + ch.len_utf8();
        }
    }
    while start < point {
        let ch = line[start..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    start
}

fn completion_command_end(line: &str, point: usize) -> usize {
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for ch in line[..point].chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
        } else if ch == '"' && !single {
            double = !double;
        }
    }
    escaped = false;
    for (offset, ch) in line[point..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
        } else if ch == '"' && !single {
            double = !double;
        } else if !single && !double && matches!(ch, ';' | '|' | '&' | '\n' | '(' | ')') {
            return point + offset;
        }
    }
    line.len()
}

fn completion_tokens(segment: &str, word_breaks: &str) -> Vec<CompletionToken> {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut start = 0usize;
    let mut active = false;
    let mut single = false;
    let mut double = false;
    let mut escaped = false;

    let flush = |tokens: &mut Vec<CompletionToken>, text: &mut String, active: &mut bool, start| {
        if *active {
            tokens.push(CompletionToken {
                text: std::mem::take(text),
                start,
            });
            *active = false;
        }
    };

    for (index, ch) in segment.char_indices() {
        if escaped {
            if !active {
                start = index;
                active = true;
            }
            text.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !single {
            if !active {
                start = index;
                active = true;
            }
            escaped = true;
            continue;
        }
        if ch == '\'' && !double {
            if !active {
                start = index;
                active = true;
            }
            single = !single;
            continue;
        }
        if ch == '"' && !single {
            if !active {
                start = index;
                active = true;
            }
            double = !double;
            continue;
        }
        if !single && !double && ch.is_whitespace() {
            flush(&mut tokens, &mut text, &mut active, start);
            continue;
        }
        if !single && !double && word_breaks.contains(ch) {
            flush(&mut tokens, &mut text, &mut active, start);
            tokens.push(CompletionToken {
                text: ch.to_string(),
                start: index,
            });
            continue;
        }
        if !active {
            start = index;
            active = true;
        }
        text.push(ch);
    }
    if escaped {
        text.push('\\');
    }
    flush(&mut tokens, &mut text, &mut active, start);
    tokens
}

fn current_completion_word(
    segment: &str,
    point: usize,
    word_breaks: &str,
) -> CurrentCompletionWord {
    let mut text = String::new();
    let mut start = 0usize;
    let mut quote_start = None;
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for (index, ch) in segment[..point].char_indices() {
        if escaped {
            text.push(ch);
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
            if single && text.is_empty() {
                quote_start = Some(index + ch.len_utf8());
            }
        } else if ch == '"' && !single {
            double = !double;
            if double && text.is_empty() {
                quote_start = Some(index + ch.len_utf8());
            }
        } else if !single && !double && (ch.is_whitespace() || word_breaks.contains(ch)) {
            text.clear();
            start = index + ch.len_utf8();
            quote_start = None;
        } else {
            text.push(ch);
        }
    }
    let quote = if single {
        CompletionQuote::Single
    } else if double {
        CompletionQuote::Double
    } else {
        CompletionQuote::None
    };
    CurrentCompletionWord {
        text,
        start: quote_start.unwrap_or(start),
        quote,
    }
}

fn is_completion_delimiter(word: &str, word_breaks: &str) -> bool {
    word.chars().count() == 1
        && word
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace() || word_breaks.contains(ch))
}
