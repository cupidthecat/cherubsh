struct DescriptorRedirect {
    target: c_int,
    saved: c_int,
}

impl DescriptorRedirect {
    unsafe fn new(source: c_int, target: c_int) -> Option<Self> {
        if source < 0 || source == target {
            return None;
        }
        let saved = libc::dup(target);
        if saved < 0 || libc::dup2(source, target) < 0 {
            if saved >= 0 {
                libc::close(saved);
            }
            return None;
        }
        Some(Self { target, saved })
    }
}

impl Drop for DescriptorRedirect {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, self.target);
            libc::close(self.saved);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn readline(prompt: *const c_char) -> *mut c_char {
    rl_initialize();
    let prompt_text = c_text(prompt).unwrap_or_default();
    rl_set_prompt(prompt);
    rl_done = 0;
    rl_eof_found = 0;
    rl_readline_state &= !(RL_STATE_TIMEOUT | RL_STATE_EOF);
    set_line_buffer("", 0);
    if let Some(hook) = rl_startup_hook {
        hook();
    }
    let input_descriptor = if rl_instream.is_null() {
        libc::STDIN_FILENO
    } else {
        libc::fileno(rl_instream)
    };
    let output_descriptor = if rl_outstream.is_null() {
        libc::STDOUT_FILENO
    } else {
        libc::fileno(rl_outstream)
    };
    let _input_redirect = DescriptorRedirect::new(input_descriptor, libc::STDIN_FILENO);
    let _output_redirect = DescriptorRedirect::new(output_descriptor, libc::STDERR_FILENO);
    let (mut editor, timeout_deadline) = {
        let mut store = readline_store().lock().expect("readline lock");
        let deadline = store
            .timeout_duration
            .map(|duration| Instant::now() + duration);
        store.timeout_deadline = deadline;
        let editor = store.editor.take().unwrap_or_else(|| {
            let mut keymap = RustKeymap::new("emacs");
            keymap.install_emacs_defaults();
            LineEditor::new(keymap)
        });
        (editor, deadline)
    };
    set_input_deadline(timeout_deadline);
    let mut history = FfiHistory::snapshot();
    let mut completion = FfiCompleter;
    let input_is_tty = libc::isatty(libc::STDIN_FILENO) != 0;
    if !input_is_tty {
        set_errno(libc::ENOTTY);
    }
    let result = if input_is_tty {
        editor.readline(&prompt_text, &mut history, &mut completion)
    } else {
        editor.readline_scripted(&prompt_text, &mut history, &mut completion)
    };
    set_input_deadline(None);
    readline_store().lock().expect("readline lock").editor = Some(editor);
    match result {
        Ok(line) => {
            set_line_buffer(&line, line.len());
            malloc_string(&line)
        }
        Err(EditError::Eof) => {
            rl_eof_found = 1;
            rl_readline_state |= RL_STATE_EOF;
            ptr::null_mut()
        }
        Err(EditError::Interrupted) => {
            set_errno(libc::EINTR);
            ptr::null_mut()
        }
        Err(EditError::Io(error)) => {
            if error.kind() == std::io::ErrorKind::TimedOut {
                rl_readline_state |= RL_STATE_TIMEOUT;
                if let Some(hook) = rl_timeout_event_hook {
                    hook();
                }
            } else {
                set_errno(error.raw_os_error().unwrap_or(libc::EIO));
            }
            ptr::null_mut()
        }
    }
}

unsafe fn set_errno(value: c_int) {
    *libc::__errno_location() = value;
}
