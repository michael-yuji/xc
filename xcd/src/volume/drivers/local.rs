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

use freebsd::libc::{ENOENT, ENOTDIR, EPERM};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use xc::models::MountSpec;
use xc::{
    container::{
        error::PreconditionFailure,
        request::{Mount, MountReq},
    },
    precondition_failure,
};

use crate::volume::VolumeDriverKind;
use crate::{auth::Credential, volume::Volume};

use super::VolumeDriver;

pub struct LocalDriver {
    pub(crate) default_subdir: Option<PathBuf>,
}

impl VolumeDriver for LocalDriver {
    fn create(
        &self,
        name: &str,
        _template: Option<MountSpec>,
        source: Option<std::path::PathBuf>,
        _props: HashMap<String, String>,
    ) -> Result<Volume, PreconditionFailure> {
        let path = match source {
            None => {
                let Some(mut parent) = self.default_subdir.clone() else {
                    precondition_failure!(ENOENT, "Default volume directory not found")
                };
                parent.push(name);
                if parent.exists() {
                    precondition_failure!(ENOENT, "Target directory already exists")
                }
                parent
            }
            Some(path) => {
                if !path.exists() {
                    precondition_failure!(ENOENT, "No such directory")
                } else if !path.is_dir() {
                    precondition_failure!(ENOTDIR, "Source path is not a directory")
                }
                path
            }
        };
        Ok(Volume {
            name: Some(name.to_string()),
            rw_users: None,
            authorized_users: None,
            driver: VolumeDriverKind::Directory,
            mount_options: Vec::new(),
            driver_options: HashMap::new(),
            device: path,
        })
    }

    fn mount(
        &self,
        cred: &Credential,
        mount_req: &MountReq,
        mount_spec: Option<&MountSpec>,
        volume: &Volume,
    ) -> Result<Mount, PreconditionFailure> {
        let source_path = &volume.device;
        if !&source_path.exists() {
            precondition_failure!(ENOENT, "source mount point does not exist: {source_path:?}");
        }
        if !&source_path.is_dir() && !source_path.is_file() {
            precondition_failure!(
                ENOTDIR,
                "mount point source is not a file nor directory: {source_path:?}"
            )
        }
        let Ok(meta) = std::fs::metadata(&volume.device) else {
            precondition_failure!(ENOENT, "invalid nullfs mount source")
        };

        if !cred.can_mount(&meta, false) {
            precondition_failure!(EPERM, "permission denied: {source_path:?}")
        }

        let mut mount_options = HashSet::new();

        if !volume.can_mount_rw(cred.uid())
            || mount_spec.map(|spec| spec.read_only).unwrap_or_default()
        {
            mount_options.insert("ro".to_string());
        }

        for option in volume.mount_options.iter() {
            mount_options.insert(option.to_string());
        }

        Ok(Mount {
            options: Vec::from_iter(mount_options),
            ..Mount::nullfs(source_path, &mount_req.dest)
        })
    }
}
