//! Master table of `shopt` options. Mirrors bash-5.2.21/builtins/shopt.def.

#[derive(Debug, Clone, Copy)]
pub struct ShoptOption {
    pub name: &'static str,
    pub default: bool,
}

pub const SHOPT_OPTIONS: &[ShoptOption] = &[
    ShoptOption {
        name: "autocd",
        default: false,
    },
    ShoptOption {
        name: "assoc_expand_once",
        default: false,
    },
    ShoptOption {
        name: "cdable_vars",
        default: false,
    },
    ShoptOption {
        name: "cdspell",
        default: false,
    },
    ShoptOption {
        name: "checkhash",
        default: false,
    },
    ShoptOption {
        name: "checkjobs",
        default: false,
    },
    ShoptOption {
        name: "checkwinsize",
        default: true,
    },
    ShoptOption {
        name: "cmdhist",
        default: true,
    },
    ShoptOption {
        name: "compat31",
        default: false,
    },
    ShoptOption {
        name: "compat32",
        default: false,
    },
    ShoptOption {
        name: "compat40",
        default: false,
    },
    ShoptOption {
        name: "compat41",
        default: false,
    },
    ShoptOption {
        name: "compat42",
        default: false,
    },
    ShoptOption {
        name: "compat43",
        default: false,
    },
    ShoptOption {
        name: "compat44",
        default: false,
    },
    ShoptOption {
        name: "complete_fullquote",
        default: true,
    },
    ShoptOption {
        name: "direxpand",
        default: false,
    },
    ShoptOption {
        name: "dirspell",
        default: false,
    },
    ShoptOption {
        name: "dotglob",
        default: false,
    },
    ShoptOption {
        name: "execfail",
        default: false,
    },
    ShoptOption {
        name: "expand_aliases",
        default: false,
    },
    ShoptOption {
        name: "extdebug",
        default: false,
    },
    ShoptOption {
        name: "extglob",
        default: false,
    },
    ShoptOption {
        name: "extquote",
        default: true,
    },
    ShoptOption {
        name: "failglob",
        default: false,
    },
    ShoptOption {
        name: "force_fignore",
        default: true,
    },
    ShoptOption {
        name: "globasciiranges",
        default: true,
    },
    ShoptOption {
        name: "globskipdots",
        default: true,
    },
    ShoptOption {
        name: "globstar",
        default: false,
    },
    ShoptOption {
        name: "gnu_errfmt",
        default: false,
    },
    ShoptOption {
        name: "histappend",
        default: false,
    },
    ShoptOption {
        name: "histreedit",
        default: false,
    },
    ShoptOption {
        name: "histverify",
        default: false,
    },
    ShoptOption {
        name: "hostcomplete",
        default: true,
    },
    ShoptOption {
        name: "huponexit",
        default: false,
    },
    ShoptOption {
        name: "inherit_errexit",
        default: false,
    },
    ShoptOption {
        name: "interactive_comments",
        default: true,
    },
    ShoptOption {
        name: "lastpipe",
        default: false,
    },
    ShoptOption {
        name: "lithist",
        default: false,
    },
    ShoptOption {
        name: "localvar_inherit",
        default: false,
    },
    ShoptOption {
        name: "localvar_unset",
        default: false,
    },
    ShoptOption {
        name: "login_shell",
        default: false,
    },
    ShoptOption {
        name: "mailwarn",
        default: false,
    },
    ShoptOption {
        name: "no_empty_cmd_completion",
        default: false,
    },
    ShoptOption {
        name: "nocaseglob",
        default: false,
    },
    ShoptOption {
        name: "nocasematch",
        default: false,
    },
    ShoptOption {
        name: "noexpand_translation",
        default: false,
    },
    ShoptOption {
        name: "nullglob",
        default: false,
    },
    ShoptOption {
        name: "patsub_replacement",
        default: true,
    },
    ShoptOption {
        name: "progcomp",
        default: true,
    },
    ShoptOption {
        name: "progcomp_alias",
        default: false,
    },
    ShoptOption {
        name: "promptvars",
        default: true,
    },
    ShoptOption {
        name: "restricted_shell",
        default: false,
    },
    ShoptOption {
        name: "shift_verbose",
        default: false,
    },
    ShoptOption {
        name: "sourcepath",
        default: true,
    },
    ShoptOption {
        name: "varredir_close",
        default: false,
    },
    ShoptOption {
        name: "xpg_echo",
        default: false,
    },
];

pub fn lookup(name: &str) -> Option<&'static ShoptOption> {
    SHOPT_OPTIONS.iter().find(|o| o.name == name)
}

pub fn iter() -> impl Iterator<Item = &'static ShoptOption> {
    SHOPT_OPTIONS.iter()
}
