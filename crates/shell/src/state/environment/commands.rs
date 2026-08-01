macro_rules! environment_commands {
    () => {
        fn hash_set(&mut self, name: &str, path: std::path::PathBuf) {
            self.command_hash.insert(name.to_string(), path);
            self.command_hash_hits.insert(name.to_string(), 0);
        }
        fn hash_get(&self, name: &str) -> Option<std::path::PathBuf> {
            self.command_hash.get(name).cloned()
        }
        fn hash_get_with_hit(&mut self, name: &str) -> Option<std::path::PathBuf> {
            let path = self.command_hash.get(name).cloned();
            if path.is_some() {
                *self.command_hash_hits.entry(name.to_string()).or_insert(0) += 1;
            }
            path
        }
        fn hash_remove(&mut self, name: &str) {
            self.command_hash.remove(name);
            self.command_hash_hits.remove(name);
        }
        fn hash_clear(&mut self) {
            self.command_hash.clear();
            self.command_hash_hits.clear();
        }
        fn hash_iter(&self) -> Vec<(String, std::path::PathBuf)> {
            self.command_hash
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }
        fn hash_iter_with_hits(&self) -> Vec<(String, std::path::PathBuf, u64)> {
            let mut entries = self
                .command_hash
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.clone(),
                        self.command_hash_hits.get(k).copied().unwrap_or(0),
                    )
                })
                .collect::<Vec<_>>();
            entries.sort_by_key(|(k, _, _)| bash_command_bucket(k));
            entries
        }

        fn dirs_iter(&self) -> Vec<std::path::PathBuf> {
            let mut out = Vec::with_capacity(self.dirs_stack.len() + 1);
            if let Some(pwd) = self
                .get("PWD")
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
            {
                out.push(pwd);
            } else if let Ok(cur) = std::env::current_dir() {
                out.push(cur);
            }
            out.extend(self.dirs_stack.iter().cloned());
            out
        }
        fn dirs_push(&mut self, path: std::path::PathBuf) {
            self.dirs_stack.insert(0, path);
        }
        fn dirs_pop(&mut self, index: usize) -> Option<std::path::PathBuf> {
            if index >= self.dirs_stack.len() {
                return None;
            }
            Some(self.dirs_stack.remove(index))
        }
        fn dirs_set_top(&mut self, path: std::path::PathBuf) {
            if !self.dirs_stack.is_empty() {
                self.dirs_stack[0] = path;
            } else {
                self.dirs_stack.push(path);
            }
        }
        fn dirs_set_stack(&mut self, stack: Vec<std::path::PathBuf>) {
            self.dirs_stack = stack;
        }
        fn dirs_clear(&mut self) {
            self.dirs_stack.clear();
        }

        fn builtin_enabled(&self, name: &str) -> bool {
            !self.disabled_builtins.get(name).copied().unwrap_or(false)
        }
        fn builtin_set_enabled(&mut self, name: &str, on: bool) {
            if on {
                self.disabled_builtins.remove(name);
            } else {
                self.disabled_builtins.insert(name.to_string(), true);
            }
        }

        fn function_is_readonly(&self, name: &str) -> bool {
            self.function_readonly.contains(name)
        }
        fn function_set_readonly(&mut self, name: &str) {
            self.function_readonly.insert(name.to_string());
        }
    };
}
