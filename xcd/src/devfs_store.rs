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
use sha2::Digest;
use std::collections::HashMap;

pub struct DevfsRulesetStore {
    pub next_ruleset_id: u16,
    pub rules: HashMap<[u8; 32], u16>,
    pub force_id: Option<u16>,
}

impl DevfsRulesetStore {
    pub fn new(next_ruleset_id: u16, force_id: Option<u16>) -> DevfsRulesetStore {
        DevfsRulesetStore {
            next_ruleset_id,
            rules: HashMap::new(),
            force_id,
        }
    }
    pub fn get_ruleset_id(&mut self, rules: &[String]) -> u16 {
        if let Some(force_id) = self.force_id {
            force_id
        } else {
            let mut hasher = sha2::Sha256::new();
            for rule in rules {
                hasher.update(rule.as_bytes());
            }
            let digest: [u8; 32] = hasher.finalize().into();
            match self.rules.get(&digest) {
                Some(rule_id) => *rule_id,
                None => {
                    _ = freebsd::fs::devfs::devfs_del_ruleset(self.next_ruleset_id);
                    let id = freebsd::fs::devfs::devfs_add_ruleset(
                        self.next_ruleset_id,
                        rules.join("\n"),
                    )
                    .expect("");
                    self.next_ruleset_id += 1;
                    self.rules.insert(digest, id);
                    id
                }
            }
        }
    }
}
