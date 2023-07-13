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
pub mod cmd_arg;
pub mod exec;
pub mod jail_image;
pub mod network;

use crate::util::default_on_missing;

use cmd_arg::CmdArg;
use exec::ResolvedExec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use varutil::string_interpolation::{InterpolatedString, Var};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    pub description: String,
    pub destination: String,
    pub volume_hints: HashMap<String, Value>,
    pub read_only: bool,
    #[serde(default, deserialize_with = "default_on_missing")]
    pub required: bool,
}

impl MountSpec {
    pub fn new(destination: &str) -> MountSpec {
        MountSpec {
            description: "".to_string(),
            destination: destination.to_string(),
            volume_hints: HashMap::new(),
            read_only: false,
            required: false,
        }
    }
}

// Given some environment variables that may depends on other variables, resolve the order we
// should take to apply the variables
fn resolve_environ_order(environ: &HashMap<Var, InterpolatedString>) -> Vec<String> {
    let mut keys = HashSet::new();
    let mut resolved = Vec::new();
    let mut last_resolved = Vec::new();
    let mut dependencies = HashMap::new();

    /* seed */
    for (key, value) in environ.iter() {
        let mut deps = std::collections::HashSet::new();
        value.collect_variable_dependencies(&mut deps);
        keys.insert(key.to_string());
        dependencies.insert(key.to_string(), deps);
    }

    // remove external dependencies
    for key in keys.iter() {
        let deps = dependencies.get_mut(key).unwrap();
        let mut removing = Vec::new();
        for dep in deps.iter() {
            if !keys.contains(dep) {
                removing.push(dep.to_string());
            }
        }
        for dep in removing.iter() {
            deps.remove(&dep.to_string());
        }
    }

    loop {
        let mut local_resolved = Vec::new();

        for key in &keys {
            let deps = dependencies.get_mut(key).unwrap();

            for resolved_key in last_resolved.iter() {
                deps.remove(resolved_key);
            }

            if deps.is_empty() {
                local_resolved.push(key.to_string());
            }
        }
        // remove resolved key from running in next iteration
        for key in local_resolved.iter() {
            keys.remove(key);
        }

        resolved.extend(last_resolved);

        // we are no longer able to resolve more
        if local_resolved.is_empty() || keys.is_empty() {
            resolved.extend(local_resolved);
            break;
        }

        last_resolved = local_resolved;
    }

    resolved
}

/// Specification about an environment variable
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EnvSpec {
    /// Description about what the environment variable is for
    pub description: Option<String>,
    pub required: bool,
}

#[derive(PartialEq, Eq, Hash, Deserialize, Serialize, Copy, Clone, Debug, Default)]
pub enum SystemVPropValue {
    #[serde(rename = "new")]
    New,
    #[serde(rename = "inherit")]
    Inherit,
    #[default]
    #[serde(rename = "disable")]
    Disable,
}

impl std::fmt::Display for SystemVPropValue {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SystemVPropValue::Disable => write!(f, "disable"),
            SystemVPropValue::New => write!(f, "new"),
            SystemVPropValue::Inherit => write!(f, "inherit"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EntryPoint {
    pub exec: String,
    pub args: Vec<CmdArg>,
    #[serde(default)]
    pub default_args: Vec<InterpolatedString>,
    pub required_envs: Vec<Var>,
    pub environ: HashMap<Var, InterpolatedString>,
    pub work_dir: Option<String>
}

impl EntryPoint {
    pub fn resolve_environ(&self, supplied: &mut HashMap<String, String>) {
        let order = resolve_environ_order(&self.environ);
        for key in order.iter() {
            if !supplied.contains_key(key) {
                let format = self.environ.get(&Var::new(key.as_str()).unwrap()).unwrap();
                let value = format.apply(supplied);
                supplied.insert(key.to_string(), value);
            }
        }
    }

    pub fn resolve_args(&self, envs: &HashMap<String, String>, args: &[String]) -> ResolvedExec {
        let mut resolved_envs = envs.clone();
        self.resolve_environ(&mut resolved_envs);

        let mut argv = Vec::new();

        if args.is_empty() {
            for arg in self.args.iter() {
                match arg {
                    CmdArg::All => argv.extend_from_slice(args),
                    CmdArg::Positional(i) => {
                        let arg = &args.get(*i as usize).unwrap();
                        argv.push(arg.to_string())
                    }
                    CmdArg::Var(istr) => argv.push(istr.apply(&resolved_envs)),
                }
            }
        } else {
            for arg in args {
                argv.push(arg.to_string());
            }
        }

        ResolvedExec {
            exec: self.exec.to_string(),
            args: argv,
            env: resolved_envs,
        }
    }
}
