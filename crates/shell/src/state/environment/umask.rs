macro_rules! environment_umask {
    () => {
        fn umask_get(&self) -> u32 {
            unsafe {
                let cur = libc::umask(0);
                libc::umask(cur);
                cur as u32 & 0o777
            }
        }

        fn umask_set(&mut self, mask: u32) {
            unsafe { libc::umask(mask as libc::mode_t) };
        }
    };
}
