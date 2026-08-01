macro_rules! environment_aliases {
    () => {
    fn alias_set(&mut self, name: &str, value: String) {
        self.aliases.insert(name.to_string(), value);
    }
    fn alias_get(&self, name: &str) -> Option<String> {
        self.aliases.get(name).cloned()
    }
    fn alias_unset(&mut self, name: &str) {
        self.aliases.remove(name);
    }
    fn alias_iter(&self) -> Vec<(String, String)> {
        self.aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    fn aliases_enabled(&self) -> bool {
        self.aliases_enabled
    }
    fn set_aliases_enabled(&mut self, on: bool) {
        self.aliases_enabled = on;
    }

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
