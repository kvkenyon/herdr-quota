//! Minimal process signalling needed for bounded collector shutdown.

use std::io;

pub(crate) fn terminate(process_id: u32) -> io::Result<()> {
    signal(process_id, libc::SIGTERM)
}

pub(crate) fn kill(process_id: u32) -> io::Result<()> {
    signal(process_id, libc::SIGKILL)
}

fn signal(process_id: u32, signal: libc::c_int) -> io::Result<()> {
    let process_id = libc::pid_t::try_from(process_id).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "process identifier is invalid")
    })?;

    // SAFETY: `kill` does not retain pointers and receives a validated PID and
    // one of the two signal constants selected above.
    if unsafe { libc::kill(process_id, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
