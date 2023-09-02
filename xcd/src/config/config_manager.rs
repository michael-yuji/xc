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
use anyhow::Context;
use std::path::{Path, PathBuf};
use tracing::error;

use crate::config::inventory::Inventory;

pub(crate) struct InventoryManager {
    path: PathBuf,
    cached: Inventory,
}

impl InventoryManager {
    pub(crate) fn load_from_path(
        path: impl AsRef<Path>,
    ) -> Result<InventoryManager, anyhow::Error> {
        let path = path.as_ref().to_path_buf();
        let data = std::fs::read_to_string(&path).context("Cannot read config file")?;
        let cached: Inventory = match serde_json::from_str(&data).context("Cannot parse config") {
            Ok(inventory) => inventory,
            Err(_) => {
                let default = Inventory::default();
                std::fs::write(&path, serde_json::to_vec_pretty(&default).unwrap())
                    .context("cannot write default inventory")?;
                default
            }
        };
        Ok(InventoryManager { path, cached })
    }

    pub(crate) fn borrow(&self) -> &Inventory {
        &self.cached
    }

    /// Modify the underlying config, and synchronize the changes to underlying config file
    pub(crate) fn modify<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Inventory),
    {
        let mut config = self.cached.clone();
        let old_config = config.clone();

        f(&mut config);

        if old_config != config {
            if let Ok(serialized) = serde_json::to_vec_pretty(&config) {
                if std::fs::write(&self.path, serialized).is_err() {
                    error!(
                        "failed to write new config to config file at {:?}",
                        self.path
                    );
                }
            } else {
                error!("failed to serialize new config to bytes");
            }
            self.cached = config
        }
    }
}
