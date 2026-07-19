use cherubsh_common::Environment;
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_parser::{pretty_print, Parser};

use crate::state::ShellState;

pub fn run_source_output_mode(state: &ShellState, name: &str, source: &str) -> Option<i32> {
    if state.pretty_print_mode {
        return Some(run_pretty_print(state, source));
    }
    if state.dump_translatable_strings || state.dump_po_strings {
        dump_translatable_strings(name, source, state.dump_po_strings);
        return Some(if source_parses(state, source) { 0 } else { 2 });
    }
    None
}

fn run_pretty_print(state: &ShellState, source: &str) -> i32 {
    let mut lexer = configured_lexer(state, source);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        let end = token.kind == TokenKind::End;
        tokens.push(token);
        if end {
            break;
        }
    }
    let mut parser = Parser::new(tokens, source);
    match parser.parse_input_unit() {
        Ok(Some(ast)) => {
            print!("{}", pretty_print(&ast));
            0
        }
        Ok(None) => 0,
        Err(error) => {
            eprintln!("{}: {}", state.shell_invocation_name, error.message);
            2
        }
    }
}

fn source_parses(state: &ShellState, source: &str) -> bool {
    let mut lexer = configured_lexer(state, source);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        let end = token.kind == TokenKind::End;
        tokens.push(token);
        if end {
            break;
        }
    }
    Parser::new(tokens, source).parse_input_unit().is_ok()
}

fn configured_lexer<'a>(state: &ShellState, source: &'a str) -> Lexer<'a> {
    let mut lexer = Lexer::new(source);
    lexer.set_extglob_patterns(state.option("extglob"));
    lexer.set_posix_mode(state.posixly_correct);
    lexer.set_comments_enabled(true);
    lexer
}

#[derive(Debug, PartialEq, Eq)]
struct TranslationString {
    line: usize,
    text: String,
}

fn dump_translatable_strings(name: &str, source: &str, po: bool) {
    for entry in scan_translatable_strings(source) {
        if po {
            println!("#: {name}:{}", entry.line);
            print_po_string(&entry.text);
            println!("msgstr \"\"");
        } else {
            println!("\"{}\"", entry.text);
        }
    }
}

fn scan_translatable_strings(source: &str) -> Vec<TranslationString> {
    let bytes = source.as_bytes();
    let mut strings = Vec::new();
    let mut index = 0usize;
    let mut line = 1usize;
    let mut single_quoted = false;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                line += 1;
                index += 1;
            }
            b'\'' => {
                single_quoted = !single_quoted;
                index += 1;
            }
            b'\\' if !single_quoted => {
                index = (index + 2).min(bytes.len());
            }
            b'$' if !single_quoted && bytes.get(index + 1) == Some(&b'"') => {
                let start_line = line;
                index += 2;
                let start = index;
                let mut escaped = false;
                while index < bytes.len() {
                    let byte = bytes[index];
                    if byte == b'\n' {
                        line += 1;
                    }
                    if byte == b'"' && !escaped {
                        break;
                    }
                    escaped = byte == b'\\' && !escaped;
                    index += 1;
                }
                let text = String::from_utf8_lossy(&bytes[start..index]).into_owned();
                strings.push(TranslationString {
                    line: start_line,
                    text,
                });
                if index < bytes.len() {
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    strings
}

fn print_po_string(text: &str) {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    if !escaped.contains('\n') {
        println!("msgid \"{escaped}\"");
        return;
    }

    println!("msgid \"\"");
    let mut lines = escaped.split('\n').peekable();
    while let Some(line) = lines.next() {
        if lines.peek().is_some() {
            println!("\"{line}\\n\"");
        } else if !line.is_empty() {
            println!("\"{line}\"");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{scan_translatable_strings, TranslationString};

    #[test]
    fn finds_locale_strings_and_tracks_lines() {
        assert_eq!(
            scan_translatable_strings("echo $\"one\"\n'$\"ignored\"'\necho $\"two\\\"x\"\n"),
            vec![
                TranslationString {
                    line: 1,
                    text: "one".to_string()
                },
                TranslationString {
                    line: 3,
                    text: "two\\\"x".to_string()
                }
            ]
        );
    }
}
