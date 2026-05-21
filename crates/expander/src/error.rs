use cherubsh_common::{ShellError, Span};

/// Errors raised by the expansion pipeline. Mirrors bash's `expand_wdesc_error`
/// and `expand_wdesc_fatal` sentinels (subst.c:1086, :1093) with named variants.
#[derive(Debug, Clone)]
pub enum ExpandError {
    BadSubstitution(String),
    UnboundVariable(String),
    UnboundColonError(String, String),
    AssignToReadonly(String),
    DivisionByZero,
    ArithSyntax(String),
    ArithOverflow,
    ArithRecursion,
    BadPattern(String),
    InvalidArraySubscript(String),
    CommandSubstFailed(i32),
    ExitShell(i32),
    AlreadyReported(i32),
    SubstitutionRecursive(u32),
    AmbiguousRedirect(String),
    FailGlob(String),
    Io(String),
    Other(String),
    Fatal(String),
}

impl ExpandError {
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            ExpandError::AssignToReadonly(_)
                | ExpandError::UnboundColonError(_, _)
                | ExpandError::Fatal(_)
        )
    }

    pub fn into_shell_error(self, span: Option<Span>) -> ShellError {
        let mut err = match self {
            ExpandError::BadSubstitution(s) => ShellError::new(format!("{}: bad substitution", s)),
            ExpandError::UnboundVariable(s) => ShellError::new(format!("{}: unbound variable", s)),
            ExpandError::UnboundColonError(name, msg) if msg.is_empty() => {
                ShellError::new(format!("{}: parameter null or not set", name))
            }
            ExpandError::UnboundColonError(name, msg) => {
                ShellError::new(format!("{}: {}", name, msg))
            }
            ExpandError::AssignToReadonly(s) => {
                ShellError::new(format!("{}: readonly variable", s))
            }
            ExpandError::DivisionByZero => ShellError::new("division by 0 (error token is \"0\")"),
            ExpandError::ArithSyntax(s) if is_bash_style_arith_syntax(&s) => ShellError::new(s),
            ExpandError::ArithSyntax(s) => ShellError::new(format!("syntax error: {}", s)),
            ExpandError::ArithOverflow => ShellError::new("arithmetic overflow"),
            ExpandError::ArithRecursion => {
                ShellError::new("arithmetic expression recursion limit exceeded")
            }
            ExpandError::BadPattern(p) => ShellError::new(format!("{}: bad pattern", p)),
            ExpandError::InvalidArraySubscript(s) => {
                ShellError::new(format!("{}: bad array subscript", s))
            }
            ExpandError::CommandSubstFailed(c) => {
                ShellError::with_code("command substitution failed".to_string(), c)
            }
            ExpandError::ExitShell(c) => ShellError::with_code(String::new(), c),
            ExpandError::AlreadyReported(c) => ShellError::with_code(String::new(), c),
            ExpandError::SubstitutionRecursive(d) => {
                ShellError::new(format!("expansion nesting too deep: {}", d))
            }
            ExpandError::AmbiguousRedirect(s) => {
                ShellError::new(format!("{}: ambiguous redirect", s))
            }
            ExpandError::FailGlob(s) => ShellError::new(format!("no match: {}", s)),
            ExpandError::Io(s) => ShellError::new(s),
            ExpandError::Other(s) => ShellError::new(s),
            ExpandError::Fatal(s) => ShellError::new(s),
        };
        if let Some(sp) = span {
            err = err.at(sp);
        }
        err
    }

    pub fn already_reported(&self) -> bool {
        matches!(
            self,
            ExpandError::AlreadyReported(_) | ExpandError::ExitShell(_)
        )
    }
}

fn is_bash_style_arith_syntax(message: &str) -> bool {
    message.contains(": syntax error")
        || message.contains(": arithmetic syntax error")
        || message.contains("(error token is ")
        || (message.starts_with('`') && message.contains("not a valid identifier"))
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.clone().into_shell_error(None).message)
    }
}

impl std::error::Error for ExpandError {}

impl From<std::io::Error> for ExpandError {
    fn from(e: std::io::Error) -> Self {
        ExpandError::Io(e.to_string())
    }
}
