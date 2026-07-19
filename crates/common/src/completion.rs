//! Programmable-completion data model.
//!
//! `CompSpec` mirrors the bash `complete -p` rendering: each registered
//! command has a single spec built from the `-A/-F/-G/-W/-X/-P/-S/-o` flags.
//! The completion engine lives in `crates/shell/src/completion.rs` which
//! consumes these and emits candidate matches; this module is just the
//! storage and option-flag types so every crate can refer to them.

use bitflags::bitflags;
use std::collections::HashSet;
use std::path::Path;

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
    pub const GENERATION_ORDER: [Self; 24] = [
        Self::Alias,
        Self::ArrayVar,
        Self::Binding,
        Self::Builtin,
        Self::Disabled,
        Self::Enabled,
        Self::Export,
        Self::Function,
        Self::HelpTopic,
        Self::HostName,
        Self::Job,
        Self::Keyword,
        Self::Running,
        Self::SetOpt,
        Self::ShOpt,
        Self::Signal,
        Self::Stopped,
        Self::Variable,
        Self::Command,
        Self::File,
        Self::User,
        Self::Group,
        Self::Service,
        Self::Directory,
    ];

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
        const FULLQUOTE   = 1 << 8;
    }
}

impl CompOpts {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bashdefault" => Self::BASHDEFAULT,
            "default" => Self::DEFAULT,
            "dirnames" => Self::DIRNAMES,
            "filenames" => Self::FILENAMES,
            "fullquote" => Self::FULLQUOTE,
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
            (CompOpts::FULLQUOTE, "fullquote"),
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
        for a in [
            CompAction::Alias,
            CompAction::Builtin,
            CompAction::Command,
            CompAction::Directory,
            CompAction::Export,
            CompAction::File,
            CompAction::Group,
            CompAction::Job,
            CompAction::Keyword,
            CompAction::Service,
            CompAction::User,
            CompAction::Variable,
            CompAction::ArrayVar,
            CompAction::Binding,
            CompAction::Disabled,
            CompAction::Enabled,
            CompAction::Function,
            CompAction::HelpTopic,
            CompAction::HostName,
            CompAction::Running,
            CompAction::SetOpt,
            CompAction::ShOpt,
            CompAction::Signal,
            CompAction::Stopped,
        ]
        .into_iter()
        .filter(|action| self.actions.contains(action))
        {
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
            out.push_str(&quote_complete_arg_always(g));
        }
        if let Some(w) = &self.wordlist {
            out.push_str(" -W ");
            out.push_str(&quote_complete_arg_always(w));
        }
        if let Some(f) = &self.function {
            out.push_str(" -F ");
            out.push_str(&quote_complete_arg(f));
        }
        if let Some(c) = &self.command {
            out.push_str(" -C ");
            out.push_str(&quote_complete_arg_always(c));
        }
        if let Some(x) = &self.filterpat {
            out.push_str(" -X ");
            out.push_str(&quote_complete_arg_always(x));
        }
        if let Some(p) = &self.prefix {
            out.push_str(" -P ");
            out.push_str(&quote_complete_arg_always(p));
        }
        if let Some(s) = &self.suffix {
            out.push_str(" -S ");
            out.push_str(&quote_complete_arg_always(s));
        }
        out
    }
}

fn quote_complete_arg_always(value: &str) -> String {
    shell_single_quote(value)
}

fn quote_complete_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '/' | '.' | '-'))
    {
        return value.to_string();
    }
    shell_single_quote(value)
}

fn shell_single_quote(value: &str) -> String {
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

pub fn system_user_names() -> Vec<String> {
    parse_colon_names("/etc/passwd")
}

pub fn system_group_names() -> Vec<String> {
    parse_colon_names("/etc/group")
}

fn parse_colon_names(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn matching_service_names(prefix: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in std::fs::read_to_string("/etc/services")
        .unwrap_or_default()
        .lines()
    {
        let fields = line
            .split('#')
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>();
        if fields.len() >= 2 {
            if fields[0].starts_with(prefix) {
                names.push(fields[0].to_string());
            } else if let Some(alias) = fields[2..].iter().find(|name| name.starts_with(prefix)) {
                names.push((*alias).to_string());
            }
        }
    }
    names
}

pub fn hostname_names(source: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    let mut active = HashSet::new();
    read_hostname_file(
        Path::new(source.unwrap_or("/etc/hosts")),
        &mut active,
        &mut names,
    );
    names
}

fn read_hostname_file(
    path: &Path,
    active: &mut HashSet<std::path::PathBuf>,
    names: &mut Vec<String>,
) {
    let key = path.to_path_buf();
    if !active.insert(key.clone()) {
        return;
    }
    let contents = std::fs::read_to_string(path).unwrap_or_default();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(include) = trimmed.strip_prefix("$include ") {
            if let Some(include) = include.split_whitespace().next() {
                read_hostname_file(Path::new(include), active, names);
            }
            continue;
        }
        let content = trimmed.split('#').next().unwrap_or_default();
        let mut fields = content.split_whitespace();
        if content.starts_with(|ch: char| ch.is_ascii_digit()) {
            fields.next();
        }
        names.extend(fields.map(str::to_string));
    }
    active.remove(&key);
}

pub fn signal_names() -> Vec<String> {
    [
        "EXIT",
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
        "SIGJUNK(32)",
        "SIGJUNK(33)",
        "SIGRTMIN",
        "SIGRTMIN+1",
        "SIGRTMIN+2",
        "SIGRTMIN+3",
        "SIGRTMIN+4",
        "SIGRTMIN+5",
        "SIGRTMIN+6",
        "SIGRTMIN+7",
        "SIGRTMIN+8",
        "SIGRTMIN+9",
        "SIGRTMIN+10",
        "SIGRTMIN+11",
        "SIGRTMIN+12",
        "SIGRTMIN+13",
        "SIGRTMIN+14",
        "SIGRTMIN+15",
        "SIGRTMAX-14",
        "SIGRTMAX-13",
        "SIGRTMAX-12",
        "SIGRTMAX-11",
        "SIGRTMAX-10",
        "SIGRTMAX-9",
        "SIGRTMAX-8",
        "SIGRTMAX-7",
        "SIGRTMAX-6",
        "SIGRTMAX-5",
        "SIGRTMAX-4",
        "SIGRTMAX-3",
        "SIGRTMAX-2",
        "SIGRTMAX-1",
        "SIGRTMAX",
        "DEBUG",
        "ERR",
        "RETURN",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn locale_sort(values: &mut [String]) {
    values.sort_by(|left, right| {
        let left = std::ffi::CString::new(left.as_str()).expect("completion contains a NUL byte");
        let right = std::ffi::CString::new(right.as_str()).expect("completion contains a NUL byte");
        unsafe { libc::strcoll(left.as_ptr(), right.as_ptr()) }.cmp(&0)
    });
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
