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
use anyhow::Context;
use ipcidr::IpCidr;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use thiserror::Error;
use tokio::sync::watch::Receiver;
use xc::config::XcConfig;
use xc::container::request::NetworkAllocRequest;
use xc::models::network::IpAssign;
use xc::res::network::{Netpool, Network};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub name: String,
    pub subnet: IpCidr,
    pub start_addr: IpAddr,
    pub end_addr: IpAddr,
    pub last_addr: Option<IpAddr>,
    pub alias_iface: String,
    pub bridge_iface: String,
}

impl NetworkInfo {
    pub fn new(network: &Network, netpool: &Netpool) -> NetworkInfo {
        NetworkInfo {
            name: netpool.network.clone(),
            subnet: netpool.subnet.clone(),
            start_addr: netpool.start_addr,
            end_addr: netpool.end_addr,
            last_addr: netpool.last_addr,
            alias_iface: network.alias_iface.clone(),
            bridge_iface: network.bridge_iface.clone(),
        }
    }
}

pub(crate) struct NetworkManager {
    db: Connection,
    recv: Receiver<XcConfig>,
}

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(rusqlite::Error),
    #[error("cannot allocate address from network {0}")]
    AllocationFailure(String),
    #[error("address {0} is being used")]
    AddressUsed(std::net::IpAddr),
    #[error("address {0} and network {1} in different subnet")]
    InvalidAddress(IpAddr, String),
    #[error("no such network {0}")]
    NoSuchNetwork(String),
    #[error("no such network {0} in database")]
    NoSuchNetworkDatabase(String),
    #[error("{0}")]
    Other(anyhow::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Sqlite(error)
    }
}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Error {
        Error::Other(error)
    }
}

impl NetworkManager {
    pub(crate) fn new(db: Connection, recv: Receiver<XcConfig>) -> NetworkManager {
        NetworkManager { db, recv }
    }

    pub(crate) fn get_network_info(&self) -> Result<Vec<NetworkInfo>, anyhow::Error> {
        let config = self.recv.borrow().clone();
        let mut info = Vec::new();
        for (name, network) in config.networks.iter() {
            let netpool = Netpool::from_conn(&self.db, name.to_string())?.context("")?;
            info.push(NetworkInfo::new(network, &netpool))
        }
        Ok(info)
    }

    pub(crate) fn create_network(
        &self,
        name: &str,
        network: &Network,
    ) -> Result<(), rusqlite::Error> {
        _ = Netpool::create_or_load(&self.db, name, network)?;
        Ok(())
    }

    pub(crate) fn release_addresses(&self, token: &str) -> anyhow::Result<()> {
        Netpool::release_addresses(&self.db, token).context("fail on address release")?;
        Ok(())
    }
    pub(crate) fn allocate(
        &self,
        vnet: bool,
        req: &NetworkAllocRequest,
        token: &str,
    ) -> Result<(IpAssign, Option<IpAddr>), Error> {
        let network_name = req.network();
        let config = self.recv.borrow().clone();
        let network = config
            .networks
            .get(&network_name)
            .ok_or_else(|| Error::NoSuchNetwork(network_name.to_string()))?
            .clone();

        let interface = if vnet {
            network.bridge_iface
        } else {
            network.alias_iface
        };

        let mut netpool = Netpool::from_conn(&self.db, network_name.to_string())?
            .ok_or_else(|| Error::NoSuchNetworkDatabase(network_name.to_string()))?;

        match req {
            NetworkAllocRequest::Any { .. } => {
                let Some(address) = netpool.next_cidr(&self.db, token)? else {
                    return Err(Error::AllocationFailure(network_name))
                };

                Ok((
                    IpAssign {
                        network: Some(network_name),
                        interface,
                        addresses: vec![address],
                    },
                    network.default_router,
                ))
            }
            NetworkAllocRequest::Explicit { ip, .. } => {
                let address = IpCidr::from_addr(*ip, netpool.subnet.mask()).unwrap();
                if netpool.subnet.network_addr() == address.network_addr() {
                    if netpool.is_address_consumed(&self.db, ip)? {
                        return Err(Error::AddressUsed(*ip));
                    }
                    netpool.register_address(&self.db, ip, token)?;
                    Ok((
                        IpAssign {
                            network: Some(network_name),
                            interface,
                            addresses: vec![address],
                        },
                        network.default_router,
                    ))
                } else {
                    Err(Error::InvalidAddress(*ip, network_name))
                }
            }
        }
    }
}
