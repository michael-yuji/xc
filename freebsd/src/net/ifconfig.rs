//! Helper utilities to work with (IFCONFIG_CMD)

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

use command_macros::cmd;
use ipcidr::*;
use std::process::Command;
use thiserror::Error;

pub const IFCONFIG_CMD: &str = crate::env_or_default!("XC_IFCONFIG_CMD", "/sbin/ifconfig");

trait CmdExt {
    fn collect_stdout_str(&mut self) -> Result<String, std::io::Error>;
}

impl CmdExt for Command {
    fn collect_stdout_str(&mut self) -> Result<String, std::io::Error> {
        let output = self.output()?;
        Ok(std::str::from_utf8(&output.stdout)
            .expect("non-utf8 output")
            .trim()
            .to_string())
    }
}

#[derive(Error, Debug)]
pub enum IfconfigError {
    #[error("interface already exists: {0}")]
    InterfaceAlreadyExists(String),
    #[error("interface does not exist: {0}")]
    InterfaceDoesNotExist(String),
    #[error("{0}")]
    RunError(std::io::Error),
    #[error("{0}")]
    CliError(String),
}

fn has_whitespace(s: &str) -> bool {
    for char in s.chars() {
        if char.is_whitespace() {
            return true;
        }
    }
    false
}

pub fn interfaces() -> Result<Vec<String>, std::io::Error> {
    let string = cmd!((IFCONFIG_CMD)("-l")).collect_stdout_str()?;
    let result = string
        .split(' ')
        .filter_map(|value| {
            if !has_whitespace(value) {
                Some(value.to_string())
            } else {
                None
            }
        })
        .collect();
    Ok(result)
}

pub fn destroy_interface(interface: String) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD) (interface) destroy).status()?;
    Ok(())
}

pub fn create_alias(interface: &str, addr: &IpCidr) -> Result<(), std::io::Error> {
    match addr {
        IpCidr::V4(cidr) => {
            cmd!((IFCONFIG_CMD) (interface) inet (format!("{cidr}")) alias).status()?;
            Ok(())
        }
        IpCidr::V6(cidr) => {
            cmd!((IFCONFIG_CMD) (interface) inet6 (format!("{cidr}")) alias).status()?;
            Ok(())
        }
    }
}

pub fn interface_up(interface: &str) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD) (interface) up).status()?;
    Ok(())
}

pub fn interface_down(interface: &str) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD) (interface) down).status()?;
    Ok(())
}

pub fn remove_alias(interface: &str, addr: &IpCidr) -> Result<(), std::io::Error> {
    match addr {
        IpCidr::V4(cidr) => {
            cmd!((IFCONFIG_CMD) (interface) inet (format!("{cidr}")) delete).status()?;
            Ok(())
        }
        IpCidr::V6(cidr) => {
            cmd!((IFCONFIG_CMD) (interface) inet6 (format!("{cidr}")) delete).status()?;
            Ok(())
        }
    }
}

pub fn rename_no_check<A: AsRef<str>, B: AsRef<str>>(
    orig: A,
    new: B,
) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD) (orig.as_ref()) name (new.as_ref())).status()?;
    Ok(())
}

pub fn create_epair() -> Result<(String, String), IfconfigError> {
    let iface = cmd!((IFCONFIG_CMD) epair create)
        .collect_stdout_str()
        .map_err(IfconfigError::RunError)?;
    let epair_base = iface[..(iface.len() - 1)].to_string();
    Ok((iface, format!("{epair_base}b")))
}

pub fn create_epair_with_name<A: AsRef<str>, B: AsRef<str>>(
    a_name: A,
    b_name: B,
) -> Result<(), IfconfigError> {
    let interfaces = interfaces().map_err(IfconfigError::RunError)?;
    if interfaces.contains(&a_name.as_ref().to_string()) {
        return Err(IfconfigError::InterfaceAlreadyExists(
            a_name.as_ref().to_string(),
        ));
    }
    if interfaces.contains(&b_name.as_ref().to_string()) {
        return Err(IfconfigError::InterfaceAlreadyExists(
            a_name.as_ref().to_string(),
        ));
    }

    let (epair_a, epair_b) = create_epair()?;

    rename_no_check(epair_a, a_name).map_err(IfconfigError::RunError)?;
    rename_no_check(epair_b, b_name).map_err(IfconfigError::RunError)?;

    Ok(())
}

pub fn remove_from_jail_cmd<A: AsRef<str>>(
    interface: A,
    jid: i32,
) -> Result<Command, std::io::Error> {
    Ok(cmd!((IFCONFIG_CMD)(interface.as_ref())("-vnet")(
        jid.to_string()
    )))
}

pub fn remove_from_jail<A: AsRef<str>>(interface: A, jid: i32) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD)(interface.as_ref())("-vnet")(jid.to_string())).status()?;
    Ok(())
}

pub fn move_to_jail<A: AsRef<str>>(interface: A, jid: i32) -> Result<(), std::io::Error> {
    cmd!((IFCONFIG_CMD)(interface.as_ref())("vnet")(jid.to_string())).status()?;
    Ok(())
}

pub fn add_to_bridge<A: AsRef<str>, B: AsRef<str>>(
    bridge: B,
    interface: A,
) -> Result<(), IfconfigError> {
    cmd!((IFCONFIG_CMD) (bridge.as_ref()) addm (interface.as_ref()))
        .status()
        .map_err(IfconfigError::RunError)?;
    Ok(())
}

pub fn remove_from_bridge<A: AsRef<str>, B: AsRef<str>>(
    bridge: B,
    interface: A,
) -> Result<(), IfconfigError> {
    if !interfaces()
        .map_err(IfconfigError::RunError)?
        .contains(&interface.as_ref().to_string())
    {
        return Err(IfconfigError::InterfaceDoesNotExist(
            interface.as_ref().to_string(),
        ));
    }
    if !interfaces()
        .map_err(IfconfigError::RunError)?
        .contains(&bridge.as_ref().to_string())
    {
        return Err(IfconfigError::InterfaceDoesNotExist(
            interface.as_ref().to_string(),
        ));
    }
    cmd!((IFCONFIG_CMD) (bridge.as_ref()) deletem (interface.as_ref()))
        .status()
        .map_err(IfconfigError::RunError)?;
    Ok(())
}

pub fn create_tap() -> Result<String, IfconfigError> {
    let output = cmd!((IFCONFIG_CMD) tap create)
        .output()
        .map_err(IfconfigError::RunError)?;
    if output.status.success() {
        Ok(std::str::from_utf8(&output.stdout).unwrap().trim_end().to_string())
    } else {
        Err(IfconfigError::CliError(std::str::from_utf8(&output.stderr).unwrap().trim_end().to_string()))
    }
}

pub fn create_tun<A: AsRef<str>>() -> Result<String, IfconfigError> {
    let output = cmd!((IFCONFIG_CMD) tap create)
        .output()
        .map_err(IfconfigError::RunError)?;
    if output.status.success() {
        Ok(std::str::from_utf8(&output.stdout).unwrap().trim_end().to_string())
    } else {
        Err(IfconfigError::CliError(std::str::from_utf8(&output.stderr).unwrap().trim_end().to_string()))
    }
}
