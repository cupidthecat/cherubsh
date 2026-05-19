//! Master table of shell options consulted by `set -o` and `set -<flag>`.
//!
//! Mirrors bash-5.2.21/builtins/set.def. Names without a short letter still
//! have a long name reachable via `set -o`.

#[derive(Debug, Clone, Copy)]
pub struct SetOption {
    pub short: Option<char>,
    pub long: &'static str,
    pub default: bool,
}

pub const SET_OPTIONS: &[SetOption] = &[
    SetOption {
        short: Some('a'),
        long: "allexport",
        default: false,
    },
    SetOption {
        short: Some('B'),
        long: "braceexpand",
        default: true,
    },
    SetOption {
        short: None,
        long: "emacs",
        default: false,
    },
    SetOption {
        short: Some('e'),
        long: "errexit",
        default: false,
    },
    SetOption {
        short: Some('E'),
        long: "errtrace",
        default: false,
    },
    SetOption {
        short: Some('T'),
        long: "functrace",
        default: false,
    },
    SetOption {
        short: Some('h'),
        long: "hashall",
        default: true,
    },
    SetOption {
        short: Some('H'),
        long: "histexpand",
        default: false,
    },
    SetOption {
        short: None,
        long: "history",
        default: false,
    },
    SetOption {
        short: None,
        long: "ignoreeof",
        default: false,
    },
    SetOption {
        short: None,
        long: "interactive-comments",
        default: true,
    },
    SetOption {
        short: Some('k'),
        long: "keyword",
        default: false,
    },
    SetOption {
        short: Some('m'),
        long: "monitor",
        default: false,
    },
    SetOption {
        short: Some('C'),
        long: "noclobber",
        default: false,
    },
    SetOption {
        short: Some('n'),
        long: "noexec",
        default: false,
    },
    SetOption {
        short: Some('f'),
        long: "noglob",
        default: false,
    },
    SetOption {
        short: None,
        long: "nolog",
        default: false,
    },
    SetOption {
        short: Some('b'),
        long: "notify",
        default: false,
    },
    SetOption {
        short: Some('u'),
        long: "nounset",
        default: false,
    },
    SetOption {
        short: Some('t'),
        long: "onecmd",
        default: false,
    },
    SetOption {
        short: Some('P'),
        long: "physical",
        default: false,
    },
    SetOption {
        short: None,
        long: "pipefail",
        default: false,
    },
    SetOption {
        short: None,
        long: "posix",
        default: false,
    },
    SetOption {
        short: Some('p'),
        long: "privileged",
        default: false,
    },
    SetOption {
        short: Some('v'),
        long: "verbose",
        default: false,
    },
    SetOption {
        short: None,
        long: "vi",
        default: false,
    },
    SetOption {
        short: Some('x'),
        long: "xtrace",
        default: false,
    },
];

pub fn lookup_short(ch: char) -> Option<&'static SetOption> {
    SET_OPTIONS.iter().find(|o| o.short == Some(ch))
}

pub fn lookup_long(name: &str) -> Option<&'static SetOption> {
    SET_OPTIONS.iter().find(|o| o.long == name)
}

pub fn iter_long() -> impl Iterator<Item = &'static SetOption> {
    SET_OPTIONS.iter()
}
