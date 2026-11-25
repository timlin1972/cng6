//! 偽終端機(PTY): 開一個 shell, 讓它以為自己接在真的終端機上(顏色、vim、top 都能用)。

use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub struct Pty {
    /// 主控端: 讀到的是 shell 的輸出, 寫進去的是鍵盤輸入
    pub master: File,
    pub child: Child,
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// 開一個 login shell(會讀 ~/.profile, 環境和 ssh 登入時一樣)
pub fn spawn(shell: &str, cols: u16, rows: u16, cwd: &Path) -> io::Result<Pty> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let ws = winsize(cols, rows);
    // SAFETY: 參數都是有效的指標; 失敗時回傳 -1
    let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty 成功, 兩個 fd 都是新開的, 交給 OwnedFd / File 管理(會自動關閉)
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    // 主控端不要被 shell 繼承
    // SAFETY: 對有效的 fd 設定旗標
    unsafe {
        let fd = std::os::fd::AsRawFd::as_raw_fd(&master);
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let mut cmd = Command::new(shell);
    cmd.arg("-l")
        .current_dir(cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        // 不是由 cng6.sh 管理的程式
        .env_remove("CNG6_SUPERVISED")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // 新的 session, 並把這個 PTY 當成控制終端機(Ctrl-C、工作控制才會正常)
    // SAFETY: pre_exec 在 fork 之後、exec 之前執行, 只呼叫 async-signal-safe 的函式
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(Pty { master, child })
}

/// 改變終端機大小(shell 會收到 SIGWINCH)
pub fn resize(master: &File, cols: u16, rows: u16) -> io::Result<()> {
    let ws = winsize(cols, rows);
    let fd = std::os::fd::AsRawFd::as_raw_fd(master);
    // SAFETY: fd 有效, ws 是正確的結構
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 結束 shell 和它開的所有程式(同一個 process group)
pub fn hangup(pid: u32) {
    // SAFETY: 對 process group 送訊號
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGHUP);
    }
}
