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
pub mod add_env;
pub mod copy;
pub mod from;
pub mod run;
pub mod volume;

use super::JailContext;
use crate::jailfile::parse::Action;

use anyhow::Context;
use std::collections::HashMap;
use std::ffi::OsString;
use varutil::string_interpolation::{InterpolatedString, Var};
use xc::models::exec::Exec;
use xc::models::jail_image::{JailConfig, SpecialMount};
use xc::models::{EnvSpec, MountSpec, SystemVPropValue};

pub(crate) trait Directive: Sized {
    fn from_action(action: &Action) -> Result<Self, anyhow::Error>;
    fn run_in_context(&self, context: &mut JailContext) -> Result<(), anyhow::Error>;
    fn up_to_date(&self) -> bool;
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigMod {
    Allow(Vec<String>),
    ReplaceAllow(Vec<String>),
    WorkDir(String, String),
    Init,
    NoInit,
    Deinit,
    NoDeinit,
    Expose,
    EntryPoint(String, String, HashMap<Var, InterpolatedString>),
    Cmd(String, Vec<String>),
    Volume(OsString, MountSpec),
    Mount(String, String),
    SysV(Vec<String>),
    AddEnv(Var, EnvSpec),
}

impl ConfigMod {
    pub(crate) fn apply_config(&self, config: &mut JailConfig) {
        match self {
            Self::NoInit => config.init = Vec::new(),
            Self::NoDeinit => config.deinit = Vec::new(),
            Self::AddEnv(variable, spec) => {
                config.envs.insert(variable.clone(), spec.clone());
            }
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
            Self::Volume(name, mount_spec) => {
                config.mounts.insert(name.clone(), mount_spec.clone());
            }
            Self::WorkDir(entry_point, dir) => {
                let work_dir = dir.to_string();
                match config.entry_points.get_mut(entry_point) {
                    None => {
                        let entrypoint = Exec {
                            exec: String::new(),
                            args: Vec::new(),
                            default_args: Vec::new(),
                            required_envs: Vec::new(),
                            environ: HashMap::new(),
                            work_dir: Some(work_dir),
                            clear_env: false,
                            user: None,
                            group: None,
                        };
                        config
                            .entry_points
                            .insert(entry_point.to_string(), entrypoint);
                    }
                    Some(entry_point) => {
                        entry_point.work_dir = Some(work_dir);
                    }
                }
            }
            Self::EntryPoint(entry_point, cmd, environ) => {
                let exec = cmd.to_string();
                match config.entry_points.get_mut(entry_point) {
                    None => {
                        let entrypoint = Exec {
                            exec,
                            args: Vec::new(),
                            default_args: Vec::new(),
                            required_envs: Vec::new(),
                            environ: environ.clone(),
                            work_dir: None,
                            clear_env: false,
                            user: None,
                            group: None,
                        };
                        config
                            .entry_points
                            .insert(entry_point.to_string(), entrypoint);
                    }
                    Some(entry_point) => {
                        entry_point.exec = exec;
                    }
                }
            }
            Self::Cmd(entry_point, args) => {
                let default_args = args
                    .iter()
                    .map(|arg| {
                        let parsed = InterpolatedString::new(arg.as_str())
                            .unwrap_or_else(|| panic!("cannot parse interpolate string: {arg}"));
                        parsed
                    })
                    .collect();

                match config.entry_points.get_mut(entry_point) {
                    None => {
                        let entrypoint = Exec {
                            exec: String::new(),
                            args: Vec::new(),
                            default_args,
                            required_envs: Vec::new(),
                            environ: HashMap::new(),
                            work_dir: None,
                            clear_env: false,
                            user: None,
                            group: None,
                        };
                        config
                            .entry_points
                            .insert(entry_point.to_string(), entrypoint);
                    }
                    Some(entry_point) => {
                        entry_point.default_args = default_args;
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn implemented_directives() -> &'static [&'static str] {
        &[
            "ALLOW",
            "NOINIT",
            "NODEINIT",
            "SYSVIPC",
            "MOUNT",
            "WORKDIR",
            "ENTRYPOINT",
            "CMD",
        ]
    }
}

impl Directive for ConfigMod {
    fn up_to_date(&self) -> bool {
        true
    }

    fn from_action(action: &Action) -> Result<Self, anyhow::Error> {
        match action.directive_name.as_str() {
            "WORKDIR" => {
                let entry_point = action
                    .directive_args
                    .get("entry_point")
                    .map(|s| s.as_str())
                    .unwrap_or_else(|| "main")
                    .to_string();
                let arg0 = action
                    .args
                    .get(0)
                    .context("entry point requires one variable")?;
                Ok(ConfigMod::WorkDir(entry_point, arg0.to_string()))
            }
            "ENTRYPOINT" => {
                let entry_point = action
                    .directive_args
                    .get("entry_point")
                    .map(|s| s.as_str())
                    .unwrap_or_else(|| "main")
                    .to_string();
                let mut args_iter = action.args.iter();
                let mut curr = args_iter.next();
                let mut envs = HashMap::new();

                while let Some((key, value)) = curr.and_then(|c| c.split_once('=')) {
                    envs.insert(
                        Var::new(key)
                            .context("Invalid environ key, must conform to IEEE Std 1003.1-2001")?,
                        InterpolatedString::new(value).context("Invalid environ value")?,
                    );
                    curr = args_iter.next();
                }

                let arg0 = curr.as_ref().context("entry point requires one variable")?;
                Ok(ConfigMod::EntryPoint(entry_point, arg0.to_string(), envs))
            }
            "CMD" => {
                let entry_point = action
                    .directive_args
                    .get("entry_point")
                    .map(|s| s.as_str())
                    .unwrap_or_else(|| "main")
                    .to_string();
                let args = action.args.clone();
                Ok(ConfigMod::Cmd(entry_point, args))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_directive_entrypoint_no_env() {
        let action = Action {
            directive_name: "ENTRYPOINT".to_string(),
            directive_args: HashMap::new(),
            args: vec!["arg0".to_string()],
            heredoc: None,
        };
        let config_mod = ConfigMod::from_action(&action).expect("cannot parse");
        assert_eq!(
            config_mod,
            ConfigMod::EntryPoint("main".to_string(), "arg0".to_string(), HashMap::new())
        );
    }

    #[test]
    fn test_parse_directive_entrypoint_with_env() {
        let action = Action {
            directive_name: "ENTRYPOINT".to_string(),
            directive_args: HashMap::new(),
            args: vec![
                "A=BCD".to_string(),
                "PATH=/usr/bin:/usr/sbin".to_string(),
                "arg0".to_string(),
            ],
            heredoc: None,
        };
        let config_mod = ConfigMod::from_action(&action).expect("cannot parse");
        let mut expect_hashmap = HashMap::new();

        expect_hashmap.insert(
            Var::new("A").unwrap(),
            InterpolatedString::new("BCD").unwrap(),
        );
        expect_hashmap.insert(
            Var::new("PATH").unwrap(),
            InterpolatedString::new("/usr/bin:/usr/sbin").unwrap(),
        );
        assert_eq!(
            config_mod,
            ConfigMod::EntryPoint("main".to_string(), "arg0".to_string(), expect_hashmap)
        );
    }
}
