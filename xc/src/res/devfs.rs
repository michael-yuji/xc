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
use freebsd::fs::devfs::{devfs_add_ruleset, devfs_del_ruleset};

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DevfsError {
    #[error("limit exhaused")]
    LimitExhaused,
    #[error("other error")]
    Other(std::io::Error),
}

/// Data structure keeps track of allocated devfs rulset_id and the rules
pub struct DevfsRulesetStore {
    max_ruleset_id: u16,
    min_ruleset_id: u16,
    last_ruleset_id: Option<u16>,
    rules: HashMap<[u8; 32], u16>,
}

impl DevfsRulesetStore {
    /// Create a new ruleset store given a starting ruleset id and capacity
    ///
    /// # Arguments
    ///
    /// * `min_ruleset` - The starting ruleset id for all subsequent created rulesets
    /// * `capacity`    - Maximum number of rulesets this store can generate
    pub fn new(min_ruleset: u16, capacity: u16) -> DevfsRulesetStore {
        DevfsRulesetStore {
            min_ruleset_id: min_ruleset,
            last_ruleset_id: None,
            max_ruleset_id: min_ruleset + capacity,
            rules: HashMap::new(),
        }
    }

    /// Get a devfs ruleset id for the given ruleset, if the ruleset has not previously registered,
    /// this function also create the ruleset
    pub fn get_ruleset_id(&mut self, rules: &[impl AsRef<str>]) -> Result<u16, DevfsError> {
        let mut hasher = Sha256::new();
        for rule in rules {
            hasher.update(rule.as_ref());
        }
        let digest: [u8; 32] = hasher.finalize().into();

        match self.rules.get(&digest) {
            Some(rule_id) => Ok(*rule_id),
            None => {
                if let Some(last_ruleset_id) = self.last_ruleset_id {
                    if last_ruleset_id == u16::MAX || last_ruleset_id == self.max_ruleset_id {
                        return Err(DevfsError::LimitExhaused);
                    }
                }

                if self.min_ruleset_id == self.max_ruleset_id {
                    return Err(DevfsError::LimitExhaused);
                }

                let next_ruleset_id = self
                    .last_ruleset_id
                    .map(|id| id + 1)
                    .unwrap_or_else(|| self.min_ruleset_id);

                _ = devfs_del_ruleset(next_ruleset_id);

                let mut ruleset = String::new();
                for rule in rules {
                    if !ruleset.is_empty() {
                        ruleset.push('\n');
                    }
                    ruleset.push_str(rule.as_ref());
                }

                let id = devfs_add_ruleset(next_ruleset_id, ruleset).map_err(DevfsError::Other)?;
                self.last_ruleset_id = Some(id);
                self.rules.insert(digest, id);
                Ok(id)
            }
        }
    }
}
