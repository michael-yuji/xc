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
use xc::container::request::{MountReq, NetworkAllocRequest};

#[derive(Parser, Debug)]
pub(crate) struct CreateArgs {
    #[clap(long, default_value_t, action)]
    pub(crate) no_clean: bool,

    #[clap(long, default_value_t, action)]
    pub(crate) persist: bool,

    #[clap(long = "network", multiple_occurrences = true)]
    pub(crate) networks: Vec<NetworkAllocRequest>,

    #[clap(short = 'v', multiple_occurrences = true)]
    pub(crate) mounts: Vec<BindMount>,

    #[clap(long = "env", short = 'e', multiple_occurrences = true)]
    pub(crate) envs: Vec<EnvPair>,

    #[clap(long = "name")]
    pub(crate) name: Option<String>,

    #[clap(long = "hostname")]
    pub(crate) hostname: Option<String>,

    #[clap(long = "vnet", action)]
    pub(crate) vnet: bool,

    #[clap(long = "ip", action)]
    pub(crate) ips: Vec<IpWant>,

    #[clap(long = "copy", multiple_occurrences = true)]
    pub(crate) copy: Vec<BindMount>,

    #[clap(long = "extra-layer", multiple_occurrences = true)]
    pub(crate) extra_layers: Vec<PathBuf>,

    #[clap(long = "publish", short = 'p', multiple_occurrences = true)]
    pub(crate) publish: Vec<PublishSpec>,

    pub(crate) image_reference: ImageReference,
}

#[derive(Parser, Debug)]
pub(crate) struct RunArg {
    #[clap(long, default_value_t, action)]
    pub(crate) no_clean: bool,

    #[clap(long, default_value_t, action)]
    pub(crate) persist: bool,

    #[clap(long = "create-only", action)]
    pub(crate) create_only: bool,

    #[clap(long = "link", action)]
    pub(crate) link: bool,

    #[clap(long = "publish", short = 'p', multiple_occurrences = true)]
    pub(crate) publish: Vec<PublishSpec>,

    /// Use empty resolv.conf
    #[clap(long = "empty-dns", action)]
    pub(crate) empty_dns: bool,

    /// Do not attempt to generate resolv.conf
    #[clap(long = "dns-nop", action)]
    pub(crate) dns_nop: bool,

    #[clap(long = "dns", multiple_occurrences = true)]
    pub(crate) dns_servers: Vec<String>,

    #[clap(long = "dns-search", multiple_occurrences = true)]
    pub(crate) dns_searchs: Vec<String>,

    #[clap(long = "detach", short = 'd', action)]
    pub(crate) detach: bool,

    #[clap(long = "network", multiple_occurrences = true)]
    pub(crate) networks: Vec<NetworkAllocRequest>,

    #[clap(short = 'v', multiple_occurrences = true)]
    pub(crate) mounts: Vec<BindMount>,

    #[clap(long = "env", short = 'e', multiple_occurrences = true)]
    pub(crate) envs: Vec<EnvPair>,

    #[clap(long = "name")]
    pub(crate) name: Option<String>,

    #[clap(long = "hostname")]
    pub(crate) hostname: Option<String>,

    #[clap(long = "vnet", action)]
    pub(crate) vnet: bool,

    #[clap(long = "ip", action)]
    pub(crate) ips: Vec<IpWant>,

    #[clap(long = "copy", multiple_occurrences = true)]
    pub(crate) copy: Vec<BindMount>,

    #[clap(long = "extra-layer", multiple_occurrences = true)]
    pub(crate) extra_layers: Vec<PathBuf>,

    pub(crate) image_reference: ImageReference,

    pub(crate) entry_point: Option<String>,

    pub(crate) entry_point_args: Vec<String>,
}
