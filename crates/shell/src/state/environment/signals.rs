macro_rules! environment_signals {
    () => {
    fn pending_signal_take(&mut self, sig: i32) -> u32 {
        crate::signals::pending_signal_take(sig)
    }

    fn acknowledge_trapped_signal(&mut self, sig: i32) {
        crate::signals::acknowledge_trapped_signal(sig);
    }
    fn running_trap(&self) -> Option<i32> {
        self.running_trap_sig
    }
    fn set_running_trap(&mut self, sig: Option<i32>) {
        self.running_trap_sig = sig;
    }
    fn run_debug_trap_hook(&mut self) {
        crate::traps::run_debug_trap(self);
    }
    fn run_err_trap_hook(&mut self) {
        crate::traps::run_err_trap(self);
    }
    fn run_return_trap_hook(&mut self) {
        crate::traps::run_return_trap(self);
    }
    fn run_pending_traps_hook(&mut self) {
        crate::traps::run_pending_traps(self);
    }
    fn run_exit_trap_hook(&mut self) -> Option<i32> {
        crate::traps::run_exit_trap(self)
    }
    fn shell_pgrp(&self) -> i32 {
        self.shell_pgrp_value
    }
    fn set_shell_pgrp(&mut self, pgrp: i32) {
        self.shell_pgrp_value = pgrp;
    }
    fn tty_fd(&self) -> Option<i32> {
        self.tty_fd_value
    }
    fn set_tty_fd(&mut self, fd: Option<i32>) {
        self.tty_fd_value = fd;
    }
    fn job_control_enabled(&self) -> bool {
        self.job_control
    }
    fn set_job_control_enabled(&mut self, on: bool) {
        self.job_control = on;
    }
    };
}
