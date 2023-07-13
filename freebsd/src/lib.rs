//! Library for FreeBSD system bits

// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.

pub mod event;
pub mod fs;
pub mod net;
pub mod procdesc;

pub use jail;
pub use nix;
pub use nix::libc;

use nix::fcntl::{open, OFlag};
use nix::libc::{ioctl, TIOCNOTTY, TIOCSCTTY};
use nix::pty::OpenptyResult;
use nix::sys::stat::Mode;
use nix::unistd::{close, dup2, setsid, setuid, chdir};
use serde::Deserialize;
use std::os::raw::{c_int, c_uint};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

#[macro_export]
macro_rules! env_or_default {
    ($env:expr, $default:expr) => {
        match option_env!($env) {
            Some(value) => value,
            None => $default,
        }
    };
}

#[link(name = "c")]
extern "C" {
    fn kldfind(file: *const std::os::raw::c_char) -> std::os::raw::c_int;
}

pub fn exists_kld(file: impl AsRef<str>) -> bool {
    unsafe {
        let c_str = std::ffi::CString::new(file.as_ref()).ok().unwrap();
        kldfind(c_str.as_ptr()) != -1
    }
}

#[derive(Deserialize)]
struct Process {
    pid: String,
}

#[derive(Deserialize)]
struct ProcessInformation {
    process: Vec<Process>,
}

#[derive(Deserialize)]
struct Ps {
    #[serde(rename = "process-information")]
    process_information: ProcessInformation,
}

pub fn pids_in_jail(jail: i32) -> Vec<u32> {
    let mut pids = Vec::new();
    let ps_output = Command::new("ps")
        .arg("--libxo=json")
        .arg("-J")
        .arg(jail.to_string())
        .output()
        .expect("cannot spawn `ps`");
    let ps: Ps = serde_json::from_slice(&ps_output.stdout).expect("cannot decode ps output");
    for process in ps.process_information.process.iter() {
        let pid = &process.pid.parse::<u32>().expect("unexpected pid format");
        pids.push(*pid);
    }
    pids
}

pub fn tag_io_err<S: AsRef<str>>(tag: S, err: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("{}: {err:?}", tag.as_ref()),
    )
}

extern "C" {
    /// Close all file descriptors >= lowfd
    pub fn closefrom(lowfd: c_int);
    pub fn close_range(lowfd: c_uint, highfd: c_uint, flags: c_int) -> c_int;
}

pub trait FreeBSDCommandExt {
    /// Set the uid from the process before exec, after clone. Unilke the uid() implementation in
    /// std::process:Command, this works for jail as well.
    fn juid(&mut self, uid: u32) -> &mut Command;

    /// Detach the child process from controlling terminal, attach to the replica side of the pty
    /// and use it as controlling terminal
    fn pty(&mut self, pty: &OpenptyResult) -> &mut Command;

    fn jwork_dir(&mut self, wd: impl AsRef<Path>) -> &mut Command;
}

#[cfg(feature = "tokio")]
pub trait FreeBSDTokioCommandExt {
    /// Set the uid from the process before exec, after clone. Unilke the uid() implementation in
    /// std::process:Command, this works for jail as well.
    fn juid(&mut self, uid: u32) -> &mut tokio::process::Command;

    /// Detach the child process from controlling terminal, attach to the replica side of the pty
    /// and use it as controlling terminal
    fn pty(&mut self, pty: &OpenptyResult) -> &mut tokio::process::Command;

    fn jail(&mut self, jail: &jail::RunningJail) -> &mut tokio::process::Command;

    fn jwork_dir(&mut self, wd: impl AsRef<Path>) -> &mut tokio::process::Command;
}

#[cfg(feature = "tokio")]
impl FreeBSDTokioCommandExt for tokio::process::Command {
    fn juid(&mut self, uid: u32) -> &mut tokio::process::Command {
        unsafe {
            self.pre_exec(move || {
                setuid(nix::unistd::Uid::from_raw(uid))?;
                Ok(())
            });
        }
        self
    }

    fn jwork_dir(&mut self, wd: impl AsRef<Path>) -> &mut tokio::process::Command {
        let os_str = wd.as_ref().to_path_buf().as_os_str().to_os_string();
        unsafe {
            self.pre_exec(move || {
                chdir(os_str.as_os_str())?;
                Ok(())
            });
        }
        self
    }

    fn jail(&mut self, jail: &jail::RunningJail) -> &mut tokio::process::Command {
        let jail = *jail;
        unsafe {
            self.pre_exec(move || jail.attach().map_err(|_err| panic!()));
        }
        self
    }

    fn pty(&mut self, pty: &OpenptyResult) -> &mut tokio::process::Command {
        let primary = pty.master;
        let replica = pty.slave;
        unsafe {
            self.pre_exec(move || {
                // detach from the controlling terminal
                if let Ok(fd) = open("/dev/tty", OFlag::O_RDWR, Mode::empty()) {
                    ioctl(fd, TIOCNOTTY);
                }
                setsid().expect("Cannot setsid");
                if ioctl(replica, TIOCSCTTY) == -1 {
                    Err(std::io::Error::last_os_error())?;
                }
                close(primary)?;
                dup2(replica, 0)?;
                dup2(replica, 1)?;
                dup2(replica, 2)?;
                close(replica)?;
                Ok(())
            });
        }
        self
    }
}

impl FreeBSDCommandExt for std::process::Command {
    fn juid(&mut self, uid: u32) -> &mut Command {
        unsafe {
            self.pre_exec(move || {
                setuid(nix::unistd::Uid::from_raw(uid))?;
                Ok(())
            });
        }
        self
    }

    fn jwork_dir(&mut self, wd: impl AsRef<Path>) -> &mut Command {
        let os_str = wd.as_ref().to_path_buf().as_os_str().to_os_string();
        unsafe {
            self.pre_exec(move || {
                chdir(os_str.as_os_str())?;
                Ok(())
            });
        }
        self
    }

    fn pty(&mut self, pty: &OpenptyResult) -> &mut Command {
        let primary = pty.master;
        let replica = pty.slave;
        unsafe {
            self.pre_exec(move || {
                // detach from the controlling terminal
                if let Ok(fd) = open("/dev/tty", OFlag::O_RDWR, Mode::empty()) {
                    ioctl(fd, TIOCNOTTY);
                }
                setsid().expect("Cannot setsid");
                if ioctl(replica, TIOCSCTTY) == -1 {
                    Err(std::io::Error::last_os_error())?;
                }
                close(primary)?;
                dup2(replica, 0)?;
                dup2(replica, 1)?;
                dup2(replica, 2)?;
                close(replica)?;
                Ok(())
            });
        }
        self
    }
}
