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
use crate::models::network::IpAssign;

use freebsd::event::EventFdNotify;
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::watch::Receiver;

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

    pub destroyed: Option<u64>,
    pub origin_image: Option<JailImage>,
    pub image_reference: Option<ImageReference>,

    pub allowing: Vec<String>,

    pub default_router: Option<IpAddr>,

    pub main_started_notify: Arc<EventFdNotify>,
}

impl RunningContainer {
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
        }
    }
}
