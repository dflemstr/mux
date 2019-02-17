use std::os::unix;
use std::{io, mem};

use super::libc::c_int;
use super::{cvt, Termios};

pub fn get_terminal_attr(fd: unix::io::RawFd) -> io::Result<Termios> {
    extern "C" {
        pub fn tcgetattr(fd: c_int, termptr: *mut Termios) -> c_int;
    }
    unsafe {
        let mut termios = mem::zeroed();
        cvt(tcgetattr(fd, &mut termios))?;
        Ok(termios)
    }
}

pub fn set_terminal_attr(fd: unix::io::RawFd, termios: &Termios) -> io::Result<()> {
    extern "C" {
        pub fn tcsetattr(fd: c_int, opt: c_int, termptr: *const Termios) -> c_int;
    }
    cvt(unsafe { tcsetattr(fd, 0, termios) }).and(Ok(()))
}

pub fn raw_terminal_attr(termios: &mut Termios) {
    extern "C" {
        pub fn cfmakeraw(termptr: *mut Termios);
    }
    unsafe { cfmakeraw(termios) }
}
