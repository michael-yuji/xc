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

/// A buffer only capable of storeing fixed size data and retain only the last
/// appended data.
pub struct Buffer<const N: usize> {
    pub buffer: Box<[u8; N]>,
    pub input_count: usize,
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Buffer<N> {
        Buffer::new()
    }
}

impl<const N: usize> Buffer<N> {
    pub fn new() -> Buffer<N> {
        Buffer {
            buffer: Box::new([0u8; N]),
            input_count: 0,
        }
    }

    pub fn append_from_slice(&mut self, src: &[u8]) {
        let start_index = self.input_count % N;
        let first_bits_len = N - start_index;

        // if we are copying more data than we can hold, we can discard all but the last bits of
        // data and fill the entire buffer with it
        if src.len() >= N {
            let delimiter_offset = (self.input_count + src.len()) % N;
            let src = &src[src.len() - N..];
            self.buffer[delimiter_offset..].copy_from_slice(&src[..(N - delimiter_offset)]);
            self.buffer[..delimiter_offset].copy_from_slice(&src[(N - delimiter_offset)..]);
        } else if first_bits_len > src.len() {
            self.buffer[start_index..(start_index + src.len())].copy_from_slice(src);
        } else {
            let remaining = src.len() - first_bits_len;
            self.buffer[start_index..].copy_from_slice(&src[..first_bits_len]);
            self.buffer[..remaining].copy_from_slice(&src[first_bits_len..]);
        }

        self.input_count += src.len();
    }

    pub fn read_to_sync(
        &self,
        offset: usize,
        writer: &mut std::os::unix::net::UnixStream,
    ) -> Result<(usize, usize), std::io::Error> {
        use std::io::Write;
        if offset > self.input_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "client offset greater than total bytes received",
            ));
        }

        // if the offset the client held is from way too early that we don't hold anymore, we can
        // only gives all new ones
        if self.input_count - offset >= N {
            let start_index = self.input_count % N;
            let len_to_bound = N - start_index;
            let written = writer.write(&self.buffer[start_index..])?;
            if written < len_to_bound {
                Ok((written, self.input_count))
            } else {
                Ok((
                    writer.write(&self.buffer[..(N - len_to_bound)])?,
                    self.input_count,
                ))
            }
        } else {
            let prev_index = offset % N;
            let curr_index = self.input_count % N;
            // if historically there are less than N bytes of data entered the buffer, we can
            // safely treat it as a normal vector
            if self.input_count < N {
                let written = writer.write(&self.buffer[offset..self.input_count])?;
                Ok((written, self.input_count))
            } else if curr_index > prev_index {
                // handle the case where the buffer looks like [......<reader>.....<current>....],
                // in this configuration, it is impossible to have bytes between 0..reader that the
                // reader haven't take. Because if there are bytes between 0..reader such that the
                // reader have not yet consumed, that means either <current> in at least N bytes
                // ahead of <reader> such that it's physical offset is after <reader>, or it should
                // be pointing at an offset before <reader>; which the first case is guarded by the
                // outter if statement with (>=N) and the second case is guarded by this if
                // statement

                // Send bytes in between reader offset and current offset
                let written = writer.write(&self.buffer[prev_index..curr_index])?;
                Ok((written, offset + written))
            } else {
                // handle the case where the buffer looks like [...<current>...<reader>...]
                // In this case, we need to handle we set of writes, first the write of bytes
                // between [<reader>..], and the second write of [..<current>]
                let len_to_bound = N - prev_index;
                let written0 = writer.write(&self.buffer[prev_index..])?;

                if written0 != len_to_bound {
                    // if we are not able to drain the bytes from <reader> to end, we are good
                    Ok((written0, offset + written0))
                } else {
                    // now we have successfully drained the bytes between [<reader>..], we now need
                    // to send the bytes [..<current>]
                    let written1 = writer.write(&self.buffer[..curr_index])?;
                    Ok((written0 + written1, offset + written0 + written1))
                }
            }
        }
    }

    pub fn read(&self, offset: usize, out: &mut [u8]) -> Result<usize, std::io::Error> {
        if offset > self.input_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "client offset greater than total bytes received",
            ));
        }

        let start_index = self.input_count % N;
        let len_to_bound = N - start_index;

        // normalize the output buffer
        let buf = if out.len() > N { &mut out[..N] } else { out };

        if buf.len() > len_to_bound {
            let remaining = buf.len() - len_to_bound;
            buf[..len_to_bound].copy_from_slice(&self.buffer[start_index..]);
            buf[len_to_bound..len_to_bound + remaining].copy_from_slice(&self.buffer[..remaining]);
        } else {
            buf.copy_from_slice(&self.buffer[start_index..buf.len()]);
        }

        Ok(buf.len())
    }
}
