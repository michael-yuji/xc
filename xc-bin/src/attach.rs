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
use freebsd::event::{KEventExt, KqueueExt};
use freebsd::nix::libc::{STDIN_FILENO, STDOUT_FILENO, VMIN, VTIME};
use freebsd::nix::sys::event::{EventFilter, EventFlag, KEvent, Kqueue};
use freebsd::nix::sys::socket::{recv, send, MsgFlags};
use freebsd::nix::sys::termios::{tcsetattr, InputFlags, LocalFlags, OutputFlags, SetArg, Termios};
use freebsd::nix::unistd::{read, write};
use std::os::unix::net::UnixStream;
use std::os::unix::prelude::AsRawFd;

fn enable_raw(orig: &Termios) -> Result<(), freebsd::nix::Error> {
    let mut tio = orig.clone();
    let mut input_flags = tio.input_flags;
    let mut local_flags = tio.local_flags;
    let mut output_flags = tio.output_flags;

    input_flags = input_flags.union(InputFlags::IGNPAR);

    let blah = !(InputFlags::ISTRIP
        | InputFlags::INLCR
        | InputFlags::IGNCR
        | InputFlags::ICRNL
        | InputFlags::IXON
        | InputFlags::IXANY
        | InputFlags::IXOFF);

    input_flags = input_flags.intersection(blah);

    let blah2 = !(LocalFlags::ISIG
        | LocalFlags::ICANON
        | LocalFlags::ECHO
        | LocalFlags::ECHOE
        | LocalFlags::ECHOK
        | LocalFlags::ECHONL
        | LocalFlags::IEXTEN);

    local_flags = local_flags.intersection(blah2);

    output_flags = output_flags.intersection(OutputFlags::OPOST);

    tio.input_flags = input_flags;
    tio.output_flags = output_flags;
    tio.local_flags = local_flags;

    // send every character as soon as they arrive
    tio.control_chars[VMIN] = 1;
    tio.control_chars[VTIME] = 0;

    let stdin = std::io::stdin();
    tcsetattr(stdin, SetArg::TCSADRAIN, &tio)
}

fn with_raw_terminal<F, R: Copy>(f: F) -> Result<R, freebsd::nix::Error>
where
    F: Fn() -> Result<R, freebsd::nix::Error>,
{
    let tio = freebsd::nix::sys::termios::tcgetattr(std::io::stdin())?;
    enable_raw(&tio)?;
    let res = f()?;
    tcsetattr(std::io::stdin(), SetArg::TCSAFLUSH, &tio)?;
    Ok(res)
}

struct ForwardState {
    buffer: [u8; 512],
    escaped: bool,
    stream_to_local: Vec<u8>,
    local_to_stream: Vec<u8>,
}

enum Control {
    Break,
}

impl ForwardState {
    fn new() -> ForwardState {
        ForwardState {
            buffer: [0u8; 512],
            escaped: false,
            stream_to_local: Vec::new(),
            local_to_stream: Vec::new(),
        }
    }

    fn process_local_to_stream(&mut self, n_read: usize) -> Option<Control> {
        for byte in &self.buffer[..n_read] {
            if self.escaped {
                if byte == &b'q' {
                    return Some(Control::Break);
                } else {
                    self.escaped = false;
                }
            } else if byte == &0x10
            /* ctrl-p */
            {
                self.escaped = true;
            } else {
                self.local_to_stream.push(*byte);
            }
        }
        None
    }
}

pub fn run(path: impl AsRef<std::path::Path>) -> Result<bool, std::io::Error> {
    let path = path.as_ref();
    //    let path = "/var/run/xc.abcde";
    let stream = UnixStream::connect(path)?;
    let stream_fd = stream.as_raw_fd();

    Ok(with_raw_terminal(move || {
        let kq = Kqueue::new()?;
        let mut add_events = vec![
            KEvent::from_read(stream_fd),
            KEvent::from_read(STDIN_FILENO),
        ];
        let mut events = [KEvent::zero(); 4];
        let mut state = ForwardState::new();

        let mut break_by_user = false;

        'm: loop {
            let n_ev = kq.wait_events(&add_events, &mut events)?;
            //            let n_ev = kevent_ts(kq, &add_events, &mut events, None)?;
            add_events.clear();
            for event in events.iter().take(n_ev) {
                if event.ident() == stream_fd as usize {
                    if event.filter()? == EventFilter::EVFILT_READ {
                        let n_available = event.data() as usize;
                        if n_available == 0 {
                            state.stream_to_local.clear();
                            break 'm;
                        } else {
                            let to_read = n_available.min(state.buffer.len());
                            let n_recv =
                                recv(stream_fd, &mut state.buffer[..to_read], MsgFlags::empty())?;
                            state
                                .stream_to_local
                                .extend_from_slice(&state.buffer[..n_recv]);
                        }
                    } else if event.filter()? == EventFilter::EVFILT_WRITE {
                        let n_writable = event.data() as usize;
                        let n_write = n_writable.min(state.local_to_stream.len());
                        let n_send = send(
                            stream_fd,
                            &state.local_to_stream[..n_write],
                            MsgFlags::empty(),
                        )?;
                        state.local_to_stream.drain(..n_send);
                    }
                } else if event.ident() == STDIN_FILENO as usize {
                    let n_read = read(STDIN_FILENO, &mut state.buffer[..event.data() as usize])?;
                    if state.process_local_to_stream(n_read).is_some() {
                        break_by_user = true;
                        break 'm;
                    }
                } else if event.ident() == STDOUT_FILENO as usize {
                    let n_writable = event.data() as usize;
                    let n_write = n_writable.min(state.stream_to_local.len());
                    let w = write(STDOUT_FILENO, &state.stream_to_local[..n_write])?;
                    state.stream_to_local.drain(..w);
                }
            }

            if !state.stream_to_local.is_empty() {
                add_events.push(
                    KEvent::from_write(STDOUT_FILENO)
                        .set_flags(EventFlag::EV_ADD | EventFlag::EV_ONESHOT),
                );
            }

            if !state.local_to_stream.is_empty() {
                add_events.push(
                    KEvent::from_write(stream_fd)
                        .set_flags(EventFlag::EV_ADD | EventFlag::EV_ONESHOT),
                );
            }
        }

        // flush everything stream to local here

        Ok(break_by_user)
    })?)
}
