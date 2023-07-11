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
pub mod copy;
pub mod from;
pub mod run;

use super::JailContext;
use crate::jailfile::parse::Action;

use anyhow::Context;
use xc::models::jail_image::{JailConfig, SpecialMount};
use xc::models::SystemVPropValue;

pub(crate) trait Directive: Sized {
    fn from_action(action: &Action) -> Result<Self, anyhow::Error>;
    fn run_in_context(&self, context: &mut JailContext) -> Result<(), anyhow::Error>;
}

#[derive(Clone)]
pub(crate) enum ConfigMod {
    Allow(Vec<String>),
    ReplaceAllow(Vec<String>),
    WorkDir,
    Init,
    NoInit,
    Deinit,
    NoDeinit,
    Cmd,
    Expose,
    Volume,
    Mount(String, String),
    SysV(Vec<String>),
}

impl ConfigMod {
    pub(crate) fn apply_config(&self, config: &mut JailConfig) {
        match self {
            Self::NoInit => config.init = Vec::new(),
            Self::NoDeinit => config.deinit = Vec::new(),
            Self::Allow(allows) => {
                for allow in allows.iter() {
                    if let Some(param) = allow.strip_prefix('-') {
                        for i in (0..config.allow.len()).rev() {
                            if config.allow[i] == param {
                                config.allow.remove(i);
                            }
                        }
                    } else if !config.allow.contains(allow) {
                        config.allow.push(allow.to_string());
                    }
                }
            }
            Self::ReplaceAllow(allows) => {
                for allow in allows.iter() {
                    if allow.strip_prefix('-').is_none() {
                        config.allow.push(allow.to_string());
                    }
                }
            }
            Self::Mount(mount_type, mount_point) => {
                let special_mount = SpecialMount {
                    mount_type: mount_type.to_string(),
                    mount_point: mount_point.to_string(),
                };
                if !config.special_mounts.contains(&special_mount) {
                    config.special_mounts.push(special_mount)
                }
            }
            Self::SysV(sysvattrs) => {
                for attr in sysvattrs.iter() {
                    match attr.as_str() {
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
            _ => {}
        }
    }

    pub(crate) fn implemented_directives() -> &'static [&'static str] {
        &["ALLOW", "NOINIT", "NODEINIT", "SYSVIPC", "MOUNT"]
    }
}

impl Directive for ConfigMod {
    fn from_action(action: &Action) -> Result<Self, anyhow::Error> {
        match action.directive_name.as_str() {
            "ALLOW" => match action.directive_args.get("replace") {
                Some(value) if value.as_str() == "true" => {
                    Ok(ConfigMod::ReplaceAllow(action.args.clone()))
                }
                _ => Ok(ConfigMod::Allow(action.args.clone())),
            },
            "NOINIT" => Ok(ConfigMod::NoInit),
            "NODEINIT" => Ok(ConfigMod::NoDeinit),
            "SYSVIPC" => Ok(ConfigMod::SysV(action.args.clone())),
            "MOUNT" => {
                let fstype = action.args.get(0).context("cannot get fstype")?;
                let mountpoint = action.args.get(1).context("cannot get mountpoint")?;
                Ok(ConfigMod::Mount(fstype.to_string(), mountpoint.to_string()))
            }
            _ => unreachable!(),
        }
    }
    fn run_in_context(&self, context: &mut JailContext) -> Result<(), anyhow::Error> {
        context.config_mods.push(self.clone());
        Ok(())
    }
}
