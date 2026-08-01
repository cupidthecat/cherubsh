macro_rules! environment_status_diagnostics {
    () => {
        fn last_status(&self) -> i32 {
            self.last_command_exit_value
        }

        fn set_last_status(&mut self, status: i32) {
            self.last_command_exit_value = status;
        }

        fn diagnostic_source_name(&self) -> Option<String> {
            if let Some(source) = self.funcname_source_stack.last() {
                return Some(source.clone());
            }
            if self.startup_state == StartupMode::DashC {
                return self.dollar_vars.first().cloned();
            }
            if matches!(self.input, BashInput::Stdin { .. }) {
                return self.dollar_vars.first().cloned();
            }
            let name = self.input.name();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        }

        fn call_stack_source_name(&self) -> Option<String> {
            if let Some(source) = self.funcname_source_stack.last() {
                return Some(source.clone());
            }
            Some(self.main_source_name())
        }

        fn diagnostic_line(&self) -> Option<u32> {
            if let Some(line) = self.diagnostic_line_stack.last() {
                return Some(*line);
            }
            (self.current_command_line_count > 0).then_some(self.current_command_line_count)
        }

        fn push_diagnostic_line(&mut self, line: u32) {
            self.diagnostic_line_stack.push(line);
        }

        fn pop_diagnostic_line(&mut self) {
            self.diagnostic_line_stack.pop();
        }

        fn set_current_command(&mut self, command: Option<String>) {
            self.current_command_string = command;
        }

        fn arithmetic_expansion_errors_exit_shell(&self) -> bool {
            self.startup_state == StartupMode::DashC
        }

        fn is_login_shell(&self) -> bool {
            self.login_shell != 0
        }
    };
}
