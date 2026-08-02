//! `/dev/tcp` redirection parity tests.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::{cherub_path, diff, required_oracle_bash_path, RunOutput};

fn bash_oracle() -> std::path::PathBuf {
    required_oracle_bash_path().expect("resolve pinned Bash oracle")
}

#[test]
fn tcp_redirection_supports_numeric_ports_and_host_lookup() {
    for host in ["127.0.0.1", "localhost"] {
        for assign_fd in [false, true] {
            let bash = run_round_trip(&bash_oracle(), host, None, assign_fd);
            let cherub = run_round_trip(
                &cherub_path().expect("cherub binary"),
                host,
                None,
                assign_fd,
            );
            assert_eq!(cherub, bash, "host={host} assign_fd={assign_fd}");
            assert_eq!(cherub.status, 0);
            assert_eq!(cherub.stdout, "banner=<ready>\nreply=<pong>\n");
            assert!(cherub.stderr.is_empty(), "stderr={:?}", cherub.stderr);
        }
    }
}

#[test]
fn tcp_redirection_resolves_service_names() {
    let (service, bash_listener) = bind_known_service();
    let bash =
        run_round_trip_with_listener(&bash_oracle(), "127.0.0.1", &service, bash_listener, false);

    let cherub_listener = TcpListener::bind(("127.0.0.1", service_port(&service)))
        .expect("rebind selected service port");
    let cherub = run_round_trip_with_listener(
        &cherub_path().expect("cherub binary"),
        "127.0.0.1",
        &service,
        cherub_listener,
        false,
    );

    assert_eq!(cherub, bash, "service={service}");
    assert_eq!(cherub.status, 0);
}

#[test]
fn tcp_redirection_reports_connection_failures() {
    let (_reservation, port) = reserve_unlistened_port();
    let script = format!(": <>/dev/tcp/127.0.0.1/{port}; printf 'status=%s\\n' \"$?\"");
    let bash = run_shell_bounded(&bash_oracle(), &script);
    let cherub = run_shell_bounded(&cherub_path().expect("cherub binary"), &script);
    let outcome = diff(&bash, &cherub);

    assert!(
        outcome.is_match(),
        "{outcome:?}; bash={bash:?}; cherub={cherub:?}"
    );
    assert_eq!(bash.status, 0, "Bash timed out or failed: {bash:?}");
    assert_eq!(bash.stdout, "status=1\n");
}

#[test]
fn tcp_redirection_reports_lookup_failures_like_bash() {
    for (endpoint, subject) in [
        ("127.0.0.1/service-does-not-exist", "service-does-not-exist"),
        (
            "::1%cherub-no-such-interface/http",
            "::1%cherub-no-such-interface",
        ),
    ] {
        let script = format!(": <>/dev/tcp/{endpoint}; printf 'status=%s\\n' \"$?\"");
        let bash = run_shell_bounded(&bash_oracle(), &script);
        let cherub = run_shell_bounded(&cherub_path().expect("cherub binary"), &script);
        let outcome = diff(&bash, &cherub);

        assert!(outcome.is_match(), "endpoint={endpoint}: {outcome:?}");
        assert!(cherub.stderr.contains(subject));
        assert!(cherub.stderr.contains("Invalid argument"));
    }
}

fn reserve_unlistened_port() -> (OwnedFd, u16) {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    assert!(
        fd >= 0,
        "create reservation socket: {}",
        std::io::Error::last_os_error()
    );
    let reservation = unsafe { OwnedFd::from_raw_fd(fd) };
    let address = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        sin_zero: [0; 8],
    };
    let bind_result = unsafe {
        libc::bind(
            fd,
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    assert_eq!(
        bind_result,
        0,
        "bind reservation socket: {}",
        std::io::Error::last_os_error()
    );
    let mut bound: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let name_result =
        unsafe { libc::getsockname(fd, (&raw mut bound).cast::<libc::sockaddr>(), &mut length) };
    assert_eq!(
        name_result,
        0,
        "read reservation address: {}",
        std::io::Error::last_os_error()
    );
    (reservation, u16::from_be(bound.sin_port))
}

fn run_round_trip(shell: &Path, host: &str, service: Option<&str>, assign_fd: bool) -> RunOutput {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
    let port = listener.local_addr().expect("listener address").port();
    let service = service
        .map(str::to_string)
        .unwrap_or_else(|| port.to_string());
    run_round_trip_with_listener(shell, host, &service, listener, assign_fd)
}

fn run_round_trip_with_listener(
    shell: &Path,
    host: &str,
    service: &str,
    listener: TcpListener,
    assign_fd: bool,
) -> RunOutput {
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    let server = thread::spawn(move || serve_one(listener));
    let script = if assign_fd {
        format!(
            "exec {{socket}}<>/dev/tcp/{host}/{service}; \
             IFS= read -r -u \"$socket\" banner; printf 'banner=<%s>\\n' \"$banner\"; \
             printf 'ping\\n' >&\"$socket\"; IFS= read -r -u \"$socket\" reply; \
             printf 'reply=<%s>\\n' \"$reply\"; exec {{socket}}>&-"
        )
    } else {
        format!(
            "exec 9<>/dev/tcp/{host}/{service}; \
             IFS= read -r banner <&9; printf 'banner=<%s>\\n' \"$banner\"; \
             printf 'ping\\n' >&9; IFS= read -r reply <&9; printf 'reply=<%s>\\n' \"$reply\"; \
             exec 9<&-"
        )
    };
    let output = run_shell_bounded(shell, &script);
    server.join().expect("join loopback server");
    output
}

fn serve_one(listener: TcpListener) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "shell did not connect");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept loopback connection: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set server read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("set server write timeout");
    stream.write_all(b"ready\n").expect("write banner");
    let mut request = String::new();
    BufReader::new(stream.try_clone().expect("clone stream"))
        .read_line(&mut request)
        .expect("read request");
    assert_eq!(request, "ping\n");
    stream.write_all(b"pong\n").expect("write reply");
}

fn run_shell_bounded(shell: &Path, script: &str) -> RunOutput {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("cherubsh-tcp-{}-{suffix}", std::process::id()));
    fs::create_dir_all(&directory).expect("create TCP output directory");
    let stdout_path = directory.join("stdout");
    let stderr_path = directory.join("stderr");
    let mut command = Command::new(shell);
    command
        .args(["--norc", "--noprofile", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            fs::File::create(&stdout_path).expect("create stdout"),
        ))
        .stderr(Stdio::from(
            fs::File::create(&stderr_path).expect("create stderr"),
        ))
        .process_group(0);
    let mut child = command.spawn().expect("run bounded shell");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll bounded shell") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = RunOutput {
        status: status.map_or(124, |status| {
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap())
        }),
        stdout: fs::read_to_string(&stdout_path).expect("read stdout"),
        stderr: fs::read_to_string(&stderr_path).expect("read stderr"),
    };
    fs::remove_dir_all(directory).expect("remove TCP output directory");
    output
}

fn bind_known_service() -> (String, TcpListener) {
    let services = fs::read_to_string("/etc/services").expect("read /etc/services");
    for line in services.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else { continue };
        let Some(port_and_protocol) = fields.next() else {
            continue;
        };
        let Some((port, protocol)) = port_and_protocol.split_once('/') else {
            continue;
        };
        if protocol != "tcp" {
            continue;
        }
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        if port < 1024 {
            continue;
        }
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return (name.to_string(), listener);
        }
    }
    panic!("no bindable TCP service found in /etc/services");
}

fn service_port(service: &str) -> u16 {
    let services = fs::read_to_string("/etc/services").expect("read /etc/services");
    services
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let (port, protocol) = fields.next()?.split_once('/')?;
            (name == service && protocol == "tcp")
                .then(|| port.parse::<u16>().ok())
                .flatten()
        })
        .next()
        .expect("selected service remains available")
}
