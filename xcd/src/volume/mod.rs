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
pub mod drivers;

use crate::auth::Credential;
use crate::config::config_manager::InventoryManager;
use crate::volume::drivers::VolumeDriver;
use crate::volume::drivers::zfs::ZfsDriver;
use crate::volume::drivers::local::LocalDriver;

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use xc::container::error::PreconditionFailure;
use xc::container::request::{MountReq, Mount};
use xc::models::MountSpec;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Default, PartialEq, Eq, Debug, Clone)]
pub enum VolumeDriverKind {
    #[default]
    Directory,
    ZfsDataset,
}

impl std::fmt::Display for VolumeDriverKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Directory => "directory",
            Self::ZfsDataset => "zfs",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for VolumeDriverKind {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<VolumeDriverKind, Self::Err> {
        match s {
            "zfs" => Ok(Self::ZfsDataset),
            "directory" => Ok(Self::Directory),
            _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "invalid value"))
        }
    }
}

impl Serialize for VolumeDriverKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

impl<'de> Deserialize<'de> for VolumeDriverKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        match string.as_str() {
            "directory" => Ok(Self::Directory),
            "zfs" => Ok(Self::ZfsDataset),
            _ => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&string),
                &"expected 'zfs' or 'directory'",
            )),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    // if this field is null, we do not have restrictions on who can mount this volume with write
    // access, and the volume will by default mount as rw. Otherwise, only the specific users
    // (and root) can mount the volume as read-write, and the volume will by default mount as ro
    #[serde(default)]
    pub rw_users: Option<Vec<String>>,

    // The origin of the volume, currently local path only
    pub device: PathBuf,

    // There are no restrictions on who can mount this volume if authorized_users is null,
    // otherwise, only the specific users (and root) can mount the volume
    #[serde(default)]
    pub authorized_users: Option<Vec<String>>,

    #[serde(default)]
    pub driver: VolumeDriverKind,

    #[serde(default)]
    pub mount_options: Vec<String>,

    #[serde(default)]
    pub driver_options: HashMap<String, String>,
}

impl Volume {

    /// Creates an instance of volume for ad-hoc mounting purpose, e.g. -v /source:/other/location
    pub(crate) fn adhoc(device: impl AsRef<Path>) -> Self {
        Self {
            rw_users: None,
            device: device.as_ref().to_path_buf(),
            authorized_users: None,
            driver: VolumeDriverKind::Directory,
            mount_options: Vec::new(),
            driver_options: HashMap::new(),
        }
    }

    pub(crate) fn can_mount(&self, uid: u32) -> bool {
        match &self.authorized_users {
            None => true,
            Some(users) => {
                let uid_string = uid.to_string();
                users.contains(&uid_string)
                    || users.contains(
                        &freebsd::get_username(uid)
                            .ok()
                            .flatten()
                            .unwrap_or_default(),
                    )
            }
        }
    }

    pub(crate) fn can_mount_rw(&self, uid: u32) -> bool {
        self.can_mount(uid)
            && match &self.rw_users {
                None => true,
                Some(users) => {
                    let uid_string = uid.to_string();
                    users.contains(&uid_string)
                        || users.contains(
                            &freebsd::get_username(uid)
                                .ok()
                                .flatten()
                                .unwrap_or_default(),
                        )
                }
            }
    }
}

pub(crate) struct VolumeManager {
    inventory: Arc<Mutex<InventoryManager>>,
    default_volume_dataset: Option<PathBuf>,
    default_volume_dir: Option<PathBuf>,
}

impl VolumeManager {

    pub(crate) fn new(
        inventory: Arc<Mutex<InventoryManager>>,
        default_volume_dataset: Option<PathBuf>,
        default_volume_dir: Option<PathBuf>,
    ) -> VolumeManager
    {
        VolumeManager { inventory, default_volume_dataset, default_volume_dir }
    }

    // insert or override a volume
    pub(crate) fn add_volume(&mut self, name: &str, volume: &Volume) {
        self.inventory.lock().unwrap().modify(|inventory| {
            inventory
                .volumes
                .insert(name.to_string(), volume.clone());
        });
    }

    pub(crate) fn list_volumes(&self) -> HashMap<String, Volume> {
        self.inventory.lock().unwrap().borrow().volumes.clone()
    }

    pub(crate) fn query_volume(&self, name: &str) -> Option<Volume> {
        self.inventory.lock().unwrap().borrow().volumes.get(name).cloned()
    }

    pub(crate) fn create_volume(
        &mut self,
        kind: VolumeDriverKind,
        name: &str,
        template: Option<MountSpec>,
        source: Option<PathBuf>,
        props: HashMap<String, String>) -> Result<(), PreconditionFailure>
    {
        let volume = match kind {
            VolumeDriverKind::Directory => {
                let local_driver = LocalDriver {
                    default_subdir: self.default_volume_dir.clone()
                };
                local_driver.create(name, template, source, props)?
            },
            VolumeDriverKind::ZfsDataset => {
                let zfs_driver = ZfsDriver {
                    handle: freebsd::fs::zfs::ZfsHandle::default(),
                    default_dataset: self.default_volume_dataset.clone()
                };
                zfs_driver.create(name, template, source, props)?
            }
        };

        self.add_volume(name, &volume);

        Ok(())
    }

    pub(crate) fn mount(
        &self,
        cred: &Credential,
        mount_req: &MountReq,
        mount_spec: Option<&MountSpec>,
        volume: &Volume
    ) -> Result<Mount, PreconditionFailure>
    {
        match volume.driver {
            VolumeDriverKind::Directory => {
                let local_driver = LocalDriver {
                    default_subdir: self.default_volume_dir.clone()
                };
                local_driver.mount(cred, mount_req, mount_spec, volume)
            },
            VolumeDriverKind::ZfsDataset => {
                let zfs_driver = ZfsDriver {
                    handle: freebsd::fs::zfs::ZfsHandle::default(),
                    default_dataset: self.default_volume_dataset.clone()
                };
                zfs_driver.mount(cred, mount_req, mount_spec, volume)
            }
        }
    }
}
