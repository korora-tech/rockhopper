//! Narrow safe wrappers for the few POSIX calls Rockhopper needs.
//!
//! These calls use the existing `libc` dependency rather than adding a safe
//! syscall crate for a small set of basic operations. Keeping the FFI here
//! makes the pointer and descriptor invariants auditable in one place.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

pub fn uid() -> libc::uid_t {
    // SAFETY: getuid takes no pointers or other arguments and returns the
    // calling process's uid; it has no memory-safety preconditions.
    unsafe { libc::getuid() }
}

/// Start a new session. Used only in a child process's `pre_exec` hook.
pub fn setsid() -> io::Result<()> {
    // SAFETY: setsid takes no pointers or other arguments and is async-signal-safe.
    let result = unsafe { libc::setsid() };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Set `O_NONBLOCK` on a valid borrowed descriptor.
pub fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: `fd` guarantees a live descriptor for both calls. F_GETFL does
    // not dereference a pointer, and F_SETFL receives the flags read from this
    // same descriptor with only O_NONBLOCK added.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is still borrowed and live; `flags` came from F_GETFL on
    // this descriptor, with only the valid O_NONBLOCK status flag added.
    if unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Wait for events on initialized poll descriptors. The slice owns the memory
/// that libc may update, and the descriptor numbers themselves need not be valid.
pub fn poll(fds: &mut [libc::pollfd], timeout_ms: libc::c_int) -> io::Result<libc::c_int> {
    let nfds: libc::nfds_t = fds
        .len()
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many poll descriptors"))?;
    // SAFETY: `fds` is a mutable slice of initialized pollfd values, so its
    // pointer addresses `nfds` writable elements for the duration of the call.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), nfds, timeout_ms) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{poll, set_nonblocking, uid};
    use std::io::{Read, Write};
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::net::UnixStream;

    #[test]
    fn uid_matches_this_process() {
        assert_eq!(uid(), std::fs::metadata("/proc/self").unwrap().uid());
    }

    #[test]
    fn set_nonblocking_makes_empty_read_return_would_block() {
        let (mut reader, _writer) = UnixStream::pair().unwrap();
        set_nonblocking(reader.as_fd()).unwrap();
        let err = reader.read(&mut [0u8; 1]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn poll_reports_readable_descriptors() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"x").unwrap();
        let mut fds = [libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        assert_eq!(poll(&mut fds, 100).unwrap(), 1);
        assert_ne!(fds[0].revents & libc::POLLIN, 0);
    }
}
