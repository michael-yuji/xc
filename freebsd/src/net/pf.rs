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
use std::io::Write;
use std::net::IpAddr;
use std::process::{Command, Stdio};

pub const PFCTL_CMD: &str = crate::env_or_default!("XC_PFCTL_CMD", "/sbin/pfctl");

pub fn is_pf_enabled() -> Result<bool, std::io::Error> {
    Command::new(PFCTL_CMD)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .arg("-s")
        .arg("Running")
        .status()
        .map(|status| status.success())
}

pub fn get_filter_rules(anchor: Option<String>) -> Result<Vec<String>, std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }
    let output = cmd.arg("-Psr").output()?;
    Ok(std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>())
}

pub fn get_nat_rules(anchor: Option<String>) -> Result<Vec<String>, std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }
    let output = cmd.arg("-Psn").output()?;
    Ok(std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>())
}

pub fn set_rules(anchor: Option<String>, rules: &[impl AsRef<str>]) -> Result<(), std::io::Error> {
    let mut rules = rules
        .iter()
        .map(|s| s.as_ref())
        .collect::<Vec<_>>()
        .join("\n");
    rules.push('\n');
    set_rules_unchecked(anchor, rules)
}

pub fn set_rules_unchecked<S: AsRef<str>>(
    anchor: Option<String>,
    rules: S,
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    cmd.stdin(std::process::Stdio::piped());
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }
    let mut child = cmd.arg("-f-").spawn()?;
    let rules_str = rules.as_ref();
    let bytes = rules_str.as_bytes();
    child.stdin.as_mut().unwrap().write_all(bytes)?;
    child.wait().map(|_| ())
}

pub fn table_list_address<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
) -> Result<Vec<String>, std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    let output = cmd
        .arg("-t")
        .arg(table.as_ref())
        .arg("-T")
        .arg("show")
        .output()?;

    let stdout = std::str::from_utf8(&output.stdout).unwrap();

    Ok(stdout
        .lines()
        .map(|line| line.trim().to_string())
        .collect::<Vec<_>>())
}

pub fn table_flush<S: AsRef<str>>(anchor: Option<String>, table: S) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t")
        .arg(table.as_ref())
        .arg("-T")
        .arg("flush")
        .status()
        .map(|_| ())
}

pub fn table_del_addresses(
    anchor: Option<String>,
    table: &str,
    addresses: &[IpAddr],
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t").arg(table).arg("-T").arg("delete");

    for address in addresses {
        cmd.arg(address.to_string());
    }

    cmd.status().map(|_| ())
}

pub fn table_del_cidrs<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
    addresses: &[IpCidr],
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t").arg(table.as_ref()).arg("-T").arg("delete");

    for address in addresses {
        cmd.arg(address.to_string());
    }

    cmd.status().map(|_| ())
}

pub fn table_del_address<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
    address: &IpCidr,
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t")
        .arg(table.as_ref())
        .arg("-T")
        .arg("delete")
        .arg(address.to_string())
        .status()
        .map(|_| ())
}

pub fn table_add_addresses<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
    addresses: &[IpAddr],
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }
    cmd.arg("-t").arg(table.as_ref()).arg("-T").arg("add");
    for address in addresses {
        cmd.arg(address.to_string());
    }
    cmd.status().map(|_| ())
}

pub fn table_add_cidrs<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
    addresses: &[IpCidr],
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t").arg(table.as_ref()).arg("-T").arg("add");

    for address in addresses {
        cmd.arg(address.to_string());
    }

    cmd.status().map(|_| ())
}

pub fn table_add_address<S: AsRef<str>>(
    anchor: Option<String>,
    table: S,
    address: &IpCidr,
) -> Result<(), std::io::Error> {
    let mut cmd = Command::new(PFCTL_CMD);
    if let Some(anchor) = anchor {
        cmd.arg("-a").arg(anchor);
    }

    cmd.arg("-t")
        .arg(table.as_ref())
        .arg("-T")
        .arg("add")
        .arg(address.to_string())
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_delete_addresses_to_table() {
        let anchor = Some("rust/test_pf".to_string());
        let table = "mytable";

        let addresses = vec![
            IpCidr::V4("192.168.110.1".parse().unwrap()),
            IpCidr::V4("192.168.110.2".parse().unwrap()),
        ];

        table_add_cidrs(anchor.clone(), table, &addresses).expect("failed to add addresses");
        let a = table_list_address(Some("rust/test_pf".to_string()), "mytable").unwrap();

        eprintln!("a: {a:#?}");

        assert!(a.contains(&"192.168.110.1".to_string()));
        assert!(a.contains(&"192.168.110.2".to_string()));

        table_del_addresses(anchor, table, &addresses).expect("failed to delete addresses");
        let a = table_list_address(Some("rust/test_pf".to_string()), "mytable").unwrap();

        assert!(a.is_empty());
    }
}
