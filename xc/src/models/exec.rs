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

use super::resolve_environ_order;
use ipc::packet::codec::{Fd, FromPacket, Maybe};
use ipc_macro::FromPacket;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::fd::RawFd;
use std::path::PathBuf;
use varutil::string_interpolation::{InterpolatedString, Var};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StdioMode {
    #[serde(rename = "terminal")]
    Terminal,
    #[serde(rename = "inherit")]
    Inherit,
    #[serde(rename = "files")]
    Files {
        // we can't really use /dev/null here because it is possible devfs is not mounted
        stdout: Option<PathBuf>,
        stderr: Option<PathBuf>,
    },
    Forward {
        stdin: Option<RawFd>,
        stdout: Option<RawFd>,
        stderr: Option<RawFd>,
    },
}

pub enum IpcStdioMode {
    Terminal,
    Inherit,
    Files {
        stdout: Option<PathBuf>,
        stderr: Option<PathBuf>,
    },
    Forward {
        stdin: Maybe<Fd>,
        stdout: Maybe<Fd>,
        stderr: Maybe<Fd>,
    },
}

impl IpcStdioMode {
    pub fn to_local(self) -> StdioMode {
        match self {
            Self::Terminal => StdioMode::Terminal,
            Self::Inherit => StdioMode::Inherit,
            Self::Files { stdout, stderr } => StdioMode::Files { stdout, stderr },
            Self::Forward {
                stdin,
                stdout,
                stderr,
            } => StdioMode::Forward {
                stdin: stdin.to_option().map(|fd| fd.0),
                stdout: stdout.to_option().map(|fd| fd.0),
                stderr: stderr.to_option().map(|fd| fd.0),
            },
        }
    }
}

impl FromPacket for IpcStdioMode {
    // we are abusing the `StdioMode` struct, in this mode, we store offset to the raw fd instead
    // of the raw fds
    type Dual = StdioMode;
    fn encode_to_dual(self, fds: &mut Vec<i32>) -> Self::Dual {
        match self {
            Self::Terminal => StdioMode::Terminal,
            Self::Inherit => StdioMode::Inherit,
            Self::Files { stdout, stderr } => StdioMode::Files { stdout, stderr },
            Self::Forward {
                stdin,
                stdout,
                stderr,
            } => {
                let stdin = if let Maybe::Some(fd) = stdin {
                    let idx = fds.len();
                    fds.push(fd.0);
                    Some(idx as i32)
                } else {
                    None
                };
                let stdout = if let Maybe::Some(fd) = stdout {
                    let idx = fds.len();
                    fds.push(fd.0);
                    Some(idx as i32)
                } else {
                    None
                };
                let stderr = if let Maybe::Some(fd) = stderr {
                    let idx = fds.len();
                    fds.push(fd.0);
                    Some(idx as i32)
                } else {
                    None
                };
                StdioMode::Forward {
                    stdin,
                    stdout,
                    stderr,
                }
            }
        }
    }
    fn decode_from_dual(value: Self::Dual, fds: &[RawFd]) -> Self {
        match value {
            StdioMode::Terminal => Self::Terminal,
            StdioMode::Inherit => Self::Inherit,
            StdioMode::Files { stdout, stderr } => Self::Files { stdout, stderr },
            StdioMode::Forward {
                stdin,
                stdout,
                stderr,
            } => Self::Forward {
                stdin: Maybe::from_option(stdin.map(|idx| Fd(*fds.get(idx as usize).unwrap()))),
                stdout: Maybe::from_option(stdout.map(|idx| Fd(*fds.get(idx as usize).unwrap()))),
                stderr: Maybe::from_option(stderr.map(|idx| Fd(*fds.get(idx as usize).unwrap()))),
            },
        }
    }
}

/// Executable parameters to be executed in container
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Jexec {
    pub arg0: String,
    pub args: Vec<String>,
    pub envs: std::collections::HashMap<String, String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub output_mode: StdioMode,
    pub notify: Option<RawFd>,
    pub work_dir: Option<String>,
}

#[derive(FromPacket)]
pub struct IpcJexec {
    pub arg0: String,
    pub args: Vec<String>,
    pub envs: std::collections::HashMap<String, String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub output_mode: IpcStdioMode,
    pub notify: Maybe<Fd>,
    pub work_dir: Option<String>,
}

impl IpcJexec {
    pub fn to_local(self) -> Jexec {
        Jexec {
            arg0: self.arg0,
            args: self.args,
            envs: self.envs,
            uid: self.uid,
            gid: self.gid,
            user: self.user,
            group: self.group,
            work_dir: self.work_dir,
            notify: self.notify.to_option().map(|fd| fd.0),
            output_mode: self.output_mode.to_local(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Exec {
    pub exec: String,
    pub args: Vec<InterpolatedString>,
    pub environ: HashMap<Var, InterpolatedString>,
    #[serde(default)]
    pub clear_env: bool,
    pub default_args: Vec<InterpolatedString>,
    pub required_envs: Vec<Var>,
    pub work_dir: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
}

impl Exec {
    /// Applied the supplied environment variables and derive all the parametized
    /// Environment variables, reinsert back to the hashmap
    ///
    /// # Arguments
    ///
    /// * `suppiled` - The inout argument
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

    /// Given a map of environment variables, return the exec, applied environment variables, and
    /// argument list
    ///
    /// # Arguments
    ///
    /// * `envs` - The parameters
    pub fn resolve_args(
        &self,
        envs: &HashMap<String, String>,
        args: &[String],
    ) -> Result<Jexec, crate::container::error::Error> {
        let mut argv = Vec::new();
        let mut resolved_envs = if self.clear_env {
            HashMap::new()
        } else {
            envs.clone()
        };

        self.resolve_environ(&mut resolved_envs);

        for env in self.required_envs.iter() {
            if !resolved_envs.contains_key(env.as_str()) {
                return Err(crate::container::error::Error::new(
                    freebsd::libc::ENOENT,
                    anyhow::anyhow!("missing required environment variable {env}"),
                ));
            }
        }

        for arg in self.args.iter() {
            argv.push(arg.apply(&resolved_envs));
        }

        if args.is_empty() {
            for arg in self.default_args.iter() {
                argv.push(arg.apply(&resolved_envs));
            }
        }

        let uid = self.user.as_ref().and_then(|user| user.parse::<u32>().ok());
        let gid = self
            .user
            .as_ref()
            .and_then(|group| group.parse::<u32>().ok());

        Ok(Jexec {
            arg0: self.exec.to_string(),
            args: argv,
            envs: resolved_envs,
            uid,
            gid,
            user: self.user.clone(),
            group: self.group.clone(),
            output_mode: StdioMode::Files {
                stdout: None,
                stderr: None,
            },
            notify: None,
            work_dir: self.work_dir.clone(),
        })
    }
}
