//! OCI Image layer utilities

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

use crate::digest::*;
use serde::{Deserialize, Serialize};
use std::convert::From;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Debug, Hash)]
pub struct ChainId(OciDigest);

impl std::fmt::Display for ChainId {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.0.fmt(fmt)
    }
}

impl std::str::FromStr for ChainId {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, std::io::Error> {
        Ok(ChainId(s.parse()?))
    }
}

impl From<OciDigest> for ChainId {
    fn from(digest: OciDigest) -> ChainId {
        ChainId(digest)
    }
}

impl AsRef<str> for ChainId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl ChainId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn new(input: &OciDigest) -> ChainId {
        ChainId(input.clone())
    }

    pub fn calculate_chain_id<'a>(
        alg: DigestAlgorithm,
        mut diff_ids: impl Iterator<Item = &'a OciDigest>,
    ) -> ChainId {
        let mut chain_id = ChainId(diff_ids.next().unwrap().clone());
        for digest in diff_ids {
            chain_id.consume_diff_id(alg, digest);
        }
        chain_id
    }

    /// Calculate the chain id base on the sequence of diff ids using the same digest algorithm
    /// each iteration
    pub fn validate_assume_digest_algorithm(
        algo: DigestAlgorithm,
        diff_ids: &[OciDigest],
    ) -> ChainId {
        let mut chain_id = ChainId(diff_ids[0].clone());
        for diff_id in &diff_ids[1..] {
            chain_id.consume_diff_id(algo, diff_id);
        }
        chain_id
    }

    /// Generate the chain id base on the next diff id
    pub fn next_chain_id(&self, alg: DigestAlgorithm, diff_id: &OciDigest) -> ChainId {
        let input = format!("{self} {diff_id}");
        let mut digest = Hasher::new(alg);
        digest.update(&input);
        ChainId(digest.finalize())
    }

    /// Take the diff_id and make this chain id reflect the new chain_id
    pub fn consume_diff_id(&mut self, alg: DigestAlgorithm, diff_id: &OciDigest) {
        let input = format!("{self} {diff_id}");
        let mut digest = Hasher::new(alg);
        digest.update(&input);
        self.0 = digest.finalize()
    }
}
