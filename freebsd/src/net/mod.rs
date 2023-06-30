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
pub mod ifconfig;
pub mod pf;

use nix::sys::socket::{getsockopt, XuCred};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone)]
pub struct UnixCredential {
    pub uid: u32,
    pub gids: Vec<u32>,
}

impl UnixCredential {
    pub fn from_socket(fd: &impl AsRawFd) -> Result<UnixCredential, std::io::Error> {
        let cred = getsockopt(fd.as_raw_fd(), nix::sys::socket::sockopt::LocalPeerCred)?;
        Ok(UnixCredential {
            uid: cred.uid(),
            gids: cred.groups().to_vec(),
        })
    }
}

pub trait UnixStreamExt {
    fn xucred(&self) -> Result<XuCred, std::io::Error>;
}

impl UnixStreamExt for UnixStream {
    fn xucred(&self) -> Result<XuCred, std::io::Error> {
        let cred = getsockopt(self.as_raw_fd(), nix::sys::socket::sockopt::LocalPeerCred)?;
        Ok(cred)
    }
}
