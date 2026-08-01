const NAMEREF_MAX_DEPTH: usize = 8;

/// Default startup file for an interactive, non-login CherubSH session.
fn default_rc_file() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".cherubrc");
        path
    } else {
        PathBuf::from(".cherubrc")
    }
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_epoch_realtime() -> (u64, u32) {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let micros = duration.subsec_micros().max(100_000);
    (duration.as_secs(), micros)
}

fn current_mono_seconds() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } == 0 && ts.tv_sec >= 0 {
        ts.tv_sec as u64
    } else {
        0
    }
}

fn bash_intrand32(last: u32) -> u32 {
    let seed = if last == 0 { 123_459_876 } else { last };
    let h = seed / 127_773;
    let l = seed - (127_773 * h);
    let t = 16_807_i64 * l as i64 - 2_836_i64 * h as i64;
    if t < 0 {
        (t + 0x7fff_ffff) as u32
    } else {
        t as u32
    }
}

fn bash_random_from_seed(seed: u32) -> (u32, i32) {
    let next_seed = bash_intrand32(seed);
    let mixed = ((next_seed >> 16) ^ (next_seed & 65_535)) & 32_767;
    (next_seed, mixed as i32)
}

fn bash_srandom_from_seed(seed: u32) -> (u32, u32) {
    let first = bash_intrand32(seed);
    let second = bash_intrand32(first ^ 0xa5a5_5a5a);
    (second, first ^ second.rotate_left(11))
}

fn initial_srandom_seed(pid: i32) -> u32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.subsec_nanos() ^ (now.as_secs() as u32) ^ (pid as u32).rotate_left(16) ^ 0x9e37_79b9
}

fn bash_hash(value: &str) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in value.bytes() {
        let old = hash;
        hash = old
            .wrapping_add(old << 1)
            .wrapping_add(old << 4)
            .wrapping_add(old << 7)
            .wrapping_add(old << 8)
            .wrapping_add(old << 24);
        hash ^= u32::from(byte);
    }
    hash
}

fn bash_assoc_bucket(value: &str) -> u32 {
    bash_hash(value) & 1023
}

fn bash_alias_bucket(value: &str) -> u32 {
    bash_hash(value) & 63
}

fn bash_command_bucket(value: &str) -> u32 {
    bash_hash(value) & 255
}

fn apply_case_attrs(mut value: String, attrs: VarAttrs) -> String {
    if attrs.contains(VarAttrs::UPPERCASE) {
        value = value.to_uppercase();
    }
    if attrs.contains(VarAttrs::LOWERCASE) {
        value = value.to_lowercase();
    }
    if attrs.contains(VarAttrs::CAPCASE) {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        if let Some(first) = chars.next() {
            if first.is_alphabetic() {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
        value = out;
    }
    value
}

fn bash_progcomp_bucket(value: &str) -> u32 {
    bash_hash(value) & 511
}

fn default_shopt_value(name: &str, interactive: bool, posix: bool) -> bool {
    match name {
        "expand_aliases" => interactive,
        "inherit_errexit" if posix => true,
        _ => cherubsh_builtins::shopt_table::lookup(name)
            .map(|option| option.default)
            .unwrap_or(false),
    }
}

const COMPAT_SHOPT_OPTIONS: &[&str] = &[
    "compat31", "compat32", "compat40", "compat41", "compat42", "compat43", "compat44",
];

fn compatibility_level(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if let Some((major, minor)) = trimmed.split_once('.') {
        return Some(major.parse::<u32>().ok()? * 10 + minor.parse::<u32>().ok()?);
    }
    trimmed.parse().ok()
}

fn compatibility_option(level: u32) -> Option<&'static str> {
    COMPAT_SHOPT_OPTIONS
        .iter()
        .copied()
        .find(|option| option["compat".len()..].parse::<u32>().ok() == Some(level))
}

#[derive(Clone, Copy)]
struct MailStatus {
    modified: i64,
    accessed: i64,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMode {
    Script = 0,
    Interactive = 1,
    DashC = 2,
}
