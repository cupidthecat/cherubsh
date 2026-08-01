#![no_main]

use cherubsh_lexer::{Token, TokenKind, TokenValue};
use cherubsh_parser::Parser;
use libfuzzer_sys::fuzz_target;

static TOKEN_KINDS: &[TokenKind] = &[
    TokenKind::Newline,
    TokenKind::Word,
    TokenKind::AssignmentWord,
    TokenKind::RedirWord,
    TokenKind::Number,
    TokenKind::ArithCmd,
    TokenKind::ArithForExprs,
    TokenKind::CondCmd,
    TokenKind::If,
    TokenKind::Then,
    TokenKind::Else,
    TokenKind::Elif,
    TokenKind::Fi,
    TokenKind::Case,
    TokenKind::Esac,
    TokenKind::For,
    TokenKind::Select,
    TokenKind::While,
    TokenKind::Until,
    TokenKind::Do,
    TokenKind::Done,
    TokenKind::Function,
    TokenKind::Coproc,
    TokenKind::In,
    TokenKind::Bang,
    TokenKind::Time,
    TokenKind::TimeOpt,
    TokenKind::TimeIgn,
    TokenKind::AndAnd,
    TokenKind::OrOr,
    TokenKind::Pipe,
    TokenKind::BarAnd,
    TokenKind::Semicolon,
    TokenKind::Ampersand,
    TokenKind::Less,
    TokenKind::Greater,
    TokenKind::LessLess,
    TokenKind::LessLessMinus,
    TokenKind::LessLessLess,
    TokenKind::GreaterGreater,
    TokenKind::GreaterBar,
    TokenKind::LessAnd,
    TokenKind::GreaterAnd,
    TokenKind::AndGreater,
    TokenKind::AndGreaterGreater,
    TokenKind::LessGreater,
    TokenKind::LParen,
    TokenKind::RParen,
    TokenKind::LBrace,
    TokenKind::RBrace,
    TokenKind::DblLParen,
    TokenKind::DblRParen,
    TokenKind::DblLBracket,
    TokenKind::DblRBracket,
    TokenKind::DblSemicolon,
    TokenKind::SemiAmp,
    TokenKind::DblSemiAmp,
];

fuzz_target!(|data: &[u8]| {
    let mut tokens = Vec::new();
    for chunk in data.chunks(3).take(256) {
        let kind = TOKEN_KINDS[usize::from(chunk[0]) % TOKEN_KINDS.len()].clone();
        let number = i64::from(*chunk.get(1).unwrap_or(&0));
        let text = format!("w{number}");
        let value = match kind {
            TokenKind::Word
            | TokenKind::AssignmentWord
            | TokenKind::RedirWord
            | TokenKind::ArithCmd
            | TokenKind::ArithForExprs
            | TokenKind::CondCmd => TokenValue::Text(text),
            TokenKind::Number => TokenValue::Number {
                value: number,
                raw: number.to_string(),
            },
            _ => TokenValue::None,
        };
        tokens.push(Token {
            kind,
            value,
            span: cherubsh_common::Span::dummy(),
            word_flags: u32::from(*chunk.get(2).unwrap_or(&0)),
        });
    }
    tokens.push(Token {
        kind: TokenKind::End,
        value: TokenValue::None,
        span: cherubsh_common::Span::dummy(),
        word_flags: 0,
    });
    let _ = Parser::new(tokens, "").parse();
});
