//! Files inside a session's directory, opened relative to held directory descriptors: no
//! validation-then-reopen race, and no symlink is ever followed.

use super::{Error, invalid, other};
use std::collections::hash_map::RandomState;
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};

/// Opens a regular file inside `dir` for reading.
pub(super) fn open(dir: &Path, name: &str) -> Result<File, Error> {
    let (dir, name) = parent(dir, name, false)?;
    let file = at(&dir, &name, libc::O_RDONLY | libc::O_NONBLOCK)?;
    if !file.metadata()?.is_file() {
        return invalid("The requested file is not a regular file.");
    }
    Ok(file)
}

/// Replaces `name` inside `dir` with `bytes` in one rename, making its parent directories.
pub(super) fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    let (dir, name) = parent(dir, name, true)?;
    let temp = CString::new(format!(".upload-{:016x}", random()))
        .map_err(other("naming a temporary file"))?;
    let mut file = at(&dir, &temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
    let fd = dir.as_raw_fd();
    let written = file.write_all(bytes).and_then(|()| {
        // SAFETY: both names are single components in the held directory; the rename replaces
        // the target and never follows it.
        match unsafe { libc::renameat(fd, temp.as_ptr(), fd, name.as_ptr()) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    });
    if written.is_err() {
        // SAFETY: only the temporary file made above, in the same held directory.
        unsafe { libc::unlinkat(fd, temp.as_ptr(), 0) };
    }
    Ok(written?)
}

/// 64 bits from the OS's randomness, for names that must not collide.
pub(super) fn random() -> u64 {
    RandomState::new().build_hasher().finish()
}

/// The directory holding `name` inside `dir` (made, with `create`), and its last component.
fn parent(dir: &Path, name: &str, create: bool) -> Result<(File, CString), Error> {
    let mut parts = Vec::new();
    for part in Path::new(name).components() {
        let Component::Normal(part) = part else {
            return invalid("The file name must stay inside the session directory.");
        };
        let Ok(part) = CString::new(part.as_encoded_bytes()) else {
            return invalid("The file name contains a zero byte.");
        };
        parts.push(part);
    }
    let Some(last) = parts.pop() else {
        return invalid("Give a file name inside the session directory.");
    };
    let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW;
    let mut held = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(dir)?;
    for part in parts {
        // SAFETY: `held` is an open directory and `part` one NUL-terminated component.
        if create && unsafe { libc::mkdirat(held.as_raw_fd(), part.as_ptr(), 0o700) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        held = at(&held, &part, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    Ok((held, last))
}

/// `openat` relative to the held directory `dir`, never following a symlink.
fn at(dir: &File, name: &CStr, flags: libc::c_int) -> Result<File, Error> {
    let flags = flags | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `dir` is an open directory and `name` one NUL-terminated component.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: openat returned a new descriptor that nothing else owns.
    Ok(unsafe { File::from_raw_fd(fd) })
}
