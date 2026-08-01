impl ShellState {
    fn option_letters(&self) -> String {
        let mut letters = String::new();
        if self.interactive {
            letters.push('i');
        }
        if self.allexport {
            letters.push('a');
        }
        if self.errexit {
            letters.push('e');
        }
        if self.noglob {
            letters.push('f');
        }
        if self.hashall {
            letters.push('h');
        }
        if self.keyword {
            letters.push('k');
        }
        if self.monitor {
            letters.push('m');
        }
        if self.noexec {
            letters.push('n');
        }
        if self.privileged_mode {
            letters.push('p');
        }
        if self.just_one_command {
            letters.push('t');
        }
        if self.nounset {
            letters.push('u');
        }
        if self.verbose_flag {
            letters.push('v');
        }
        if self.xtrace {
            letters.push('x');
        }
        if self.braceexpand {
            letters.push('B');
        }
        if self.noclobber {
            letters.push('C');
        }
        if self.errtrace {
            letters.push('E');
        }
        if self.option("histexpand") {
            letters.push('H');
        }
        if self.physical {
            letters.push('P');
        }
        if self.functrace {
            letters.push('T');
        }
        if self.startup_state == StartupMode::DashC {
            letters.push('c');
        } else if self.read_from_stdin {
            letters.push('s');
        }
        letters
    }

    fn shellopts_value(&self) -> String {
        cherubsh_builtins::options::SET_OPTIONS
            .iter()
            .filter_map(|opt| self.option(opt.long).then_some(opt.long))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn bashopts_value(&self) -> String {
        let mut names = cherubsh_builtins::shopt_table::SHOPT_OPTIONS
            .iter()
            .filter(|opt| self.option(opt.name))
            .map(|opt| opt.name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.join(":")
    }

    fn sync_export_env(&self, name: &str) {
        if matches!(name, "BASHOPTS" | "SHELLOPTS")
            && self.variables.get(name).is_some_and(|entry| entry.exported)
        {
            let value = if name == "BASHOPTS" {
                self.bashopts_value()
            } else {
                self.shellopts_value()
            };
            std::env::set_var(name, value);
            return;
        }
        if let Some(entry) = self.variables.get(name) {
            if entry.exported && entry.has_value {
                std::env::set_var(name, &entry.value);
                return;
            }
            if let Some(saved_entry) = self
                .global_saved_snapshot(name)
                .and_then(|saved| saved.entry.as_ref())
            {
                if saved_entry.exported && saved_entry.has_value {
                    std::env::set_var(name, &saved_entry.value);
                    return;
                }
            }
        }
        std::env::remove_var(name);
    }

    fn snapshot_options(&self) -> SavedOptions {
        SavedOptions {
            allexport: self.allexport,
            errexit: self.errexit,
            nounset: self.nounset,
            noglob: self.noglob,
            hashall: self.hashall,
            keyword: self.keyword,
            pipefail: self.pipefail,
            lastpipe: self.lastpipe,
            noclobber: self.noclobber,
            xtrace: self.xtrace,
            braceexpand: self.braceexpand,
            errtrace: self.errtrace,
            functrace: self.functrace,
            physical: self.physical,
            notify: self.notify,
            monitor: self.monitor,
            job_control: self.job_control,
            noexec: self.noexec,
            just_one_command: self.just_one_command,
            privileged_mode: self.privileged_mode,
            verbose_flag: self.verbose_flag,
            restricted: self.restricted,
            posixly_correct: self.posixly_correct,
            shopt_options: self.shopt_options.clone(),
        }
    }

    fn restore_options(&mut self, saved: SavedOptions) {
        self.allexport = saved.allexport;
        self.errexit = saved.errexit;
        self.nounset = saved.nounset;
        self.noglob = saved.noglob;
        self.hashall = saved.hashall;
        self.keyword = saved.keyword;
        self.pipefail = saved.pipefail;
        self.lastpipe = saved.lastpipe;
        self.noclobber = saved.noclobber;
        self.xtrace = saved.xtrace;
        self.braceexpand = saved.braceexpand;
        self.errtrace = saved.errtrace;
        self.functrace = saved.functrace;
        self.physical = saved.physical;
        self.notify = saved.notify;
        self.monitor = saved.monitor;
        self.job_control = saved.job_control;
        self.noexec = saved.noexec;
        self.just_one_command = saved.just_one_command;
        self.privileged_mode = saved.privileged_mode;
        self.verbose_flag = saved.verbose_flag;
        self.restricted = saved.restricted;
        self.posixly_correct = saved.posixly_correct;
        self.shopt_options = saved.shopt_options;
        if self.option("ignoreeof") {
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
}
