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

use crate::auth::Credential;
use crate::devfs_store::DevfsRulesetStore;
use crate::ipc::{EntryPointSpec, InstantiateRequest};
use crate::network_manager::NetworkManager;

use anyhow::Context;
use freebsd::event::EventFdNotify;
use freebsd::libc::{EEXIST, EINVAL, EIO, ENOENT, ENOTDIR, EPERM};
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, RawFd};
use varutil::string_interpolation::InterpolatedString;
use xc::container::request::{CopyFileReq, Mount, NetworkAllocRequest};
use xc::models::exec::{Jexec, StdioMode};
use xc::models::jail_image::JailImage;
use xc::models::network::{DnsSetting, IpAssign};
use xc::models::EntryPoint;
use xc::precondition_failure;

pub struct InstantiateBlueprint {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub image_reference: Option<ImageReference>,
    pub vnet: bool,
    pub mount_req: Vec<Mount>,
    pub copies: Vec<CopyFileReq>,
    pub main_norun: bool,
    pub init_norun: bool,
    pub deinit_norun: bool,
    pub extra_layers: Vec<RawFd>,
    pub persist: bool,
    pub no_clean: bool,
    pub dns: DnsSetting,
    pub origin_image: Option<JailImage>,
    pub allowing: Vec<String>,
    pub linux: bool,
    pub init: Vec<Jexec>,
    pub deinit: Vec<Jexec>,
    pub main: Option<Jexec>,
    pub ips: Vec<IpAssign>,
    pub ipreq: Vec<NetworkAllocRequest>,
    pub envs: HashMap<String, String>,
    pub devfs_ruleset_id: u16,
    pub ip_alloc: Vec<IpAssign>,
    pub default_router: Option<IpAddr>,
    pub main_started_notify: Option<EventFdNotify>,
    pub create_only: bool,
    pub linux_no_create_sys_dir: bool,
    pub linux_no_create_proc_dir: bool,
    pub linux_no_mount_sys: bool,
    pub linux_no_mount_proc: bool,
}

impl InstantiateBlueprint {
    pub(crate) fn new(
        id: &str,
        oci_config: &JailImage,
        request: InstantiateRequest,
        devfs_store: &mut DevfsRulesetStore,
        cred: &Credential,
        network_manager: &mut NetworkManager,
    ) -> anyhow::Result<InstantiateBlueprint> {
        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let config = oci_config.jail_config();
        let name = request.name.unwrap_or_else(|| id.to_string());
        let hostname = request.hostname.unwrap_or_else(|| name.to_string());
        let vnet = request.vnet || config.vnet;
        let available_allows = xc::util::jail_allowables();
        let mut envs = request.envs.clone();

        if config.linux && !freebsd::exists_kld("linux64") {
            precondition_failure!(
                EIO,
                "Linux image require linux64 kmod but it is missing from the system"
            );
        }

        let main_started_notify = match request.main_started_notify {
            ipc::packet::codec::Maybe::None => None,
            ipc::packet::codec::Maybe::Some(x) => Some(EventFdNotify::from_fd(x.as_raw_fd())),
        };

        for (key, env_spec) in config.envs.iter() {
            let key_string = key.to_string();
            if !request.envs.contains_key(&key_string) {
                if let Some(value) = &env_spec.default_value {
                    envs.insert(key_string, value.clone());
                } else if env_spec.required {
                    let extra_info = env_spec
                        .description
                        .as_ref()
                        .map(|d| format!(" - {d}"))
                        .unwrap_or_default();
                    precondition_failure!(
                        ENOENT,
                        "missing required environment variable: {name}{extra_info}"
                    );
                }
            }
        }

        let main = match request.entry_point {
            Some(spec) => {
                let args = spec
                    .entry_point_args
                    .iter()
                    .map(|a| InterpolatedString::new(a.as_str()).unwrap())
                    .collect::<Vec<_>>();
                let entry_point =
                    if let Some(entry_point) = config.entry_points.get(&spec.entry_point) {
                        entry_point.clone()
                    } else {
                        EntryPoint {
                            exec: spec.entry_point.to_string(),
                            args,
                            default_args: Vec::new(),
                            environ: HashMap::new(),
                            work_dir: None,
                            required_envs: Vec::new(),
                        }
                    };

                let entry_point_args = if spec.entry_point_args.is_empty() {
                    entry_point
                        .default_args
                        .iter()
                        .map(|arg| arg.apply(&envs))
                        .collect()
                } else {
                    spec.entry_point_args.clone()
                };

                let resolved_env = entry_point.resolve_args(&envs, &entry_point_args);
                let envs = resolved_env.env;

                for env in entry_point.required_envs.iter() {
                    let name = env.as_str();
                    if !envs.contains_key(name) {
                        precondition_failure!(
                            ENOENT,
                            "missing required environment variable {name}"
                        );
                    }
                }

                Some(Jexec {
                    arg0: resolved_env.exec,
                    args: resolved_env.args,
                    envs,
                    uid: 0,
                    output_mode: StdioMode::Terminal,
                    notify: None,
                    work_dir: entry_point.work_dir.clone(),
                })
            }
            None => None,
        };

        for assign in request.ips.iter() {
            let iface = &assign.interface;
            if !existing_ifaces.contains(iface) {
                precondition_failure!(ENOENT, "missing network interface {iface}");
            }
        }

        let allowing = {
            let mut allows = Vec::new();
            for allow in config.allow.iter() {
                if available_allows.contains(allow) {
                    allows.push(allow.to_string());
                } else {
                    precondition_failure!(EIO, "allow.{allow} is not available on this system");
                }
            }
            allows
        };

        let copies: Vec<xc::container::request::CopyFileReq> = request
            .copies
            .to_vec()
            .iter()
            .map(|c| xc::container::request::CopyFileReq {
                source: c.source.as_raw_fd(),
                destination: c.destination.to_string(),
            })
            .collect();

        let mut mount_req = Vec::new();

        for special_mount in config.special_mounts.iter() {
            if special_mount.mount_type.as_str() == "procfs" {
                mount_req.push(Mount::procfs(&special_mount.mount_point));
            } else if special_mount.mount_type.as_str() == "fdescfs" {
                mount_req.push(Mount::fdescfs(&special_mount.mount_point));
            }
        }

        let mut mount_specs = oci_config.jail_config().mounts;
        let mut added_mount_specs = HashMap::new();

        for req in request.mount_req.iter() {
            let source_path = std::path::Path::new(&req.source);
            if !source_path.exists() {
                precondition_failure!(ENOENT, "source mount point does not exist: {source_path:?}");
            }
            if !source_path.is_dir() && !source_path.is_file() {
                precondition_failure!(
                    ENOTDIR,
                    "mount point source is not a file nor directory: {source_path:?}"
                )
            }
            let Ok(meta) = std::fs::metadata(source_path) else {
                precondition_failure!(ENOENT, "invalid nullfs mount source")
            };

            if !cred.can_mount(&meta, false) {
                precondition_failure!(EPERM, "permission denied: {source_path:?}")
            }

            if let Some(mount_spec) = mount_specs.remove(&req.dest) {
                // XXX: mount options
                let mut mount = Mount::nullfs(&req.source, &mount_spec.destination);
                if mount_spec.read_only {
                    mount.options.push("ro".to_string());
                }
                mount_req.push(mount);
                added_mount_specs.insert(&req.dest, mount_spec);
            } else if req.dest.starts_with('/') {
                // use snapdir to check destination mount-ability
                mount_req.push(Mount::nullfs(&req.source, &req.dest));
            } else if added_mount_specs.get(&req.dest).is_some() {
                precondition_failure!(EEXIST, "duplicated mount detected {}", &req.dest);
            } else {
                precondition_failure!(ENOENT, "no such volume {}", &req.dest);
            }
        }

        for (key, spec) in mount_specs.iter() {
            if spec.required {
                precondition_failure!(ENOENT, "Required volume {key} is not mounted");
            }
        }

        let mut ip_alloc = request.ips.clone();

        let mut default_router = None;

        for req in request.ipreq.iter() {
            match network_manager.allocate(vnet, req, id) {
                Ok((alloc, router)) => {
                    if !existing_ifaces.contains(&alloc.interface) {
                        precondition_failure!(
                            ENOENT,
                            "missing network interface {}",
                            &alloc.interface
                        );
                    }
                    if let Some(router) = router {
                        if default_router.is_none() {
                            default_router = Some(router);
                        }
                    }
                    ip_alloc.push(alloc);
                }
                Err(error) => match error {
                    crate::network_manager::Error::Sqlite(error) => {
                        Err(error).context("sqlite error on address allocation")?;
                    }
                    crate::network_manager::Error::AllocationFailure(network) => {
                        precondition_failure!(
                            ENOENT,
                            "cannot allocate address from network {network}"
                        )
                    }
                    crate::network_manager::Error::AddressUsed(addr) => {
                        precondition_failure!(ENOENT, "address {addr} already consumed")
                    }
                    crate::network_manager::Error::InvalidAddress(addr, network) => {
                        precondition_failure!(EINVAL, "{addr} is not in the subnet of {network}")
                    }
                    crate::network_manager::Error::NoSuchNetwork(network) => {
                        precondition_failure!(
                            ENOENT,
                            "network {network} is missing from config file"
                        )
                    }
                    crate::network_manager::Error::NoSuchNetworkDatabase(network) => {
                        precondition_failure!(ENOENT, "network {network} is missing from database")
                    }
                    crate::network_manager::Error::Other(error) => {
                        Err(error).context("error occured during address allocation")?;
                    }
                },
            };
        }

        let mut devfs_rules = Vec::new();
        let rules = config
            .devfs_rules
            .iter()
            .map(|s| s.apply(&envs))
            .collect::<Vec<_>>();
        devfs_rules.push("include 1".to_string());
        devfs_rules.push("include 2".to_string());
        devfs_rules.push("include 3".to_string());
        devfs_rules.push("include 4".to_string());
        devfs_rules.push("include 5".to_string());
        devfs_rules.push("path dtrace unhide".to_string());
        // allow USDT to be registered
        devfs_rules.push("path dtrace/helper unhide".to_string());
        devfs_rules.extend(rules);

        let devfs_ruleset_id = devfs_store.get_ruleset_id(&devfs_rules);

        let extra_layers = request
            .extra_layers
            .to_vec()
            .into_iter()
            .map(|fd| fd.as_raw_fd())
            .collect::<Vec<_>>();

        Ok(InstantiateBlueprint {
            name,
            hostname,
            id: id.to_string(),
            vnet,
            init: config
                .init
                .clone()
                .into_iter()
                .map(|s| s.resolve_args(&envs).jexec())
                .collect(),
            deinit: config
                .clone()
                .deinit
                .into_iter()
                .map(|s| s.resolve_args(&envs).jexec())
                .collect(),
            extra_layers,
            main,
            ips: request.ips,
            ipreq: request.ipreq,
            mount_req,
            linux: config.linux,
            deinit_norun: request.deinit_norun,
            init_norun: request.init_norun,
            main_norun: request.main_norun,
            persist: request.persist,
            no_clean: request.no_clean,
            dns: request.dns,
            origin_image: Some(oci_config.clone()),
            allowing,
            image_reference: Some(request.image_reference),
            copies,
            envs,
            ip_alloc,
            devfs_ruleset_id,
            default_router,
            main_started_notify,
            create_only: request.create_only,
            linux_no_create_sys_dir: request.linux_no_create_sys_dir,
            linux_no_create_proc_dir: request.linux_no_create_proc_dir,
            linux_no_mount_sys: request.linux_no_mount_sys,
            linux_no_mount_proc: request.linux_no_mount_proc,
        })
    }
}
