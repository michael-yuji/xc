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
pub mod config_manager;
pub mod inventory;

use anyhow::{bail, Context};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

fn default_socket_path() -> PathBuf {
    PathBuf::from("/var/run/xc.sock")
}

fn default_database_store() -> PathBuf {
    PathBuf::from("/var/db/xc.sqlite")
}

fn default_registries() -> PathBuf {
    std::path::PathBuf::from("/var/db/xc.registries.json")
}

fn default_inventory() -> PathBuf {
    std::path::PathBuf::from("/usr/local/etc/inventory.json")
}

fn default_layers_dir() -> PathBuf {
    std::path::PathBuf::from("/var/cache")
}

fn default_logs_dir() -> PathBuf {
    std::path::PathBuf::from("/var/log/xc")
}

fn default_devfs_offset() -> u16 {
    1000
}

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct XcConfigArg {
    /// Network interfaces should "xc" consider external
    #[arg(long = "ext-if")]
    pub ext_ifs: Option<Vec<String>>,

    /// Dataset for images
    #[arg(long = "image-dataset")]
    pub image_dataset: Option<String>,

    /// Dataset holding the rootfs of the running containers
    #[arg(long = "container-dataset")]
    pub container_dataset: Option<String>,

    /// Sqlite database storing the rootfs of the images
    #[arg(long = "image-dataset-store")]
    pub image_database_store: Option<PathBuf>,

    /// Directory storing the file system layers
    #[arg(long = "layers-dir")]
    pub layers_dir: Option<PathBuf>,

    /// Directory to store the log directories
    #[arg(long = "logs-dir")]
    pub logs_dir: Option<PathBuf>,

    #[arg(long = "devfs-id-offset")]
    pub devfs_id_offset: Option<u16>,

    /// The sqlite database file for generic data
    #[arg(long = "database-store")]
    pub database_store: Option<PathBuf>,

    /// socket path to listen
    #[arg(long = "socket-path")]
    pub socket_path: Option<PathBuf>,

    /// path to the file containing the registry credentials
    #[arg(long = "registries")]
    pub registries: Option<PathBuf>,

    /// force jails to use a static devfs rule number
    #[arg(long = "force-devfs-ruleset")]
    pub force_devfs_ruleset: Option<u16>,

    /// file to the inventory configuration / index
    #[arg(long = "inventory")]
    pub inventory: Option<PathBuf>,

    /// warn instead of bail during configuration sanity check
    #[arg(long = "warn-only", action)]
    pub warn_only: Option<bool>,

    #[arg(default_value = "/usr/local/etc/xc.conf")]
    pub config_dir: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct XcConfig {
    /// Network interfaces should "xc" consider external
    pub ext_ifs: Vec<String>,

    /// Dataset for images
    pub image_dataset: String,

    /// Dataset for running containers
    pub container_dataset: String,

    #[serde(default = "default_database_store")]
    pub image_database_store: PathBuf,

    #[serde(default = "default_layers_dir")]
    pub layers_dir: PathBuf,

    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,

    #[serde(default = "default_devfs_offset")]
    pub devfs_id_offset: u16,

    #[serde(default = "default_database_store")]
    pub database_store: PathBuf,

    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,

    #[serde(default = "default_registries")]
    pub registries: PathBuf,

    pub force_devfs_ruleset: Option<u16>,

    #[serde(default = "default_inventory")]
    pub inventory: PathBuf,

    #[serde(default)]
    pub warn_only: bool,
}

impl XcConfig {
    pub fn prepare(&self) -> anyhow::Result<()> {

        macro_rules! wb {
            ($($t:tt)*) => {
                if self.warn_only {
                    warn!($($t)*);
                } else {
                    bail!($($t)*);
                }
            }
        }

        macro_rules! mkdir {
            ($e:expr) => {
                if !$e.exists() {
                    std::fs::create_dir_all(&$e)
                        .with_context(|| format!("error creating {:?}", &$e))?;
                }
            };
        }

        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let zfs = freebsd::fs::zfs::ZfsHandle::default();
        for iface in self.ext_ifs.iter() {
            if !existing_ifaces.contains(iface) {
                wb!("network interface {iface} does not exist on the this system")
            }
        }

        if !zfs.exists(&self.image_dataset) {
            wb!("image dataset {} does not exist", &self.image_dataset);
        }

        if !zfs.exists(&self.container_dataset) {
            wb!(
                "container dataset {} does not exist",
                &self.container_dataset
            );
        }

        mkdir!(self.layers_dir);
        mkdir!(self.logs_dir);
        mkdir!(self.socket_path.parent().unwrap());
        mkdir!(self.registries.parent().unwrap());
        mkdir!(self.image_database_store.parent().unwrap());
        mkdir!(self.database_store.parent().unwrap());

        Ok(())
    }

    pub fn merge(&mut self, arg: XcConfigArg) {
        macro_rules! x {
            ($field:ident) => {
                if let Some($field) = arg.$field {
                    self.$field = $field;
                }
            };
            ($($fields:ident,)*) => {
                $(
                    x!($fields);
                )*
            }
        }
        x!(
            ext_ifs,
            image_dataset,
            container_dataset,
            image_database_store,
            layers_dir,
            logs_dir,
            devfs_id_offset,
            database_store,
            socket_path,
            registries,
            inventory,
            warn_only,
        );
        if let Some(force_devfs_ruleset) = arg.force_devfs_ruleset {
            self.force_devfs_ruleset = Some(force_devfs_ruleset);
        }
    }
}
