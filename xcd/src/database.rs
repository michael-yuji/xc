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
use std::sync::Mutex;
use std::net::IpAddr;
use rusqlite::{Connection, Params, Row, OptionalExtension};

use crate::network::AddressStore;

pub struct Database {
    db: Mutex<Connection>,
}

impl From<Connection> for Database {
    fn from(db: Connection) -> Database {
        Database { db: Mutex::new(db) }
    }
}

impl Database {
    pub fn perform<F, T>(&self, func: F) -> T
        where F: FnOnce(&Connection) -> T
    {
        let conn = self.db.lock().unwrap();
        func(&conn)
    }

    pub fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        let conn = self.db.lock().unwrap();
        conn.execute(sql, params)
    }

    pub fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
        where
            P: Params,
            F: FnOnce(&Row<'_>) -> rusqlite::Result<T>
    {
        let conn = self.db.lock().unwrap();
        conn.query_row(sql, params, f)
    }
}

impl AddressStore for Database {
    fn all_allocated_addresses(&self, network: &str) -> rusqlite::Result<Vec<IpAddr>> {
        self.perform(|conn| {
            let mut stmt = conn.prepare("select address from address_allocation where network=?")?;
            let rows = stmt.query_map([network], |row| {
                let column: String = row.get(0)?;
                let ipaddr: IpAddr = column.parse().unwrap();
                Ok(ipaddr)
            })?;

            Ok(rows.map(|row| row.unwrap()).collect())
        })
    }

    fn release_addresses(&self, token: &str) -> rusqlite::Result<()> {
        self.execute("delete from address_allocation where token=?", [token])?;
        Ok(())
    }
    fn last_allocated_adddress(&self, network: &str) -> rusqlite::Result<Option<IpAddr>> {
        self.query_row(
            "select addr from network_last_addrs where network=?",
            [network], |row| {
                let addr_text: String = row.get(0)?;
                let addr = addr_text.parse().unwrap();
                Ok(addr)
            }).optional()
    }
    fn is_address_allocated(&self, network: &str, addr: &IpAddr) -> rusqlite::Result<bool> {
        self.perform(|conn| {
            let address = addr.to_string();
            let count: usize = conn.query_row(
                "select count(*) from address_allocation where network=? and address=?",
                [network, address.as_str()],
                |row| row.get(0),
            )?;
            Ok(count != 0)
        })
    }
    fn add_address(&self, network: &str, addr: &IpAddr, token: &str) -> rusqlite::Result<()> {
        self.perform(|conn| {
            let address = addr.to_string();
            conn.execute(
                "insert into address_allocation (network, token, address) values (?, ?, ?)",
                [network.to_string(), token.to_string(), address],
            )?;
            Ok(())
        })
    }
    fn tag_last_addr(&self, network: &str, addr: &IpAddr) -> rusqlite::Result<()> {
        let addr = addr.to_string();
        self.execute(
            "
            insert into network_last_addrs (network, addr) values (?, ?)
                on conflict (network) do update set addr=?
            ", [network, addr.as_str(), addr.as_str()])?;
        Ok(())
    }
}
