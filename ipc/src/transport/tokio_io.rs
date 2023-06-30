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
use crate::packet::Packet;
use crate::transport::{recv_packet_once, send_once, ChannelError};
use std::os::fd::AsRawFd;
use tokio::net::UnixStream;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A trait to implement support in sending a channel packet
#[async_trait::async_trait]
pub trait AsyncPacketTransport {
    type Err: std::error::Error;
    async fn send_packet(&mut self, packet: &Packet) -> Result<(), ChannelError<Self::Err>>;
    async fn recv_packet(&mut self) -> Result<Packet, ChannelError<Self::Err>>;

    /// The maximum fds supported by the implementation, if the implementation does not support
    /// sending file descriptor at all, return 0
    fn max_supported_fds(&self) -> usize;
}

#[async_trait::async_trait]
impl AsyncPacketTransport for UnixStream {
    type Err = std::io::Error;

    fn max_supported_fds(&self) -> usize {
        64
    }

    async fn send_packet(&mut self, packet: &Packet) -> Result<(), ChannelError<Self::Err>> {
        let data = &packet.data;
        let bytes_count = packet.data.len() as u64;
        let fds_len = packet.fds.len() as u64;

        let mut header = Vec::new();
        header.extend(bytes_count.to_be_bytes());
        header.extend(fds_len.to_be_bytes());
        self.write_all(&header).await?;
        if packet.fds.len() > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }
        let mut count = send_once(self.as_raw_fd(), data, &packet.fds)?;
        while count != bytes_count as usize {
            let len = self.write(&data[count..]).await?;
            count += len;
        }
        Ok(())
    }

    async fn recv_packet(&mut self) -> Result<Packet, ChannelError<Self::Err>> {
        let mut header_bytes = [0u8; 16];
        _ = self.read(&mut header_bytes).await?;

        let bytes_count = u64::from_be_bytes(header_bytes[0..8].try_into().unwrap()) as usize;
        let fds_count = u64::from_be_bytes(header_bytes[8..].try_into().unwrap()) as usize;

        if fds_count > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }
        let mut data = vec![0u8; bytes_count];
        let mut fds = Vec::new();

        let mut received = recv_packet_once(self.as_raw_fd(), fds_count, &mut data, &mut fds)?;
        while received != bytes_count {
            let bytes = self.read(&mut data[received..]).await?;
            received += bytes;
        }
        Ok(Packet { data, fds })
    }
}
