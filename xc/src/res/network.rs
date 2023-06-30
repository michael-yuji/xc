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
use ipcidr::IpCidr;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

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

#[derive(Serialize, Deserialize, Debug)]
pub struct Netpool {
    pub network: String,
    pub subnet: IpCidr,
    pub start_addr: IpAddr,
    pub end_addr: IpAddr,
    pub last_addr: Option<IpAddr>,
}

impl Netpool {
    pub fn release_addresses(conn: &Connection, token: &str) -> rusqlite::Result<()> {
        conn.execute("delete from address_allocation where token=?", [token])?;
        Ok(())
    }

    pub fn from_network(name: &str, config: &Network) -> Netpool {
        Netpool {
            network: name.to_string(),
            subnet: config.subnet.clone(),
            start_addr: config
                .start_addr
                .unwrap_or_else(|| config.subnet.network_addr()),
            end_addr: config
                .end_addr
                .unwrap_or_else(|| config.subnet.broadcast_addr()),
            last_addr: None,
        }
    }

    pub fn create_or_load(
        conn: &Connection,
        name: &str,
        config: &Network,
    ) -> rusqlite::Result<Netpool> {
        let pool = Netpool::from_network(name, config);
        match Netpool::from_conn(conn, name.to_string())? {
            Some(netpool) => {
                if netpool.subnet != pool.subnet
                    || netpool.start_addr != pool.start_addr
                    || netpool.end_addr != pool.end_addr
                {
                    conn.execute(
                        "
                        update netpool
                            set subnet=?, start_addr=?, end_addr=?
                            where network=?
                        ",
                        [
                            &pool.subnet.to_string(),
                            &pool.start_addr.to_string(),
                            &pool.end_addr.to_string(),
                            &pool.network,
                        ],
                    )?;
                    Ok(pool)
                } else {
                    Ok(netpool)
                }
            }
            None => {
                let netpool = pool;
                conn.execute(
                    "
                    insert into netpool 
                        (network, subnet, start_addr, end_addr) 
                    values
                        (?, ?, ?, ?)",
                    [
                        &netpool.network,
                        &netpool.subnet.to_string(),
                        &netpool.start_addr.to_string(),
                        &netpool.end_addr.to_string(),
                    ],
                )?;
                Ok(netpool)
            }
        }
    }

    pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Netpool>> {
        let mut stmt =
            conn.prepare("select network, subnet, start_addr, end_addr, last_addr from netpool")?;
        let mut pools = Vec::new();
        let netpool_iter = stmt.query_map([], |row| {
            let network = row.get(0)?;
            let subnet_text: String = row.get(1)?;
            let startaddr_text: String = row.get(2)?;
            let endaddr_text: String = row.get(3)?;
            let lastaddr_text: Option<String> = row.get(4)?;

            let subnet = subnet_text.parse().unwrap();
            let start_addr = startaddr_text.parse().unwrap();
            let end_addr = endaddr_text.parse().unwrap();
            let last_addr = lastaddr_text.map(|s| s.parse().unwrap());

            Ok(Netpool {
                network,
                subnet,
                start_addr,
                end_addr,
                last_addr,
            })
        })?;

        for pool in netpool_iter {
            pools.push(pool.unwrap());
        }

        Ok(pools)
    }

    pub fn from_conn(
        connection: &Connection,
        network: String,
    ) -> rusqlite::Result<Option<Netpool>> {
        //        connection.query_row(sql, params, f)
        connection
            .query_row(
                "select network, subnet, start_addr, end_addr, last_addr from netpool where network=?",
                [network],
                |row| {
                    let network = row.get(0)?;
                    let subnet_text: String = row.get(1)?;
                    let startaddr_text: String = row.get(2)?;
                    let endaddr_text: String = row.get(3)?;
                    let lastaddr_text: Option<String> = row.get(4)?;

                    let subnet = subnet_text.parse().unwrap();
                    let start_addr = startaddr_text.parse().unwrap();
                    let end_addr = endaddr_text.parse().unwrap();
                    let last_addr = lastaddr_text.map(|s| s.parse().unwrap());

                    Ok(Netpool {
                        network,
                        subnet,
                        start_addr,
                        end_addr,
                        last_addr,
                    })
                },
            )
            .optional()
    }

    pub fn all_allocated_addresses(&self, conn: &Connection) -> rusqlite::Result<Vec<IpAddr>> {
        let mut stmt = conn.prepare("select address from address_allocation where network=?")?;
        let rows = stmt.query_map([self.network.to_string()], |row| {
            let column: String = row.get(0)?;
            let ipaddr: IpAddr = column.parse().unwrap();
            Ok(ipaddr)
        })?;

        Ok(rows.map(|row| row.unwrap()).collect())
    }

    pub fn is_address_consumed(&self, conn: &Connection, addr: &IpAddr) -> rusqlite::Result<bool> {
        let address = addr.to_string();
        let count: usize = conn.query_row(
            "select count(*) from address_allocation where network=? and address=?",
            [&self.network, &address],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }

    pub fn register_address(
        &self,
        conn: &Connection,
        addr: &IpAddr,
        token: &str,
    ) -> rusqlite::Result<()> {
        let address = addr.to_string();
        conn.execute(
            "update netpool set last_addr=? where network=?",
            [
                self.last_addr.map(|a| a.to_string()),
                Some(self.network.to_string()),
            ],
        )?;
        conn.execute(
            "insert into address_allocation (network, token, address) values (?, ?, ?)",
            [self.network.to_string(), token.to_string(), address],
        )?;
        Ok(())
    }

    pub fn next_cidr(
        &mut self,
        conn: &Connection,
        token: &str,
    ) -> rusqlite::Result<Option<IpCidr>> {
        self.next_address(conn, token)
            .map(|x| x.and_then(|a| IpCidr::from_addr(a, self.subnet.mask())))
    }

    pub fn next_address(
        &mut self,
        conn: &Connection,
        token: &str,
    ) -> rusqlite::Result<Option<IpAddr>> {
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
                    if addr != self.subnet.network_addr()
                        && addr != self.subnet.broadcast_addr()
                        && !self.is_address_consumed(conn, &addr)?
                    {
                        self.last_addr = Some(addr);
                        self.register_address(conn, &addr, token)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    #[test]
    fn test_assignment() -> rusqlite::Result<()> {
        let network = Network {
            ext_if: None,
            alias_iface: "jeth0".to_string(),
            bridge_iface: "jeth0".to_string(),
            subnet: "192.168.2.0/24".parse().unwrap(),
            start_addr: Some("192.168.2.5".parse().unwrap()),
            end_addr: None,
        };

        let conn = Connection::open_in_memory()?;

        crate::res::create_tables(&conn)?;

        let mut pool = Netpool::create_or_load(&conn, "testtest", &network)?;

        let addr1 = pool.next_address(&conn, "1")?.unwrap();
        let addr2 = pool.next_address(&conn, "1")?.unwrap();
        let addr3 = pool.next_address(&conn, "3")?.unwrap();
        let addr4 = pool.next_address(&conn, "4")?.unwrap();
        let addr5 = pool.next_address(&conn, "1")?.unwrap();

        assert_eq!(addr1, IpAddr::V4("192.168.2.5".parse().unwrap()));
        assert_eq!(addr2, IpAddr::V4("192.168.2.6".parse().unwrap()));
        assert_eq!(addr3, IpAddr::V4("192.168.2.7".parse().unwrap()));
        assert_eq!(addr4, IpAddr::V4("192.168.2.8".parse().unwrap()));
        assert_eq!(addr5, IpAddr::V4("192.168.2.9".parse().unwrap()));

        Netpool::release_addresses(&conn, "1")?;

        let remaining_addresses = pool.all_allocated_addresses(&conn)?;
        assert!(remaining_addresses.contains(&addr3));
        assert!(remaining_addresses.contains(&addr4));

        assert_eq!(remaining_addresses.len(), 2);

        Ok(())
    }
}
