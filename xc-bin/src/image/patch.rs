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

use crate::format::EnvPair;

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use varutil::string_interpolation::Var;
use xc::models::jail_image::{JailConfig, SpecialMount};
use xc::models::{EnvSpec, MountSpec, SystemVPropValue};

#[derive(Parser, Debug)]
pub(crate) enum PatchActions {
    AddEnv {
        #[arg(long, action)]
        required: bool,
        #[arg(short = 'd', long = "description")]
        description: Option<String>,
        env: String,
    },
    /// add a new volume spec to the image
    AddVolume {
        /// some good information about what the volume is for
        #[arg(short = 'd', long = "description")]
        description: Option<String>,
        /// hints to create this volume for intelligently, for example the right block size
        #[arg(long = "hint", /*multiple_occurrences = true*/)]
        hints: Vec<EnvPair>,
        /// if the volume should be mounted as read-only
        #[arg(long = "read-only", action)]
        read_only: bool,
        /// a name for the
        #[arg(long = "name")]
        name: Option<String>,
        /// mount point in the container
        mount_point: PathBuf,

        #[arg(long = "required", action)]
        required: bool,
    },
    MountFdescfs,
    MountProcfs,
    ModAllow {
        allows: Vec<String>,
    },
    SysvIpc {
        #[arg(long = "enable", /*multiple_occurrences = true*/)]
        enable: Vec<String>,
    },
}

impl PatchActions {
    pub(super) fn do_patch(&self, config: &mut JailConfig) {
        match self {
            PatchActions::AddEnv {
                required,
                description,
                env,
            } => {
                let env_var = Var::new(env).expect("invalid environment variable name");
                config.envs.insert(
                    env_var,
                    EnvSpec {
                        description: description.clone(),
                        required: *required,
                    },
                );
            }
            PatchActions::AddVolume {
                description,
                hints,
                name,
                mount_point,
                required,
                read_only,
            } => {
                let destination = mount_point.to_string_lossy().to_string();
                let description = description.clone().unwrap_or_default();
                let key = name.clone().unwrap_or_else(|| destination.to_string());
                let mut volume_hints = HashMap::new();
                for hint in hints.into_iter() {
                    volume_hints.insert(
                        hint.key.clone(),
                        serde_json::Value::String(hint.value.clone()),
                    );
                }
                for (_, mount) in config.mounts.iter() {
                    if mount.destination == destination {
                        panic!("mounts with such mountpoint already exists");
                    }
                }
                let mountspec = MountSpec {
                    description,
                    read_only: *read_only,
                    volume_hints,
                    destination,
                    required: *required,
                };
                config.mounts.insert(key, mountspec);
            }
            PatchActions::ModAllow { allows } => {
                for allow in allows.into_iter() {
                    if let Some(param) = allow.strip_prefix('-') {
                        for i in (0..config.allow.len()).rev() {
                            if config.allow[i] == param {
                                config.allow.remove(i);
                            }
                        }
                    } else if !config.allow.contains(&allow) {
                        config.allow.push(allow.to_string());
                    }
                }
            }
            PatchActions::SysvIpc { enable } => {
                let mut enabled = Vec::new();
                for e in enable.iter() {
                    enabled.extend(e.split(',').map(|h| h.trim()).collect::<Vec<_>>());
                }
                for e in enabled.into_iter() {
                    match e {
                        "shm" => config.sysv_shm = SystemVPropValue::New,
                        "-shm" => config.sysv_shm = SystemVPropValue::Disable,
                        "msg" => config.sysv_msg = SystemVPropValue::New,
                        "-msg" => config.sysv_msg = SystemVPropValue::Disable,
                        "sem" => config.sysv_sem = SystemVPropValue::New,
                        "-sem" => config.sysv_sem = SystemVPropValue::Disable,
                        _ => continue,
                    }
                }
            }
            PatchActions::MountFdescfs => {
                for i in (0..config.special_mounts.len()).rev() {
                    let mount = &config.special_mounts[i];
                    if mount.mount_type.as_str() == "fdescfs"
                        && mount.mount_point.as_str() == "/dev/fd"
                    {
                        break;
                    }
                }
                config.special_mounts.push(SpecialMount {
                    mount_type: "fdescfs".to_string(),
                    mount_point: "/dev/fd".to_string(),
                });
            }
            PatchActions::MountProcfs => {
                for i in (0..config.special_mounts.len()).rev() {
                    let mount = &config.special_mounts[i];
                    if mount.mount_type.as_str() == "procfs"
                        && mount.mount_point.as_str() == "/proc"
                    {
                        break;
                    }
                }
                config.special_mounts.push(SpecialMount {
                    mount_type: "procfs".to_string(),
                    mount_point: "/proc".to_string(),
                });
            }
        }
    }
}
