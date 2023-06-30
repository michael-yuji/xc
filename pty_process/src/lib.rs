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
pub mod buffer;
pub mod kqueue_forwarder;

use nix::fcntl::{open, OFlag};
use nix::libc::{ioctl, TIOCNOTTY, TIOCSCTTY};
use nix::pty::OpenptyResult;
use nix::sys::stat::Mode;
use nix::unistd::{close, dup2, setsid};
use std::os::unix::process::CommandExt;
use std::process::Command;
use tokio::process::Command as TokioCommand;

pub trait PtyCommandExt {
    fn pty(&mut self, pty: &OpenptyResult) -> &mut Command;
}
pub trait TokioPtyCommandExt {
    fn pty(&mut self, pty: &OpenptyResult) -> &mut TokioCommand;
}

macro_rules! pty_impl {
    ($self:expr, $pty:expr) => {{
        let primary = $pty.master;
        let replica = $pty.slave;
        unsafe {
            $self.pre_exec(move || {
                if let Ok(fd) = open("/dev/tty", OFlag::O_RDWR, Mode::empty()) {
                    ioctl(fd, TIOCNOTTY);
                }
                setsid().expect("setsid");
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
            /*
            $self.stdin(std::process::Stdio::from_raw_fd(replica));
            $self.stdout(std::process::Stdio::from_raw_fd(replica));
            $self.stderr(std::process::Stdio::from_raw_fd(replica));
            */
        }
        $self
    }};
}

impl TokioPtyCommandExt for TokioCommand {
    fn pty(&mut self, pty: &OpenptyResult) -> &mut TokioCommand {
        pty_impl!(self, pty)
    }
}

impl PtyCommandExt for Command {
    fn pty(&mut self, pty: &OpenptyResult) -> &mut Command {
        pty_impl!(self, pty)
    }
}
