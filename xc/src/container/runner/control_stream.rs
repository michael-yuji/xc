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

use ipc::packet::codec::json::JsonPacket;
use ipc::packet::Packet;
use ipc::proto::Request;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

#[derive(Debug)]
struct ReadingPacket {
    buffer: Vec<u8>,
    read_len: usize,
    expected_len: usize,
    fds: Vec<RawFd>,
}

impl ReadingPacket {
    fn ready(&self) -> bool {
        self.read_len == self.expected_len
    }

    fn read(&mut self, socket: &mut UnixStream, known_avail: usize) -> Result<(), std::io::Error> {
        if !self.ready() {
            let len = socket.read(&mut self.buffer[self.read_len..][..known_avail])?;
            self.read_len += len;
        }
        Ok(())
    }

    fn new(socket: &mut UnixStream) -> Result<ReadingPacket, anyhow::Error> {
        let mut header_bytes = [0u8; 16];
        _ = socket.read(&mut header_bytes)?;
        let expected_len = u64::from_be_bytes(header_bytes[0..8].try_into().unwrap()) as usize;
        let fds_count = u64::from_be_bytes(header_bytes[8..].try_into().unwrap()) as usize;
        if fds_count > 64 {
            panic!("")
        }
        let mut buffer = vec![0u8; expected_len];
        let mut fds = Vec::new();
        let read_len =
            ipc::transport::recv_packet_once(socket.as_raw_fd(), fds_count, &mut buffer, &mut fds)?;

        Ok(Self {
            buffer,
            read_len,
            expected_len,
            fds,
        })
    }
}

pub(crate) enum Readiness<T> {
    Pending,
    Ready(T),
}

// XXX: naively pretend writes always successful
#[derive(Debug)]
pub(crate) struct ControlStream {
    socket: UnixStream,
    processing: Option<ReadingPacket>,
}

impl ControlStream {
    pub(crate) fn new(socket: UnixStream) -> ControlStream {
        ControlStream {
            socket,
            processing: None,
        }
    }

    pub(crate) fn socket_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    pub(crate) fn try_get_request(
        &mut self,
        known_avail: usize,
    ) -> Result<Readiness<(String, JsonPacket)>, anyhow::Error> {
        self.pour_in_bytes(known_avail)
            .and_then(|readiness| match readiness {
                Readiness::Pending => Ok(Readiness::Pending),
                Readiness::Ready(packet) => {
                    let request: ipc::packet::TypedPacket<Request> =
                        packet.map_failable(|vec| serde_json::from_slice(vec))?;

                    let method = request.data.method.to_string();
                    let packet = request.map(|req| req.value.clone());

                    Ok(Readiness::Ready((method, packet)))
                }
            })
    }

    pub(crate) fn pour_in_bytes(
        &mut self,
        known_avail: usize,
    ) -> Result<Readiness<Packet>, anyhow::Error> {
        if let Some(reading_packet) = &mut self.processing {
            if reading_packet.ready() {
                panic!("the client is sending more bytes than expected");
            }
            reading_packet.read(&mut self.socket, known_avail).unwrap();
        } else {
            let reading_packet = ReadingPacket::new(&mut self.socket).unwrap();
            self.processing = Some(reading_packet);
        }
        let Some(processing) = self.processing.take() else { panic!() };
        if processing.ready() {
            Ok(Readiness::Ready(Packet {
                data: processing.buffer,
                fds: processing.fds,
            }))
        } else {
            self.processing = Some(processing);
            Ok(Readiness::Pending)
        }
    }
}
