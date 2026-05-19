//! Programmable-completion data model.
//!
//! `CompSpec` mirrors the bash `complete -p` rendering: each registered
//! command has a single spec built from the `-A/-F/-G/-W/-X/-P/-S/-o` flags.
//! The completion engine lives in `crates/shell/src/completion.rs` which
//! consumes these and emits candidate matches; this module is just the
//! storage and option-flag types so every crate can refer to them.

use bitflags::bitflags;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompAction {
    Alias,
    ArrayVar,
    Binding,
    Builtin,
    Command,
    Directory,
    Disabled,
    Enabled,
    Export,
    File,
    Function,
    Group,
    HelpTopic,
    HostName,
    Job,
    Keyword,
    Running,
    Service,
    SetOpt,
    ShOpt,
    Signal,
    Stopped,
    User,
    Variable,
}

impl CompAction {
    /// Map a `-A name` token to an action. None on unknown.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "alias" => Self::Alias,
            "arrayvar" => Self::ArrayVar,
            "binding" => Self::Binding,
            "builtin" => Self::Builtin,
            "command" => Self::Command,
            "directory" => Self::Directory,
            "disabled" => Self::Disabled,
            "enabled" => Self::Enabled,
            "export" => Self::Export,
            "file" => Self::File,
            "function" => Self::Function,
            "group" => Self::Group,
            "helptopic" => Self::HelpTopic,
            "hostname" => Self::HostName,
            "job" => Self::Job,
            "keyword" => Self::Keyword,
            "running" => Self::Running,
            "service" => Self::Service,
            "setopt" => Self::SetOpt,
            "shopt" => Self::ShOpt,
            "signal" => Self::Signal,
            "stopped" => Self::Stopped,
            "user" => Self::User,
            "variable" => Self::Variable,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::ArrayVar => "arrayvar",
            Self::Binding => "binding",
            Self::Builtin => "builtin",
            Self::Command => "command",
            Self::Directory => "directory",
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
            Self::Export => "export",
            Self::File => "file",
            Self::Function => "function",
            Self::Group => "group",
            Self::HelpTopic => "helptopic",
            Self::HostName => "hostname",
            Self::Job => "job",
            Self::Keyword => "keyword",
            Self::Running => "running",
            Self::Service => "service",
            Self::SetOpt => "setopt",
            Self::ShOpt => "shopt",
            Self::Signal => "signal",
            Self::Stopped => "stopped",
            Self::User => "user",
            Self::Variable => "variable",
        }
    }
}

bitflags! {
    /// Flags driven by `complete -o <name>`. Bash treats them as a bitmask.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CompOpts: u32 {
        const BASHDEFAULT = 1 << 0;
        const DEFAULT     = 1 << 1;
        const DIRNAMES    = 1 << 2;
        const FILENAMES   = 1 << 3;
        const NOQUOTE     = 1 << 4;
        const NOSORT      = 1 << 5;
        const NOSPACE     = 1 << 6;
        const PLUSDIRS    = 1 << 7;
    }
}

impl CompOpts {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bashdefault" => Self::BASHDEFAULT,
            "default" => Self::DEFAULT,
            "dirnames" => Self::DIRNAMES,
            "filenames" => Self::FILENAMES,
            "noquote" => Self::NOQUOTE,
            "nosort" => Self::NOSORT,
            "nospace" => Self::NOSPACE,
            "plusdirs" => Self::PLUSDIRS,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CompSpec {
    /// `-A` action bits.
    pub actions: Vec<CompAction>,
    /// `-G glob` pattern.
    pub glob_pattern: Option<String>,
    /// `-W word-list` (raw, expanded at completion time).
    pub wordlist: Option<String>,
    /// `-F function` name to invoke (COMPREPLY-style).
    pub function: Option<String>,
    /// `-C cmd` to invoke and read whitespace-separated matches from stdout.
    pub command: Option<String>,
    /// `-X filterpat` - patterns to exclude from matches.
    pub filterpat: Option<String>,
    /// `-P prefix` prepended to every returned match.
    pub prefix: Option<String>,
    /// `-S suffix` appended to every returned match.
    pub suffix: Option<String>,
    /// `-o` option bits.
    pub options: CompOpts,
}

impl CompSpec {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
            && self.glob_pattern.is_none()
            && self.wordlist.is_none()
            && self.function.is_none()
            && self.command.is_none()
            && self.filterpat.is_none()
            && self.prefix.is_none()
            && self.suffix.is_none()
            && self.options.is_empty()
    }

    /// Reproduce `complete -p`'s output flags (used in builtin listing).
    pub fn render_flags(&self) -> String {
        let mut out = String::new();
        for (bit, name) in [
            (CompOpts::BASHDEFAULT, "bashdefault"),
            (CompOpts::DEFAULT, "default"),
            (CompOpts::DIRNAMES, "dirnames"),
            (CompOpts::FILENAMES, "filenames"),
            (CompOpts::NOQUOTE, "noquote"),
            (CompOpts::NOSORT, "nosort"),
            (CompOpts::NOSPACE, "nospace"),
            (CompOpts::PLUSDIRS, "plusdirs"),
        ] {
            if self.options.contains(bit) {
                out.push_str(" -o ");
                out.push_str(name);
            }
        }
        for a in &self.actions {
            if let Some(short) = a.short_flag() {
                out.push_str(" -");
                out.push(short);
            } else {
                out.push_str(" -A ");
                out.push_str(a.as_str());
            }
        }
        if let Some(g) = &self.glob_pattern {
            out.push_str(" -G ");
            out.push_str(&quote_complete_arg(g));
        }
        if let Some(w) = &self.wordlist {
            out.push_str(" -W ");
            out.push_str(&quote_complete_arg(w));
        }
        if let Some(f) = &self.function {
            out.push_str(" -F ");
            out.push_str(&quote_complete_arg(f));
        }
        if let Some(c) = &self.command {
            out.push_str(" -C ");
            out.push_str(&quote_complete_arg(c));
        }
        if let Some(x) = &self.filterpat {
            out.push_str(" -X ");
            out.push_str(&quote_complete_arg(x));
        }
        if let Some(p) = &self.prefix {
            out.push_str(" -P ");
            out.push_str(&quote_complete_arg(p));
        }
        if let Some(s) = &self.suffix {
            out.push_str(" -S ");
            out.push_str(&quote_complete_arg(s));
        }
        out
    }
}

fn quote_complete_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '/' | '.' | '-'))
    {
        return value.to_string();
    }
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

impl CompAction {
    fn short_flag(self) -> Option<char> {
        Some(match self {
            Self::Alias => 'a',
            Self::Builtin => 'b',
            Self::Command => 'c',
            Self::Directory => 'd',
            Self::Export => 'e',
            Self::File => 'f',
            Self::Group => 'g',
            Self::Job => 'j',
            Self::Keyword => 'k',
            Self::Service => 's',
            Self::User => 'u',
            Self::Variable => 'v',
            _ => return None,
        })
    }
}

/// Type discriminator on which "slot" a spec occupies - bash supports
/// command-keyed, default (`-D`), initial-word (`-I`), and empty-line (`-E`)
/// fallbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompSlot {
    Command,
    Default,
    Initial,
    Empty,
}
