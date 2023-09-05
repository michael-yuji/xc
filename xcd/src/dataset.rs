//! Utility to keep track of jailed dataset so we can alert the user before actually calling ZFS
//! unjail and rip the dataset from containers still using them

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

use crate::util::TwoWayMap;
use std::path::{Path, PathBuf};

/// A tracker keep tracks of mapping between containers and jailed dataset. This construct does not
/// perform any actual effects (jailing and un-jailing datasets).
#[derive(Default)]
pub(crate) struct JailedDatasetTracker {
    jailed: TwoWayMap<String, PathBuf>,
}

impl JailedDatasetTracker {
    pub(crate) fn set_jailed(&mut self, token: &str, dataset: impl AsRef<Path>) {
        if self.is_jailed(&dataset) {
            panic!("dataset is jailed by other container")
        } else {
            self.jailed
                .insert(token.to_string(), dataset.as_ref().to_path_buf());
        }
    }

    pub(crate) fn is_jailed(&self, dataset: impl AsRef<Path>) -> bool {
        self.jailed.contains_value(dataset.as_ref())
    }

    #[allow(dead_code)]
    pub(crate) fn unjail(&mut self, dataset: impl AsRef<Path>) {
        self.jailed.remove_all_referenced(dataset.as_ref());
    }

    pub(crate) fn release_container(&mut self, token: &str) {
        self.jailed.remove(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jailed_dataset_allocation_detection() {
        let mut tracker = JailedDatasetTracker::default();
        let dataset = "zroot/test/dataset";
        let token = "abc";
        assert!(!tracker.is_jailed(dataset));
        tracker.set_jailed(token, dataset);
        assert!(tracker.is_jailed(dataset));
        tracker.release_container(token);
        assert!(!tracker.is_jailed(dataset));
    }

    #[test]
    fn test_jailed_dataset_unjail() {
        let mut tracker = JailedDatasetTracker::default();
        let dataset = "zroot/test/dataset";
        let token = "abc";
        assert!(!tracker.is_jailed(dataset));
        tracker.set_jailed(token, dataset);
        assert!(tracker.is_jailed(dataset));
        tracker.unjail(dataset);
        assert!(!tracker.is_jailed(dataset));
    }
}
