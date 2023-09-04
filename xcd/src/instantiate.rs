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
use crate::ipc::InstantiateRequest;
use crate::network_manager::NetworkManager;
use crate::volume::{VolumeManager, Volume};

use anyhow::Context;
use freebsd::event::EventFdNotify;
use freebsd::libc::{EINVAL, EIO, ENOENT, EPERM};
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, RawFd};
use varutil::string_interpolation::InterpolatedString;
use xc::container::request::{CopyFileReq, Mount, NetworkAllocRequest};
use xc::format::devfs_rules::DevfsRule;
use xc::models::exec::{Jexec, StdioMode};
use xc::models::jail_image::JailImage;
use xc::models::network::{DnsSetting, IpAssign};
use xc::precondition_failure;

pub struct AppliedInstantiateRequest {
    base: InstantiateRequest,
    devfs_rules: Vec<DevfsRule>,
    init: Vec<Jexec>,
    deinit: Vec<Jexec>,
    main: Option<Jexec>,
    envs: HashMap<String, String>,
    allowing: Vec<String>,
    copies: Vec<xc::container::request::CopyFileReq>,
    mount_req: Vec<Mount>,
}

impl AppliedInstantiateRequest {
    pub(crate) fn new(
        mut request: InstantiateRequest,
        oci_config: &JailImage,
        cred: &Credential,
        network_manager: &NetworkManager,
        volume_manager: &VolumeManager,
    ) -> anyhow::Result<AppliedInstantiateRequest> {
        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let available_allows = xc::util::jail_allowables();
        let config = oci_config.jail_config();

        let mut envs = request.envs.clone();
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
                        "missing required environment variable: {key}{extra_info}"
                    );
                }
            }
        }

        let main = match &request.entry_point {
            Some(spec) => {
                let args = {
                    let mut args = Vec::new();
                    for arg in spec.entry_point_args.iter() {
                        args.push(arg.parse::<InterpolatedString>().context("invalid arg")?);
                    }
                    args
                };

                let selected_entry = match &spec.entry_point {
                    Some(name) => name.to_string(),
                    None => config
                        .default_entry_point
                        .unwrap_or_else(|| "main".to_string()),
                };

                let mut entry_point =
                    if let Some(entry_point) = config.entry_points.get(&selected_entry) {
                        entry_point.clone()
                    } else {
                        xc::models::exec::Exec {
                            exec: selected_entry,
                            args,
                            default_args: Vec::new(),
                            environ: HashMap::new(),
                            work_dir: None,
                            required_envs: Vec::new(),
                            clear_env: false,
                            user: request.user.clone(),
                            group: request.group.clone(),
                        }
                    };

                if request.user.is_some() {
                    entry_point.user = request.user.clone();
                }

                if request.group.is_some() {
                    entry_point.group = request.group.clone();
                }

                let mut jexec = entry_point.resolve_args(&envs, &spec.entry_point_args)?;
                jexec.output_mode = StdioMode::Terminal;
                Some(jexec)
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
            .move_to_vec()
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

            let volume = if !source_path.is_absolute() {
                let name = source_path.to_string_lossy().to_string();
                match volume_manager.query_volume(&name) {
                    None => {
                        precondition_failure!(ENOENT, "no such volume {name}")
                    },
                    Some(volume) => {
                        if !volume.can_mount(cred.uid()) {
                            precondition_failure!(EPERM, "this user is not allowed to mount the volume")
                        } else {
                            volume
                        }
                    }
                }
            } else {
                Volume::adhoc(source_path)
            };

            let mount_spec = mount_specs.remove(&req.dest);

            if mount_spec.is_some() {
                added_mount_specs.insert(&req.dest, mount_spec.clone().unwrap());
            }

            let mount = volume_manager.mount(cred, req, mount_spec.as_ref(), &volume)?;
/*
            let mount = match volume.driver {
                VolumeDriverKind::Directory => {
                    LocalDriver::default().mount(cred, req, mount_spec.as_ref(), &volume)?
                },
                VolumeDriverKind::ZfsDataset => {
                    ZfsDriver::default().mount(cred, req, mount_spec.as_ref(), &volume)?
                }
            };
*/
            mount_req.push(mount);
        }

        for (key, spec) in mount_specs.iter() {
            if spec.required {
                precondition_failure!(ENOENT, "Required volume {key} is not mounted");
            }
        }

        for req in request.ipreq.iter() {
            let network = req.network();
            if !network_manager.has_network(&network) {
                precondition_failure!(ENOENT, "no such network: {network}");
            }
        }

        let init = config
            .init
            .clone()
            .into_iter()
            .map(|s| s.resolve_args(&envs, &[]))
            .collect::<Result<Vec<_>, _>>()?;

        let deinit = config
            .deinit
            .clone()
            .into_iter()
            .map(|s| s.resolve_args(&envs, &[]))
            .collect::<Result<Vec<_>, _>>()?;

        let mut devfs_rules = Vec::new();
        for rule in config.devfs_rules.iter() {
            let applied = rule.apply(&envs);
            match applied.parse::<xc::format::devfs_rules::DevfsRule>() {
                Err(error) => {
                    precondition_failure!(EINVAL, "invaild devfs rule: [{applied}], {error}")
                }
                Ok(rule) => devfs_rules.push(rule),
            }
        }

        Ok(AppliedInstantiateRequest {
            base: request,
            copies,
            devfs_rules,
            init,
            deinit,
            main,
            envs,
            allowing,
            mount_req,
        })
    }
}

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
        request: AppliedInstantiateRequest,
        devfs_store: &mut DevfsRulesetStore,
        _cred: &Credential,
        network_manager: &mut NetworkManager,
    ) -> anyhow::Result<InstantiateBlueprint> {
        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let config = oci_config.jail_config();
        let name = match request.base.name {
            None => format!("xc-{id}"),
            Some(name) => {
                if name.parse::<isize>().is_ok() {
                    precondition_failure!(EINVAL, "Jail name cannot be numeric")
                } else if name.contains('.') {
                    precondition_failure!(EINVAL, "Jail name cannot contain dot (.)")
                } else {
                    name
                }
            }
        };
        let hostname = request.base.hostname.unwrap_or_else(|| name.to_string());
        let vnet = request.base.vnet || config.vnet;
        let envs = request.envs.clone();

        if config.linux && !freebsd::exists_kld("linux64") {
            precondition_failure!(
                EIO,
                "Linux image require linux64 kmod but it is missing from the system"
            );
        }

        let main_started_notify = match request.base.main_started_notify {
            ipc::packet::codec::Maybe::None => None,
            ipc::packet::codec::Maybe::Some(x) => Some(EventFdNotify::from_fd(x.as_raw_fd())),
        };

        let mut ip_alloc = request.base.ips.clone();

        let mut default_router = None;

        for req in request.base.ipreq.iter() {
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
                    crate::network_manager::Error::Other(error) => {
                        Err(error).context("error occured during address allocation")?;
                    }
                },
            };
        }

        let mut devfs_rules = vec![
            "include 1".to_string(),
            "include 2".to_string(),
            "include 3".to_string(),
            "include 4".to_string(),
            "include 5".to_string(),
            "path dtrace unhide".to_string(),
            // allow USDT to be registered
            "path dtrace/helper unhide".to_string(),
        ];

        for rule in request.devfs_rules.iter() {
            devfs_rules.push(rule.to_string());
        }

        let devfs_ruleset_id = devfs_store.get_ruleset_id(&devfs_rules);

        let extra_layers = request
            .base
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
            init: request.init,
            deinit: request.deinit,
            extra_layers,
            main: request.main,
            ips: request.base.ips,
            ipreq: request.base.ipreq,
            mount_req: request.mount_req,
            linux: config.linux,
            deinit_norun: request.base.deinit_norun,
            init_norun: request.base.init_norun,
            main_norun: request.base.main_norun,
            persist: request.base.persist,
            no_clean: request.base.no_clean,
            dns: request.base.dns,
            origin_image: Some(oci_config.clone()),
            allowing: request.allowing,
            image_reference: Some(request.base.image_reference),
            copies: request.copies,
            envs,
            ip_alloc,
            devfs_ruleset_id,
            default_router,
            main_started_notify,
            create_only: request.base.create_only,
            linux_no_create_sys_dir: request.base.linux_no_create_sys_dir,
            linux_no_create_proc_dir: request.base.linux_no_create_proc_dir,
            linux_no_mount_sys: request.base.linux_no_mount_sys,
            linux_no_mount_proc: request.base.linux_no_mount_proc,
        })
    }
}
