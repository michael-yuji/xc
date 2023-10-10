//! Process descriptor specific bits

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

use crate::event::{KEventExt, KqueueExt};

use nix::sys::event::{KEvent, Kqueue};
use std::os::unix::io::AsRawFd;

pub struct ProcFd(i32);

extern "C" {
    fn pdfork(fdp: *mut std::os::raw::c_int, flags: std::os::raw::c_int) -> nix::libc::pid_t;
}

pub fn pdwait(fd: i32) -> nix::Result<()> {
    let kq = Kqueue::new()?;
    let change_list = vec![KEvent::from_wait_pfd(fd)];
    let mut event_list = vec![KEvent::zero()];
    kq.wait_events(&change_list, &mut event_list)?;
    Ok(())
}

impl AsRawFd for ProcFd {
    fn as_raw_fd(&self) -> i32 {
        self.0
    }
}

pub enum PdForkResult {
    Parent { child: ProcFd, pid: i32 },
    Child,
}

#[allow(unused_unsafe)]
pub unsafe fn pd_fork() -> nix::Result<PdForkResult> {
    let mut pfd: std::os::raw::c_int = 0;
    unsafe {
        let pid = pdfork(&mut pfd, 0);
        if pid == 0 {
            Ok(PdForkResult::Child)
        } else {
            Ok(PdForkResult::Parent {
                child: ProcFd(pfd),
                pid,
            })
        }
    }
}
