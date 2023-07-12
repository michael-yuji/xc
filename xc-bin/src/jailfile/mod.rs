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
pub mod directives;
pub mod parse;

use ipc::packet::codec::{Fd, Maybe};
use oci_util::image_reference::ImageReference;
use std::collections::{HashMap, HashSet};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use tracing::error;
use xc::container::request::NetworkAllocRequest;
use xc::models::network::DnsSetting;
use xcd::ipc::*;

pub(crate) struct JailContext {
    /// The current container we are operating in
    pub(crate) container_id: Option<String>,
    /// Mapping to different containers for multi-stage build
    pub(crate) containers: HashMap<String, String>,

    pub(crate) conn: UnixStream,

    pub(crate) dns: DnsSetting,

    pub(crate) network: Vec<NetworkAllocRequest>,

    pub(crate) config_mods: Vec<self::directives::ConfigMod>,

    pub(crate) output_inplace: bool,
}

impl JailContext {
    pub(crate) fn new(
        conn: UnixStream,
        dns: DnsSetting,
        network: Vec<NetworkAllocRequest>,
        output_inplace: bool,
    ) -> JailContext {
        JailContext {
            conn,
            container_id: None,
            containers: HashMap::new(),
            dns,
            network,
            config_mods: Vec::new(),
            output_inplace,
        }
    }

    pub(crate) fn release(self, image_reference: ImageReference) -> anyhow::Result<()> {
        let mut conn = self.conn;
        let config_mods = self.config_mods;

        let local_id = xc::util::gen_id();
        let tempfile = if self.output_inplace {
            Some(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(&local_id)?,
            )
        } else {
            None
        };

        let req = match &tempfile {
            Some(tempfile) => {
                let fd = tempfile.as_raw_fd();

                CommitRequest {
                    name: image_reference.name.to_string(),
                    tag: image_reference.tag.to_string(),
                    container_name: self.container_id.clone().unwrap(),
                    alt_out: Maybe::Some(Fd(fd)),
                }
            }
            None => CommitRequest {
                name: image_reference.name.to_string(),
                tag: image_reference.tag.to_string(),
                container_name: self.container_id.clone().unwrap(),
                alt_out: Maybe::None,
            },
        };

        let response = do_commit_container(&mut conn, req)?.unwrap();

        if self.output_inplace {
            let container = do_show_container(
                &mut conn,
                ShowContainerRequest {
                    id: self.container_id.clone().unwrap(),
                },
            )?
            .unwrap();

            let mut image = container.running_container.origin_image.unwrap_or_default();
            let mut config = image.jail_config();
            for config_mod in config_mods.iter() {
                config_mod.apply_config(&mut config);
            }
            image.set_config(&config);

            std::fs::rename(local_id, &response.commit_id);
            std::fs::write("jail.json", serde_json::to_string_pretty(&image).unwrap());
        } else {
            crate::image::patch_image(&mut conn, &image_reference, |config| {
                for config_mod in config_mods.iter() {
                    config_mod.apply_config(config);
                }
            })?;
        }

        let mut containers = HashSet::new();
        if let Some(container) = self.container_id {
            containers.insert(container);
        }
        for (_, container) in self.containers.into_iter() {
            containers.insert(container);
        }
        for name in containers.into_iter() {
            let kill = KillContainerRequest {
                name: name.to_string(),
            };
            match do_kill_container(&mut conn, kill)? {
                Ok(_) => {}
                Err(error) => {
                    error!("cannot kill container {name}: {error:?}");
                }
            }
        }
        Ok(())
    }
}
