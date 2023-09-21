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
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub struct DelegateCredential {
    pub uid: i32,
    pub gid: i32,
}

#[derive(Clone, Debug)]
pub struct CopyFileReq {
    pub source: std::os::fd::RawFd,
    pub destination: OsString,
}

/// Ip allocation requests from user
#[derive(Serialize, Clone, Deserialize, Debug)]
pub enum NetworkAllocRequest {
    Any { network: String },
    Explicit { network: String, ip: IpAddr },
}

impl NetworkAllocRequest {
    pub fn network<'a>(&'a self) -> &'a String {
        match self {
            Self::Any { network } => network,
            Self::Explicit { network, .. } => network,
        }
    }
}

impl FromStr for NetworkAllocRequest {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once('|') {
            Some((network, spec)) => {
                let ip: IpAddr = spec.parse().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::Other, "cannot parse address")
                })?;
                Ok(Self::Explicit {
                    network: network.to_string(),
                    ip,
                })
            }
            None => Ok(Self::Any {
                network: s.to_string(),
            }),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Mount {
    pub source: String,
    pub dest: PathBuf,
    pub fs: String,
    pub options: Vec<String>,
}

impl Mount {
    pub fn procfs(mountpoint: impl AsRef<Path>) -> Mount {
        Mount {
            source: "proc".to_string(),
            dest: mountpoint.as_ref().to_path_buf(),
            fs: "procfs".to_string(),
            options: Vec::new(),
        }
    }
    pub fn fdescfs(mountpoint: impl AsRef<Path>) -> Mount {
        Mount {
            source: "fdescfs".to_string(),
            dest: mountpoint.as_ref().to_path_buf(),
            fs: "fdescfs".to_string(),
            options: Vec::new(),
        }
    }
    pub fn nullfs(
        source: impl AsRef<std::path::Path>,
        mountpoint: impl AsRef<std::path::Path>,
    ) -> Mount {
        Mount {
            source: source.as_ref().to_string_lossy().to_string(),
            dest: mountpoint.as_ref().to_path_buf(),
            fs: "nullfs".to_string(),
            options: Vec::new(),
        }
    }
}
