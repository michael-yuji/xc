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
use crate::config::inventory::InventoryManager;
use crate::config::XcConfig;
use crate::database::Database;
use crate::dataset::JailedDatasetTracker;
use crate::resources::volume::VolumeShareMode;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) mod network;
pub mod volume;

pub(crate) struct Resources {
    pub(crate) db: Arc<Database>,
    pub(crate) inventory_manager: InventoryManager,
    // network: { container: [address] }
    pub(crate) net_addr_alloc_cache: HashMap<String, HashMap<String, Vec<IpAddr>>>,

    pub(crate) default_volume_dataset: Option<PathBuf>,
    pub(crate) default_volume_dir: Option<PathBuf>,

    pub(crate) constrained_shares: HashMap<String, VolumeShareMode>,

    pub(crate) dataset_tracker: JailedDatasetTracker,
}

impl Resources {
    pub(crate) fn new(database: Arc<Database>, config: &XcConfig) -> Resources {
        let inventory_manager =
            InventoryManager::load_from_path(&config.inventory).expect("cannot read inventory");
        Resources {
            db: database,
            inventory_manager,
            default_volume_dataset: config.default_volume_dataset.clone(),
            default_volume_dir: None,
            net_addr_alloc_cache: HashMap::new(),
            constrained_shares: HashMap::new(),
            dataset_tracker: JailedDatasetTracker::default(),
        }
    }
}
