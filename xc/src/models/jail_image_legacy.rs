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

use super::cmd_arg::CmdArg;
use super::exec::Exec;
use super::jail_image::SpecialMount;
use super::MountSpec;
use super::{EntryPoint, EnvSpec, SystemVPropValue};
use oci_util::digest::{sha256_once, DigestAlgorithm, OciDigest};
use oci_util::layer::ChainId;
use oci_util::models::{FreeOciConfig, OciConfig, OciConfigRootFs};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use varutil::string_interpolation::{InterpolatedString, Var};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct JailConfigPrehistorial {
    /// The secure level this jail is required
    pub secure_level: i8,

    pub original_oci_config: Option<OciConfig>,

    /// If this jail require vnet to run
    #[serde(default)]
    pub vnet: bool,

    /// The mappings between the alias and variable name, for example, a jail can expect an
    /// interface myext0 to exists, and map the variable as $MY_EXT0
    pub nics: Option<HashMap<Var, String>>,

    /// Required ports
    pub ports: HashMap<u16, String>,

    pub devfs_rules: Vec<InterpolatedString>,

    pub allow: Vec<String>,

    #[serde(default)]
    pub sysv_msg: SystemVPropValue,

    #[serde(default)]
    pub sysv_shm: SystemVPropValue,

    #[serde(default)]
    pub sysv_sem: SystemVPropValue,

    /// IEEE Std 1003.1-2001
    #[serde(default)]
    pub envs: HashMap<Var, EnvSpec>,

    pub entry_points: HashMap<String, EntryPoint>,

    pub special_mounts: Vec<SpecialMount>,

    pub mounts: HashMap<String, MountSpec>,

    #[serde(default)]
    pub init: Vec<Exec>,

    #[serde(default)]
    pub deinit: Vec<Exec>,

    #[serde(default)]
    pub linux: bool,
}
