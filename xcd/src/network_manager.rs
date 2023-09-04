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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use xc::container::request::NetworkAllocRequest;
use xc::models::network::IpAssign;

use crate::network::{Network, AddressStore};
use crate::database::Database;
use crate::config::config_manager::InventoryManager;

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
    fn new(db: &impl AddressStore, name: String, network: &Network) -> NetworkInfo {
        let netpool = network.parameterize(&name, db);
        NetworkInfo {
            name,
            subnet: netpool.network.subnet.clone(),
            start_addr: netpool.start_addr,
            end_addr: netpool.end_addr,
            last_addr: netpool.last_addr,
            alias_iface: network.alias_iface.clone(),
            bridge_iface: network.bridge_iface.clone(),
        }
    }
}

pub(crate) struct NetworkManager {
    db: Arc<Database>,
    inventory_manager: Arc<Mutex<InventoryManager>>,
    table_cache: HashMap<String, HashMap<String, Vec<IpAddr>>>,
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
    pub(crate) fn new(
        db: Arc<Database>,
        inventory_manager: Arc<Mutex<InventoryManager>>,
    ) -> NetworkManager {
        NetworkManager {
            db,
            inventory_manager,
            table_cache: HashMap::new(),
        }
    }

    pub(crate) fn has_network(&self, name: &str) -> bool {
        self.inventory_manager
            .lock()
            .unwrap()
            .borrow()
            .networks
            .contains_key(name)
    }

    pub(crate) fn get_network_info(&self) -> Result<Vec<NetworkInfo>, anyhow::Error> {
        let config = self.inventory_manager.lock().unwrap().borrow().clone();
        let mut info = Vec::new();
        for (name, network) in config.networks.iter() {
            let db: &Database = &self.db;
            info.push(NetworkInfo::new(db, name.to_string(), network));
        }
        Ok(info)
    }

    pub(crate) fn create_network(
        &self,
        name: &str,
        network: &Network,
    ) -> Result<(), rusqlite::Error> {
        self.inventory_manager.lock().unwrap().modify(|inventory| {
            inventory.networks.insert(name.to_string(), network.clone());
        });
        Ok(())
    }

    pub(crate) fn release_addresses(
        &mut self,
        token: &str,
    ) -> anyhow::Result<HashMap<String, Vec<IpAddr>>> {
        self.db.release_addresses(token).context("fail to release addresses")?;
        let networks = self.table_cache.remove(token).unwrap_or_default();
        Ok(networks)
    }

    pub(crate) fn get_allocated_addresses(
        &self,
        token: &str,
    ) -> Option<&HashMap<String, Vec<IpAddr>>> {
        self.table_cache.get(token)
    }

    /// Request allociation using `req`, and return (an_assignment, default_router)
    pub(crate) fn allocate(
        &mut self,
        vnet: bool,
        req: &NetworkAllocRequest,
        token: &str,
    ) -> Result<(IpAssign, Option<IpAddr>), Error> {
        let network_name = req.network();
        let config = self.inventory_manager.lock().unwrap().borrow().clone();
        let network = config
            .networks
            .get(&network_name)
            .ok_or_else(|| Error::NoSuchNetwork(network_name.to_string()))?
            .clone();

        let interface = if vnet {
            network.bridge_iface.to_string()
        } else {
            network.alias_iface.to_string()
        };

        let db: &Database = &self.db;

        let mut netpool = network.parameterize(&network_name, db);

        match req {
            NetworkAllocRequest::Any { .. } => {
                let Some(address) = netpool.next_cidr(token)? else {
                    return Err(Error::AllocationFailure(network_name));
                };

                self.insert_to_cache(token, &network_name, &address.addr());

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
                let address = IpCidr::from_addr(*ip, netpool.network.subnet.mask()).unwrap();
                if netpool.network.subnet.network_addr() == address.network_addr() {
                    if netpool.is_address_consumed(ip)? {
                        return Err(Error::AddressUsed(*ip));
                    }
                    netpool.register_address(ip, token)?;
                    self.insert_to_cache(token, &network_name, ip);

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

    #[inline]
    fn insert_to_cache(&mut self, token: &str, network: &str, address: &IpAddr) {
        if let Some(network_address) = self.table_cache.get_mut(token) {
            if let Some(addresses) = network_address.get_mut(network) {
                addresses.push(*address);
            } else {
                network_address.insert(network.to_string(), vec![*address]);
            }
        } else {
            let mut hmap = HashMap::new();
            hmap.insert(network.to_string(), vec![*address]);
            self.table_cache.insert(token.to_string(), hmap);
        }
    }
}
