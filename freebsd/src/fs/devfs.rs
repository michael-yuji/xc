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
use super::{mount, MountError};

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const DEVFS_CMD: &str = crate::env_or_default!("XC_DEVFS_CMD", "/sbin/devfs");

pub fn mount_devfs<P: AsRef<Path>>(ruleset: u16, mountpoint: P) -> Result<(), MountError> {
    mount("devfs", "devfs", mountpoint, [format!("ruleset={ruleset}")])
}

pub fn devfs_list_ruleset_ids() -> Result<Vec<u16>, std::io::Error> {
    let output = Command::new(DEVFS_CMD)
        .arg("rule")
        .arg("showsets")
        .output()?;
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    Ok(stdout
        .lines()
        .map(|line| line.parse::<u16>().unwrap())
        .collect())
}

pub fn devfs_del_ruleset(ruleset: u16) -> Result<(), std::io::Error> {
    Command::new(DEVFS_CMD)
        .arg("rule")
        .arg("-s")
        .arg(ruleset.to_string())
        .arg("delset")
        .status()
        .map(|_| ())
}

pub fn devfs_add_ruleset(ruleset: u16, rules: String) -> Result<u16, std::io::Error> {
    let mut child = Command::new(DEVFS_CMD)
        .stdin(Stdio::piped())
        .arg("rule")
        .arg("-s")
        .arg(ruleset.to_string())
        .arg("add")
        .arg("-")
        .spawn()?;

    let mut stdin = child.stdin.as_ref().unwrap();
    stdin.write_all(rules.as_bytes())?;

    child.wait()?;
    Ok(ruleset)
}
