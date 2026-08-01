const PROMPT_ESCAPED_DOLLAR: char = '\u{e000}';
const PROMPT_LITERAL_BACKSLASH: char = '\u{e001}';
const RL_PROMPT_START_IGNORE: char = '\x01';
const RL_PROMPT_END_IGNORE: char = '\x02';

fn prompt_expand(value: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            if ctx.env.option("posix") && bytes[i] == b'!' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'!' {
                    out.push('!');
                    i += 2;
                } else {
                    out.push_str(&ctx.env.prompt_history_number().to_string());
                    i += 1;
                }
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push(PROMPT_LITERAL_BACKSLASH);
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b'0'..=b'7' => {
                let mut n = 0u32;
                let mut digits = 0;
                let mut j = i + 1;
                while digits < 3 && j < bytes.len() && (b'0'..=b'7').contains(&bytes[j]) {
                    n = n * 8 + (bytes[j] - b'0') as u32;
                    j += 1;
                    digits += 1;
                }
                if n <= 0xff {
                    out.push(n as u8 as char);
                }
                i = j;
            }
            b'[' => {
                if ctx.env.prompt_nonprinting_markers() {
                    out.push(RL_PROMPT_START_IGNORE);
                }
                i += 2;
            }
            b']' => {
                if ctx.env.prompt_nonprinting_markers() {
                    out.push(RL_PROMPT_END_IGNORE);
                }
                i += 2;
            }
            b'a' => {
                out.push('\x07');
                i += 2;
            }
            b'e' | b'E' => {
                out.push('\x1b');
                i += 2;
            }
            b'r' => {
                out.push('\r');
                i += 2;
            }
            b't' => {
                out.push_str(&strftime_now("%H:%M:%S"));
                i += 2;
            }
            b'T' => {
                out.push_str(&strftime_now("%I:%M:%S"));
                i += 2;
            }
            b'@' => {
                out.push_str(&strftime_now("%I:%M %p"));
                i += 2;
            }
            b'A' => {
                out.push_str(&strftime_now("%H:%M"));
                i += 2;
            }
            b'd' => {
                out.push_str(&strftime_now("%a %b %d"));
                i += 2;
            }
            b'D' => {
                if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
                    let start = i + 3;
                    let end = (start..bytes.len())
                        .find(|&j| bytes[j] == b'}')
                        .unwrap_or(bytes.len());
                    let fmt = std::str::from_utf8(&bytes[start..end]).unwrap_or("%X");
                    out.push_str(&strftime_now(if fmt.is_empty() { "%X" } else { fmt }));
                    i = if end < bytes.len() { end + 1 } else { end };
                } else {
                    out.push(PROMPT_LITERAL_BACKSLASH);
                    out.push('D');
                    i += 2;
                }
            }
            b'h' => {
                let host = current_host_name(ctx.env);
                out.push_str(host.split('.').next().unwrap_or(&host));
                i += 2;
            }
            b'H' => {
                out.push_str(&current_host_name(ctx.env));
                i += 2;
            }
            b's' => {
                let shell = ctx
                    .env
                    .prompt_shell_name()
                    .or_else(|| ctx.env.positional(0))
                    .unwrap_or_else(|| "cherubsh".to_string());
                out.push_str(&base_name(&shell));
                i += 2;
            }
            b'u' => {
                out.push_str(&current_user_name());
                i += 2;
            }
            b'w' => {
                out.push_str(&render_pwd(ctx.env, false));
                i += 2;
            }
            b'W' => {
                out.push_str(&render_pwd(ctx.env, true));
                i += 2;
            }
            b'v' => {
                out.push_str(&bash_version(ctx.env, false));
                i += 2;
            }
            b'V' => {
                out.push_str(&bash_version(ctx.env, true));
                i += 2;
            }
            b'$' => {
                out.push(PROMPT_ESCAPED_DOLLAR);
                i += 2;
            }
            b'\\' => {
                out.push(PROMPT_LITERAL_BACKSLASH);
                i += 2;
            }
            b'n' => {
                out.push('\n');
                i += 2;
            }
            b'#' => {
                out.push_str(&ctx.env.prompt_command_number().to_string());
                i += 2;
            }
            b'!' => {
                out.push_str(&ctx.env.prompt_history_number().to_string());
                i += 2;
            }
            b'j' => {
                out.push_str(&ctx.env.prompt_job_count().to_string());
                i += 2;
            }
            other => {
                out.push(PROMPT_LITERAL_BACKSLASH);
                out.push(other as char);
                i += 2;
            }
        }
    }
    if ctx.env.option("promptvars") {
        let expanded = crate::expand_string_to_string_impl(&out, ctx)?;
        Ok(restore_escaped_prompt_dollars(&expanded))
    } else {
        Ok(restore_escaped_prompt_dollars(&out))
    }
}

fn str_to_bytes(value: String) -> Vec<u8> {
    value.into_bytes()
}

fn restore_escaped_prompt_dollars(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            PROMPT_ESCAPED_DOLLAR => '$',
            PROMPT_LITERAL_BACKSLASH => '\\',
            other => other,
        })
        .collect()
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn current_user_name() -> String {
    if let Some(name) = std::env::var_os("USER") {
        if let Some(s) = name.to_str() {
            return s.to_string();
        }
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() {
            let cstr = CStr::from_ptr((*pw).pw_name);
            if let Ok(s) = cstr.to_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn current_host_name(env: &dyn Environment) -> String {
    if let Some(host) = env.get("HOSTNAME").filter(|s| !s.is_empty()) {
        return host;
    }
    let mut buf = [0u8; 256];
    let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if result != 0 {
        return String::new();
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

fn render_pwd(env: &dyn Environment, basename_only: bool) -> String {
    let pwd = env
        .get("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| ".".to_string());
    if basename_only {
        if pwd == "/" {
            return "/".to_string();
        }
        if env.get("HOME").as_deref() == Some(pwd.as_str()) {
            return "~".to_string();
        }
        return base_name(&pwd);
    }
    let home = env.get("HOME").unwrap_or_default();
    if !home.is_empty() && pwd.starts_with(&home) {
        let mut tilde = String::from("~");
        tilde.push_str(&pwd[home.len()..]);
        return tilde;
    }
    pwd
}

fn bash_version(env: &dyn Environment, release: bool) -> String {
    let raw = env
        .get("BASH_VERSION")
        .or_else(|| std::env::var("CHERUBSH_BASH_COMPAT_VERSION").ok())
        .unwrap_or_else(|| "5.3.15".to_string());
    let numeric = raw
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|s| !s.is_empty())
        .unwrap_or("5.3.15");
    let mut parts = numeric.split('.');
    let major = parts.next().unwrap_or("5");
    let minor = parts.next().unwrap_or("3");
    let patch = parts.next().unwrap_or("0");
    if release {
        format!("{major}.{minor}.{patch}")
    } else {
        format!("{major}.{minor}")
    }
}

fn strftime_now(fmt: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    unsafe {
        let tm = libc::localtime(&secs as *const libc::time_t);
        if tm.is_null() {
            return String::new();
        }
        let c_fmt = match std::ffi::CString::new(fmt) {
            Ok(value) => value,
            Err(_) => return String::new(),
        };
        let mut buf = [0u8; 256];
        let written = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_fmt.as_ptr(),
            tm,
        );
        if written == 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
}

fn attribute_letters(attrs: VarAttrs) -> String {
    let mut s = String::new();
    if attrs.contains(VarAttrs::ARRAY) {
        s.push('a');
    }
    if attrs.contains(VarAttrs::ASSOC) {
        s.push('A');
    }
    if attrs.contains(VarAttrs::INTEGER) {
        s.push('i');
    }
    if attrs.contains(VarAttrs::NAMEREF) {
        s.push('n');
    }
    if attrs.contains(VarAttrs::READONLY) {
        s.push('r');
    }
    if attrs.contains(VarAttrs::EXPORT) {
        s.push('x');
    }
    if attrs.contains(VarAttrs::UPPERCASE) {
        s.push('u');
    }
    if attrs.contains(VarAttrs::LOWERCASE) {
        s.push('l');
    }
    if attrs.contains(VarAttrs::TRACE) {
        s.push('t');
    }
    s
}

fn report_circular_name_reference(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: circular name reference");
    } else {
        eprintln!("cherubsh: warning: {name}: circular name reference");
    }
}

fn escape_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}
