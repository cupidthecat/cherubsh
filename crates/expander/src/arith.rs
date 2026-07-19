//! Arithmetic evaluator for `$((expr))`, `((expr))`, `let`, `${var:off:len}`.
//! Mirrors expr.c precedence and operator set.

use std::borrow::Cow;
use std::cell::Cell;

use crate::ctx::{ExpCtx, MAX_ARITH_NESTING};
use crate::error::ExpandError;
use cherubsh_common::{AssignError, Environment, VarAttrs, VarKind};

thread_local! {
    static VALUE_RECURSION_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct ValueRecursionGuard;

impl ValueRecursionGuard {
    fn enter() -> Result<Self, ExpandError> {
        VALUE_RECURSION_DEPTH.with(|depth| {
            let current = depth.get();
            if current >= MAX_ARITH_NESTING {
                Err(ExpandError::ArithRecursion)
            } else {
                depth.set(current + 1);
                Ok(Self)
            }
        })
    }
}

impl Drop for ValueRecursionGuard {
    fn drop(&mut self) {
        VALUE_RECURSION_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

#[derive(Debug, Clone)]
enum LValue {
    Scalar(String),
    Indexed { name: String, index: i64 },
    Assoc { name: String, key: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Num(i64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    StarStarEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,
    LParen,
    RParen,
    Bang,
    Tilde,
    AmpAmp,
    PipePipe,
    Amp,
    Pipe,
    Caret,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    Question,
    Colon,
    Comma,
    PlusPlus,
    MinusMinus,
    Eof,
}

#[derive(Clone)]
struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    cur: Tok,
    tok_start: usize,
    tok_end: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a [u8]) -> Result<Self, ExpandError> {
        let mut l = Self {
            src,
            i: 0,
            cur: Tok::Eof,
            tok_start: 0,
            tok_end: 0,
        };
        l.advance()?;
        Ok(l)
    }

    fn skip_ws(&mut self) {
        while self.i < self.src.len() && self.src[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn advance(&mut self) -> Result<(), ExpandError> {
        self.skip_ws();
        self.tok_start = self.i;
        if self.i >= self.src.len() {
            self.cur = Tok::Eof;
            self.tok_end = self.i;
            return Ok(());
        }
        let b = self.src[self.i];
        if b.is_ascii_digit() {
            self.cur = self.lex_number()?;
            self.tok_end = self.i;
            return Ok(());
        }
        if b == b'_' || b.is_ascii_alphabetic() {
            let start = self.i;
            while self.i < self.src.len()
                && (self.src[self.i] == b'_' || self.src[self.i].is_ascii_alphanumeric())
            {
                self.i += 1;
            }
            // Array subscript: ident[expr]
            if self.i < self.src.len() && self.src[self.i] == b'[' {
                let mut depth = 1;
                let sub_start = self.i;
                self.i += 1;
                while self.i < self.src.len() && depth > 0 {
                    if self.src[self.i] == b'\\' && self.i + 1 < self.src.len() {
                        self.i += 2;
                        continue;
                    }
                    if depth == 1
                        && self.src[self.i] == b'['
                        && self.src.get(self.i + 1) == Some(&b']')
                    {
                        self.i += 1;
                        continue;
                    }
                    if depth == 1
                        && self.src[self.i] == b']'
                        && self.src.get(self.i + 1) == Some(&b']')
                    {
                        self.i += 1;
                        continue;
                    }
                    if self.src[self.i] == b'[' {
                        depth += 1;
                    } else if self.src[self.i] == b']' {
                        depth -= 1;
                    }
                    self.i += 1;
                }
                let s = std::str::from_utf8(&self.src[start..self.i])
                    .map_err(|_| ExpandError::ArithSyntax("invalid identifier".into()))?;
                self.cur = Tok::Ident(s.to_string());
                let _ = sub_start;
                self.tok_end = self.i;
                return Ok(());
            }
            let s = std::str::from_utf8(&self.src[start..self.i])
                .map_err(|_| ExpandError::ArithSyntax("invalid identifier".into()))?;
            self.cur = Tok::Ident(s.to_string());
            self.tok_end = self.i;
            return Ok(());
        }
        let two = if self.i + 1 < self.src.len() {
            (b, self.src[self.i + 1])
        } else {
            (b, 0)
        };
        let three = if self.i + 2 < self.src.len() {
            (b, self.src[self.i + 1], self.src[self.i + 2])
        } else {
            (b, two.1, 0)
        };
        let (tok, adv) = match three {
            (b'*', b'*', b'=') => (Tok::StarStarEq, 3),
            (b'<', b'<', b'=') => (Tok::ShlEq, 3),
            (b'>', b'>', b'=') => (Tok::ShrEq, 3),
            _ => match two {
                (b'*', b'*') => (Tok::StarStar, 2),
                (b'<', b'<') => (Tok::Shl, 2),
                (b'>', b'>') => (Tok::Shr, 2),
                (b'<', b'=') => (Tok::Le, 2),
                (b'>', b'=') => (Tok::Ge, 2),
                (b'=', b'=') => (Tok::EqEq, 2),
                (b'!', b'=') => (Tok::BangEq, 2),
                (b'&', b'&') => (Tok::AmpAmp, 2),
                (b'|', b'|') => (Tok::PipePipe, 2),
                (b'+', b'=') => (Tok::PlusEq, 2),
                (b'-', b'=') => (Tok::MinusEq, 2),
                (b'*', b'=') => (Tok::StarEq, 2),
                (b'/', b'=') => (Tok::SlashEq, 2),
                (b'%', b'=') => (Tok::PercentEq, 2),
                (b'&', b'=') => (Tok::AmpEq, 2),
                (b'|', b'=') => (Tok::PipeEq, 2),
                (b'^', b'=') => (Tok::CaretEq, 2),
                (b'+', b'+') => (Tok::PlusPlus, 2),
                (b'-', b'-') => (Tok::MinusMinus, 2),
                _ => match b {
                    b'+' => (Tok::Plus, 1),
                    b'-' => (Tok::Minus, 1),
                    b'*' => (Tok::Star, 1),
                    b'/' => (Tok::Slash, 1),
                    b'%' => (Tok::Percent, 1),
                    b'(' => (Tok::LParen, 1),
                    b')' => (Tok::RParen, 1),
                    b'!' => (Tok::Bang, 1),
                    b'~' => (Tok::Tilde, 1),
                    b'<' => (Tok::Lt, 1),
                    b'>' => (Tok::Gt, 1),
                    b'=' => (Tok::Eq, 1),
                    b'&' => (Tok::Amp, 1),
                    b'|' => (Tok::Pipe, 1),
                    b'^' => (Tok::Caret, 1),
                    b'?' => (Tok::Question, 1),
                    b':' => (Tok::Colon, 1),
                    b',' => (Tok::Comma, 1),
                    _ => {
                        if matches!(b, b'[' | b']' | b'\\') {
                            return Err(arith_invalid_operator(self.src, self.tok_start));
                        }
                        return Err(arith_operand_expected(self.src, self.tok_start));
                    }
                },
            },
        };
        self.cur = tok;
        self.i += adv;
        self.tok_end = self.i;
        Ok(())
    }

    fn consume_first_char_of_double_operator(&mut self) -> Result<(), ExpandError> {
        self.i = self.tok_end.saturating_sub(1);
        self.advance()
    }

    fn lex_number(&mut self) -> Result<Tok, ExpandError> {
        let start = self.i;
        // Parse a token of digits / letters / `#` for base#digits.
        while self.i < self.src.len() {
            let b = self.src[self.i];
            if b.is_ascii_alphanumeric() || b == b'#' || b == b'@' || b == b'_' {
                self.i += 1;
            } else {
                break;
            }
        }
        let raw = std::str::from_utf8(&self.src[start..self.i])
            .map_err(|_| ExpandError::ArithSyntax("invalid number".into()))?;
        let v = parse_number(raw)?;
        Ok(Tok::Num(v))
    }
}

fn arith_expr_from(src: &[u8], start: usize) -> String {
    String::from_utf8_lossy(&src[start.min(src.len())..])
        .trim_start()
        .to_string()
}

fn quote_error_token(token: &str) -> String {
    token.replace('\\', "\\\\")
}

fn arith_operand_expected(src: &[u8], start: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, start));
    ExpandError::ArithSyntax(format!(
        "{expr}: arithmetic syntax error: operand expected (error token is \"{token}\")"
    ))
}

fn arith_expression_expected(src: &[u8], token_start: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: expression expected (error token is \"{token}\")"
    ))
}

fn arith_conditional_colon_expected(src: &[u8], token_start: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: `:' expected for conditional expression (error token is \"{token}\")"
    ))
}

fn arith_syntax_error_in_expression(
    src: &[u8],
    token_start: usize,
    _token_end: usize,
) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: arithmetic syntax error in expression (error token is \"{token}\")"
    ))
}

fn arith_invalid_operator(src: &[u8], token_start: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: arithmetic syntax error: invalid arithmetic operator (error token is \"{token}\")"
    ))
}

fn arith_assignment_to_non_variable(
    src: &[u8],
    token_start: usize,
    _token_end: usize,
) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: attempted assignment to non-variable (error token is \"{token}\")"
    ))
}

fn arith_number_error(raw: &str, message: &str) -> ExpandError {
    let token = quote_error_token(raw);
    ExpandError::ArithSyntax(format!("{raw}: {message} (error token is \"{token}\")"))
}

fn arith_division_by_zero(src: &[u8], token_start: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&arith_expr_from(src, token_start));
    ExpandError::ArithSyntax(format!(
        "{expr}: division by 0 (error token is \"{token}\")"
    ))
}

fn arith_rhs_expected(lex: &Lexer<'_>, op_start: usize) -> Result<(), ExpandError> {
    if lex.cur == Tok::Eof {
        Err(arith_operand_expected(lex.src, op_start))
    } else {
        Ok(())
    }
}

fn arith_missing_rparen(src: &[u8], token_before: usize) -> ExpandError {
    let expr = arith_expr_from(src, 0);
    let token = quote_error_token(&previous_arith_token(src, token_before));
    ExpandError::ArithSyntax(format!("{expr}: missing `)' (error token is \"{token}\")"))
}

fn previous_arith_token(src: &[u8], before: usize) -> String {
    let mut end = before.min(src.len());
    while end > 0 && src[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return String::new();
    }
    let mut start = end;
    while start > 0 {
        let b = src[start - 1];
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'#' | b'@') {
            start -= 1;
        } else {
            break;
        }
    }
    if start == end {
        start -= 1;
    }
    String::from_utf8_lossy(&src[start..end]).to_string()
}

fn parse_number(s: &str) -> Result<i64, ExpandError> {
    if let Some(idx) = s.find('#') {
        let (base_part, digit_part) = s.split_at(idx);
        let digit_part = &digit_part[1..];
        let base: i64 = base_part
            .parse()
            .map_err(|_| arith_number_error(s, "invalid arithmetic base"))?;
        if base == 0 {
            return Err(arith_number_error(s, "invalid number"));
        }
        if !(2..=64).contains(&base) {
            return Err(arith_number_error(s, "invalid arithmetic base"));
        }
        if digit_part.is_empty() {
            return Err(arith_number_error(s, "invalid integer constant"));
        }
        let mut value: u64 = 0;
        for ch in digit_part.chars() {
            let d: u64 = match ch {
                '0'..='9' => (ch as u64) - ('0' as u64),
                'a'..='z' => (ch as u64) - ('a' as u64) + 10,
                'A'..='Z' => {
                    if base <= 36 {
                        (ch as u64) - ('A' as u64) + 10
                    } else {
                        (ch as u64) - ('A' as u64) + 36
                    }
                }
                '@' => 62,
                '_' => 63,
                _ => {
                    return Err(arith_number_error(s, "invalid number"));
                }
            };
            if d >= base as u64 {
                return Err(arith_number_error(s, "value too great for base"));
            }
            value = value.wrapping_mul(base as u64).wrapping_add(d);
        }
        return Ok(value as i64);
    }
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return parse_radix_wrapping(rest, 16, s);
    }
    if s.len() > 1 && s.starts_with('0') && s.chars().skip(1).all(|c| c.is_ascii_digit()) {
        return parse_radix_wrapping(&s[1..], 8, s);
    }
    parse_radix_wrapping(s, 10, s)
}

fn parse_radix_wrapping(digits: &str, radix: u32, raw: &str) -> Result<i64, ExpandError> {
    if digits.is_empty() {
        return Err(ExpandError::ArithSyntax(format!("invalid number {}", raw)));
    }
    let mut value: u64 = 0;
    for ch in digits.chars() {
        let Some(digit) = ch.to_digit(radix) else {
            return Err(ExpandError::ArithSyntax(format!("invalid number {}", raw)));
        };
        value = value.wrapping_mul(radix as u64).wrapping_add(digit as u64);
    }
    Ok(value as i64)
}

/// Public arithmetic entry point.
pub fn eval(expr: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    eval_inner(expr, ctx)
}

/// Evaluate an arithmetic string that has already been through arithmetic
/// context expansion. Array subscripts are still parsed as arithmetic, but
/// shell expansions inside them must not be run a second time.
pub fn eval_preexpanded(expr: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    let prev = ctx.arith_expand_subscripts;
    ctx.arith_expand_subscripts = false;
    let result = eval_inner(expr, ctx);
    ctx.arith_expand_subscripts = prev;
    result
}

fn eval_inner(expr: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    if expr.trim().is_empty() {
        return Ok(0);
    }
    if let Some(err) = prefix_postfix_lvalue_error(expr) {
        return Err(err);
    }
    ctx.arith_depth = ctx.arith_depth.saturating_add(1);
    if ctx.arith_depth > MAX_ARITH_NESTING {
        ctx.arith_depth -= 1;
        return Err(ExpandError::ArithRecursion);
    }
    let result = (|| {
        let mut lex = Lexer::new(expr.as_bytes())?;
        let val = parse_comma(&mut lex, ctx, true)?;
        if lex.cur != Tok::Eof {
            if matches!(lex.cur, Tok::PlusPlus | Tok::MinusMinus) {
                let trimmed = expr.trim_start();
                if trimmed.starts_with("--") || trimmed.starts_with("++") {
                    let op = if matches!(lex.cur, Tok::PlusPlus) {
                        "++"
                    } else {
                        "--"
                    };
                    return Err(ExpandError::ArithSyntax(format!(
                        "{expr}: {op}: assignment requires lvalue (error token is \"{op} \")"
                    )));
                }
            }
            if assignment_opcode(&lex.cur).is_some() {
                return Err(arith_assignment_to_non_variable(
                    lex.src,
                    lex.tok_start,
                    lex.tok_end,
                ));
            }
            return Err(arith_syntax_error_in_expression(
                lex.src,
                lex.tok_start,
                lex.tok_end,
            ));
        }
        Ok(val)
    })();
    ctx.arith_depth -= 1;
    result
}

fn prefix_postfix_lvalue_error(expr: &str) -> Option<ExpandError> {
    let trimmed = expr.trim();
    let (prefix, suffix) = if trimmed.starts_with("--") && trimmed.ends_with("++") {
        ("--", "++")
    } else if trimmed.starts_with("++") && trimmed.ends_with("--") {
        ("++", "--")
    } else {
        return None;
    };
    let name = &trimmed[prefix.len()..trimmed.len() - suffix.len()];
    if !is_arith_identifier(name) {
        return None;
    }
    Some(ExpandError::ArithSyntax(format!(
        "{trimmed} : {suffix}: assignment requires lvalue (error token is \"{suffix} \")"
    )))
}

fn parse_comma(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_assign(lex, ctx, active)?;
    while lex.cur == Tok::Comma {
        lex.advance()?;
        v = parse_assign(lex, ctx, active)?;
    }
    Ok(if active { v } else { 0 })
}

fn parse_assign(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    if let Tok::Ident(name) = lex.cur.clone() {
        let mut lookahead = lex.clone();
        lookahead.advance()?;
        if let Some(opcode) = assignment_opcode(&lookahead.cur) {
            lex.advance()?;
            lex.advance()?;
            let precomputed = if active && opcode != 0 {
                Some(resolve_lvalue(&name, ctx)?)
            } else {
                None
            };
            let lhs_val = match precomputed.as_ref() {
                Some(lvalue) => load_lvalue_value(lvalue, ctx)?,
                None => 0,
            };
            let rhs_start = lex.tok_start;
            let rhs = parse_assign(lex, ctx, active)?;
            if active && matches!(opcode, 4 | 5) && rhs == 0 {
                return Err(arith_division_by_zero(lex.src, rhs_start));
            }
            let new_val = assigned_value(opcode, lhs_val, rhs)?;
            if active {
                if let Some(lvalue) = precomputed {
                    store_lvalue_value(ctx.env, &lvalue, new_val)?;
                } else {
                    store_ident_value(ctx, &name, new_val)?;
                }
            }
            return Ok(if active { new_val } else { 0 });
        }
    }

    let lhs_val = parse_ternary(lex, ctx, active)?;
    Ok(if active { lhs_val } else { 0 })
}

fn assignment_opcode(tok: &Tok) -> Option<u8> {
    match tok {
        Tok::Eq => Some(0),
        Tok::PlusEq => Some(1),
        Tok::MinusEq => Some(2),
        Tok::StarEq => Some(3),
        Tok::SlashEq => Some(4),
        Tok::PercentEq => Some(5),
        Tok::StarStarEq => Some(6),
        Tok::AmpEq => Some(7),
        Tok::PipeEq => Some(8),
        Tok::CaretEq => Some(9),
        Tok::ShlEq => Some(10),
        Tok::ShrEq => Some(11),
        _ => None,
    }
}

fn assigned_value(opcode: u8, lhs: i64, rhs: i64) -> Result<i64, ExpandError> {
    Ok(match opcode {
        0 => rhs,
        1 => lhs.wrapping_add(rhs),
        2 => lhs.wrapping_sub(rhs),
        3 => lhs.wrapping_mul(rhs),
        4 => {
            if rhs == 0 {
                return Err(ExpandError::DivisionByZero);
            }
            lhs.wrapping_div(rhs)
        }
        5 => {
            if rhs == 0 {
                return Err(ExpandError::DivisionByZero);
            }
            lhs.wrapping_rem(rhs)
        }
        6 => pow_i64(lhs, rhs)?,
        7 => lhs & rhs,
        8 => lhs | rhs,
        9 => lhs ^ rhs,
        10 => lhs.wrapping_shl((rhs as u32) & 63),
        11 => lhs.wrapping_shr((rhs as u32) & 63),
        _ => unreachable!(),
    })
}

fn parse_ternary(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let cond = parse_logor(lex, ctx, active)?;
    if lex.cur == Tok::Question {
        lex.advance()?;
        let true_start = lex.tok_start;
        if lex.cur == Tok::Colon {
            return Err(arith_expression_expected(lex.src, lex.tok_start));
        }
        let t = parse_comma(lex, ctx, active && cond != 0)?;
        if lex.cur != Tok::Colon {
            return Err(arith_conditional_colon_expected(lex.src, true_start));
        }
        let colon_start = lex.tok_start;
        lex.advance()?;
        if lex.cur == Tok::Eof {
            return Err(arith_expression_expected(lex.src, colon_start));
        }
        let f = parse_ternary(lex, ctx, active && cond == 0)?;
        return Ok(if !active {
            0
        } else if cond != 0 {
            t
        } else {
            f
        });
    }
    Ok(if active { cond } else { 0 })
}

fn parse_logor(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_logand(lex, ctx, active)?;
    while lex.cur == Tok::PipePipe {
        lex.advance()?;
        let rhs_active = active && v == 0;
        let r = parse_logand(lex, ctx, rhs_active)?;
        if active {
            v = if v != 0 || r != 0 { 1 } else { 0 };
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_logand(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_bitor(lex, ctx, active)?;
    while lex.cur == Tok::AmpAmp {
        lex.advance()?;
        let rhs_active = active && v != 0;
        let r = parse_bitor(lex, ctx, rhs_active)?;
        if active {
            v = if v != 0 && r != 0 { 1 } else { 0 };
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_bitor(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_bitxor(lex, ctx, active)?;
    while lex.cur == Tok::Pipe {
        lex.advance()?;
        let r = parse_bitxor(lex, ctx, active)?;
        if active {
            v |= r;
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_bitxor(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_bitand(lex, ctx, active)?;
    while lex.cur == Tok::Caret {
        lex.advance()?;
        let r = parse_bitand(lex, ctx, active)?;
        if active {
            v ^= r;
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_bitand(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_equality(lex, ctx, active)?;
    while lex.cur == Tok::Amp {
        lex.advance()?;
        let r = parse_equality(lex, ctx, active)?;
        if active {
            v &= r;
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_equality(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_relate(lex, ctx, active)?;
    loop {
        match lex.cur {
            Tok::EqEq => {
                lex.advance()?;
                let r = parse_relate(lex, ctx, active)?;
                if active {
                    v = if v == r { 1 } else { 0 };
                }
            }
            Tok::BangEq => {
                lex.advance()?;
                let r = parse_relate(lex, ctx, active)?;
                if active {
                    v = if v != r { 1 } else { 0 };
                }
            }
            _ => break,
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_relate(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_shift(lex, ctx, active)?;
    loop {
        let op = match lex.cur {
            Tok::Lt => 0,
            Tok::Le => 1,
            Tok::Gt => 2,
            Tok::Ge => 3,
            _ => break,
        };
        lex.advance()?;
        let r = parse_shift(lex, ctx, active)?;
        if active {
            v = match op {
                0 => (v < r) as i64,
                1 => (v <= r) as i64,
                2 => (v > r) as i64,
                3 => (v >= r) as i64,
                _ => unreachable!(),
            };
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_shift(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_add(lex, ctx, active)?;
    loop {
        match lex.cur {
            Tok::Shl => {
                lex.advance()?;
                let r = parse_add(lex, ctx, active)?;
                if active {
                    v = v.wrapping_shl((r as u32) & 63);
                }
            }
            Tok::Shr => {
                lex.advance()?;
                let r = parse_add(lex, ctx, active)?;
                if active {
                    v = v.wrapping_shr((r as u32) & 63);
                }
            }
            _ => break,
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_add(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_mul(lex, ctx, active)?;
    loop {
        match lex.cur {
            Tok::Plus => {
                let op_start = lex.tok_start;
                lex.advance()?;
                arith_rhs_expected(lex, op_start)?;
                let r = parse_mul(lex, ctx, active)?;
                if active {
                    v = v.wrapping_add(r);
                }
            }
            Tok::PlusPlus => {
                if lex.tok_end == lex.src.len()
                    && arith_expr_from(lex.src, 0).trim_start().starts_with("--")
                {
                    return Err(ExpandError::ArithSyntax(format!(
                        "{}: ++: assignment requires lvalue (error token is \"++ \")",
                        arith_expr_from(lex.src, 0)
                    )));
                }
                lex.consume_first_char_of_double_operator()?;
                let r = parse_mul(lex, ctx, active)?;
                if active {
                    v = v.wrapping_add(r);
                }
            }
            Tok::Minus => {
                let op_start = lex.tok_start;
                lex.advance()?;
                arith_rhs_expected(lex, op_start)?;
                let r = parse_mul(lex, ctx, active)?;
                if active {
                    v = v.wrapping_sub(r);
                }
            }
            Tok::MinusMinus => {
                lex.consume_first_char_of_double_operator()?;
                let r = parse_mul(lex, ctx, active)?;
                if active {
                    v = v.wrapping_sub(r);
                }
            }
            _ => break,
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_mul(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let mut v = parse_power(lex, ctx, active)?;
    loop {
        match lex.cur {
            Tok::Star => {
                let op_start = lex.tok_start;
                lex.advance()?;
                arith_rhs_expected(lex, op_start)?;
                let r = parse_power(lex, ctx, active)?;
                if active {
                    v = v.wrapping_mul(r);
                }
            }
            Tok::Slash => {
                let op_start = lex.tok_start;
                lex.advance()?;
                arith_rhs_expected(lex, op_start)?;
                let rhs_start = lex.tok_start;
                let r = parse_power(lex, ctx, active)?;
                if active {
                    if r == 0 {
                        return Err(arith_division_by_zero(lex.src, rhs_start));
                    }
                    v = v.wrapping_div(r);
                }
            }
            Tok::Percent => {
                let op_start = lex.tok_start;
                lex.advance()?;
                arith_rhs_expected(lex, op_start)?;
                let rhs_start = lex.tok_start;
                let r = parse_power(lex, ctx, active)?;
                if active {
                    if r == 0 {
                        return Err(arith_division_by_zero(lex.src, rhs_start));
                    }
                    v = v.wrapping_rem(r);
                }
            }
            _ => break,
        }
    }
    Ok(if active { v } else { 0 })
}

fn parse_power(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let base = parse_unary(lex, ctx, active)?;
    if lex.cur == Tok::StarStar {
        let exp_start = lex.tok_end.min(lex.src.len());
        lex.advance()?;
        let exp = parse_power(lex, ctx, active)?;
        if active && exp < 0 {
            let token_start = if exp_start < lex.src.len() && lex.src[exp_start] == b'-' {
                exp_start + 1
            } else {
                exp_start
            };
            let expr = arith_expr_from(lex.src, 0);
            let token = quote_error_token(&arith_expr_from(lex.src, token_start));
            return Err(ExpandError::ArithSyntax(format!(
                "{expr}: exponent less than 0 (error token is \"{token}\")"
            )));
        }
        return if active { pow_i64(base, exp) } else { Ok(0) };
    }
    Ok(if active { base } else { 0 })
}

fn parse_unary(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    match lex.cur {
        Tok::Plus => {
            let op_start = lex.tok_start;
            lex.advance()?;
            if lex.cur == Tok::Eof {
                return Err(arith_operand_expected(lex.src, op_start));
            }
            parse_unary(lex, ctx, active)
        }
        Tok::Minus => {
            let op_start = lex.tok_start;
            lex.advance()?;
            if lex.cur == Tok::Eof {
                return Err(arith_operand_expected(lex.src, op_start));
            }
            let v = parse_unary(lex, ctx, active)?;
            Ok(if active { v.wrapping_neg() } else { 0 })
        }
        Tok::Bang => {
            let op_start = lex.tok_start;
            lex.advance()?;
            if lex.cur == Tok::Eof {
                return Err(arith_operand_expected(lex.src, op_start));
            }
            let v = parse_unary(lex, ctx, active)?;
            Ok(if !active {
                0
            } else if v == 0 {
                1
            } else {
                0
            })
        }
        Tok::Tilde => {
            let op_start = lex.tok_start;
            lex.advance()?;
            if lex.cur == Tok::Eof {
                return Err(arith_operand_expected(lex.src, op_start));
            }
            let v = parse_unary(lex, ctx, active)?;
            Ok(if active { !v } else { 0 })
        }
        Tok::PlusPlus => {
            if prefix_inc_dec_has_target(lex)? {
                let prefix_end = lex.tok_end;
                lex.advance()?;
                apply_pre(lex, ctx, 1, active, prefix_end)
            } else {
                lex.consume_first_char_of_double_operator()?;
                parse_unary(lex, ctx, active)
            }
        }
        Tok::MinusMinus => {
            if prefix_inc_dec_has_target(lex)? {
                let prefix_end = lex.tok_end;
                lex.advance()?;
                apply_pre(lex, ctx, -1, active, prefix_end)
            } else {
                lex.consume_first_char_of_double_operator()?;
                let v = parse_unary(lex, ctx, active)?;
                Ok(if active { v.wrapping_neg() } else { 0 })
            }
        }
        _ => parse_postfix(lex, ctx, active),
    }
}

fn prefix_inc_dec_has_target(lex: &Lexer<'_>) -> Result<bool, ExpandError> {
    let mut lookahead = lex.clone();
    let prefix_end = lookahead.tok_end;
    lookahead.advance()?;
    Ok(prefix_inc_dec_target_name(&mut lookahead, prefix_end, inc_dec_sign(&lex.cur))?.is_some())
}

fn inc_dec_sign(tok: &Tok) -> i64 {
    match tok {
        Tok::PlusPlus => 1,
        Tok::MinusMinus => -1,
        _ => 0,
    }
}

fn prefix_inc_dec_target_name(
    lex: &mut Lexer<'_>,
    mut prev_end: usize,
    sign: i64,
) -> Result<Option<String>, ExpandError> {
    loop {
        match lex.cur.clone() {
            Tok::Ident(name) => return Ok(Some(name)),
            Tok::Plus | Tok::PlusPlus if sign == 1 && lex.tok_start == prev_end => {
                prev_end = lex.tok_end;
                lex.advance()?;
            }
            Tok::Minus | Tok::MinusMinus if sign == -1 && lex.tok_start == prev_end => {
                prev_end = lex.tok_end;
                lex.advance()?;
            }
            _ => return Ok(None),
        }
    }
}

fn apply_pre(
    lex: &mut Lexer<'_>,
    ctx: &mut ExpCtx,
    delta: i64,
    active: bool,
    prefix_end: usize,
) -> Result<i64, ExpandError> {
    if let Some(nm) = prefix_inc_dec_target_name(lex, prefix_end, delta)? {
        lex.advance()?;
        let lvalue = if active {
            Some(resolve_lvalue(&nm, ctx)?)
        } else {
            None
        };
        let cur = match lvalue.as_ref() {
            Some(lvalue) => load_lvalue_value(lvalue, ctx)?,
            None => 0,
        };
        let new = if active { cur.wrapping_add(delta) } else { 0 };
        if active {
            store_lvalue_value(ctx.env, &lvalue.expect("active lvalue"), new)?;
        }
        return Ok(new);
    }
    Err(ExpandError::ArithSyntax("++/-- requires identifier".into()))
}

fn parse_postfix(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    let tok = lex.cur.clone();
    if let Tok::Ident(name) = &tok {
        let nm = name.clone();
        let lvalue = if active {
            Some(resolve_lvalue(&nm, ctx)?)
        } else {
            None
        };
        let cur = match lvalue.as_ref() {
            Some(lvalue) => load_lvalue_value(lvalue, ctx)?,
            None => 0,
        };
        lex.advance()?;
        match lex.cur {
            Tok::PlusPlus => {
                lex.advance()?;
                if active {
                    store_lvalue_value(
                        ctx.env,
                        lvalue.as_ref().expect("active lvalue"),
                        cur.wrapping_add(1),
                    )?;
                }
                return Ok(cur);
            }
            Tok::MinusMinus => {
                lex.advance()?;
                if active {
                    store_lvalue_value(
                        ctx.env,
                        lvalue.as_ref().expect("active lvalue"),
                        cur.wrapping_sub(1),
                    )?;
                }
                return Ok(cur);
            }
            _ => return Ok(cur),
        }
    }
    parse_primary(lex, ctx, active)
}

fn resolve_lvalue(name: &str, ctx: &mut ExpCtx) -> Result<LValue, ExpandError> {
    if array_ref_parts(name).is_none() && ctx.env.attrs(name).contains(VarAttrs::NAMEREF) {
        if let Some(target) = ctx.env.resolve_nameref(name) {
            if target != name {
                return resolve_lvalue(&target, ctx);
            }
        }
    }
    if let Some((base, subscript)) = array_ref_parts(name) {
        if ctx.env.kind(base) == VarKind::Assoc {
            Ok(LValue::Assoc {
                name: base.to_string(),
                key: unescape_assoc_subscript(subscript),
            })
        } else {
            if subscript.is_empty() {
                return Err(ExpandError::InvalidArraySubscript(format!("{base}[]")));
            }
            Ok(LValue::Indexed {
                name: base.to_string(),
                index: eval_array_index(subscript, ctx)?,
            })
        }
    } else {
        Ok(LValue::Scalar(name.to_string()))
    }
}

fn load_lvalue_value(lvalue: &LValue, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    match lvalue {
        LValue::Scalar(name) => {
            if let Some(v) = ctx.env.get_cow(name) {
                finish_loaded_value(prepare_loaded_value(v), ctx, 0)
            } else if ctx.eval_unbound_error {
                Err(ExpandError::UnboundVariable(name.clone()))
            } else {
                Ok(0)
            }
        }
        LValue::Indexed { name, index } => {
            if let Some(v) = ctx.env.get_array_indexed_cow(name, *index) {
                finish_loaded_value(prepare_loaded_value(v), ctx, 0)
            } else if *index == 0 {
                if let Some(v) = ctx.env.get_cow(name) {
                    finish_loaded_value(prepare_loaded_value(v), ctx, 0)
                } else if ctx.env.kind(name) == VarKind::Indexed {
                    Ok(0)
                } else if ctx.eval_unbound_error {
                    Err(ExpandError::UnboundVariable(name.clone()))
                } else {
                    Ok(0)
                }
            } else if ctx.env.kind(name) == VarKind::Indexed {
                Ok(0)
            } else if ctx.eval_unbound_error {
                Err(ExpandError::UnboundVariable(format!("{name}[{index}]")))
            } else {
                Ok(0)
            }
        }
        LValue::Assoc { name, key } => {
            if let Some(v) = ctx.env.get_array_assoc_cow(name, key) {
                finish_loaded_value(prepare_loaded_value(v), ctx, 0)
            } else {
                Ok(0)
            }
        }
    }
}

fn store_lvalue_value(
    env: &mut dyn Environment,
    lvalue: &LValue,
    value: i64,
) -> Result<(), ExpandError> {
    match lvalue {
        LValue::Scalar(name) => store_scalar_ident_value(env, name, value),
        LValue::Indexed { name, index } => {
            if env.is_readonly(name) {
                return Err(ExpandError::AssignToReadonly(name.to_string()));
            }
            env.set_array_indexed(name, *index, value.to_string());
            Ok(())
        }
        LValue::Assoc { name, key } => {
            if env.is_readonly(name) {
                return Err(ExpandError::AssignToReadonly(name.to_string()));
            }
            env.set_array_assoc(name, key, value.to_string());
            Ok(())
        }
    }
}

fn store_ident_value(ctx: &mut ExpCtx, name: &str, value: i64) -> Result<(), ExpandError> {
    let lvalue = resolve_lvalue(name, ctx)?;
    store_lvalue_value(ctx.env, &lvalue, value)
}

fn store_scalar_ident_value(
    env: &mut dyn Environment,
    name: &str,
    value: i64,
) -> Result<(), ExpandError> {
    env.assign(name, value.to_string())
        .map_err(|err| match err {
            AssignError::ReadOnly(name) => ExpandError::AssignToReadonly(name),
            AssignError::InvalidInteger(value) => {
                ExpandError::ArithSyntax(format!("{value}: invalid integer"))
            }
            AssignError::InvalidName(name) => {
                ExpandError::ArithSyntax(format!("`{name}': not a valid identifier"))
            }
            AssignError::BadArraySubscript(name) => ExpandError::InvalidArraySubscript(name),
            AssignError::CircularNameReference(name) => {
                ExpandError::ArithSyntax(format!("{name}: circular name reference"))
            }
        })
}

fn array_ref_parts(name: &str) -> Option<(&str, &str)> {
    let bracket = name.find('[')?;
    if !name.ends_with(']') {
        return None;
    }
    let base = &name[..bracket];
    if !is_arith_identifier(base) {
        return None;
    }
    let subscript = &name[bracket + 1..name.len() - 1];
    Some((base, subscript))
}

fn is_arith_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn unescape_assoc_subscript(subscript: &str) -> String {
    let mut out = String::with_capacity(subscript.len());
    let mut chars = subscript.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            } else {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn eval_array_index(subscript: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    let expand_subscript = ctx.arith_expand_subscripts && !ctx.env.option("assoc_expand_once");
    let expanded = if expand_subscript {
        crate::expand_arith_string_to_string_impl(subscript, ctx)?
    } else {
        subscript.to_string()
    };
    if expanded.trim().is_empty() {
        return Ok(0);
    }
    if !ctx.arith_expand_subscripts {
        if let Some(unquoted) = strip_arith_subscript_double_quotes(&expanded) {
            if unquoted.trim().is_empty() {
                return Ok(0);
            }
        }
    }
    if let Some(pos) = expanded.find("\\]") {
        return Err(ExpandError::ArithSyntax(format!(
            "{}: arithmetic syntax error: invalid arithmetic operator (error token is \"{}\")",
            expanded,
            quote_error_token(&expanded[pos..])
        )));
    }
    let prev = ctx.arith_expand_subscripts;
    ctx.arith_expand_subscripts = false;
    let result = (|| {
        let mut sub_lex = Lexer::new(expanded.as_bytes())?;
        let idx = parse_comma(&mut sub_lex, ctx, true)?;
        if sub_lex.cur != Tok::Eof {
            return Err(arith_syntax_error_in_expression(
                sub_lex.src,
                sub_lex.tok_start,
                sub_lex.tok_end,
            ));
        }
        Ok(idx)
    })();
    ctx.arith_expand_subscripts = prev;
    result
}

fn strip_arith_subscript_double_quotes(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = 1usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() - 1 {
            i += 1;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Some(out)
}

fn parse_primary(lex: &mut Lexer<'_>, ctx: &mut ExpCtx, active: bool) -> Result<i64, ExpandError> {
    match lex.cur.clone() {
        Tok::Num(n) => {
            lex.advance()?;
            Ok(if active { n } else { 0 })
        }
        Tok::Ident(name) => {
            lex.advance()?;
            if active {
                load_ident_value(&name, ctx)
            } else {
                Ok(0)
            }
        }
        Tok::LParen => {
            lex.advance()?;
            let v = parse_comma(lex, ctx, active)?;
            if lex.cur != Tok::RParen {
                return Err(arith_missing_rparen(lex.src, lex.tok_start));
            }
            lex.advance()?;
            Ok(if active { v } else { 0 })
        }
        _ => Err(arith_operand_expected(lex.src, lex.tok_start)),
    }
}

fn pow_i64(b: i64, e: i64) -> Result<i64, ExpandError> {
    if e < 0 {
        return Ok(0);
    }
    let mut result: i64 = 1;
    let mut base = b;
    let mut exp = e as u64;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        exp >>= 1;
        if exp > 0 {
            base = base.wrapping_mul(base);
        }
    }
    Ok(result)
}

/// Load `name` as an integer, recursively resolving when the variable's value
/// is itself an identifier (bash's quirk: `x=y; y=5; $((x))` yields 5). Empty
/// or undefined → 0.
fn load_ident_value(name: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    // Strip subscript suffix like `arr[5]` and resolve.
    if array_ref_parts(name).is_some() {
        let lvalue = resolve_lvalue(name, ctx)?;
        return load_lvalue_value(&lvalue, ctx);
    }
    if let Some(v) = ctx.env.get_cow(name) {
        return finish_loaded_value(prepare_loaded_value(v), ctx, 0);
    }
    if ctx.eval_unbound_error {
        return Err(ExpandError::UnboundVariable(name.to_string()));
    }
    Ok(0)
}

enum LoadedValue {
    Parsed(i64),
    Recurse(String),
}

fn prepare_loaded_value(value: Cow<'_, str>) -> LoadedValue {
    if let Some(value) = parse_plain_value(value.as_ref()) {
        return LoadedValue::Parsed(value);
    }
    LoadedValue::Recurse(value.into_owned())
}

fn finish_loaded_value(
    value: LoadedValue,
    ctx: &mut ExpCtx,
    depth: u32,
) -> Result<i64, ExpandError> {
    match value {
        LoadedValue::Parsed(value) => Ok(value),
        LoadedValue::Recurse(value) => parse_value_recursive(&value, ctx, depth),
    }
}

fn parse_plain_value(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Some(n);
    }
    parse_number(trimmed).ok()
}

fn parse_value_recursive(s: &str, ctx: &mut ExpCtx, depth: u32) -> Result<i64, ExpandError> {
    let trimmed = s.trim();
    if depth > MAX_ARITH_NESTING {
        return Err(arith_recursion_error(trimmed));
    }
    let _guard = ValueRecursionGuard::enter().map_err(|_| arith_recursion_error(trimmed))?;
    if trimmed.is_empty() {
        return Ok(0);
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(n);
    }
    if let Ok(n) = parse_number(trimmed) {
        return Ok(n);
    }
    if array_ref_parts(trimmed).is_some() {
        let lvalue = resolve_lvalue(trimmed, ctx)?;
        return load_lvalue_value(&lvalue, ctx);
    }
    // If it looks like another identifier, follow.
    if trimmed
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '[' || c == ']')
    {
        if let Some(v) = ctx.env.get_cow(trimmed) {
            if v.as_ref().trim() == trimmed {
                return Err(arith_recursion_error(trimmed));
            }
            if v.as_ref() != s {
                return finish_loaded_value(prepare_loaded_value(v), ctx, depth + 1);
            }
        }
        return Ok(0);
    }
    // Otherwise: evaluate the value as an arithmetic expression.
    let prev = ctx.arith_expand_subscripts;
    ctx.arith_expand_subscripts = true;
    let expr = s.trim_start();
    let result = (|| {
        let mut lex = Lexer::new(expr.as_bytes())?;
        parse_comma(&mut lex, ctx, true)
    })();
    ctx.arith_expand_subscripts = prev;
    result
}

fn arith_recursion_error(token: &str) -> ExpandError {
    let token = if token.is_empty() { "" } else { token };
    ExpandError::ArithSyntax(format!(
        "{token}: expression recursion level exceeded (error token is \"{}\")",
        quote_error_token(token)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{ExpCtx, NullRunner};
    use std::collections::HashMap;

    #[derive(Default)]
    struct E {
        vars: HashMap<String, String>,
        arrays: HashMap<String, HashMap<i64, String>>,
    }
    impl Environment for E {
        fn get(&self, n: &str) -> Option<String> {
            self.vars.get(n).cloned()
        }
        fn set(&mut self, n: &str, v: String) {
            self.vars.insert(n.into(), v);
        }
        fn unset(&mut self, n: &str) {
            self.vars.remove(n);
        }
        fn exported(&self, _: &str) -> bool {
            false
        }
        fn export(&mut self, _: &str) {}
        fn positional(&self, _: usize) -> Option<String> {
            None
        }
        fn positional_count(&self) -> usize {
            0
        }
        fn set_positionals(&mut self, _: Vec<String>) {}
        fn last_status(&self) -> i32 {
            0
        }
        fn set_last_status(&mut self, _: i32) {}

        fn set_array_indexed(&mut self, name: &str, index: i64, value: String) {
            self.arrays
                .entry(name.to_string())
                .or_default()
                .insert(index, value);
        }

        fn get_array_indexed(&self, name: &str, index: i64) -> Option<String> {
            self.arrays
                .get(name)
                .and_then(|arr| arr.get(&index).cloned())
        }

        fn kind(&self, name: &str) -> VarKind {
            if self.arrays.contains_key(name) {
                VarKind::Indexed
            } else if self.vars.contains_key(name) {
                VarKind::Scalar
            } else {
                VarKind::Unset
            }
        }
    }

    fn ev(expr: &str, env: &mut E) -> i64 {
        let mut runner = NullRunner;
        let mut ctx = ExpCtx::new(env, &mut runner);
        eval(expr, &mut ctx).unwrap()
    }

    #[test]
    fn add_sub() {
        let mut e = E::default();
        assert_eq!(ev("1 + 2 - 3", &mut e), 0);
    }
    #[test]
    fn precedence() {
        let mut e = E::default();
        assert_eq!(ev("2 + 3 * 4", &mut e), 14);
        assert_eq!(ev("(2 + 3) * 4", &mut e), 20);
    }
    #[test]
    fn power_right_assoc() {
        let mut e = E::default();
        assert_eq!(ev("2 ** 3 ** 2", &mut e), 512);
    }
    #[test]
    fn assign_and_read() {
        let mut e = E::default();
        assert_eq!(ev("x = 7", &mut e), 7);
        assert_eq!(e.get("x"), Some("7".into()));
        assert_eq!(ev("x += 3", &mut e), 10);
    }
    #[test]
    fn ternary() {
        let mut e = E::default();
        assert_eq!(ev("1 ? 10 : 20", &mut e), 10);
        assert_eq!(ev("0 ? 10 : 20", &mut e), 20);
    }

    #[test]
    fn ternary_skips_unselected_side_effects_and_errors() {
        let mut e = E::default();
        e.vars.insert("x".into(), "1".into());
        e.vars.insert("y".into(), "1".into());
        e.vars.insert("z".into(), "1".into());

        assert_eq!(ev("1 ? 20 : (x += 2)", &mut e), 20);
        assert_eq!(e.get("x"), Some("1".into()));
        assert_eq!(ev("0 ? (y += 2) : 30", &mut e), 30);
        assert_eq!(e.get("y"), Some("1".into()));
        assert_eq!(ev("0 ? z += 2 : 40", &mut e), 40);
        assert_eq!(e.get("z"), Some("1".into()));
        assert_eq!(ev("1 ? 32 : 1 / 0", &mut e), 32);
        assert_eq!(ev("0 ? 1 / 0 : 32", &mut e), 32);
    }

    #[test]
    fn logical_operators_skip_unselected_side_effects_and_errors() {
        let mut e = E::default();
        e.vars.insert("x".into(), "1".into());
        e.vars.insert("y".into(), "1".into());

        assert_eq!(ev("0 && (x += 2)", &mut e), 0);
        assert_eq!(e.get("x"), Some("1".into()));
        assert_eq!(ev("1 || (y += 2)", &mut e), 1);
        assert_eq!(e.get("y"), Some("1".into()));
        assert_eq!(ev("0 && 1 / 0", &mut e), 0);
        assert_eq!(ev("1 || 1 / 0", &mut e), 1);
    }

    #[test]
    fn arithmetic_expansion_disables_process_substitution() {
        let mut e = E::default();
        let mut runner = NullRunner;

        assert_eq!(
            crate::expand_for_arith("4>(2+3) ? 1 : 32", &mut e, &mut runner).unwrap(),
            32
        );
        assert_eq!(
            crate::expand_for_arith("4<(2+3) ? 1 : 32", &mut e, &mut runner).unwrap(),
            1
        );
        assert_eq!(
            crate::expand_string_to_string("$((4>(2+3) ? 1 : 32))", &mut e, &mut runner).unwrap(),
            "32"
        );
    }

    #[test]
    fn base_hash() {
        let mut e = E::default();
        assert_eq!(ev("16#ff", &mut e), 255);
        assert_eq!(ev("2#1010", &mut e), 10);
    }

    #[test]
    fn arithmetic_wraps_like_bash_intmax_t() {
        let mut e = E::default();
        assert_eq!(ev("9223372036854775807 + 1", &mut e), i64::MIN);
        assert_eq!(ev("9223372036854775808", &mut e), i64::MIN);
        assert_eq!(ev("16#ffffffffffffffff", &mut e), -1);
        assert_eq!(ev("2 ** 64", &mut e), 0);
        assert_eq!(ev("-9223372036854775808 / -1", &mut e), i64::MIN);
        assert_eq!(ev("-9223372036854775808 % -1", &mut e), 0);
    }

    #[test]
    fn ident_recursion() {
        let mut e = E::default();
        e.vars.insert("y".into(), "5".into());
        e.vars.insert("x".into(), "y".into());
        assert_eq!(ev("x", &mut e), 5);
    }
    #[test]
    fn post_inc() {
        let mut e = E::default();
        e.vars.insert("c".into(), "1".into());
        assert_eq!(ev("c++", &mut e), 1);
        assert_eq!(e.get("c"), Some("2".into()));
    }

    #[test]
    fn indexed_array_increment_stores_element() {
        let mut e = E::default();
        assert_eq!(ev("array[0]++", &mut e), 0);
        assert_eq!(e.get_array_indexed("array", 0), Some("1".into()));
        assert_eq!(ev("++array[1 + 1]", &mut e), 1);
        assert_eq!(e.get_array_indexed("array", 2), Some("1".into()));
    }

    #[test]
    fn recursive_array_zero_expression_can_mutate_array() {
        let mut e = E::default();
        e.vars.insert("n".into(), "0".into());
        e.set_array_indexed("a", 0, "(a[n]=++n)<7&&a[0]".into());

        assert_eq!(ev("a[0]", &mut e), 0);
        assert_eq!(e.get("n"), Some("7".into()));
        for index in 1..=7 {
            assert_eq!(e.get_array_indexed("a", index), Some(index.to_string()));
        }
    }

    #[test]
    fn recursive_array_reference_expands_unset_dollar_subscript_to_zero() {
        let mut e = E::default();
        e.vars.insert("d".into(), "1-+1".into());
        e.vars.insert("x".into(), "b[$d]".into());

        assert_eq!(ev("x", &mut e), 0);
    }

    #[test]
    fn compound_array_assignment_evaluates_subscript_once() {
        let mut e = E::default();
        e.vars.insert("n".into(), "0".into());
        e.set_array_indexed("a", 0, "5".into());

        assert_eq!(ev("a[n++] += n", &mut e), 6);
        assert_eq!(e.get("n"), Some("1".into()));
        assert_eq!(e.get_array_indexed("a", 0), Some("6".into()));
        assert_eq!(e.get_array_indexed("a", 1), None);
    }

    #[test]
    fn adjacent_binary_plus_minus_preserve_prefix_inc_dec() {
        let mut e = E::default();
        e.vars.insert("a".into(), "1".into());

        assert_eq!(ev("4+++a", &mut e), 6);
        assert_eq!(e.get("a"), Some("2".into()));
        assert_eq!(ev("4---a", &mut e), 3);
        assert_eq!(e.get("a"), Some("1".into()));

        e.vars.insert("a".into(), "1".into());
        assert_eq!(ev("+++a", &mut e), 2);
        assert_eq!(e.get("a"), Some("2".into()));

        e.vars.insert("a".into(), "1".into());
        assert_eq!(ev("++ +a", &mut e), 1);
        assert_eq!(e.get("a"), Some("1".into()));
    }

    #[test]
    fn repeated_unary_signs_are_not_prefix_inc_dec_without_ident() {
        let mut e = E::default();

        assert_eq!(ev("++7", &mut e), 7);
        assert_eq!(ev("--7", &mut e), 7);
        assert_eq!(ev("+++7", &mut e), 7);
        assert_eq!(ev("---7", &mut e), -7);
        assert_eq!(ev("++ + 7", &mut e), 7);
        assert_eq!(ev("-- - 7", &mut e), -7);
        assert_eq!(ev("++-7", &mut e), -7);
        assert_eq!(ev("+--7", &mut e), 7);
    }
}
