macro_rules! environment_input_runtime {
    () => {
        fn next_shell_input_line(&mut self) -> Option<String> {
            if !matches!(self.input, BashInput::Stdin { .. }) {
                return None;
            }
            let line = self.input.next_line().ok().flatten();
            if line.is_some() {
                self.current_command_line_count = self.current_command_line_count.saturating_add(1);
            }
            line
        }

        fn enter_loadable_child(&mut self) {
            let pid = unsafe { libc::getpid() };
            self.shell_pid_value = Some(pid);
            self.bashpid_cache = Some(pid);
            self.last_async_pid = None;
            self.pending_coproc_cleanups.clear();
            self.jobs = JobTable::new();
        }
    };
}
