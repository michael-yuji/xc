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
pub mod devfs;
pub mod network;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub fn sha256_hex<B: AsRef<[u8]>>(input: B) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&input);
    let digest: [u8; 32] = hasher.finalize().into();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

pub fn sha256_hex_file<P: AsRef<Path>>(path: P) -> Result<String, anyhow::Error> {
    let stat = std::fs::metadata(path.as_ref())?;
    let mut file = std::fs::OpenOptions::new().read(true).open(path.as_ref())?;
    let mut buf = [0u8; 4096];
    let mut hasher = Sha256::new();
    let mut remaining = stat.len() as usize;
    while remaining > 0 {
        let nread = file.read(&mut buf)?;
        hasher.update(&buf[..(4096.min(nread))]);
        remaining -= nread;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(""))
}

pub fn create_tables(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "

        create table if not exists datasets (
            id text not null primary key,
            digest_chain text not null
        );

        create unique index if not exists digest_chain_index on datasets(digest_chain);

        create table if not exists drafts (
            name       text not null,
            manifest   blob not null,
            base_image text,
            primary key (name)
        );

        create table if not exists image_manifests (
            manifest text not null,
            digest text not null primary key
        );

        create table if not exists image_manifest_tags (
            name text not null,
            tag text not null,
            digest text not null,
            primary key (name, tag)
        );

        create table if not exists netpool (
            network text not null primary key,
            subnet text not null,
            start_addr text not null,
            end_addr text not null,
            last_addr text
        );

        create table if not exists address_allocation (
            network text not null,
            token   text not null,
            address text not null primary key
        );

        create index if not exists address_alloc_network_index on address_allocation(network);
        create index if not exists address_token_index on address_allocation(address);
    ",
    )?;

    Ok(())
}
