//! A job's terminal: a fixed 40-by-120 pseudo-terminal, read without a thread of its own, its
//! screen rendered by vt100, which answers the queries programs wait on.

use super::{Error, Screen};
use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::{Mutex, PoisonError};
use tokio::{fs::File, io::AsyncWriteExt, io::unix::AsyncFd};

// Evaluated at compile time: neither size is zero.
const ROWS: NonZeroU16 = NonZeroU16::new(40).unwrap();
const COLUMNS: NonZeroU16 = NonZeroU16::new(120).unwrap();

pub(super) struct Terminal {
    master: AsyncFd<std::fs::File>,
    /// No scrollback, so its size is fixed: 40 by 120 cells.
    parser: Mutex<vt100::Parser<Replies>>,
}

/// Answers to the program's queries; at most what one 8 KiB read can ask for.
#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (first, second, param, c) {
            (None, None, 6, 'n') => {
                let (row, column) = screen.cursor_position();
                let column = column.min(COLUMNS.get() - 1);
                self.0
                    .extend(format!("\x1b[{};{}R", row + 1, column + 1).bytes());
            }
            (None, None, 5, 'n') => self.0.extend(b"\x1b[0n"),
            (None, None, 0, 'c') => self.0.extend(b"\x1b[?1;2c"),
            (Some(b'>'), None, 0, 'c') => self.0.extend(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

impl Terminal {
    /// A new terminal, and its slave three times: the job's stdin, stdout and stderr.
    pub(super) fn open() -> io::Result<(Self, [std::fs::File; 3])> {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: ROWS.get(),
            ws_col: COLUMNS.get(),
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let (name, termios) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: openpty writes two new descriptors, owned at once below.
        if unsafe { libc::openpty(&mut master, &mut slave, name, termios, &raw mut size) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: each descriptor is open and owned by nothing else.
        let (master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let (m, s) = (master.as_raw_fd(), slave.as_raw_fd());
        // SAFETY: flag changes on descriptors owned here. Neither may leak into a job's exec, and
        // reading the master must never block.
        if unsafe {
            libc::fcntl(m, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                || libc::fcntl(s, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                || libc::fcntl(m, libc::F_SETFL, libc::O_NONBLOCK) == -1
        } {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the File owns the descriptor for as long as the AsyncFd holds it.
        let master = unsafe { AsyncFd::register(master) }.map_err(|e| e.into_parts().1)?;
        let parser = vt100::Parser::new_with_callbacks(ROWS, COLUMNS, 0, Replies::default());
        let parser = Mutex::new(parser);
        let slaves = [slave.try_clone()?, slave.try_clone()?, slave];
        Ok((Self { master, parser }, slaves))
    }

    pub(super) fn screen(&self) -> Screen {
        let parser = self.parser.lock().unwrap_or_else(PoisonError::into_inner);
        let screen = parser.screen();
        let (row, column) = screen.cursor_position();
        let rows = screen.rows(0, COLUMNS.get());
        Screen {
            rows: rows
                .map(|row| row.trim_end_matches(' ').to_owned())
                .collect(),
            cursor: (row, column.min(COLUMNS.get() - 1)),
        }
    }

    /// Whether the program asked for application cursor keys.
    pub(super) fn application(&self) -> bool {
        let parser = self.parser.lock().unwrap_or_else(PoisonError::into_inner);
        parser.screen().application_cursor()
    }

    /// Types `bytes` into the terminal.
    pub(super) async fn send(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut ready = self.master.writable().await?;
            if let Ok(written) = ready.try_io(|master| master.get_ref().write(bytes)) {
                bytes = &bytes[written?..];
            }
        }
        Ok(())
    }

    /// Copies the program's output to `spool` and the screen, answering its queries, until the
    /// terminal hangs up; with `wait` false, only what is already buffered, unanswered.
    pub(super) async fn copy(&self, spool: &mut File, wait: bool) -> io::Result<()> {
        let mut bytes = [0; 8192];
        loop {
            let read = if wait {
                let mut ready = self.master.readable().await?;
                match ready.try_io(|master| master.get_ref().read(&mut bytes)) {
                    Ok(read) => read,
                    Err(_would_block) => continue,
                }
            } else {
                self.master.get_ref().read(&mut bytes)
            };
            let n = match read {
                Ok(n) if n > 0 => n,
                // Linux reports a hung-up terminal as EIO, macOS as the end of the file.
                Err(e)
                    if e.kind() != io::ErrorKind::WouldBlock
                        && e.raw_os_error() != Some(libc::EIO) =>
                {
                    return Err(e);
                }
                _ => return spool.flush().await,
            };
            let replies = {
                let mut parser = self.parser.lock().unwrap_or_else(PoisonError::into_inner);
                parser.process(&bytes[..n]);
                std::mem::take(&mut parser.callbacks_mut().0)
            };
            spool.write_all(&bytes[..n]).await?;
            if wait && !replies.is_empty() {
                // A reply the hung-up terminal cannot take is moot.
                let _ = self.send(&replies).await;
            }
        }
    }
}

/// The bytes a named key types: Enter, Tab, Backspace, Escape, the arrows, Home, End (both in
/// application mode when asked), Delete, PageUp, PageDown, and Ctrl-A to Ctrl-Z.
pub(super) fn key(name: &str, application: bool) -> Result<Vec<u8>, Error> {
    let cursor = |end| vec![0x1b, if application { b'O' } else { b'[' }, end];
    Ok(match name {
        "Enter" => b"\r".to_vec(),
        "Tab" => b"\t".to_vec(),
        "Backspace" => b"\x7f".to_vec(),
        "Escape" => b"\x1b".to_vec(),
        "Up" => cursor(b'A'),
        "Down" => cursor(b'B'),
        "Right" => cursor(b'C'),
        "Left" => cursor(b'D'),
        "Home" => cursor(b'H'),
        "End" => cursor(b'F'),
        "Delete" => b"\x1b[3~".to_vec(),
        "PageUp" => b"\x1b[5~".to_vec(),
        "PageDown" => b"\x1b[6~".to_vec(),
        _ => match name.strip_prefix("Ctrl-").map(str::as_bytes) {
            Some(&[letter @ b'A'..=b'Z']) => vec![letter - b'@'],
            _ => {
                return Err(Error::Refused(
                    "That key is not supported; send text or a named terminal key.",
                ));
            }
        },
    })
}
