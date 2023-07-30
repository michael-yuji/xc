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

use crate::format::{BindMount, EnvPair, IpWant, PublishSpec};

use clap::Parser;
use oci_util::image_reference::ImageReference;
use std::path::PathBuf;
use xc::container::request::NetworkAllocRequest;

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

    #[arg(long = "ip", action)]
    pub(crate) ips: Vec<IpWant>,

    #[arg(long = "copy", /* multiple_occurrences */)]
    pub(crate) copy: Vec<BindMount>,

    #[arg(long = "extra-layer", /* multiple_occurrences */)]
    pub(crate) extra_layers: Vec<PathBuf>,

    #[arg(long = "publish", short = 'p', /* multiple_occurrences */)]
    pub(crate) publish: Vec<PublishSpec>,

    pub(crate) image_reference: ImageReference,
}

#[derive(Parser, Debug)]
pub(crate) struct RunArg {
    #[arg(long, default_value_t, action)]
    pub(crate) no_clean: bool,

    #[arg(long, default_value_t, action)]
    pub(crate) persist: bool,

    #[arg(long = "create-only", action)]
    pub(crate) create_only: bool,

    #[arg(long = "link", action)]
    pub(crate) link: bool,

    #[arg(long = "publish", short = 'p', /* multiple_occurrences */)]
    pub(crate) publish: Vec<PublishSpec>,

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

    #[arg(long = "detach", short = 'd', action)]
    pub(crate) detach: bool,

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

    #[arg(long = "ip", action)]
    pub(crate) ips: Vec<IpWant>,

    #[arg(long = "user", short = 'u', action)]
    pub(crate) user: Option<String>,

    #[arg(long = "group", short = 'u', action)]
    pub(crate) group: Option<String>,

    #[arg(long = "copy", /* multiple_occurrences */)]
    pub(crate) copy: Vec<BindMount>,

    #[arg(long = "extra-layer", /* multiple_occurrences */)]
    pub(crate) extra_layers: Vec<PathBuf>,

    pub(crate) image_reference: ImageReference,

    pub(crate) entry_point: Option<String>,

    pub(crate) entry_point_args: Vec<String>,
}
