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

pub mod effect;
pub mod error;
pub mod process;
pub mod request;
pub mod runner;
pub mod running;

use self::process::ProcessStat;
use crate::container::running::RunningContainer;
use crate::models::exec::Jexec;
use crate::models::jail_image::JailImage;
use crate::models::network::IpAssign;
use crate::models::EnforceStatfs;
use crate::util::realpath;

use anyhow::Context;
use effect::UndoStack;
use freebsd::event::EventFdNotify;
use freebsd::net::ifconfig::IFCONFIG_CMD;
use ipcidr::IpCidr;
use jail::param::Value;
use jail::StoppedJail;
use oci_util::image_reference::ImageReference;
use request::Mount;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[usdt::provider]
mod create_container_provider {
    fn jail_created(jid: i32, id: &str) {}
}

#[derive(Debug, Clone)]
pub struct CreateContainer {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub root: String,
    /// The devfs ruleset id assigned to this container
    pub devfs_ruleset_id: u16,
    pub ip_alloc: Vec<IpAssign>,
    pub mount_req: Vec<Mount>,
    pub vnet: bool,
    pub init: Vec<Jexec>,
    pub deinit: Vec<Jexec>,
    pub main: Option<Jexec>,
    pub linux: bool,
    pub main_norun: bool,
    pub init_norun: bool,
    pub deinit_norun: bool,
    pub persist: bool,
    pub no_clean: bool,
    /// Do not create /proc automatically and abort mounting procfs if the directory is missing.
    pub linux_no_create_proc_dir: bool,
    /// Do not cerate /sys automatically and abort mounting sysfs if the directory is missing
    pub linux_no_create_sys_dir: bool,
    /// Do not mount linux sysfs
    pub linux_no_mount_sys: bool,
    /// Do not mount linux procfs
    pub linux_no_mount_proc: bool,
    pub zfs_origin: Option<String>,

    pub origin_image: Option<JailImage>,

    pub allowing: Vec<String>,

    pub image_reference: Option<ImageReference>,

    pub default_router: Option<IpAddr>,

    pub log_directory: Option<PathBuf>,

    pub override_props: HashMap<String, String>,

    pub enforce_statfs: EnforceStatfs,

    pub jailed_datasets: Vec<PathBuf>,

    pub children_max: u32,
}

impl CreateContainer {
    pub fn create_transactionally(self, undo: &mut UndoStack) -> anyhow::Result<RunningContainer> {
        info!(name = self.name, "starting jail");

        let root = &self.root;
        let anchor = format!("xc/{}", self.id);

        undo.pf_create_anchor(anchor)
            .with_context(|| format!("failed to create pf anchor for container {}", self.id))?;

        let devfs_ruleset = self.devfs_ruleset_id;

        let mut proto = StoppedJail::new(root).name(&self.name).param(
            "devfs_ruleset".to_string(),
            Value::Int(self.devfs_ruleset_id.into()),
        );

        proto = proto.hostname(self.hostname.to_string());

        if std::path::Path::new(&format!("{root}/dev")).exists() {
            undo.mount(
                "devfs".to_string(),
                vec![format!("ruleset={devfs_ruleset}")],
                OsString::from("devfs"),
                PathBuf::from(format!("{root}/dev")),
            )?;
        }

        for mreq in self.mount_req.iter() {
            let dest = realpath(root, &mreq.dest)?;
            undo.mount(
                mreq.fs.to_string(),
                Vec::new(),
                OsString::from(&mreq.source),
                dest,
            )?;
        }

        let mut ifaces_to_move = HashMap::new();

        if !self.vnet {
            let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
            for alloc in self.ip_alloc.iter() {
                if !existing_ifaces.contains(&alloc.interface) {
                    // such interface does not exists
                    panic!()
                } else {
                    for address in alloc.addresses.iter() {
                        undo.iface_create_alias(alloc.interface.to_string(), address.clone())
                            .with_context(|| {
                                format!("failed during interface creation, container: {}", self.id)
                            })?;
                        proto = proto.ip(address.addr());
                    }
                }
            }
        } else {
            proto = proto.param("vnet", Value::Int(1));
            for alloc in self.ip_alloc.iter() {
                if let Some(_network) = &alloc.network {
                    let (epair_a, epair_b) = undo.create_epair().with_context(|| {
                        format!("failed during epair creation, container: {}", self.id)
                    })?;
                    undo.iface_up(epair_a.to_owned()).with_context(|| {
                        format!("failed to bring up epair_a, container: {}", self.id)
                    })?;
                    undo.iface_up(epair_b.to_owned()).with_context(|| {
                        format!("failed to bring up epair_a, container: {}", self.id)
                    })?;
                    undo.bridge_add_iface(alloc.interface.to_string(), epair_a)
                        .with_context(|| {
                            format!("failed adding epair_a to bridge, container: {}", self.id)
                        })?;

                    match ifaces_to_move.get_mut(&alloc.interface) {
                        None => {
                            ifaces_to_move.insert(epair_b.to_string(), alloc.addresses.clone());
                        }
                        Some(addresses) => {
                            addresses.extend(alloc.addresses.clone());
                        }
                    }
                } else {
                    match ifaces_to_move.get_mut(&alloc.interface) {
                        None => {
                            ifaces_to_move
                                .insert(alloc.interface.to_string(), alloc.addresses.clone());
                        }
                        Some(addresses) => {
                            addresses.extend(alloc.addresses.clone());
                        }
                    }
                }
            }
        }

        for allow in self.allowing.iter() {
            proto = proto.param(format!("allow.{allow}"), Value::Int(1));
        }

        let config = &self.origin_image.as_ref().map(|ji| ji.jail_config());

        if let Some(osrelease) = config.as_ref().and_then(|s| s.osrelease.clone()) {
            proto = proto.param("osrelease", Value::String(osrelease));
        }
        if let Some(osreldate) = config.as_ref().and_then(|s| s.osreldate) {
            proto = proto.param("osreldate", Value::Int(osreldate));
        }

        if self.linux {
            proto = proto.param("linux", Value::Int(1));

            if let Some(osname) = config.as_ref().and_then(|s| s.linux_osname.clone()) {
                proto = proto.param("linux.osname", Value::String(osname));
            }
            if let Some(osrelease) = config.as_ref().and_then(|s| s.linux_osrelease.clone()) {
                proto = proto.param("linux.osrelease", Value::String(osrelease));
            }
            if let Some(oss_version) = config.as_ref().and_then(|s| s.linux_oss_version.clone()) {
                proto = proto.param("linux.osreldate", Value::String(oss_version));
            }

            let proc_path = format!("{root}/proc");
            let sys_path = format!("{root}/sys");

            if !self.linux_no_mount_proc {
                let path = std::path::Path::new(&proc_path);
                let path_existed = path.exists();

                if path_existed || !self.linux_no_create_proc_dir {
                    if !path_existed {
                        std::fs::create_dir(&proc_path).unwrap();
                    }
                    undo.mount(
                        "linprocfs".to_string(),
                        Vec::new(),
                        OsString::from("linprocfs"),
                        path.to_path_buf(),
                    )?;
                }
            }

            if !self.linux_no_mount_sys {
                let path = std::path::Path::new(&sys_path);
                let path_existed = path.exists();

                if path_existed || !self.linux_no_create_sys_dir {
                    if !path_existed {
                        std::fs::create_dir(&sys_path).unwrap();
                    }
                    undo.mount(
                        "linsysfs".to_string(),
                        Vec::new(),
                        OsString::from("linsysfs"),
                        path.to_path_buf(),
                    )?;
                }
            }
        }

        proto = proto.param(
            "enforce_statfs",
            match self.enforce_statfs {
                EnforceStatfs::Strict => Value::Int(2),
                EnforceStatfs::BelowRoot => Value::Int(1),
                EnforceStatfs::ExposeAll => Value::Int(0),
            },
        );

        if self.children_max > 0 {
            proto = proto.param("children.max", Value::Int(self.children_max as i32));
        }

        for (key, value) in self.override_props.iter() {
            const STR_KEYS: [&str; 6] = [
                "host.hostuuid",
                "host.domainname",
                "host.hostname",
                "osrelease",
                "path",
                "name",
            ];

            let v = if STR_KEYS.contains(&key.as_str()) {
                Value::String(value.to_string())
            } else if let Ok(num) = value.parse::<i32>() {
                Value::Int(num)
            } else {
                Value::String(value.to_string())
            };

            proto = proto.param(key, v);
        }

        let jail = proto.start()?;
        create_container_provider::jail_created!(|| (jail.jid, &self.id));

        if self.vnet {
            let dmillis = std::time::Duration::from_millis(10);

            for (iface, addresses) in ifaces_to_move.iter() {
                undo.move_if(iface.to_owned(), jail.jid)?;
                std::thread::sleep(dmillis);
                for address in addresses.iter() {
                    match address {
                        ipcidr::IpCidr::V4(cidr) => {
                            info!(
                                "/sbin/ifconfig -j {} {} inet {cidr} alias",
                                jail.jid.to_string(),
                                iface.to_string(),
                            );
                            _ = std::process::Command::new(IFCONFIG_CMD)
                                .arg("-j")
                                .arg(jail.jid.to_string())
                                .arg(iface)
                                .arg("inet")
                                .arg(format!("{cidr}"))
                                .arg("alias")
                                .status();
                        }
                        ipcidr::IpCidr::V6(cidr) => {
                            info!(
                                "/sbin/ifconfig -j {} {} inet6 {cidr} alias",
                                jail.jid.to_string(),
                                iface.to_string(),
                            );
                            _ = std::process::Command::new(IFCONFIG_CMD)
                                .arg("-j")
                                .arg(jail.jid.to_string())
                                .arg(iface)
                                .arg("inet6")
                                .arg("-ifdisabled")
                                .status();
                            _ = std::process::Command::new(IFCONFIG_CMD)
                                .arg("-j")
                                .arg(jail.jid.to_string())
                                .arg(iface)
                                .arg("inet6")
                                .arg(format!("{cidr}"))
                                .arg("alias")
                                .status();
                        }
                    }
                    std::thread::sleep(dmillis);
                }
            }
        }

        if let Some(default_router) = self.default_router {
            _ = std::process::Command::new("/sbin/route")
                .arg("-j")
                .arg(jail.jid.to_string())
                .arg("add")
                .arg("default")
                .arg(default_router.to_string())
                .status();
        }

        for dataset in self.jailed_datasets.iter() {
            let zfs = freebsd::fs::zfs::ZfsHandle::default();
            _ = zfs.cycle_jailed_on(dataset);
            // XXX: allow to use a non-default zfs handle?
            undo.jail_dataset(
                zfs,
                jail.jid.to_string(),
                dataset.to_path_buf(),
            )
            .with_context(|| "jail dataset: {dataset:?}")?;
        }

        let notify = Arc::new(EventFdNotify::new());

        Ok(RunningContainer {
            devfs_ruleset_id: self.devfs_ruleset_id,
            id: self.id,
            name: self.name,
            root: self.root,
            jid: jail.jid,
            vnet: self.vnet,
            init_norun: self.init_norun,
            deinit_norun: self.deinit_norun,
            persist: self.persist,
            no_clean: self.no_clean,
            main_norun: self.main_norun,
            linux: self.linux,
            linux_no_create_sys_dir: self.linux_no_create_sys_dir,
            linux_no_create_proc_dir: self.linux_no_create_proc_dir,
            processes: HashMap::new(),
            init_proto: self.init,
            deinit_proto: self.deinit,
            main_proto: self.main,
            ip_alloc: self.ip_alloc,
            mount_req: self.mount_req,
            zfs_origin: self.zfs_origin,
            notify,
            main_started_notify: Arc::new(EventFdNotify::new()),
            deleted: None,
            origin_image: self.origin_image,
            allowing: self.allowing,
            image_reference: self.image_reference,
            default_router: self.default_router,
            log_directory: self.log_directory,
            fault: None,
            created: Some(crate::util::epoch_now_nano()),
            started: None,
            finished_at: None,
            jailed_datasets: self.jailed_datasets,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContainerManifest {
    pub devfs_ruleset_id: u16,
    pub jid: i32,
    pub name: String,
    pub root: String,
    pub id: String,
    pub vnet: bool,
    pub main_norun: bool,
    pub init_norun: bool,
    pub deinit_norun: bool,
    pub persist: bool,
    pub no_clean: bool,
    pub linux: bool,
    pub linux_no_create_sys_dir: bool,
    pub linux_no_create_proc_dir: bool,
    pub processes: HashMap<String, ProcessStat>,
    pub init_proto: Vec<Jexec>,
    pub deinit_proto: Vec<Jexec>,
    pub main_proto: Option<Jexec>,
    pub ip_alloc: Vec<IpAssign>,
    pub mount_req: Vec<Mount>,
    pub zfs_origin: Option<String>,
    pub origin_image: Option<JailImage>,
    pub allowing: Vec<String>,
    pub image_reference: Option<ImageReference>,
    pub fault: Option<String>,
    pub created: Option<u64>,
    pub started: Option<u64>,
    pub finished_at: Option<u64>,
}

impl ContainerManifest {
    /// Get the main address for a given managed network, that is, defined as the
    /// first address created in such network
    pub fn main_ip_for_network(&self, network: &str) -> Option<IpCidr> {
        for alloc in self.ip_alloc.iter() {
            if alloc
                .network
                .as_ref()
                .map(|n| n == network)
                .unwrap_or_default()
            {
                if let Some(first) = alloc.addresses.first() {
                    return Some(first.clone());
                }
            }
        }
        None
    }
}
