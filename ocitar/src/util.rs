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
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;

macro_rules! err {
    ($reason:expr) => {{
        log::error!($reason);
        Err(std::io::Error::new(std::io::ErrorKind::Other, $reason))
    }};
}

pub(crate) use err;

pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    let slice = bytes.as_ref();
    let mut buf = String::with_capacity(slice.len() * 2);
    for byte in slice {
        const TBL: [char; 16] = [
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
        ];
        buf.push(TBL[(*byte >> 4) as usize]);
        buf.push(TBL[(*byte & 0x0f) as usize]);
    }
    buf
}

pub fn str_from_nul_bytes_buf(buf: &[u8]) -> Result<&str, std::io::Error> {
    let buf = std::str::from_utf8(buf).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "failed to encode utf8 string")
    })?;
    Ok(buf.trim_end_matches('\0'))
}

pub struct DigestReader<R: Read> {
    source: R,
    digest: Sha256,
}

pub struct DigestReaderHandle<R: Read>(pub Rc<RefCell<DigestReader<R>>>);

impl<T: Read> DigestReader<T> {
    pub fn new<R: Read>(source: R) -> DigestReader<R> {
        DigestReader {
            source,
            digest: Sha256::new(),
        }
    }
    pub fn consume(&self) -> [u8; 32] {
        self.digest.clone().finalize().into()
    }
}

impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let count = self.source.read(buf)?;
        if count != 0 {
            self.digest.update(&buf[..count]);
        }
        Ok(count)
    }
}

impl<R: Read> Read for DigestReaderHandle<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.0.borrow_mut().read(buf)
    }
}


pub struct DigestSink<W: Write> {
    sink: W,
    digest: Rc<RefCell<Sha256>>,
}

//pub struct DigestSinkHandle<W: Write>(pub Rc<RefCell<DigestSink<W>>>);

impl<T: Write> DigestSink<T> {
    pub fn new<W: Write>(sink: W, digest: Rc<RefCell<Sha256>>) -> DigestSink<W> {
        DigestSink {
            sink,
            digest,
        }
    }
}
impl<W: Write> Write for DigestSink<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        let size = self.sink.write(buf)?;
        self.digest.borrow_mut().update(&buf[..size]);
        Ok(size)
    }
    fn flush(&mut self) -> Result<(), std::io::Error> {
        self.sink.flush()
    }
}

pub struct DigestWriter<W: Write> {
    sink: W,
    digest: Sha256,
}

impl<T: Write> DigestWriter<T> {
    pub fn new<W: Write>(sink: W) -> DigestWriter<W> {
        DigestWriter {
            sink,
            digest: Sha256::new(),
        }
    }

    pub fn consume(self) -> [u8; 32] {
        self.digest.finalize().into()
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        let size = self.sink.write(buf)?;
        self.digest.update(&buf[..size]);
        Ok(size)
    }
    fn flush(&mut self) -> Result<(), std::io::Error> {
        self.sink.flush()
    }
}

pub struct PrebufferedSource<R: Read> {
    buffer: Vec<u8>,
    source: R,
}

impl<R: Read> PrebufferedSource<R> {
    pub fn new(buffer: &[u8], source: R) -> PrebufferedSource<R> {
        PrebufferedSource {
            buffer: buffer.to_vec(),
            source,
        }
    }
}

impl<R: Read> Read for PrebufferedSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let from_buf = self.buffer.len().min(buf.len());

        if !self.buffer.is_empty() {
            buf[..from_buf].copy_from_slice(&self.buffer[..from_buf]);
            self.buffer = self.buffer[from_buf..].to_vec();
        }
        let cnt = self.source.read(&mut buf[from_buf..])?;
        Ok(cnt + from_buf)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_str_conversion() {
        let buf = b"ustar\0\0\0\0\0";
        let value = str_from_nul_bytes_buf(buf).unwrap();
        assert_eq!(value, "ustar");
    }

    #[test]
    fn test_prebuffered_read() -> std::io::Result<()> {
        let source = b"0123456789";
        let mut pbs = PrebufferedSource::new(b"01234", source.as_slice());
        let mut sink = [0u8; 15];
        pbs.read_exact(&mut sink[0..2])?;
        pbs.read_exact(&mut sink[2..5])?;
        pbs.read_exact(&mut sink[5..10])?;
        pbs.read_exact(&mut sink[10..15])?;
        assert_eq!(&sink, b"012340123456789");
        Ok(())
    }
}
