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

use crate::ipc::MountReq;
use crate::volume::VolumeDriverKind;
use crate::{auth::Credential, volume::Volume};
use freebsd::fs::zfs::{ZfsCreate, ZfsHandle};
use freebsd::libc::{EEXIST, EIO, ENOENT, EPERM};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use xc::models::MountSpec;
use xc::{
    container::{error::PreconditionFailure, request::Mount},
    precondition_failure,
};

use super::VolumeDriver;

#[derive(Default, Clone, Debug)]
pub struct ZfsDriver {
    pub(crate) handle: ZfsHandle,
    pub(crate) default_dataset: Option<PathBuf>,
}

impl VolumeDriver for ZfsDriver {
    fn create(
        &self,
        name: &str,
        template: Option<MountSpec>,
        source: Option<std::path::PathBuf>,
        props: HashMap<String, String>,
    ) -> Result<Volume, PreconditionFailure> {
        let mut zfs_props = props;

        if let Some(template) = template {
            for (key, value) in template.volume_hints.into_iter() {
                if let Some((_, prop)) = key.split_once("zfs.") {
                    if let Some(value) = value.as_str() {
                        zfs_props.insert(prop.to_string(), value.to_string());
                    }
                }
            }
        }

        let dataset = match source {
            None => {
                let Some(mut dataset) = self.default_dataset.clone() else {
                    precondition_failure!(ENOENT, "Default volume dataset not set")
                };
                dataset.push(name);

                if self.handle.exists(&dataset) {
                    precondition_failure!(EEXIST, "Dataset already exist")
                }

                let mut zfs_create = ZfsCreate::new(&dataset, true, false);
                zfs_create.set_props(zfs_props.clone());

                if let Err(error) = self.handle.create(zfs_create) {
                    precondition_failure!(EIO, "Cannot create zfs dataset: {error:?}")
                }

                dataset
            }
            Some(dataset) => {
                if !self.handle.exists(&dataset) {
                    precondition_failure!(ENOENT, "Requested dataset does not exist")
                }
                dataset
            }
        };

        Ok(Volume {
            name: Some(name.to_string()),
            rw_users: None,
            authorized_users: None,
            driver: VolumeDriverKind::ZfsDataset,
            mount_options: Vec::new(),
            driver_options: zfs_props,
            device: dataset,
        })
    }

    fn mount(
        &self,
        cred: &Credential,
        mount_req: &MountReq,
        mount_spec: Option<&MountSpec>,
        volume: &Volume,
    ) -> Result<Mount, PreconditionFailure> {
        if !self.handle.exists(&volume.device) {
            precondition_failure!(ENOENT, "No such dataset: {:?}", volume.device);
        }
        let Ok(Some(mount_point)) = self.handle.mount_point(&volume.device) else {
            precondition_failure!(
                ENOENT,
                "Dataset {:?} does not have a mount point",
                volume.device
            );
        };
        let Ok(meta) = std::fs::metadata(&mount_point) else {
            precondition_failure!(EIO, "cannot get metadata of {mount_point:?}");
        };
        if !cred.can_mount(&meta, false) {
            precondition_failure!(EPERM, "permission denied: {mount_point:?}");
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
            ..Mount::nullfs(&mount_point, &mount_req.dest)
        })
    }
}
