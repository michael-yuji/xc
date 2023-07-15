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
use crate::models::network::{DnsSetting, IpAssign};
use crate::util::realpath;

use anyhow::Context;
use effect::UndoStack;
use freebsd::event::EventFdNotify;
use freebsd::net::ifconfig::IFCONFIG_CMD;
use jail::param::Value;
use jail::StoppedJail;
use oci_util::image_reference::ImageReference;
use request::{CopyFileReq, Mount};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Represents an instance of container
#[derive(Debug, Clone)]
pub struct Container {
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
    pub linux_no_create_proc_dir: bool,
    pub linux_no_create_sys_dir: bool,
    pub zfs_origin: Option<String>,
    pub dns: DnsSetting,
    pub origin_image: Option<JailImage>,
    pub allowing: Vec<String>,
    pub image_reference: Option<ImageReference>,
    pub copies: Vec<CopyFileReq>,
    pub default_router: Option<IpAddr>,
}

impl Container {
    pub fn start_transactionally(&self, undo: &mut UndoStack) -> anyhow::Result<RunningContainer> {
        info!(name = self.name, "starting jail");

        let root = &self.root;
        let anchor = format!("xc/{}", self.id);

        undo.pf_create_anchor(anchor)
            .with_context(|| format!("failed to create pf anchor for container {}", self.id))?;

        let resolv_conf_path = realpath(root, "/etc/resolv.conf")
            .with_context(|| format!("failed finding /etc/resolv.conf in jail {}", self.id))?;

        match &self.dns {
            DnsSetting::Nop => {},
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

        for copy in self.copies.iter() {
            let dest = realpath(root, &copy.destination)?;
            let in_fd = copy.source;
            let file = unsafe { std::fs::File::from_raw_fd(copy.source) };
            let metadata = file.metadata().unwrap();

            let sink = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .open(dest)
                .unwrap();

            let sfd = sink.as_raw_fd();

            let size = unsafe {
                nix::libc::copy_file_range(
                    in_fd,
                    std::ptr::null_mut(),
                    sfd,
                    std::ptr::null_mut(),
                    metadata.len() as usize,
                    0,
                )
            };

            eprintln!("copied: {size}");
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

        if self.linux {
            proto = proto.param("linux", Value::Int(1));
            //            proto = proto.param("elf.fallback_brand", Value::Int(3));

            let proc_path = format!("{root}/proc");
            let sys_path = format!("{root}/sys");

            {
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
            {
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

        let jail = proto.start()?;
        let dmillis = std::time::Duration::from_millis(10);

        if self.vnet {
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

        let notify = Arc::new(EventFdNotify::new());

        Ok(RunningContainer {
            devfs_ruleset_id: self.devfs_ruleset_id,
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            root: self.root.to_owned(),
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
            init_proto: self.init.clone(),
            deinit_proto: self.deinit.clone(),
            main_proto: self.main.clone(),
            ip_alloc: self.ip_alloc.clone(),
            mount_req: self.mount_req.clone(),
            zfs_origin: self.zfs_origin.clone(),
            notify,
            main_started_notify: Arc::new(EventFdNotify::new()),
            destroyed: None,
            origin_image: self.origin_image.clone(),
            allowing: self.allowing.clone(),
            image_reference: self.image_reference.clone(),
            default_router: self.default_router,
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
}
