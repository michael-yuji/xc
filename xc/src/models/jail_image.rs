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
use super::MountSpec;
use super::{EntryPoint, EnvSpec, SystemVPropValue};
use oci_util::digest::{sha256_once, DigestAlgorithm, OciDigest};
use oci_util::layer::ChainId;
use oci_util::models::{FreeOciConfig, OciConfig, OciConfigRootFs};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use varutil::string_interpolation::{InterpolatedString, Var};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SpecialMount {
    pub mount_point: String,
    pub mount_type: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct JailConfig {
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

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct JailImage(pub(crate) FreeOciConfig<JailConfig>);

impl Default for JailImage {
    fn default() -> JailImage {
        JailImage(FreeOciConfig {
            architecture: crate::util::get_current_arch().to_string(),
            os: "FreeBSD".to_string(),
            config: Some(JailConfig::default()),
            rootfs: OciConfigRootFs {
                typ: "layers".to_string(),
                diff_ids: Vec::new(),
            },
        })
    }
}

impl JailImage {
    pub fn chain_id(&self) -> Option<ChainId> {
        if self.0.rootfs.diff_ids.is_empty() {
            None
        } else {
            Some(ChainId::calculate_chain_id(
                DigestAlgorithm::Sha256,
                self.0.rootfs.diff_ids.iter(),
            ))
        }
    }
    pub fn layers(&self) -> Vec<OciDigest> {
        self.0.rootfs.diff_ids.clone()
    }
    pub fn push_layer(&mut self, diff_id: &OciDigest) {
        self.0.rootfs.diff_ids.push(diff_id.clone())
    }
    pub fn jail_config(&self) -> JailConfig {
        self.0.config.clone().unwrap()
    }
    pub fn digest(&self) -> OciDigest {
        let json = serde_json::to_string(&self).unwrap();
        sha256_once(json)
    }
    pub fn set_config(&mut self, config: &JailConfig) {
        self.0.config = Some(config.clone())
    }
}

impl JailConfig {
    pub fn to_image(&self, diff_ids: Vec<OciDigest>) -> JailImage {
        JailImage(FreeOciConfig {
            architecture: crate::util::get_current_arch().to_string(),
            os: "FreeBSD".to_string(),
            config: Some(self.clone()),
            rootfs: OciConfigRootFs {
                typ: "layers".to_string(),
                diff_ids,
            },
        })
    }
    pub fn from_json(value: serde_json::Value) -> Option<JailImage> {
        serde_json::from_value::<JailImage>(value.clone())
            .ok()
            .or_else(|| {
                serde_json::from_value::<OciConfig>(value)
                    .ok()
                    .and_then(Self::from_oci)
            })
    }

    pub fn from_oci(config: OciConfig) -> Option<JailImage> {
        let mounts = config
            .config
            .clone()
            .and_then(|config| config.volumes)
            .map(|v| {
                v.0.iter()
                    .map(|destination| (destination.to_string(), MountSpec::new(destination)))
                    .collect()
            })
            .unwrap_or_else(HashMap::new);

        let mut meta = JailConfig {
            secure_level: 0,
            vnet: false,
            nics: None,
            ports: HashMap::new(),
            devfs_rules: Vec::new(),
            allow: Vec::new(),
            sysv_msg: SystemVPropValue::New,
            sysv_sem: SystemVPropValue::New,
            sysv_shm: SystemVPropValue::New,
            envs: HashMap::new(),
            entry_points: HashMap::new(),
            init: Vec::new(),
            deinit: Vec::new(),
            mounts,
            linux: true,
            original_oci_config: Some(config.clone()),
            ..JailConfig::default()
        };

        if let Some(config) = &config.config {
            let entrypoint = config.entrypoint.clone().unwrap_or_default();
            let cmd = config.cmd.clone().unwrap_or_default();

            if !entrypoint.is_empty() || !cmd.is_empty() {
                let (exec, args, default_args) = if entrypoint.is_empty() {
                    // we already asserted that if entrypoint is empty, cmd must not be empty
                    let (arg0, args) = cmd.split_first().unwrap();
                    let args = args
                        .iter()
                        .map(|arg| CmdArg::Var(InterpolatedString::new(arg).unwrap()))
                        .collect::<Vec<_>>();
                    (arg0.to_string(), args, Vec::new())
                } else {
                    let (arg0, args) = entrypoint.split_first().unwrap();
                    let args = args
                        .iter()
                        .map(|arg| CmdArg::Var(InterpolatedString::new(arg).unwrap()))
                        .collect::<Vec<_>>();
                    let defs = cmd
                        .iter()
                        .map(|arg| InterpolatedString::new(arg).unwrap())
                        .collect::<Vec<_>>();
                    (arg0.to_string(), args, defs)
                };
                let mut environ = HashMap::new();

                if let Some(env) = &config.env {
                    for (key, value) in env.iter().filter_map(|i| i.split_once('=')) {
                        environ.insert(
                            Var::new(key).unwrap(),
                            InterpolatedString::new(value).unwrap(),
                        );
                    }
                }
                let entry_point = EntryPoint {
                    exec,
                    args,
                    default_args,
                    environ,
                    required_envs: Vec::new(),
                };
                meta.entry_points.insert("main".to_string(), entry_point);
            }
        }

        let layers = config.rootfs.diff_ids;

        Some(meta.to_image(layers))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Satis {
    MissingCap(String),
    MissingEnv(String, Option<String>),
    PreconditionFailure(String),
    Ok,
}
