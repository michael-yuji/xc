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
use nix::libc::{
    c_void, cmsghdr, iovec, msghdr, recvmsg, CMSG_DATA, CMSG_FIRSTHDR, CMSG_NXTHDR, MSG_CTRUNC,
    SCM_RIGHTS,
};
use nix::sys::socket::{c_uint, sendmsg, ControlMessage, MsgFlags, CMSG_SPACE};
use std::io::{IoSlice, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use thiserror::Error;

pub mod tokio_io;

/// A trait to implement support in sending a channel packet
pub trait PacketTransport {
    type Err: std::error::Error;
    fn send_packet(&mut self, packet: &Packet) -> Result<(), ChannelError<Self::Err>>;
    fn recv_packet(&mut self) -> Result<Packet, ChannelError<Self::Err>>;

    /// The maximum fds supported by the implementation, if the implementation does not support
    /// sending file descriptor at all, return 0
    fn max_supported_fds(&self) -> usize;
}

#[derive(Error, Debug)]
pub enum ChannelError<T: std::error::Error> {
    #[error("The number of fds embeded in this packet exceed the pre-defined limit")]
    ExceededPredefinedFdsLimit,
    #[error("The actual data size does not match")]
    DataLengthMismatch,
    #[error("The packet is truncated")]
    Truncated,
    #[error("Error in transport: {0}")]
    TransportationError(T),
}

impl<T: std::error::Error> From<T> for ChannelError<T> {
    fn from(err: T) -> ChannelError<T> {
        ChannelError::TransportationError(err)
    }
}

impl<T: std::error::Error> ChannelError<T> {
    pub fn map<A: std::error::Error, F>(self, transform: F) -> ChannelError<A>
    where
        F: Fn(T) -> A,
    {
        match self {
            Self::ExceededPredefinedFdsLimit => ChannelError::ExceededPredefinedFdsLimit,
            Self::DataLengthMismatch => ChannelError::DataLengthMismatch,
            Self::Truncated => ChannelError::Truncated,
            Self::TransportationError(t) => ChannelError::TransportationError(transform(t)),
        }
    }
}

impl PacketTransport for RawFd {
    type Err = std::io::Error;

    fn max_supported_fds(&self) -> usize {
        64
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), ChannelError<Self::Err>> {
        let data = &packet.data;
        let bytes_count = packet.data.len() as u64;
        let fds_len = packet.fds.len() as u64;

        let mut header = Vec::new();
        header.extend(bytes_count.to_be_bytes());
        header.extend(fds_len.to_be_bytes());

        let mut sptr = 0usize;

        while sptr != header.len() {
            let size = nix::sys::socket::send(*self, &header[sptr..], MsgFlags::empty())
                .map_err(std::io::Error::from)?;
            sptr += size;
        }

        if packet.fds.len() > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }

        let mut count = send_once(self.as_raw_fd(), data, &packet.fds)?;
        while count != bytes_count as usize {
            let len = nix::sys::socket::send(*self, &data[count..], MsgFlags::empty())
                .map_err(std::io::Error::from)?;
            //            let len = self.write(&data[count..])?;
            count += len;
        }
        Ok(())
    }

    fn recv_packet(&mut self) -> Result<Packet, ChannelError<Self::Err>> {
        let mut header_bytes = [0u8; 16];
        _ = nix::sys::socket::recv(*self, &mut header_bytes, MsgFlags::empty())
            .map_err(std::io::Error::from)?;
        //        _ = self.read(&mut header_bytes)?;

        let bytes_count = u64::from_be_bytes(header_bytes[0..8].try_into().unwrap()) as usize;
        let fds_count = u64::from_be_bytes(header_bytes[8..].try_into().unwrap()) as usize;

        if fds_count > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }
        let mut data = vec![0u8; bytes_count];
        let mut fds = Vec::new();

        let mut received = recv_packet_once(self.as_raw_fd(), fds_count, &mut data, &mut fds)?;

        while received != bytes_count {
            let len = nix::sys::socket::recv(*self, &mut data[received..], MsgFlags::empty())
                .map_err(std::io::Error::from)?;
            //            let len = self.read(&mut data[received..])?;
            received += len;
        }

        Ok(Packet { data, fds })
    }
}

impl PacketTransport for UnixStream {
    type Err = std::io::Error;

    fn max_supported_fds(&self) -> usize {
        64
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), ChannelError<Self::Err>> {
        let data = &packet.data;
        let bytes_count = packet.data.len() as u64;
        let fds_len = packet.fds.len() as u64;

        let mut header = Vec::new();
        header.extend(bytes_count.to_be_bytes());
        header.extend(fds_len.to_be_bytes());
        self.write_all(&header)?;
        if packet.fds.len() > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }

        let mut count = send_once(self.as_raw_fd(), data, &packet.fds)?;
        while count != bytes_count as usize {
            let len = self.write(&data[count..])?;
            count += len;
        }
        Ok(())
    }

    fn recv_packet(&mut self) -> Result<Packet, ChannelError<Self::Err>> {
        let mut header_bytes = [0u8; 16];
        _ = self.read(&mut header_bytes)?;

        let bytes_count = u64::from_be_bytes(header_bytes[0..8].try_into().unwrap()) as usize;
        let fds_count = u64::from_be_bytes(header_bytes[8..].try_into().unwrap()) as usize;

        if fds_count > self.max_supported_fds() {
            return Err(ChannelError::ExceededPredefinedFdsLimit);
        }
        let mut data = vec![0u8; bytes_count];
        let mut fds = Vec::new();

        let mut received = recv_packet_once(self.as_raw_fd(), fds_count, &mut data, &mut fds)?;

        while received != bytes_count {
            let len = self.read(&mut data[received..])?;
            received += len;
        }

        Ok(Packet { data, fds })
    }
}
pub(crate) fn send_once(
    fd: RawFd,
    data: &[u8],
    fds: &[RawFd],
) -> Result<usize, ChannelError<std::io::Error>> {
    let cmsgs = if fds.is_empty() {
        Vec::new()
    } else {
        vec![ControlMessage::ScmRights(fds)]
    };
    let iov = [IoSlice::new(data)];

    sendmsg::<()>(fd, &iov, &cmsgs, MsgFlags::empty(), None)
        .map_err(std::io::Error::from)
        .map_err(ChannelError::<std::io::Error>::from)
}

pub fn recv_packet_once(
    fd: RawFd,
    fds_count: usize,
    data: &mut Vec<u8>,
    fds: &mut Vec<RawFd>,
) -> Result<usize, ChannelError<std::io::Error>> {
    let mut space = unsafe {
        vec![0u8; CMSG_SPACE((std::mem::size_of::<RawFd>() * fds_count) as c_uint) as usize]
    };

    let mut msg_hdr: msghdr = unsafe { std::mem::zeroed() };
    let iov_base = data.as_mut_ptr() as *mut c_void;
    let mut iov = iovec {
        iov_base,
        iov_len: data.len(),
    };

    msg_hdr.msg_iov = &mut iov;
    msg_hdr.msg_iovlen = 1;
    msg_hdr.msg_controllen = space.len() as c_uint;
    msg_hdr.msg_control = space.as_mut_ptr() as *mut c_void;

    unsafe {
        let received = recvmsg(fd, &mut msg_hdr, 0);

        if received == -1 {
            Err(std::io::Error::from(nix::errno::Errno::last()))?;
        } else if msg_hdr.msg_flags & MSG_CTRUNC != 0 {
            return Err(ChannelError::Truncated);
        }
        /*
                #[allow(unused_assignments)]
                let mut cmsg: *const cmsghdr = std::ptr::null();
                cmsg = CMSG_FIRSTHDR(&msg_hdr);
        */
        let mut cmsg = CMSG_FIRSTHDR(&msg_hdr);
        let mut fds_count = fds_count;

        while !cmsg.is_null() && fds_count > 0 {
            if (*cmsg).cmsg_type != SCM_RIGHTS {
                continue;
            } else {
                // It seems the fds array we received are 0-terminated
                // (an extra 0 at the end of the array), however, I'm not sure
                // if this behaviour can be rely on hence the extra checks
                let arr_size = (*cmsg).cmsg_len - std::mem::size_of::<cmsghdr>() as u32;
                let fd_count = arr_size / std::mem::size_of::<RawFd>() as u32;
                let data_ptr = CMSG_DATA(cmsg) as *const i32;

                for offset in 0..(fd_count as isize) {
                    let fd = *data_ptr.offset(offset);
                    fds.push(fd.as_raw_fd());
                    fds_count -= 1;
                    if fds_count == 0 {
                        break;
                    }
                }
            }
            cmsg = CMSG_NXTHDR(&msg_hdr, cmsg);
        }
        Ok(received as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn test_transportation() {
        let (mut a, mut b) = UnixStream::pair().expect("cannot make socket pair");

        let join_handler = std::thread::spawn(move || {
            let bytes = [1u8, 2, 3, 4, 5];
            let (c, mut d) = UnixStream::pair().expect("cannot make socket pair");
            let (e, _) = UnixStream::pair().expect("cannot make socket pair");

            let packet = Packet {
                data: bytes.to_vec(),
                fds: vec![c.as_raw_fd(), e.as_raw_fd()],
            };
            b.send_packet(&packet)
                .expect("fail to send packet from sender thread");
            d.write_all(b"hello world")
                .expect("fail to send bytes in the inner socket pair");
        });

        let packet = a.recv_packet().expect("fail at receving packet");

        assert_eq!(packet.data, vec![1u8, 2, 3, 4, 5]);
        assert_eq!(packet.fds.len(), 2);

        let c_fd = packet.fds[0];

        let mut c = unsafe { UnixStream::from_raw_fd(c_fd) };

        let mut buf = Vec::new();
        _ = c
            .read_to_end(&mut buf)
            .expect("cannot send from inner socket pair");

        assert_eq!(buf, b"hello world".to_vec());

        join_handler.join().expect("cannot join thread");
    }
}
