use std::{fs, io};

/// Get the TTY device.
///
/// This allows for getting stdio representing _only_ the TTY, and not other streams.
pub fn get_tty() -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}
