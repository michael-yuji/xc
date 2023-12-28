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

use crate::format::docker_compat::Expose;
use crate::models::DatasetSpec;
use crate::util::default_on_missing;

use super::exec::Exec;
use super::MountSpec;
use super::{EnvSpec, SystemVPropValue};

use anyhow::anyhow;
use oci_util::digest::{sha256_once, DigestAlgorithm, OciDigest};
use oci_util::layer::ChainId;
use oci_util::models::{FreeOciConfig, OciConfig, OciConfigRootFs, OciInnerConfig};
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use varutil::string_interpolation::{InterpolatedString, Var};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SpecialMount {
    pub mount_point: String,
    pub mount_type: String,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    major_version: u32,
    minor_version: u32,
    patch_version: u32,
    tag: u32,
}

#[allow(unused)]
impl Version {
    fn prehistorial() -> Version {
        Version {
            major_version: 0,
            minor_version: 0,
            patch_version: 0,
            tag: 0,
        }
    }
    fn is_prehistorial(&self) -> bool {
        self == &Self::prehistorial()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}+{}",
            self.major_version, self.minor_version, self.patch_version, self.tag
        )
    }
}

impl std::str::FromStr for Version {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Version, anyhow::Error> {
        let (version, tag) = s.split_once('+').ok_or_else(|| anyhow!("invaild format"))?;
        let (major, other) = version
            .split_once('.')
            .ok_or_else(|| anyhow!("invaild format"))?;
        let (minor, patch) = other
            .split_once('.')
            .ok_or_else(|| anyhow!("invaild format"))?;
        Ok(Version {
            major_version: major.parse()?,
            minor_version: minor.parse()?,
            patch_version: patch.parse()?,
            tag: tag.parse()?,
        })
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct VersionVisitor;

impl<'de> Visitor<'de> for VersionVisitor {
    type Value = Version;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("x.x.x+p")
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        value.parse().map_err(|e| E::custom(format!("{e:?}")))
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(VersionVisitor)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OciJailConfig {
    #[serde(flatten)]
    oci_config: OciInnerConfig,
    xc_extension: Option<JailConfig>,
}

impl Default for OciJailConfig {
    fn default() -> OciJailConfig {
        OciJailConfig {
            oci_config: OciInnerConfig::default(),
            xc_extension: Some(JailConfig::default()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct JailConfig {
    #[serde(default)]
    pub version: Version,
    /// The secure level this jail is required
    pub secure_level: i8,

    /// is vnet required
    #[serde(default)]
    pub vnet: bool,

    /// Required ports
    pub ports: HashMap<Expose, String>,

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

    pub entry_points: HashMap<String, Exec>,

    pub default_entry_point: Option<String>,

    pub special_mounts: Vec<SpecialMount>,

    pub mounts: HashMap<String, MountSpec>,

    #[serde(default)]
    pub datasets: HashMap<String, DatasetSpec>,

    #[serde(default)]
    pub init: Vec<Exec>,

    #[serde(default)]
    pub deinit: Vec<Exec>,

    #[serde(default)]
    pub linux: bool,

    /// The Linux OS name associated with the conatiner
    pub linux_osname: Option<String>,

    /// The Linux OS relasea associated with the container
    pub linux_osrelease: Option<String>,

    /// The Linux OSS version associated with the container
    pub linux_oss_version: Option<String>,

    #[serde(default, deserialize_with = "default_on_missing")]
    pub labels: HashMap<String, String>,

    pub osrelease: Option<String>,

    pub osreldate: Option<i32>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct JailImage {
    #[serde(flatten)]
    oci_config: FreeOciConfig<OciJailConfig>,
}

impl Default for JailImage {
    fn default() -> JailImage {
        let oci_config = FreeOciConfig {
            architecture: crate::util::get_current_arch().to_string(),
            os: "FreeBSD".to_string(),
            config: Some(OciJailConfig::default()),
            rootfs: OciConfigRootFs {
                typ: "layers".to_string(),
                diff_ids: Vec::new(),
            },
            history: Vec::new(),
        };
        JailImage { oci_config }
    }
}

impl JailImage {
    pub fn os(&self) -> &str {
        &self.oci_config.os
    }
    pub fn architecture(&self) -> &str {
        &self.oci_config.architecture
    }
    pub fn chain_id(&self) -> Option<ChainId> {
        if self.oci_config.rootfs.diff_ids.is_empty() {
            None
        } else {
            Some(ChainId::calculate_chain_id(
                DigestAlgorithm::Sha256,
                self.oci_config.rootfs.diff_ids.iter(),
            ))
        }
    }

    pub fn layers(&self) -> Vec<OciDigest> {
        self.oci_config.rootfs.diff_ids.clone()
    }

    pub fn push_layer(&mut self, diff_id: &OciDigest) {
        self.oci_config.rootfs.diff_ids.push(diff_id.clone())
    }

    pub fn jail_config(&self) -> JailConfig {
        self.oci_config
            .config
            .clone()
            .and_then(|c| c.xc_extension)
            .unwrap_or_default()
    }
    pub fn digest(&self) -> OciDigest {
        let json = serde_json::to_string(&self).unwrap();
        sha256_once(json)
    }
    pub fn set_config(&mut self, config: &JailConfig) {
        self.oci_config.config = Some(config.to_oci_jail_config(None));
    }
}

impl JailConfig {
    pub fn to_image(
        &self,
        diff_ids: Vec<OciDigest>,
        origin_oci: Option<OciInnerConfig>,
    ) -> JailImage {
        JailImage {
            oci_config: FreeOciConfig {
                architecture: crate::util::get_current_arch().to_string(),
                os: "FreeBSD".to_string(),
                config: Some(OciJailConfig {
                    oci_config: origin_oci.unwrap_or_else(|| self.to_oci()),
                    xc_extension: Some(self.clone()),
                }),
                rootfs: OciConfigRootFs {
                    typ: "layers".to_string(),
                    diff_ids,
                },
                history: Vec::new(),
            },
        }
    }

    pub fn to_oci_jail_config(&self, oci: Option<OciInnerConfig>) -> OciJailConfig {
        OciJailConfig {
            xc_extension: Some(self.clone()),
            oci_config: oci.unwrap_or_else(|| self.to_oci()),
        }
    }

    pub fn to_oci(&self) -> OciInnerConfig {
        /*
        let mut config = OciInnerConfig::default();
        // TODO: implement the translations
        config
        */
        OciInnerConfig::default()
    }

    pub fn from_json(value: serde_json::Value) -> Option<JailImage> {
        let maybe_converted = serde_json::from_value::<JailImage>(value.clone()).ok()?;
        //            .expect(&format!("unknown format: {value}"));
        if maybe_converted
            .clone()
            .oci_config
            .config
            .and_then(|c| c.xc_extension)
            .is_none()
        {
            let ociconfig = serde_json::from_value::<OciConfig>(value).ok()?;
            Self::from_oci(ociconfig)
        } else {
            Some(maybe_converted)
        }
    }

    pub fn from_oci(config: OciConfig) -> Option<JailImage> {
        let mounts = config
            .config
            .clone()
            .and_then(|config| config.volumes)
            .map(|v| {
                v.0.into_iter()
                    .map(|destination| {
                        let spec = MountSpec::new(&destination);
                        (destination, spec)
                    })
                    .collect()
            })
            .unwrap_or_else(HashMap::new);

        let ports = config
            .config
            .as_ref()
            .and_then(|config| config.exposed_ports.as_ref())
            .map(|set| {
                let mut ports = HashMap::new();
                for port in set.0.iter() {
                    if let Ok(expose) = port.parse::<Expose>() {
                        ports.insert(expose, "".to_string());
                    }
                }
                ports
            })
            .unwrap_or_default();

        let labels = config
            .config
            .as_ref()
            .and_then(|config| config.labels.clone())
            .unwrap_or_default();

        let mut meta = JailConfig {
            secure_level: 0,
            vnet: false,
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
            linux: config.os != "FreeBSD",
            //            original_oci_config: Some(config.clone()),
            ports,
            labels,
            ..JailConfig::default()
        };

        // convert entrypoint, per OCI specification, entrypoint is a list of strings representing
        // a command to run in a container, with Cmd field containing the default arguments to
        // entrypoint. In the case of entrypoint is NULL, Cmd is the default command with
        // arguments.
        //
        // It is not clear in the spec if Entrypoint[1:] should be replaced when user use the
        // entrypoint with commands, nor if Entrypoin[1:] should be discard when (Entrypoint.len()
        // > 1 && Cmd.len() > 1)
        //
        if let Some(config) = &config.config {
            let entrypoint = config.entrypoint.clone().unwrap_or_default();
            let cmd = config.cmd.clone().unwrap_or_default();
            let work_dir = config.working_dir.clone(); //.map(|path| std::path::PathBuf::from(&path));

            if !entrypoint.is_empty() || !cmd.is_empty() {
                let (exec, args, default_args) = if entrypoint.is_empty() {
                    // we already asserted that if entrypoint is empty, cmd must not be empty
                    let (arg0, args) = cmd.split_first().unwrap();
                    let args = args
                        .iter()
                        .map(|arg| InterpolatedString::new(arg).unwrap())
                        .collect::<Vec<_>>();
                    (arg0.to_string(), args, Vec::new())
                } else {
                    let (arg0, args) = entrypoint.split_first().unwrap();
                    let args = args
                        .iter()
                        .map(|arg| InterpolatedString::new(arg).unwrap())
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
                let entry_point = Exec {
                    exec,
                    args,
                    default_args,
                    environ,
                    required_envs: Vec::new(),
                    work_dir,
                    clear_env: false,
                    user: None,
                    group: None,
                };
                meta.entry_points
                    .insert("oci_entry_point".to_string(), entry_point);
                meta.default_entry_point = Some("oci_entry_point".to_string());
            }
        }

        let layers = config.rootfs.diff_ids;

        let mut image = meta.to_image(layers, config.config);

        image.oci_config.os = config.os;

        Some(image)
    }
}
