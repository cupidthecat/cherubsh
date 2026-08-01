macro_rules! environment_options {
    () => {
    fn option(&self, name: &str) -> bool {
        match name {
            "allexport" | "a" => self.allexport,
            "errexit" | "e" => self.errexit,
            "noglob" | "f" => self.noglob,
            "hashall" | "h" => self.hashall,
            "keyword" | "k" => self.keyword,
            "nounset" | "u" => self.nounset,
            "pipefail" => self.pipefail,
            "lastpipe" => self.lastpipe,
            "noclobber" | "C" => self.noclobber,
            "xtrace" | "x" => self.xtrace,
            "braceexpand" | "B" => self.braceexpand,
            "errtrace" | "E" => self.errtrace,
            "functrace" | "T" => self.functrace,
            "physical" | "P" => self.physical,
            "notify" | "b" => self.notify,
            "monitor" | "m" => self.monitor,
            "ignoreeof" => self.shopt_options.get(name).copied().unwrap_or(false),
            "interactive-comments" | "interactive_comments" => self
                .shopt_options
                .get("interactive_comments")
                .copied()
                .unwrap_or(true),
            "emacs" | "history" | "nolog" | "vi" => {
                self.shopt_options.get(name).copied().unwrap_or(false)
            }
            "noexec" | "n" => self.noexec,
            "onecmd" | "t" => self.just_one_command,
            "privileged" | "p" => self.privileged_mode,
            "verbose" | "v" => self.verbose_flag,
            "histexpand" | "H" => self
                .shopt_options
                .get("histexpand")
                .copied()
                .unwrap_or(false),
            "restricted" | "r" => self.restricted,
            "restricted_shell" => self.restricted,
            "posix" => self.posixly_correct,
            "interactive" | "i" => self.interactive,
            "login_shell" => self.login_shell != 0,
            _ => self.shopt_options.get(name).copied().unwrap_or_else(|| {
                default_shopt_value(name, self.interactive, self.posixly_correct)
            }),
        }
    }

    fn prompt_nonprinting_markers(&self) -> bool {
        (self.interactive_shell && !self.no_line_editing)
            || self.option("emacs")
            || self.option("vi")
    }

    fn prompt_command_number(&self) -> u64 {
        self.current_command_number
    }

    fn prompt_history_number(&self) -> u64 {
        self.history_table.len() as u64 + 1
    }

    fn prompt_job_count(&self) -> usize {
        self.jobs
            .list()
            .iter()
            .filter(|job| job.state != cherubsh_common::JobState::Done)
            .count()
    }

    fn prompt_shell_name(&self) -> Option<String> {
        Some(self.shell_invocation_name.clone())
    }

    fn set_option(&mut self, name: &str, on: bool) {
        match name {
            "allexport" | "a" => self.allexport = on,
            "errexit" | "e" => self.errexit = on,
            "noglob" | "f" => self.noglob = on,
            "hashall" | "h" => self.hashall = on,
            "keyword" | "k" => self.keyword = on,
            "nounset" | "u" => self.nounset = on,
            "pipefail" => self.pipefail = on,
            "lastpipe" => self.lastpipe = on,
            "noclobber" | "C" => self.noclobber = on,
            "xtrace" | "x" => self.xtrace = on,
            "braceexpand" | "B" => self.braceexpand = on,
            "errtrace" | "E" => self.errtrace = on,
            "functrace" | "T" => self.functrace = on,
            "physical" | "P" => self.physical = on,
            "notify" | "b" => self.notify = on,
            "monitor" | "m" => {
                self.monitor = on;
                self.job_control = on;
            }
            "ignoreeof" => {
                self.shopt_options.insert(name.to_string(), on);
                if on {
                    self.variables.insert(
                        "IGNOREEOF".to_string(),
                        VariableEntry {
                            value: "10".to_string(),
                            has_value: true,
                            exported: false,
                            readonly: false,
                            attrs: VarAttrs::empty(),
                        },
                    );
                } else {
                    self.variables.remove("IGNOREEOF");
                    std::env::remove_var("IGNOREEOF");
                }
            }
            "interactive-comments" | "interactive_comments" => {
                self.shopt_options
                    .insert("interactive_comments".to_string(), on);
            }
            "noexec" | "n" => self.noexec = on,
            "onecmd" | "t" => self.just_one_command = on,
            "privileged" | "p" => self.privileged_mode = on,
            "verbose" | "v" => self.verbose_flag = on,
            "histexpand" | "H" => {
                self.shopt_options.insert("histexpand".to_string(), on);
            }
            "history" => {
                let was_on = self.shopt_options.get("history").copied().unwrap_or(false);
                self.shopt_options.insert("history".to_string(), on);
                if on && !was_on {
                    self.configure_history_from_vars(true, false);
                }
            }
            "emacs" => {
                self.shopt_options.insert("emacs".to_string(), on);
                if on {
                    self.shopt_options.insert("vi".to_string(), false);
                    self.keymap_set_active("emacs");
                }
            }
            "vi" => {
                self.shopt_options.insert("vi".to_string(), on);
                if on {
                    self.shopt_options.insert("emacs".to_string(), false);
                    self.keymap_set_active("vi-insert");
                }
            }
            "login_shell" | "restricted_shell" => {}
            name if COMPAT_SHOPT_OPTIONS.contains(&name) => {
                if on {
                    for option in COMPAT_SHOPT_OPTIONS {
                        self.shopt_options.insert((*option).to_string(), false);
                    }
                }
                self.shopt_options.insert(name.to_string(), on);
                let level = COMPAT_SHOPT_OPTIONS
                    .iter()
                    .find(|option| self.shopt_options.get(**option).copied().unwrap_or(false))
                    .and_then(|option| option.strip_prefix("compat"))
                    .unwrap_or("53");
                self.set("BASH_COMPAT", level.to_string());
            }
            "nolog" => {
                self.shopt_options.insert("nolog".to_string(), on);
            }
            "expand_aliases" => {
                self.aliases_enabled = on;
                self.shopt_options.insert(name.to_string(), on);
            }
            "extdebug" => {
                self.shopt_options.insert(name.to_string(), on);
                self.errtrace = on;
                self.functrace = on;
            }
            "restricted" | "r" => self.restricted = on,
            "posix" => {
                self.posixly_correct = on;
                if on {
                    self.shopt_options
                        .insert("inherit_errexit".to_string(), true);
                    self.aliases_enabled = true;
                    self.shopt_options
                        .insert("expand_aliases".to_string(), true);
                }
            }
            _ => {
                self.shopt_options.insert(name.to_string(), on);
            }
        }
        self.sync_export_env("BASHOPTS");
        self.sync_export_env("SHELLOPTS");
    }
    };
}
