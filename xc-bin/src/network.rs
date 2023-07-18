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
use clap::Subcommand;
use std::net::IpAddr;
use std::os::unix::net::UnixStream;
use xcd::ipc::*;

#[derive(Subcommand, Debug)]
pub(crate) enum NetworkAction {
    Create {
        name: String,
        subnet: ipcidr::IpCidr,
        start_addr: Option<IpAddr>,
        end_addr: Option<IpAddr>,
        #[clap(long = "bridge")]
        bridge_iface: String,
        #[clap(long = "alias")]
        alias_iface: String,
        #[clap(long = "default-router")]
        default_router: Option<IpAddr>,
    },
    List,
    Tag {
        #[clap(long = "no-commit", action)]
        no_commit: bool,
        network: String,
        container: String,
    },
    CommitTag {
        network: String,
    },
}

pub(crate) fn use_network_action(
    conn: &mut UnixStream,
    action: NetworkAction,
) -> Result<(), crate::ActionError> {
    match action {
        NetworkAction::Tag {
            no_commit,
            network,
            container,
        } => {
            let request = NetgroupAddContainerRequest {
                netgroup_name: network,
                container_name: container,
                auto_create_netgroup: true,
                commit_immediately: !no_commit,
            };
            do_add_container_to_netgroup(conn, request)?;
        }
        NetworkAction::CommitTag { network } => {
            let request = NetgroupCommit {
                netgroup_name: network.to_string(),
            };
            do_commit_netgroup(conn, request)?;
        }
        NetworkAction::List => {
            let req = ListNetworkRequest {};
            let res = do_list_networks(conn, req)?;
            eprintln!("{res:#?}");
        }
        NetworkAction::Create {
            name,
            subnet,
            start_addr,
            end_addr,
            bridge_iface,
            alias_iface,
            default_router,
        } => {
            let req = CreateNetworkRequest {
                name,
                subnet,
                start_addr,
                end_addr,
                bridge_iface,
                alias_iface,
                default_router,
            };
            let res = do_create_network(conn, req)?;
            eprintln!("{res:#?}");
        }
    }
    Ok(())
}
