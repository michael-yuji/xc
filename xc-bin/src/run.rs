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

use crate::error::ActionError;
use crate::format::dataset::DatasetParam;
use crate::format::{BindMount, EnvPair, IpWant, PublishSpec};

use clap::Parser;
use ipc::packet::codec::{Fd, List, Maybe};
use oci_util::image_reference::ImageReference;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::path::PathBuf;
use xc::container::request::NetworkAllocRequest;
use xc::models::network::{DnsSetting, MainAddressSelector};
use xcd::ipc::{CopyFile, InstantiateRequest, MountReq};

#[derive(Parser, Debug)]
pub(crate) struct DnsArgs {
    /// Use empty resolv.conf
    #[arg(long = "empty-dns", action)]
    pub(crate) empty_dns: bool,

    /// Do not attempt to generate resolv.conf
    #[arg(long = "dns-nop", action)]
    pub(crate) dns_nop: bool,

    #[arg(long = "dns", /* multiple_occurrences */)]
    pub(crate) dns_servers: Vec<String>,

    #[arg(long = "dns-search", /* multiple_occurrences */)]
    pub(crate) dns_searchs: Vec<String>,
}

impl DnsArgs {
    pub(crate) fn make(self) -> DnsSetting {
        if self.empty_dns {
            DnsSetting::Specified {
                servers: Vec::new(),
                search_domains: Vec::new(),
            }
        } else if self.dns_nop {
            DnsSetting::Nop
        } else if self.dns_servers.is_empty() && self.dns_searchs.is_empty() {
            DnsSetting::Inherit
        } else {
            DnsSetting::Specified {
                servers: self.dns_servers,
                search_domains: self.dns_searchs,
            }
        }
    }
}

#[derive(Parser, Debug)]
pub(crate) struct PublishArgs {
    #[arg(long = "publish", short = 'p')]
    pub(crate) publish: Vec<PublishSpec>,
}

#[derive(Parser, Debug)]
pub(crate) struct RunArg {
    #[arg(long = "link", action)]
    pub(crate) link: bool,

    #[arg(long = "detach", short = 'd', action)]
    pub(crate) detach: bool,

    #[arg(long = "user", short = 'u', action)]
    pub(crate) user: Option<String>,

    #[arg(long = "group", short = 'g', action)]
    pub(crate) group: Option<String>,

    pub(crate) entry_point: Option<String>,

    pub(crate) entry_point_args: Vec<String>,
}

#[derive(Parser, Debug)]
pub(crate) struct CreateArgs {
    #[arg(long, default_value_t, action)]
    pub(crate) no_clean: bool,

    #[arg(long, default_value_t, action)]
    pub(crate) persist: bool,

    #[arg(long = "network", /* multiple_occurrences */)]
    pub(crate) networks: Vec<NetworkAllocRequest>,

    #[arg(short = 'v', /* multiple_occurrences */)]
    pub(crate) mounts: Vec<BindMount>,

    #[arg(long = "env", short = 'e', /* multiple_occurrences */)]
    pub(crate) envs: Vec<EnvPair>,

    #[arg(long = "name")]
    pub(crate) name: Option<String>,

    #[arg(long = "hostname")]
    pub(crate) hostname: Option<String>,

    #[arg(long = "vnet", action)]
    pub(crate) vnet: bool,

    // When in VNET, Do not create lo0 interface with 127.0.0.1/8 and ::1/128
    #[arg(long = "no-lo0", action)]
    pub(crate) no_lo0: bool,

    #[arg(long = "ip", action)]
    pub(crate) ips: Vec<IpWant>,

    #[arg(long = "copy", /* multiple_occurrences */)]
    pub(crate) copy: Vec<BindMount>,

    #[arg(long = "extra-layer", /* multiple_occurrences */)]
    pub(crate) extra_layers: Vec<PathBuf>,

    pub(crate) image_reference: ImageReference,

    #[arg(long = "net-group")]
    pub(crate) netgroups: Vec<String>,

    #[arg(short = 'z')]
    pub(crate) jail_dataset: Vec<DatasetParam>,

    #[arg(short = 'o')]
    pub(crate) props: Vec<String>,

    /// Enable DTrace USDT registration from container
    #[arg(long = "usdt")]
    pub(crate) usdt: bool,

    #[arg(long = "main-address")]
    pub(crate) main_address_selector: Option<MainAddressSelector>,

    #[arg(long = "tap")]
    pub(crate) tap_ifaces: Vec<String>,

    #[arg(long = "tun")]
    pub(crate) tun_ifaces: Vec<String>,

    #[arg(long = "max.children", default_value="0")]
    pub(crate) max_children: u32
}

impl CreateArgs {
    pub(crate) fn create_request(self) -> Result<InstantiateRequest, ActionError> {
        let name = self.name.clone();
        let hostname = self.hostname.or(self.name);
        let mount_req = self
            .mounts
            .into_iter()
            .map(|mount| {
                let sst = mount.source.to_string_lossy().to_string();
                let (source, evid) = if sst.starts_with('.') || sst.starts_with('/') {
                    let source = std::fs::canonicalize(mount.source).unwrap();

                    let flag = if source.is_dir() {
                        freebsd::nix::fcntl::OFlag::O_DIRECTORY
                    } else {
                        freebsd::nix::fcntl::OFlag::O_RDWR
                    };

                    let fd = Maybe::Some(Fd(freebsd::nix::fcntl::open(
                        &source,
                        flag,
                        freebsd::nix::sys::stat::Mode::empty(),
                    )
                    .unwrap()));

                    (source.as_os_str().to_os_string(), fd /*Maybe::None*/)
                } else {
                    (mount.source, Maybe::None)
                };
                MountReq {
                    read_only: false,
                    source,
                    evid,
                    dest: mount.destination,
                }
            })
            .collect::<Vec<_>>();

        let copies: List<CopyFile> = self
            .copy
            .into_iter()
            .map(|bind| {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .open(bind.source)
                    .expect("cannot open file for reading");
                let source = Fd(file.into_raw_fd());
                CopyFile {
                    source,
                    destination: bind.destination,
                }
            })
            .collect();

        let mut envs = {
            let mut map = std::collections::HashMap::new();
            for env in self.envs.into_iter() {
                map.insert(env.key, env.value);
            }
            map
        };

        let mut jail_datasets = Vec::new();

        for dataset_spec in self.jail_dataset.into_iter() {
            if let Some(key) = &dataset_spec.key {
                let path_str = dataset_spec.dataset.to_string_lossy().to_string();
                envs.insert(key.to_string(), path_str);
            }
            jail_datasets.push(dataset_spec.dataset);
        }

        let mut extra_layer_files = Vec::new();

        for layer in self.extra_layers.into_iter() {
            extra_layer_files.push(std::fs::OpenOptions::new().read(true).open(layer)?);
        }

        let extra_layers =
            List::from_iter(extra_layer_files.iter().map(|file| Fd(file.as_raw_fd())));

        let mut override_props = std::collections::HashMap::new();

        for prop in self.props.iter() {
            if let Some((key, value)) = prop.split_once('=') {
                override_props.insert(key.to_string(), value.to_string());
            }
        }

        let mut ips = self.ips;

        if self.vnet && !self.no_lo0 && !ips.iter().any(|ip| ip.0.interface == "lo0") {
            ips.push(IpWant(xc::models::network::IpAssign {
                network: None,
                interface: "lo0".to_string(),
                addresses: vec![
                    "127.0.0.1/8".parse().unwrap(),
                    "::1/128".parse().unwrap()
                ]
            }))
        }

        Ok(InstantiateRequest {
            create_only: true,
            name,
            hostname,
            copies,
            envs,
            vnet: self.vnet,
            ipreq: self.networks,
            mount_req: List::from_iter(mount_req),
            entry_point: None,
            extra_layers,
            no_clean: self.no_clean,
            persist: self.persist,
            image_reference: self.image_reference,
            ips: ips.into_iter().map(|v| v.0).collect(),
            main_norun: true,
            init_norun: true,
            deinit_norun: true,
            override_props,
            jail_datasets,
            enable_usdt: self.usdt,
            netgroups: self.netgroups,
            main_ip_selector: self.main_address_selector,
            tun_interfaces: Some(self.tun_ifaces),
            tap_interfaces: Some(self.tap_ifaces),
            children_max: self.max_children,
            ..InstantiateRequest::default()
        })
    }
}
