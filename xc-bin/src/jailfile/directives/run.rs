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
//

use super::Directive;
use crate::jailfile::parse::Action;
use crate::jailfile::JailContext;

use anyhow::{bail, Result};
use freebsd::event::{kevent_classic, EventFdNotify, KEventExt};
use freebsd::nix::sys::event::{EventFilter, KEvent};
use freebsd::nix::unistd::pipe;
use ipc::packet::codec::{Fd, Maybe};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use tracing::{error, info, warn};
use xcd::ipc::*;

#[allow(dead_code)]
#[derive(Debug)]
enum Input {
    // XXX: File as input has not yet implemented
    File(File),
    Content(String),
    None,
}

impl Input {
    fn is_none(&self) -> bool {
        matches!(self, Input::None)
    }
}

trait InputSource {
    fn is_empty(&self) -> bool;
    fn read_to(&mut self, dest: &mut [u8]) -> usize;
}

struct VecSlice {
    inner: Vec<u8>,
    offset: usize,
}

impl VecSlice {
    fn new(inner: &[u8]) -> VecSlice {
        VecSlice {
            inner: inner.to_vec(),
            offset: 0,
        }
    }
}

impl InputSource for VecSlice {
    fn is_empty(&self) -> bool {
        self.inner.len() == self.offset
    }
    fn read_to(&mut self, dest: &mut [u8]) -> usize {
        let len = dest.len().min(self.inner.len() - self.offset);
        dest[..len].copy_from_slice(&self.inner[self.offset..][..len]);
        self.offset += len;
        len
    }
}

pub(crate) struct RunDirective {
    shell: String,
    command: String,
    envs: HashMap<String, String>,
    input: Input,
}

impl Directive for RunDirective {
    fn up_to_date(&self) -> bool {
        true
    }
    fn from_action(action: &Action) -> Result<RunDirective> {
        let command = action.args.join(" ").to_string();
        let input = match &action.heredoc {
            Some(value) => Input::Content(value.to_string()),
            None => Input::None,
        };

        Ok(RunDirective {
            //            arg0: arg0.to_string(),
            shell: "/bin/sh".to_string(),
            command,
            envs: HashMap::new(),
            input,
        })
    }

    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        let notify = EventFdNotify::new();
        let (stdout_a, stdout_b) = pipe()?;
        let (stderr_a, stderr_b) = pipe()?;
        let (stdin_a, stdin_b) = pipe()?;

        let kq = unsafe { freebsd::nix::libc::kqueue() };

        info!("RUN: (shell = {}) {}", self.shell, self.command);

        let request = ExecCommandRequest {
            name: context.container_id.clone().expect("container not set"),
            arg0: self.shell.clone(),
            args: vec!["-c".to_string(), self.command.to_string()],
            envs: self.envs.clone(),
            stdin: Maybe::Some(Fd(stdin_b)),
            stdout: Maybe::Some(Fd(stdout_b)),
            stderr: Maybe::Some(Fd(stderr_b)),
            user: None,
            group: None,
            notify: Maybe::Some(Fd(notify.as_raw_fd())),
            use_tty: false,
        };

        match do_exec(&mut context.conn, request)? {
            Ok(_) => {
                let stdin_event = KEvent::from_write(stdin_a);
                let stdout_event = KEvent::from_read(stdout_a);
                let stderr_event = KEvent::from_read(stderr_a);
                let exit_event = KEvent::from_read(notify.as_raw_fd());

                if self.input.is_none() {
                    _ = kevent_classic(kq, &[stdout_event, stderr_event, exit_event], &mut [])?;
                } else {
                    _ = kevent_classic(
                        kq,
                        &[stdin_event, stdout_event, stderr_event, exit_event],
                        &mut [],
                    )?;
                }

                let mut events = [KEvent::zero(); 4];

                let mut writer: Box<dyn InputSource> = match &self.input {
                    Input::None => Box::new(VecSlice::new(&[])),
                    Input::File(_file) => {
                        error!("using file as stdin has not been implemented");
                        todo!()
                    }
                    Input::Content(content) => Box::new(VecSlice::new(content.as_bytes())),
                };

                //                let mut remaining = writer.unwrap_or_default();
                let mut stdout_buf = vec![0u8; 8192];
                let mut stderr_buf = vec![0u8; 8192];
                let mut write_buf = vec![0u8; 8192];

                'kq: loop {
                    let nev = kevent_classic(kq, &[], &mut events)?;
                    for event in &events[..nev] {
                        match event.filter().unwrap() {
                            EventFilter::EVFILT_READ => {
                                let fd = event.ident() as i32;
                                let mut available = event.data() as usize;

                                if fd == notify.as_raw_fd() {
                                    break 'kq;
                                } else if fd == stdout_a {
                                    while available > 0 {
                                        match freebsd::nix::unistd::read(
                                            fd,
                                            &mut stdout_buf[..available.min(8192)],
                                        ) {
                                            Err(err) => {
                                                error!("cannot read from remote stdout: {err}");
                                                if let Err(err) = freebsd::nix::unistd::close(fd) {
                                                    warn!("cannot close receiving end of remote stdout pipe: {err}")
                                                }
                                            }
                                            Ok(bytes) => {
                                                available -= bytes;
                                                _ = std::io::stdout()
                                                    .write_all(&stdout_buf[..bytes]);
                                            }
                                        }
                                    }
                                } else if fd == stderr_a {
                                    while available > 0 {
                                        match freebsd::nix::unistd::read(
                                            fd,
                                            &mut stderr_buf[..available.min(8192)],
                                        ) {
                                            Err(err) => {
                                                error!("cannot read from remote stderr: {err}");
                                                if let Err(err) = freebsd::nix::unistd::close(fd) {
                                                    warn!("cannot close receiving end of remote stderr pipe: {err}")
                                                }
                                            }
                                            Ok(bytes) => {
                                                available -= bytes;
                                                _ = std::io::stderr()
                                                    .write_all(&stderr_buf[..bytes]);
                                            }
                                        }
                                    }
                                } else {
                                    unreachable!()
                                }
                            }
                            EventFilter::EVFILT_WRITE => {
                                let fd = event.ident() as i32;
                                let writable = event.data() as usize;

                                let bytes_to_write =
                                    writer.read_to(&mut write_buf[..writable.min(8192)]);

                                match freebsd::nix::unistd::write(fd, &write_buf[..bytes_to_write])
                                {
                                    Err(err) => {
                                        error!("cannot write to remote process stdin: {err}");
                                        _ = freebsd::nix::unistd::close(fd);
                                    }
                                    Ok(bytes) => {
                                        if bytes != bytes_to_write {
                                            error!(
                                                "expect to write {} but actual is {}",
                                                bytes_to_write, bytes
                                            );
                                            panic!(
                                                "expect to write {} but actual is {}",
                                                bytes_to_write, bytes
                                            );
                                        }
                                    }
                                }
                                if writer.is_empty() {
                                    _ = freebsd::nix::unistd::close(fd);
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }

                if let Err(err) = freebsd::nix::unistd::close(kq) {
                    warn!("cannot close kq fd: {err}")
                }
                if let Err(err) = freebsd::nix::unistd::close(stdout_a) {
                    warn!("cannot close stdout pipe: {err}")
                }
                if let Err(err) = freebsd::nix::unistd::close(stderr_a) {
                    warn!("cannot close stderr pipe: {err}")
                }
                if let Err(err) = freebsd::nix::unistd::close(stdin_a) {
                    warn!("cannot close stdin pipe: {err}")
                }
                Ok(())
            }
            Err(err) => bail!("exec failure: {err:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buf_slice() {
        let mut slice = [0, 1, 2, 3];
        let mut buf_slice = VecSlice::new(&mut slice);

        let mut buf1 = [0u8; 2];
        let mut buf2 = [0u8; 2];

        let c1 = buf_slice.read_to(&mut buf1);
        assert_eq!(c1, 2);
        assert_eq!(buf1, [0, 1]);
        assert!(!buf_slice.is_empty());
        let c2 = buf_slice.read_to(&mut buf2);
        assert_eq!(c2, 2);
        assert_eq!(buf2, [2, 3]);
        assert!(buf_slice.is_empty());
    }
}
