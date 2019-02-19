use std::fs;
use std::{io, mem};

use super::libc::c_int;
use super::{cvt, Termios};

pub fn get(file: &fs::File) -> io::Result<Termios> {
    use std::os::unix::io::AsRawFd;

    extern "C" {
        pub fn tcgetattr(fd: c_int, termptr: *mut Termios) -> c_int;
    }

    let fd = file.as_raw_fd();
    unsafe {
        let mut termios = mem::zeroed();
        cvt(tcgetattr(fd, &mut termios))?;
        Ok(termios)
    }
}

pub fn set(file: &fs::File, termios: &Termios) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    extern "C" {
        pub fn tcsetattr(fd: c_int, opt: c_int, termptr: *const Termios) -> c_int;
    }

    let fd = file.as_raw_fd();
    cvt(unsafe { tcsetattr(fd, 0, termios) }).and(Ok(()))
}

pub fn make_raw(termios: &mut Termios) {
    extern "C" {
        pub fn cfmakeraw(termptr: *mut Termios);
    }
    unsafe { cfmakeraw(termios) }
}
