use std::os::unix::io::AsRawFd;
use std::{env, fs, io};

use super::syscall;

/// Get the TTY device.
///
/// This allows for getting stdio representing _only_ the TTY, and not other streams.
pub fn get_tty() -> io::Result<fs::File> {
    let tty = env::var("TTY").map_err(|x| io::Error::new(io::ErrorKind::NotFound, x))?;
    fs::OpenOptions::new().read(true).write(true).open(tty)
}
