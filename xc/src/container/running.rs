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

use crate::container::process::ProcessStat;
use crate::container::request::Mount;
use crate::container::ContainerManifest;
use crate::models::exec::Jexec;
use crate::models::jail_image::JailImage;
use crate::models::network::{DnsSetting, IpAssign};
use crate::util::realpath;

use anyhow::Context;
use freebsd::event::EventFdNotify;
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch::Receiver;

use super::request::CopyFileReq;

#[derive(Clone, Debug)]
pub struct RunningContainer {
    pub devfs_ruleset_id: u16,
    pub jid: i32,
    pub name: String,
    pub root: String,
    pub id: String,
    pub vnet: bool,
    pub main_norun: bool,
    pub init_norun: bool,
    pub deinit_norun: bool,
    pub no_clean: bool,
    pub persist: bool,
    pub linux: bool,
    pub linux_no_create_sys_dir: bool,
    pub linux_no_create_proc_dir: bool,
    pub processes: HashMap<String, Receiver<ProcessStat>>,

    pub init_proto: Vec<Jexec>,
    pub deinit_proto: Vec<Jexec>,
    pub main_proto: Option<Jexec>,

    pub ip_alloc: Vec<IpAssign>,
    pub mount_req: Vec<Mount>,
    pub zfs_origin: Option<String>,

    pub notify: Arc<EventFdNotify>,

    pub deleted: Option<u64>,
    pub origin_image: Option<JailImage>,
    pub image_reference: Option<ImageReference>,

    pub allowing: Vec<String>,

    pub default_router: Option<IpAddr>,

    pub main_started_notify: Arc<EventFdNotify>,

    pub log_directory: Option<PathBuf>,

    pub fault: Option<String>,

    pub created: Option<u64>,

    pub finished_at: Option<u64>,

    pub started: Option<u64>,

    pub jailed_datasets: Vec<PathBuf>,
}

impl RunningContainer {
    pub fn copyin(&self, req: &CopyFileReq) -> anyhow::Result<()> {
        let dest = realpath(&self.root, &req.destination)?;
        let in_fd = req.source;
        let file = unsafe { std::fs::File::from_raw_fd(in_fd) };
        let metadata = file.metadata().unwrap();
        let sink = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(dest)
            .unwrap();

        let sfd = sink.as_raw_fd();

        let _size = unsafe {
            freebsd::nix::libc::copy_file_range(
                in_fd,
                std::ptr::null_mut(),
                sfd,
                std::ptr::null_mut(),
                metadata.len() as usize,
                0,
            )
        };

        Ok(())
    }

    pub fn setup_resolv_conf(&self, dns: &DnsSetting) -> anyhow::Result<()> {
        let resolv_conf_path = realpath(&self.root, "/etc/resolv.conf")
            .with_context(|| format!("failed finding /etc/resolv.conf in jail {}", self.id))?;

        match dns {
            DnsSetting::Nop => {}
            DnsSetting::Inherit => {
                std::fs::copy("/etc/resolv.conf", resolv_conf_path).with_context(|| {
                    format!(
                        "failed copying resolv.conf to destination container: {}",
                        self.id
                    )
                })?;
            }
            DnsSetting::Specified {
                servers,
                search_domains,
            } => {
                let servers = servers
                    .iter()
                    .map(|host| format!("nameserver {host}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let domains = search_domains
                    .iter()
                    .map(|host| format!("domain {host}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let resolv_conf = format!("{domains}\n{servers}\n");
                std::fs::write(resolv_conf_path, resolv_conf)?;
            }
        }
        Ok(())
    }

    pub fn serialized(&self) -> ContainerManifest {
        let mut processes = HashMap::new();

        for (key, value) in self.processes.iter() {
            processes.insert(key.to_string(), (*value.borrow()).clone());
        }

        ContainerManifest {
            devfs_ruleset_id: self.devfs_ruleset_id,
            jid: self.jid,
            name: self.name.to_string(),
            root: self.root.to_string(),
            id: self.id.to_string(),
            vnet: self.vnet,
            main_norun: self.main_norun,
            init_norun: self.init_norun,
            deinit_norun: self.deinit_norun,
            persist: self.persist,
            no_clean: self.no_clean,
            linux: self.linux,
            linux_no_create_sys_dir: self.linux_no_create_sys_dir,
            linux_no_create_proc_dir: self.linux_no_create_proc_dir,
            processes,
            init_proto: self.init_proto.clone(),
            deinit_proto: self.deinit_proto.clone(),
            main_proto: self.main_proto.clone(),
            ip_alloc: self.ip_alloc.clone(),
            mount_req: self.mount_req.clone(),
            zfs_origin: self.zfs_origin.clone(),
            origin_image: self.origin_image.clone(),
            allowing: self.allowing.clone(),
            image_reference: self.image_reference.clone(),
            fault: self.fault.clone(),
            started: self.started,
            finished_at: self.finished_at,
            created: self.created,
        }
    }
}
