use std::fs;
use std::io;
use std::ops;

use crate::sys;

pub struct Tty {
    file: fs::File,
}

pub struct Raw {
    tty: Tty,
    prev_ios: sys::Termios,
}

impl Tty {
    pub fn open() -> Result<Self, failure::Error> {
        let file = sys::tty::get()?;
        Ok(Self { file })
    }

    pub fn into_raw_mode(self) -> Result<Raw, failure::Error> {
        let mut ios = sys::attr::get(&self.file)?;
        let prev_ios = ios;

        sys::attr::make_raw(&mut ios);

        sys::attr::set(&self.file, &ios)?;

        let tty = self;

        Ok(Raw { tty, prev_ios })
    }

    pub fn try_clone(&mut self) -> Result<Self, failure::Error> {
        self.file
            .try_clone()
            .map(|file| Self { file })
            .map_err(failure::Error::from)
    }
}

impl Raw {
    pub fn try_clone(&mut self) -> Result<Self, failure::Error> {
        let prev_ios = self.prev_ios;
        self.tty.try_clone().map(|tty| Self { tty, prev_ios })
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

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = sys::attr::set(&self.file, &self.prev_ios);
    }
}

impl ops::Deref for Raw {
    type Target = Tty;

    fn deref(&self) -> &Tty {
        &self.tty
    }
}

impl ops::DerefMut for Raw {
    fn deref_mut(&mut self) -> &mut Tty {
        &mut self.tty
    }
}

impl io::Read for Raw {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tty.read(buf)
    }
}

impl io::Write for Raw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tty.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tty.flush()
    }
}
