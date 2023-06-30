//! Commonly used hash/codec used by oci specification

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

use crate::util::hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::str::FromStr;

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Sha256,
    Sha512,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub struct OciDigest(String);

impl OciDigest {
    pub fn new_unchecked(input: &str) -> OciDigest {
        OciDigest(input.to_string())
    }

    pub fn algorithm(&self) -> DigestAlgorithm {
        if self.0.starts_with("sha256:") {
            DigestAlgorithm::Sha256
        } else if self.0.starts_with("sha512:") {
            DigestAlgorithm::Sha512
        } else {
            panic!("unknown digest format")
        }
    }

    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

impl AsRef<str> for OciDigest {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OciDigest {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.0.fmt(fmt)
    }
}

impl FromStr for OciDigest {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<OciDigest, Self::Err> {
        let alg = if s.starts_with("sha256:") {
            Ok(DigestAlgorithm::Sha256)
        } else if s.starts_with("sha512:") {
            Ok(DigestAlgorithm::Sha512)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "unknown digest algorithm",
            ))
        }?;

        match alg {
            DigestAlgorithm::Sha256 => {
                const SHA256_DIGEST_LEN: usize = 7 + 64;
                if s.len() != SHA256_DIGEST_LEN {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "incorrect digest length",
                    ))
                } else {
                    Ok(OciDigest(s.to_string()))
                }
            }
            DigestAlgorithm::Sha512 => {
                const SHA512_DIGEST_LEN: usize = 7 + 128;
                if s.len() != SHA512_DIGEST_LEN {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "incorrect digest length",
                    ))
                } else {
                    Ok(OciDigest(s.to_string()))
                }
            }
        }
    }
}

pub enum Hasher {
    Sha256(Sha256),
    Sha512(Sha512),
}

pub fn sha256_once(input: impl AsRef<[u8]>) -> OciDigest {
    let mut hasher = Hasher::sha256();
    hasher.update(input);
    hasher.finalize()
}

pub fn sha512_once(input: impl AsRef<[u8]>) -> OciDigest {
    let mut hasher = Hasher::sha512();
    hasher.update(input);
    hasher.finalize()
}

impl Hasher {
    pub fn new(algorithm: DigestAlgorithm) -> Hasher {
        match algorithm {
            DigestAlgorithm::Sha256 => Self::sha256(),
            DigestAlgorithm::Sha512 => Self::sha512(),
        }
    }

    /// Create a Sha256 hasher
    pub fn sha256() -> Hasher {
        Hasher::Sha256(Sha256::new())
    }

    /// Create a Sha512 hasher
    pub fn sha512() -> Hasher {
        Hasher::Sha512(Sha512::new())
    }

    /// Given a digest of form {alg}:{hex_digest}, create a hasher base on  `alg`, and return
    /// `None` if the algorithm is not supported, currently supported algorithms are sha256 and sha512
    pub fn from_digest_str(digest: &str) -> Option<Hasher> {
        if digest.starts_with("sha256") {
            Some(Hasher::Sha256(Sha256::new()))
        } else if digest.starts_with("sha512") {
            Some(Hasher::Sha512(Sha512::new()))
        } else {
            None
        }
    }

    /// Update the hasher with `bytes` as input
    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        match self {
            Hasher::Sha256(hasher) => hasher.update(&bytes),
            Hasher::Sha512(hasher) => hasher.update(&bytes),
        }
    }

    /// Consume the hasher and generate the digest output in the form of {alg}:{hex_digest}, for
    /// example sha256:f3c1b56257ce8539ac269d7aab42550dacf8818d075f0bdf1990562aae3ef
    pub fn finalize(self) -> OciDigest {
        match self {
            Hasher::Sha256(hasher) => OciDigest(format!("sha256:{}", hex(hasher.finalize()))),
            Hasher::Sha512(hasher) => OciDigest(format!("sha512:{}", hex(hasher.finalize()))),
        }
    }
}
