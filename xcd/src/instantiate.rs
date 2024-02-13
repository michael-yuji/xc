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
use crate::resources::volume::Volume;
use crate::resources::Resources;

use anyhow::Context;
use freebsd::event::EventFdNotify;
use freebsd::libc::{EINVAL, EIO, ENOENT, EPERM};
use ipc::packet::codec::Maybe;
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, RawFd};
use varutil::string_interpolation::InterpolatedString;
use xc::container::request::{CopyFileReq, Mount, NetworkAllocRequest};
use xc::errx;
use xc::format::devfs_rules::DevfsRule;
use xc::models::exec::{Jexec, StdioMode};
use xc::models::jail_image::JailImage;
use xc::models::network::{DnsSetting, IpAssign, MainAddressSelector};
use xc::models::EnforceStatfs;

pub struct CheckedInstantiateRequest {
    pub(crate) request: InstantiateRequest,
    pub(crate) devfs_rules: Vec<DevfsRule>,
    allowing: Vec<String>,
    copies: Vec<xc::container::request::CopyFileReq>,
    enforce_statfs: EnforceStatfs,
    pub(crate) image: JailImage,
}

impl CheckedInstantiateRequest {
    pub(crate) fn new(
        mut request: InstantiateRequest,
        oci_config: &JailImage,
        cred: &Credential,
        resources: &mut Resources,
    ) -> anyhow::Result<CheckedInstantiateRequest> {
        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let available_allows = xc::util::jail_allowables();
        let config = oci_config.jail_config();

        let mut envs = request.envs.clone();

        if let Some(ifaces) = request.tun_interfaces.as_ref() {
            for tun in ifaces.iter() {
                envs.insert(tun.to_string(), "dummy".to_string());
            }
        }

        if let Some(ifaces) = request.tap_interfaces.as_ref() {
            for tun in ifaces.iter() {
                envs.insert(tun.to_string(), "dummy".to_string());
            }
        }

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
                    errx!(
                        ENOENT,
                        "missing required environment variable: {key}{extra_info}"
                    );
                }
            }
        }


        for assign in request.ips.iter() {
            let iface = &assign.interface;
            if !existing_ifaces.contains(iface) {
                errx!(ENOENT, "missing network interface {iface}");
            }
        }

        let mut allowing = {
            let mut allows = Vec::new();
            for allow in config.allow.iter() {
                if available_allows.contains(allow) {
                    allows.push(allow.to_string());
                } else if let Some(("mount", fs)) = allow.split_once('.') {
                    errx!(ENOENT, "{allow} is not available; maybe try kldload {fs}");
                } else {
                    errx!(EIO, "allow.{allow} is not available on this system");
                }
            }
            allows
        };

        if !request.jail_datasets.is_empty() {
            allowing.push("mount".to_string());
            allowing.push("mount.zfs".to_string());
        }

        let enforce_statfs = if request.jail_datasets.is_empty() {
            EnforceStatfs::Strict
        } else {
            EnforceStatfs::BelowRoot
        };

        let copies: Vec<xc::container::request::CopyFileReq> = request
            .copies
            .move_to_vec()
            .iter()
            .map(|c| xc::container::request::CopyFileReq {
                source: c.source.as_raw_fd(),
                destination: c.destination.clone(),
            })
            .collect();

        let mut mount_specs = oci_config.jail_config().mounts;

        for req in request.mount_req.clone().to_vec().iter() {
            let source_path = std::path::Path::new(&req.source);

            if !source_path.is_absolute() {
                let name = source_path.to_string_lossy().to_string();
                match resources.query_volume(&name) {
                    None => {
                        errx!(ENOENT, "no such volume {name}")
                    }
                    Some(volume) => {
                        if !volume.can_mount(cred.uid()) {
                            errx!(EPERM, "this user is not allowed to mount the volume")
                        }
                    }
                }
            }

            mount_specs.remove(req.dest.to_str().unwrap());
        }

        for (key, spec) in mount_specs.iter() {
            if spec.required {
                errx!(ENOENT, "Required volume {key:?} is not mounted");
            }
        }

        for req in request.ipreq.iter() {
            let network = req.network();
            if !resources.has_network(network) {
                errx!(ENOENT, "no such network: {network}");
            }
        }

        'iter_groups: for group in request.netgroups.iter() {
            for req in request.ipreq.iter() {
                if req.network() == group {
                    continue 'iter_groups;
                }
            }
            errx!(
                ENOENT,
                "cannot add container to netgroup {group} as network {group} does not exist"
            )
        }

        let mut devfs_rules = Vec::new();
        for rule in config.devfs_rules.iter() {
            let applied = rule.apply(&envs);
            match applied.parse::<xc::format::devfs_rules::DevfsRule>() {
                Err(error) => {
                    errx!(EINVAL, "invaild devfs rule: [{applied}], {error}")
                }
                Ok(rule) => devfs_rules.push(rule),
            }
        }

        Ok(CheckedInstantiateRequest {
            request,
            copies,
            devfs_rules,
            allowing,
            enforce_statfs,
            image: oci_config.clone(),
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
    pub override_props: HashMap<String, String>,
    pub enforce_statfs: EnforceStatfs,
    pub jailed_datasets: Vec<std::path::PathBuf>,
    pub children_max: u32,
    pub main_ip_selector: Option<MainAddressSelector>,
    pub created_interfaces: Vec<String>,
}

impl InstantiateBlueprint {
    pub(crate) fn new(
        id: &str,
        request: CheckedInstantiateRequest,
        devfs_store: &mut DevfsRulesetStore,
        cred: &Credential,
        resources: &mut Resources,
    ) -> anyhow::Result<InstantiateBlueprint> {
        let oci_config = &request.image;
        let existing_ifaces = freebsd::net::ifconfig::interfaces()?;
        let config = oci_config.jail_config();
        let name = match request.request.name {
            None => format!("xc-{id}"),
            Some(name) => {
                if name.parse::<isize>().is_ok() {
                    errx!(EINVAL, "Jail name cannot be numeric")
                } else if name.contains('.') {
                    errx!(EINVAL, "Jail name cannot contain dot (.)")
                } else {
                    name
                }
            }
        };

        let hostname = request.request.hostname.unwrap_or_else(|| name.to_string());
        let vnet = request.request.vnet || config.vnet;
        let mut tuntap_ifaces = Vec::new();
        let mut envs = request.request.envs.clone();

        for (key, env_spec) in config.envs.iter() {
            let key_string = key.to_string();
            if !request.request.envs.contains_key(&key_string) {
                if let Some(value) = &env_spec.default_value {
                    envs.insert(key_string, value.clone());
                } else if env_spec.required {
                    let extra_info = env_spec
                        .description
                        .as_ref()
                        .map(|d| format!(" - {d}"))
                        .unwrap_or_default();
                    errx!(
                        ENOENT,
                        "missing required environment variable: {key}{extra_info}"
                    );
                }
            }
        }

        if config.linux {
            if !freebsd::exists_kld("linux64") {
                errx!(
                    EIO,
                    "Linux image require linux64 kmod but it is missing from the system"
                );
            } else if xc::util::elf_abi_fallback_brand() != "3" {
                errx!(EIO, "kern.elf64.fallback_brand did not set to 3 (Linux)");
            }
        }

        let main_started_notify = match request.request.main_started_notify {
            ipc::packet::codec::Maybe::None => None,
            ipc::packet::codec::Maybe::Some(x) => Some(EventFdNotify::from_fd(x.as_raw_fd())),
        };

        let mut ip_alloc = request.request.ips.clone();

        let mut default_router = None;

        for req in request.request.ipreq.iter() {
            match resources.allocate(vnet, req, id) {
                Ok((alloc, router)) => {
                    if !existing_ifaces.contains(&alloc.interface) {
                        errx!(ENOENT, "missing network interface {}", &alloc.interface);
                    }
                    if let Some(router) = router {
                        if default_router.is_none() {
                            default_router = Some(router);
                        }
                    }
                    ip_alloc.push(alloc);
                }
                Err(error) => match error {
                    crate::resources::network::Error::Sqlite(error) => {
                        Err(error).context("sqlite error on address allocation")?;
                    }
                    crate::resources::network::Error::AllocationFailure(network) => {
                        errx!(ENOENT, "cannot allocate address from network {network}")
                    }
                    crate::resources::network::Error::AddressUsed(addr) => {
                        errx!(ENOENT, "address {addr} already consumed")
                    }
                    crate::resources::network::Error::InvalidAddress(addr, network) => {
                        errx!(EINVAL, "{addr} is not in the subnet of {network}")
                    }
                    crate::resources::network::Error::NoSuchNetwork(network) => {
                        errx!(ENOENT, "network {network} is missing from config file")
                    }
                    crate::resources::network::Error::Other(error) => {
                        Err(error).context("error occured during address allocation")?;
                    }
                },
            };
        }

        for tap in request.request.tap_interfaces.unwrap_or_default() {
            let interface = freebsd::net::ifconfig::create_tap()?;
            tuntap_ifaces.push(interface.to_string());
            envs.insert(tap, interface.clone());
            ip_alloc.push(IpAssign { network: None, addresses: Vec::new(), interface });
        }

        for tun in request.request.tun_interfaces.unwrap_or_default() {
            let interface = freebsd::net::ifconfig::create_tap()?;
            tuntap_ifaces.push(interface.to_string());
            envs.insert(tun, interface.clone());
            ip_alloc.push(IpAssign { network: None, addresses: Vec::new(), interface });
        }

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

        for req in request.request.mount_req.clone().to_vec().iter() {
            let source_path = std::path::Path::new(&req.source);

            let volume = if !source_path.is_absolute() {
                let name = source_path.to_string_lossy().to_string();
                match resources.query_volume(&name) {
                    None => {
                        errx!(ENOENT, "no such volume {name}")
                    }
                    Some(volume) => {
                        if !volume.can_mount(cred.uid()) {
                            errx!(EPERM, "this user is not allowed to mount the volume")
                        } else {
                            volume
                        }
                    }
                }
            } else {
                match &req.evid {
                    Maybe::None => errx!(ENOENT, "missing evidence"),
                    Maybe::Some(fd) => {
                        let Ok(stat) = freebsd::nix::sys::stat::fstat(fd.as_raw_fd()) else {
                            errx!(ENOENT, "cannot stat evidence")
                        };
                        let check_stat = freebsd::nix::sys::stat::stat(source_path).unwrap();
                        if stat.st_ino != check_stat.st_ino {
                            errx!(ENOENT, "evidence inode mismatch")
                        }
                        _ = freebsd::nix::unistd::close(fd.as_raw_fd());
                        Volume::adhoc(source_path)
                    }
                }
            };

            let mount_spec = mount_specs.remove(req.dest.to_str().unwrap());

            if mount_spec.is_some() {
                added_mount_specs.insert(&req.dest, mount_spec.clone().unwrap());
            }

            let mount = resources.mount(id, cred, req, mount_spec.as_ref(), &volume)?;
            mount_req.push(mount);
        }

        for dataset in request.request.jail_datasets.iter() {
            if resources.dataset_tracker.is_jailed(dataset) {
                errx!(
                    EPERM,
                    "another container is using this dataset: {dataset:?}"
                )
            } else {
                resources.dataset_tracker.set_jailed(id, dataset)
            }
        }

        let mut devfs_rules = vec![
            "include 1".to_string(),
            "include 2".to_string(),
            "include 3".to_string(),
            "include 4".to_string(),
            "include 5".to_string(),
        ];

        if request.request.enable_usdt {
            devfs_rules.push("path dtrace unhide".to_string());
            devfs_rules.push("path dtrace/helper unhide".to_string());
        }

        for rule in request.devfs_rules.iter() {
            devfs_rules.push(rule.to_string());
        }

        for name in tuntap_ifaces.iter() {
            devfs_rules.push(format!("path {name} unhide"));
        }

        let devfs_ruleset_id = devfs_store.get_ruleset_id(&devfs_rules);

        envs.insert("XC_DEVFS_RULESET".to_string(), devfs_ruleset_id.to_string());

        let main = match &request.request.entry_point {
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
                            user: request.request.user.clone(),
                            group: request.request.group.clone(),
                        }
                    };

                if request.request.user.is_some() {
                    entry_point.user = request.request.user.clone();
                }

                if request.request.group.is_some() {
                    entry_point.group = request.request.group.clone();
                }

                let mut jexec = entry_point.resolve_args(&envs, &spec.entry_point_args)?;
                jexec.output_mode = StdioMode::Terminal;
                Some(jexec)
            }
            None => None,
        };

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

        let extra_layers = request
            .request
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
            init,
            deinit,
            extra_layers,
            main,
            ips: request.request.ips,
            ipreq: request.request.ipreq,
            mount_req,
            linux: config.linux,
            deinit_norun: request.request.deinit_norun,
            init_norun: request.request.init_norun,
            main_norun: request.request.main_norun,
            persist: request.request.persist,
            no_clean: request.request.no_clean,
            dns: request.request.dns,
            origin_image: Some(oci_config.clone()),
            allowing: request.allowing,
            image_reference: Some(request.request.image_reference),
            copies: request.copies,
            envs,
            ip_alloc,
            devfs_ruleset_id,
            default_router,
            main_started_notify,
            create_only: request.request.create_only,
            linux_no_create_sys_dir: request.request.linux_no_create_sys_dir,
            linux_no_create_proc_dir: request.request.linux_no_create_proc_dir,
            linux_no_mount_sys: request.request.linux_no_mount_sys,
            linux_no_mount_proc: request.request.linux_no_mount_proc,
            override_props: request.request.override_props,
            enforce_statfs: request.enforce_statfs,
            jailed_datasets: request.request.jail_datasets,
            children_max: request.request.children_max,
            main_ip_selector: request.request.main_ip_selector,
            created_interfaces: tuntap_ifaces
        })
    }
}
