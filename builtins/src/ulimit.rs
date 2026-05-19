//! `ulimit` builtin.

use crate::{Builtin, BuiltinCtx};

const LIMIT_HARD: u8 = 0x01;
const LIMIT_SOFT: u8 = 0x02;
const PIPE_BUF_BYTES: u64 = 4096;

#[derive(Clone, Copy)]
enum Resource {
    Rlimit(libc::__rlimit_resource_t),
    PipeSize,
}

#[derive(Clone, Copy)]
struct LimitSpec {
    option: char,
    resource: Resource,
    factor: u64,
    description: &'static str,
    units: Option<&'static str>,
}

pub struct Ulimit;
pub static ULIMIT: Ulimit = Ulimit;

impl Builtin for Ulimit {
    fn name(&self) -> &'static str {
        "ulimit"
    }

    fn synopsis(&self) -> &'static str {
        "ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]"
    }

    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_ulimit(ctx.args)
    }
}

fn run_ulimit(args: &[String]) -> i32 {
    let mut mode: u8 = 0;
    let mut all_limits = false;
    let mut commands: Vec<(char, Option<String>)> = Vec::new();
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            idx += 1;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }

        let chars: Vec<char> = arg[1..].chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            match chars[pos] {
                'S' => mode |= LIMIT_SOFT,
                'H' => mode |= LIMIT_HARD,
                'a' => all_limits = true,
                opt => {
                    if find_limit(opt).is_none() {
                        eprintln!("cherubsh: ulimit: -{opt}: invalid option");
                        return 2;
                    }
                    let attached: String = chars[pos + 1..].iter().collect();
                    let arg_value = if attached.is_empty() {
                        None
                    } else {
                        Some(attached)
                    };
                    commands.push((opt, arg_value));
                    break;
                }
            }
            pos += 1;
        }
        idx += 1;
    }

    if all_limits {
        print_all_limits(if mode == 0 { LIMIT_SOFT } else { mode });
        return 0;
    }

    let operands = &args[idx..];
    if commands.is_empty() {
        let arg = operands.first().cloned();
        commands.push(('f', arg));
    } else if commands.last().and_then(|(_, arg)| arg.as_ref()).is_none() {
        if let Some(arg) = operands.first() {
            if let Some(last) = commands.last_mut() {
                last.1 = Some(arg.clone());
            }
        }
    }

    let multiple = commands.len() > 1;
    for (opt, arg) in commands {
        let Some(spec) = find_limit(opt) else {
            eprintln!("cherubsh: ulimit: -{opt}: invalid option");
            return 2;
        };
        if ulimit_one(spec, arg.as_deref(), mode, multiple) != 0 {
            return 1;
        }
    }
    0
}

fn ulimit_one(spec: LimitSpec, arg: Option<&str>, mut mode: u8, multiple: bool) -> i32 {
    let setting = arg.is_some();
    if mode == 0 {
        mode = if setting {
            LIMIT_HARD | LIMIT_SOFT
        } else {
            LIMIT_SOFT
        };
    }
    let (soft, hard) = match get_limit(spec) {
        Ok(v) => v,
        Err(err) => {
            eprintln!(
                "cherubsh: ulimit: {}: cannot get limit: {err}",
                spec.description
            );
            return 1;
        }
    };

    let Some(arg) = arg else {
        print_one(
            spec,
            if mode & LIMIT_SOFT != 0 { soft } else { hard },
            multiple,
        );
        return 0;
    };

    let real_limit = match parse_limit_value(arg, spec, soft, hard) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("cherubsh: ulimit: {arg}: {msg}");
            return 1;
        }
    };

    if let Err(err) = set_limit(spec, real_limit, mode) {
        eprintln!(
            "cherubsh: ulimit: {}: cannot modify limit: {err}",
            spec.description
        );
        return 1;
    }
    0
}

fn parse_limit_value(
    arg: &str,
    spec: LimitSpec,
    soft: libc::rlim_t,
    hard: libc::rlim_t,
) -> Result<libc::rlim_t, &'static str> {
    if arg == "soft" {
        return Ok(soft);
    }
    if arg == "hard" {
        return Ok(hard);
    }
    if arg == "unlimited" {
        return Ok(libc::RLIM_INFINITY);
    }
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid number");
    }
    let value = arg.parse::<u64>().map_err(|_| "invalid number")?;
    value
        .checked_mul(spec.factor)
        .map(|v| v as libc::rlim_t)
        .ok_or("limit out of range")
}

fn get_limit(spec: LimitSpec) -> std::io::Result<(libc::rlim_t, libc::rlim_t)> {
    match spec.resource {
        Resource::PipeSize => Ok((
            PIPE_BUF_BYTES as libc::rlim_t,
            PIPE_BUF_BYTES as libc::rlim_t,
        )),
        Resource::Rlimit(resource) => {
            let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
            let rc = unsafe { libc::getrlimit(resource, limit.as_mut_ptr()) };
            if rc != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                let limit = unsafe { limit.assume_init() };
                Ok((limit.rlim_cur, limit.rlim_max))
            }
        }
    }
}

fn set_limit(spec: LimitSpec, value: libc::rlim_t, mode: u8) -> std::io::Result<()> {
    let Resource::Rlimit(resource) = spec.resource else {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    };

    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    let rc = unsafe { libc::getrlimit(resource, limit.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut limit = unsafe { limit.assume_init() };
    if mode & LIMIT_SOFT != 0 {
        limit.rlim_cur = value;
    }
    if mode & LIMIT_HARD != 0 {
        limit.rlim_max = value;
    }
    let rc = unsafe { libc::setrlimit(resource, &limit) };
    if rc != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn print_all_limits(mode: u8) {
    for spec in LIMITS {
        if let Ok((soft, hard)) = get_limit(*spec) {
            print_one(
                *spec,
                if mode & LIMIT_SOFT != 0 { soft } else { hard },
                true,
            );
        }
    }
}

fn print_one(spec: LimitSpec, value: libc::rlim_t, print_description: bool) {
    if print_description {
        let unit = match spec.units {
            Some(units) => format!("({units}, -{}) ", spec.option),
            None => format!("(-{}) ", spec.option),
        };
        print!("{:<20} {:>20}", spec.description, unit);
    }
    if value == libc::RLIM_INFINITY {
        println!("unlimited");
    } else {
        println!("{}", (value as u64) / spec.factor);
    }
}

fn find_limit(option: char) -> Option<LimitSpec> {
    LIMITS.iter().copied().find(|spec| spec.option == option)
}

static LIMITS: &[LimitSpec] = &[
    LimitSpec {
        option: 'R',
        resource: Resource::Rlimit(libc::RLIMIT_RTTIME),
        factor: 1,
        description: "real-time non-blocking time",
        units: Some("microseconds"),
    },
    LimitSpec {
        option: 'c',
        resource: Resource::Rlimit(libc::RLIMIT_CORE),
        factor: 1024,
        description: "core file size",
        units: Some("blocks"),
    },
    LimitSpec {
        option: 'd',
        resource: Resource::Rlimit(libc::RLIMIT_DATA),
        factor: 1024,
        description: "data seg size",
        units: Some("kbytes"),
    },
    LimitSpec {
        option: 'e',
        resource: Resource::Rlimit(libc::RLIMIT_NICE),
        factor: 1,
        description: "scheduling priority",
        units: None,
    },
    LimitSpec {
        option: 'f',
        resource: Resource::Rlimit(libc::RLIMIT_FSIZE),
        factor: 1024,
        description: "file size",
        units: Some("blocks"),
    },
    LimitSpec {
        option: 'i',
        resource: Resource::Rlimit(libc::RLIMIT_SIGPENDING),
        factor: 1,
        description: "pending signals",
        units: None,
    },
    LimitSpec {
        option: 'l',
        resource: Resource::Rlimit(libc::RLIMIT_MEMLOCK),
        factor: 1024,
        description: "max locked memory",
        units: Some("kbytes"),
    },
    LimitSpec {
        option: 'm',
        resource: Resource::Rlimit(libc::RLIMIT_RSS),
        factor: 1024,
        description: "max memory size",
        units: Some("kbytes"),
    },
    LimitSpec {
        option: 'n',
        resource: Resource::Rlimit(libc::RLIMIT_NOFILE),
        factor: 1,
        description: "open files",
        units: None,
    },
    LimitSpec {
        option: 'p',
        resource: Resource::PipeSize,
        factor: 512,
        description: "pipe size",
        units: Some("512 bytes"),
    },
    LimitSpec {
        option: 'q',
        resource: Resource::Rlimit(libc::RLIMIT_MSGQUEUE),
        factor: 1,
        description: "POSIX message queues",
        units: Some("bytes"),
    },
    LimitSpec {
        option: 'r',
        resource: Resource::Rlimit(libc::RLIMIT_RTPRIO),
        factor: 1,
        description: "real-time priority",
        units: None,
    },
    LimitSpec {
        option: 's',
        resource: Resource::Rlimit(libc::RLIMIT_STACK),
        factor: 1024,
        description: "stack size",
        units: Some("kbytes"),
    },
    LimitSpec {
        option: 't',
        resource: Resource::Rlimit(libc::RLIMIT_CPU),
        factor: 1,
        description: "cpu time",
        units: Some("seconds"),
    },
    LimitSpec {
        option: 'u',
        resource: Resource::Rlimit(libc::RLIMIT_NPROC),
        factor: 1,
        description: "max user processes",
        units: None,
    },
    LimitSpec {
        option: 'v',
        resource: Resource::Rlimit(libc::RLIMIT_AS),
        factor: 1024,
        description: "virtual memory",
        units: Some("kbytes"),
    },
    LimitSpec {
        option: 'x',
        resource: Resource::Rlimit(libc::RLIMIT_LOCKS),
        factor: 1,
        description: "file locks",
        units: None,
    },
];
