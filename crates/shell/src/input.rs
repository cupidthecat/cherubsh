use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::Path;

/// Input source currently being read by the shell.
#[derive(Debug, Default)]
pub enum BashInput {
    #[default]
    None,
    Stdin {
        name: String,
    },
    String {
        name: String,
        buf: String,
        pos: usize,
    },
    Stream {
        name: String,
        reader: BufReader<File>,
    },
}

impl BashInput {
    pub fn stdin() -> Self {
        BashInput::Stdin {
            name: String::from("stdin"),
        }
    }

    pub fn from_string<S: Into<String>>(name: S, buf: String) -> Self {
        BashInput::String {
            name: name.into(),
            buf,
            pos: 0,
        }
    }

    pub fn from_file(path: &Path) -> io::Result<Self> {
        let file = move_to_internal_fd(File::open(path)?);
        Ok(BashInput::Stream {
            name: path.display().to_string(),
            reader: BufReader::new(file),
        })
    }

    pub fn name(&self) -> &str {
        match self {
            BashInput::None => "",
            BashInput::Stdin { name, .. } => name,
            BashInput::String { name, .. } => name,
            BashInput::Stream { name, .. } => name,
        }
    }

    pub fn is_string(&self) -> bool {
        matches!(self, BashInput::String { .. })
    }

    pub fn is_stream(&self) -> bool {
        matches!(self, BashInput::Stream { .. })
    }

    pub fn is_terminal(&self) -> bool {
        match self {
            BashInput::Stdin { .. } => io::stdin().is_terminal(),
            _ => false,
        }
    }

    /// Read one line including trailing newline. Returns Ok(None) at EOF.
    pub fn next_line(&mut self) -> io::Result<Option<String>> {
        match self {
            BashInput::None => Ok(None),
            BashInput::Stdin { .. } => read_fd_line(libc::STDIN_FILENO),
            BashInput::String { buf, pos, .. } => {
                if *pos >= buf.len() {
                    return Ok(None);
                }
                let rest = &buf[*pos..];
                let end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
                let line = rest[..end].to_string();
                *pos += end;
                Ok(Some(line))
            }
            BashInput::Stream { reader, .. } => {
                let mut line = String::new();
                let read = reader.read_line(&mut line)?;
                if read == 0 {
                    Ok(None)
                } else {
                    Ok(Some(line))
                }
            }
        }
    }
}

fn move_to_internal_fd(file: File) -> File {
    for base in [255, 50] {
        let dup = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, base) };
        if dup >= 0 {
            drop(file);
            return unsafe { File::from_raw_fd(dup) };
        }
    }
    file
}

fn read_fd_line(fd: i32) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if n == 0 {
            break;
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    if bytes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8_lossy(&bytes).to_string()))
    }
}
