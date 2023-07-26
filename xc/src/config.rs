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
use crate::res::network::Network;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_socket_path() -> String {
    "/var/run/xc.sock".to_string()
}

fn default_database_store() -> String {
    "/var/db/xc.sqlite".to_string()
}

fn default_registries() -> String {
    "/var/db/xc.registries.json".to_string()
}

fn default_layers_dir() -> String {
    "/var/cache".to_string()
}

fn default_logs_dir() -> String {
    "/var/log/xc".to_string()
}

fn default_devfs_offset() -> u16 {
    1000
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
    pub image_database_store: String,
    #[serde(default = "default_layers_dir")]
    pub layers_dir: String,
    #[serde(default = "default_logs_dir")]
    pub logs_dir: String,
    #[serde(default = "default_devfs_offset")]
    pub devfs_id_offset: u16,
    #[serde(default = "default_database_store")]
    pub database_store: String,
    #[serde(default = "default_socket_path")]
    pub socket_path: String,
    #[serde(default)]
    pub networks: HashMap<String, Network>,
    #[serde(default = "default_registries")]
    pub registries: String,
    pub force_devfs_ruleset: Option<u16>,
}
