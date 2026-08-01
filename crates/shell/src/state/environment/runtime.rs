macro_rules! environment_runtime {
    () => {
    fn last_async_pid(&self) -> Option<i32> {
        self.last_async_pid
    }

    fn set_last_async_pid(&mut self, pid: i32) {
        self.last_async_pid = Some(pid);
    }

    fn queue_coproc_cleanup(&mut self, name: String, pid_name: Option<String>) {
        self.pending_coproc_cleanups.push((name, pid_name));
    }

    fn take_coproc_cleanups(&mut self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.pending_coproc_cleanups)
    }

    fn prepare_nameref_target_assignment(&mut self, source: &str, target: &str) {
        if self.local_scopes.is_empty()
            || !self
                .local_scopes
                .last()
                .is_some_and(|scope| scope.contains_key(source))
        {
            return;
        }
        let base = state_array_reference(target)
            .map(|(name, _)| name)
            .unwrap_or(target);
        if base == source
            || self.variables.contains_key(base)
            || self.indexed_arrays.contains_key(base)
            || self.assoc_arrays.contains_key(base)
            || self.nameref_targets.contains_key(base)
            || std::env::var(base).is_ok()
        {
            return;
        }
        let _ = self.make_local(base);
    }

    fn shell_pid(&self) -> i32 {
        if let Some(v) = self.shell_pid_value {
            return v;
        }
        unsafe { libc::getpid() }
    }

    fn bashpid(&self) -> i32 {
        if let Some(v) = self.bashpid_cache {
            return v;
        }
        unsafe { libc::getpid() }
    }

    fn shell_start_epoch(&self) -> i64 {
        self.shell_start_epoch
    }

    fn subshell_level(&self) -> u32 {
        self.subshell_level
    }

    fn enter_subshell(&mut self) {
        self.subshell_level = self.subshell_level.saturating_add(1);
        self.bashpid_cache = Some(unsafe { libc::getpid() });
        self.subshell_environment = true;
        self.last_async_pid = None;
    }

    fn command_substitution_depth(&self) -> u32 {
        self.command_substitution_depth
    }

    fn enter_command_substitution(&mut self) {
        self.command_substitution_depth = self.command_substitution_depth.saturating_add(1);
    }

    fn funcname_push(&mut self, name: &str, args: &[String]) {
        let source = self.current_source_name();
        self.funcname_push_with_source(name, args, &source);
    }

    fn funcname_push_with_source(&mut self, name: &str, args: &[String], source: &str) {
        self.funcname_stack.push(name.to_string());
        self.funcname_source_stack.push(self.source_name(source));
        self.funcname_lineno_stack.push(self.current_call_line());
        self.funcname_source_frame_stack.push(false);
        self.bash_argc_stack.push(args.len());
        let mut values = args.iter().rev().cloned().collect::<Vec<_>>();
        values.append(&mut self.bash_argv_stack);
        self.bash_argv_stack = values;
    }

    fn funcname_pop(&mut self) {
        self.funcname_stack.pop();
        self.funcname_source_stack.pop();
        self.funcname_lineno_stack.pop();
        self.funcname_source_frame_stack.pop();
        if let Some(argc) = self.bash_argc_stack.pop() {
            let end = argc.min(self.bash_argv_stack.len());
            self.bash_argv_stack.drain(0..end);
        }
    }

    fn source_frame_push(&mut self, source_name: &str) {
        self.funcname_stack.push("source".to_string());
        self.funcname_source_stack
            .push(self.source_name(source_name));
        self.funcname_lineno_stack.push(self.current_call_line());
        self.funcname_source_frame_stack.push(true);
        self.bash_argc_stack.push(0);
    }

    fn source_frame_pop(&mut self) {
        self.funcname_stack.pop();
        self.funcname_source_stack.pop();
        self.funcname_lineno_stack.pop();
        self.funcname_source_frame_stack.pop();
        self.bash_argc_stack.pop();
    }
    };
}
