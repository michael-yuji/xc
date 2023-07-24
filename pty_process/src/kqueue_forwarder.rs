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
use crate::buffer::Buffer;
use crate::PtyCommandExt;
use freebsd::event::KEventExt;
use nix::pty::openpty;
use nix::sys::event::{kevent_ts, kqueue, EventFilter, EventFlag, KEvent};
use std::io::Write;
use std::os::unix::io::AsRawFd;
//use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;

struct Client {
    fd: i32,
    offset: usize,
    stream: UnixStream,
}

impl Client {
    fn new(stream: UnixStream) -> Client {
        Client {
            fd: stream.as_raw_fd(),
            offset: 0,
            stream,
        }
    }
}

pub struct PtyForwarder<W: Write + Send + Sync> {
    child: std::process::Child,
    listener: UnixListener,
    clients: Vec<Client>,
    pty: i32,
    ingress: Vec<u8>,
    egress: Buffer<1048576>,
    output_log: W,
}

impl<W: Write + Send + Sync> PtyForwarder<W> {
    pub fn from_command(
        listener: UnixListener,
        mut command: Command,
        output_log: W,
    ) -> std::io::Result<PtyForwarder<W>> {
        let pty_result = openpty(None, None)?;
        let child = command.pty(&pty_result).spawn()?;
        Ok(PtyForwarder {
            child,
            listener,
            clients: Vec::new(),
            pty: pty_result.master,
            ingress: Vec::new(),
            egress: Buffer::new(),
            output_log,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn spawn(mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        let mut buf = [0u8; 512];
        let kq = kqueue()?;

        let listener_fd = self.listener.as_raw_fd();

        // add listener and process to the kqueue
        let mut change_list = Vec::new();
        let mut remove_list = Vec::new();
        let mut event_list = [KEvent::zero(); 8];

        change_list.push(KEvent::from_read(listener_fd));
        change_list.push(KEvent::from_read(self.pty));
        change_list.push(KEvent::from_wait_pid(self.child.id()));

        'kqloop: loop {
            //                        eprintln!("kqueue enter");
            let n_events = kevent_ts(kq, &change_list, &mut event_list, None)?;
            //                        eprintln!("kqueue get events: {n_events}");
            let event_list = &event_list[..n_events];

            change_list.clear();

            for event in event_list {
                //                                eprintln!("processing: {event:?}");

                let evfilt = event.filter().expect("unknown filter flag");

                if evfilt == EventFilter::EVFILT_PROC {
                    break 'kqloop;
                }

                if event.ident() as i32 == listener_fd {
                    let (stream, _addr) = self.listener.accept()?;
                    let fd = stream.as_raw_fd();
                    change_list.push(KEvent::from_read(fd));
                    self.clients.push(Client::new(stream));
                    continue;
                }

                if evfilt == EventFilter::EVFILT_READ && event.ident() as i32 == self.pty {
                    let mut bytes_available = event.data() as usize;

                    // if the pty is closed, there are never going to be anything for clients to
                    // send/recv, so we break early and wait until the child exit
                    if bytes_available == 0 {
                        break 'kqloop;
                    }

                    while bytes_available > 0 {
                        let read_len = buf.len().min(bytes_available);
                        let buf = &mut buf[..read_len];
                        let read = nix::unistd::read(self.pty, buf)?;
                        _ = self.output_log.write_all(buf);
                        self.egress.append_from_slice(buf);
                        bytes_available -= read;
                    }
                    continue;
                }

                if evfilt == EventFilter::EVFILT_WRITE && event.ident() as i32 == self.pty {
                    let written = nix::unistd::write(self.pty, &self.ingress).unwrap();
                    self.ingress.drain(..written);
                    continue;
                }

                if let Some(mut client) = self
                    .clients
                    .iter_mut()
                    .find(|client| client.fd as usize == event.ident())
                {
                    // skip processing event of fds we intend to close
                    if !remove_list.contains(&client.fd) {
                        match evfilt {
                            EventFilter::EVFILT_READ if event.data() == 0 => {
                                // the beauty of kqueue: when the fd close they will be removed from
                                // the kq automatically. However we still need to be aware that same
                                // event containing the supposingly closed fd can still return
                                remove_list.push(client.fd);
                            }
                            EventFilter::EVFILT_READ => {
                                let mut bytes_available = event.data() as usize;
                                while bytes_available > 0 {
                                    let read_len = buf.len().min(bytes_available);
                                    let buf = &mut buf[..read_len];
                                    let read = nix::unistd::read(client.fd, buf)?;
                                    self.ingress.extend_from_slice(&buf[..read]);
                                    bytes_available -= read_len;
                                }
                            }
                            EventFilter::EVFILT_WRITE => {
                                if let Ok((_, m)) =
                                    self.egress.read_to_sync(client.offset, &mut client.stream)
                                {
                                    client.offset = m;
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }

            // if there are clients have outdated state, mark them as need to write
            for client in self.clients.iter() {
                if client.offset < self.egress.input_count {
                    change_list
                        .push(KEvent::from_write(client.fd).set_flags(EventFlag::EV_ONESHOT));
                }
            }
            // if there are client written into ingress, mark pty to handle the input
            if !self.ingress.is_empty() {
                change_list.push(KEvent::from_write(self.pty).set_flags(EventFlag::EV_ONESHOT));
            }

            // even the worst case is O(rn^2), it is probably faster with Vec than Hmap as we are
            // assuming to have only a handful and clients
            for client in remove_list.iter() {
                for i in 0..self.clients.len() {
                    if self.clients[i].fd == *client {
                        // stream should drop here and be closed
                        self.clients.remove(i);
                    }
                }
            }

            remove_list.clear();
        }

        self.clients.clear();

        self.child.wait()
    }
}
