use std::ptr;

/// Get raw fds for master/slave ends of a new pty
#[cfg(target_os = "linux")]
fn openpty(rows: u8, cols: u8) -> Result<(libc::c_int, libc::c_int), failure::Error> {
    let mut master = 0;
    let mut slave = 0;

    let win = libc::winsize {
        ws_row: libc::c_ushort::from(rows),
        ws_col: libc::c_ushort::from(cols),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let res = unsafe { libc::openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null(), &win) };

    if res < 0 {
        Err(failure::err_msg("openpty failed"))
    } else {
        Ok((master, slave))
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn openpty(rows: u8, cols: u8) -> Result<(libc::c_int, libc::c_int), failure::Error> {
    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;

    let mut win = libc::winsize {
        ws_row: libc::c_ushort::from(rows),
        ws_col: libc::c_ushort::from(cols),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let res = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut win,
        )
    };

    if res < 0 {
        Err(failure::err_msg("openpty failed"))
    } else {
        Ok((master, slave))
    }
}
