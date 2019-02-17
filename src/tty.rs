use std::fs;
use std::io;
use std::ops;
use std::os::unix;

use crate::sys;

pub struct Tty {
    file: fs::File,
}

pub struct RawTty {
    tty: Tty,
    fd: unix::io::RawFd,
    prev_ios: sys::Termios,
}

impl Tty {
    pub fn open() -> Result<Self, failure::Error> {
        let file = sys::tty::get_tty()?;
        Ok(Self { file })
    }

    pub fn into_raw_mode(self) -> Result<RawTty, failure::Error> {
        use std::os::unix::io::AsRawFd;

        let tty = self;
        let fd = tty.file.as_raw_fd();

        let mut ios = sys::attr::get_terminal_attr(fd)?;
        let prev_ios = ios;

        sys::attr::raw_terminal_attr(&mut ios);

        sys::attr::set_terminal_attr(fd, &ios)?;

        Ok(RawTty { tty, fd, prev_ios })
    }

    pub fn try_clone(&mut self) -> Result<Tty, failure::Error> {
        self.file
            .try_clone()
            .map(|file| Tty { file })
            .map_err(failure::Error::from)
    }
}

impl RawTty {
    pub fn try_clone(&mut self) -> Result<RawTty, failure::Error> {
        let fd = self.fd;
        let prev_ios = self.prev_ios;
        self.tty.try_clone().map(|tty| RawTty { tty, fd, prev_ios })
    }
}

impl io::Read for Tty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl io::Write for Tty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for RawTty {
    fn drop(&mut self) {
        let _ = sys::attr::set_terminal_attr(self.fd, &self.prev_ios);
    }
}

impl ops::Deref for RawTty {
    type Target = Tty;

    fn deref(&self) -> &Tty {
        &self.tty
    }
}

impl ops::DerefMut for RawTty {
    fn deref_mut(&mut self) -> &mut Tty {
        &mut self.tty
    }
}

impl io::Read for RawTty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tty.read(buf)
    }
}

impl io::Write for RawTty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tty.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tty.flush()
    }
}
