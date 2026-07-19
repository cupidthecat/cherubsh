//! termios raw-mode RAII.

use std::io;

pub struct RawMode {
    saved: libc::termios,
    fd: i32,
}

impl RawMode {
    pub fn enter() -> io::Result<Self> {
        let fd = 0;
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = saved;
        raw.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::IEXTEN);
        raw.c_lflag |= libc::ISIG;
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        write_terminal_mode(b"\x1b[?2004h");
        Ok(Self { saved, fd })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        write_terminal_mode(b"\x1b[?2004l");
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

fn write_terminal_mode(bytes: &[u8]) {
    unsafe {
        libc::write(
            libc::STDERR_FILENO,
            bytes.as_ptr().cast::<libc::c_void>(),
            bytes.len(),
        );
    }
}
