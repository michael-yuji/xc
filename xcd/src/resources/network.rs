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
use thiserror::Error;
use xc::container::request::NetworkAllocRequest;
use xc::models::network::IpAssign;

use crate::database::Database;
use crate::resources::Resources;

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub ext_if: Option<String>,
    pub alias_iface: String,
    pub bridge_iface: String,
    pub subnet: IpCidr,
    pub start_addr: Option<IpAddr>,
    pub end_addr: Option<IpAddr>,
    pub default_router: Option<IpAddr>,
}

impl Network {
    pub fn parameterize<'a, 'b, A: AddressStore>(
        &'b self,
        name: &str,
        store: &'a A,
    ) -> Netpool<'a, 'b, A> {
        let last_addr = store.last_allocated_adddress(name).unwrap();
        let name = name.to_string();
        Netpool {
            network: self,
            store,
            last_addr,
            start_addr: self
                .start_addr
                .unwrap_or_else(|| self.subnet.network_addr()),
            end_addr: self
                .end_addr
                .unwrap_or_else(|| self.subnet.broadcast_addr()),
            name,
        }
    }
}

pub trait AddressStore {
    fn all_allocated_addresses(&self, network: &str) -> rusqlite::Result<Vec<IpAddr>>;
    fn release_addresses(&self, token: &str) -> rusqlite::Result<()>;
    fn last_allocated_adddress(&self, network: &str) -> rusqlite::Result<Option<IpAddr>>;
    fn is_address_allocated(&self, network: &str, addr: &IpAddr) -> rusqlite::Result<bool>;
    fn add_address(&self, network: &str, addr: &IpAddr, token: &str) -> rusqlite::Result<()>;
    fn tag_last_addr(&self, network: &str, addre: &IpAddr) -> rusqlite::Result<()>;
}

pub struct Netpool<'a, 'b, A: AddressStore> {
    store: &'a A,
    pub network: &'b Network,
    pub start_addr: IpAddr,
    pub end_addr: IpAddr,
    pub last_addr: Option<IpAddr>,
    name: String,
}

#[allow(dead_code)]
impl<'a, 'b, A: AddressStore> Netpool<'a, 'b, A> {
    pub fn all_allocated_addresses(&self) -> rusqlite::Result<Vec<IpAddr>> {
        self.store.all_allocated_addresses(&self.name)
    }

    pub fn next_cidr(&mut self, token: &str) -> rusqlite::Result<Option<IpCidr>> {
        self.next_address(token)
            .map(|x| x.and_then(|a| IpCidr::from_addr(a, self.network.subnet.mask())))
    }
    pub fn next_address(&mut self, token: &str) -> rusqlite::Result<Option<IpAddr>> {
        macro_rules! next_addr {
            ($raw:ty, $ipv:ident, $start:expr, $end:expr) => {{
                let start = <$raw>::from($start);
                let end = <$raw>::from($end);

                let count = end - start + 1;
                let last_offset = match self.last_addr {
                    None => 0,
                    Some(IpAddr::$ipv(addr)) => <$raw>::from(addr) - start,
                    _ => unreachable!(),
                };

                let mut offset = last_offset;

                loop {
                    let addr = IpAddr::$ipv((start + offset).into());
                    if addr != self.network.subnet.network_addr()
                        && addr != self.network.subnet.broadcast_addr()
                        && !self.store.is_address_allocated(&self.name, &addr)?
                    {
                        self.last_addr = Some(addr);
                        self.store.tag_last_addr(&self.name, &addr)?;
                        self.store.add_address(&self.name, &addr, token)?;
                        return Ok(Some(addr));
                    }
                    offset = (offset + 1) % count;

                    if offset == last_offset {
                        break;
                    }
                }

                Ok(None)
            }};
        }

        match (self.start_addr, self.end_addr) {
            (IpAddr::V4(start), IpAddr::V4(end)) => next_addr!(u32, V4, start, end),
            (IpAddr::V6(start), IpAddr::V6(end)) => next_addr!(u128, V6, start, end),
            _ => unreachable!(),
        }
    }

    pub fn release_addresses(&self, token: &str) -> rusqlite::Result<()> {
        self.store.release_addresses(token)
    }

    pub fn is_address_consumed(&self, addr: &IpAddr) -> rusqlite::Result<bool> {
        self.store.is_address_allocated(&self.name, addr)
    }

    pub fn register_address(&self, addr: &IpAddr, token: &str) -> rusqlite::Result<()> {
        self.store.add_address(&self.name, addr, token)
    }
}

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

impl Resources {
    pub(crate) fn has_network(&self, name: &str) -> bool {
        self.inventory_manager.borrow().networks.contains_key(name)
    }

    pub(crate) fn get_network_info(&self) -> Result<Vec<NetworkInfo>, anyhow::Error> {
        let config = self.inventory_manager.borrow().clone();
        let mut info = Vec::new();
        for (name, network) in config.networks.iter() {
            let db: &Database = &self.db;
            info.push(NetworkInfo::new(db, name.to_string(), network));
        }
        Ok(info)
    }

    pub(crate) fn create_network(
        &mut self,
        name: &str,
        network: &Network,
    ) -> Result<(), rusqlite::Error> {
        self.inventory_manager.modify(|inventory| {
            inventory.networks.insert(name.to_string(), network.clone());
        });
        Ok(())
    }

    pub(crate) fn release_addresses(
        &mut self,
        token: &str,
    ) -> anyhow::Result<HashMap<String, Vec<IpAddr>>> {
        self.db
            .release_addresses(token)
            .context("fail to release addresses")?;
        let networks = self.net_addr_alloc_cache.remove(token).unwrap_or_default();
        Ok(networks)
    }

    pub(crate) fn get_allocated_addresses(
        &self,
        token: &str,
    ) -> Option<&HashMap<String, Vec<IpAddr>>> {
        self.net_addr_alloc_cache.get(token)
    }

    /// Request allociation using `req`, and return (an_assignment, default_router)
    pub(crate) fn allocate(
        &mut self,
        vnet: bool,
        req: &NetworkAllocRequest,
        token: &str,
    ) -> Result<(IpAssign, Option<IpAddr>), Error> {
        let network_name = req.network();
        let config = self.inventory_manager.borrow().clone();
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
        if let Some(network_address) = self.net_addr_alloc_cache.get_mut(token) {
            if let Some(addresses) = network_address.get_mut(network) {
                addresses.push(*address);
            } else {
                network_address.insert(network.to_string(), vec![*address]);
            }
        } else {
            let mut hmap = HashMap::new();
            hmap.insert(network.to_string(), vec![*address]);
            self.net_addr_alloc_cache.insert(token.to_string(), hmap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use rusqlite::Connection;
    use xc::res::create_tables;

    #[test]
    fn test_assignment() -> rusqlite::Result<()> {
        let network = Network {
            ext_if: None,
            default_router: None,
            alias_iface: "jeth0".to_string(),
            bridge_iface: "jeth0".to_string(),
            subnet: "192.168.2.0/24".parse().unwrap(),
            start_addr: Some("192.168.2.5".parse().unwrap()),
            end_addr: None,
        };

        let conn = Connection::open_in_memory()?;

        create_tables(&conn)?;

        let db = Database::from(conn);

        let mut pool = network.parameterize("testtest", &db);

        let addr1 = pool.next_address("1")?.unwrap();
        let addr2 = pool.next_address("1")?.unwrap();
        let addr3 = pool.next_address("3")?.unwrap();
        let addr4 = pool.next_address("4")?.unwrap();
        let addr5 = pool.next_address("1")?.unwrap();

        assert_eq!(addr1, IpAddr::V4("192.168.2.5".parse().unwrap()));
        assert_eq!(addr2, IpAddr::V4("192.168.2.6".parse().unwrap()));
        assert_eq!(addr3, IpAddr::V4("192.168.2.7".parse().unwrap()));
        assert_eq!(addr4, IpAddr::V4("192.168.2.8".parse().unwrap()));
        assert_eq!(addr5, IpAddr::V4("192.168.2.9".parse().unwrap()));

        pool.release_addresses("1")?;

        let remaining_addresses = pool.all_allocated_addresses()?;
        assert!(remaining_addresses.contains(&addr3));
        assert!(remaining_addresses.contains(&addr4));

        assert_eq!(remaining_addresses.len(), 2);

        Ok(())
    }
}
