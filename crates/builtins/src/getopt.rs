//! Builtin option parser modelled on bash's `bashgetopt.c::internal_getopt`.
//!
//! Differences from POSIX getopt:
//! - Supports `+x` to clear an option (return ('+', 'x')).
//! - Stops on `--`.
//! - Returns `?` on unknown options; missing-argument is `:`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GetOpt {
    /// A short option like `-r`. `plus` is true if invoked as `+r`.
    Opt {
        plus: bool,
        ch: char,
        arg: Option<String>,
    },
    /// `--` separator encountered.
    End,
    /// No more options; positional starts at `index`.
    Done,
    /// Unknown option `ch`.
    Unknown { plus: bool, ch: char },
    /// Option `ch` required an argument and none was provided.
    Missing { plus: bool, ch: char },
}

#[derive(Debug, Clone)]
pub struct OptParser<'a> {
    args: &'a [String],
    spec: &'a str,
    pos: usize,
    inner: usize,
    pub index: usize,
}

impl<'a> OptParser<'a> {
    pub fn new(args: &'a [String], spec: &'a str) -> Self {
        Self {
            args,
            spec,
            pos: 0,
            inner: 0,
            index: 0,
        }
    }

    pub fn next(&mut self) -> GetOpt {
        if self.pos >= self.args.len() {
            self.index = self.pos;
            return GetOpt::Done;
        }
        let arg = &self.args[self.pos];
        if self.inner == 0 {
            if arg == "--" {
                self.index = self.pos + 1;
                self.pos = self.args.len();
                return GetOpt::End;
            }
            if arg.is_empty()
                || (!arg.starts_with('-') && !arg.starts_with('+'))
                || arg == "-"
                || arg == "+"
            {
                self.index = self.pos;
                return GetOpt::Done;
            }
            self.inner = 1;
        }
        let bytes: Vec<char> = arg.chars().collect();
        let plus = bytes[0] == '+';
        if self.inner >= bytes.len() {
            self.pos += 1;
            self.inner = 0;
            return self.next();
        }
        let ch = bytes[self.inner];
        self.inner += 1;
        if let Some(idx) = self.spec.chars().position(|c| c == ch) {
            let needs_arg = self
                .spec
                .chars()
                .nth(idx + 1)
                .map(|c| c == ':')
                .unwrap_or(false);
            if needs_arg {
                let rest: String = bytes[self.inner..].iter().collect();
                let value = if !rest.is_empty() {
                    self.pos += 1;
                    self.inner = 0;
                    rest
                } else {
                    self.pos += 1;
                    self.inner = 0;
                    if self.pos >= self.args.len() {
                        return GetOpt::Missing { plus, ch };
                    }
                    let v = self.args[self.pos].clone();
                    self.pos += 1;
                    v
                };
                return GetOpt::Opt {
                    plus,
                    ch,
                    arg: Some(value),
                };
            }
            self.index = if self.inner >= bytes.len() {
                self.pos + 1
            } else {
                self.pos
            };
            return GetOpt::Opt {
                plus,
                ch,
                arg: None,
            };
        }
        GetOpt::Unknown { plus, ch }
    }

    /// Remaining args after option parsing finishes.
    pub fn remaining<'b>(&self, args: &'b [String]) -> &'b [String] {
        &args[self.index..]
    }
}

impl fmt::Display for GetOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetOpt::Opt { plus, ch, .. } => {
                write!(f, "{}{}", if *plus { "+" } else { "-" }, ch)
            }
            GetOpt::End => write!(f, "--"),
            GetOpt::Done => write!(f, ""),
            GetOpt::Unknown { plus, ch } => {
                write!(f, "{}{}", if *plus { "+" } else { "-" }, ch)
            }
            GetOpt::Missing { plus, ch } => {
                write!(f, "{}{}", if *plus { "+" } else { "-" }, ch)
            }
        }
    }
}
