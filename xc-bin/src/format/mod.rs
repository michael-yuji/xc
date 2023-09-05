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
pub(crate) mod dataset;

use ipcidr::IpCidr;
use pest::Parser;
use pest_derive::Parser;
use std::str::FromStr;
use xc::models::network::{IpAssign, NetProto, PortNum, PortRedirection};

#[derive(Parser)]
#[grammar_inline = r#"
portnum = { ASCII_DIGIT{1,5} }
portrange = { portnum ~ "-" ~ portnum }
ip4_comp = { ASCII_DIGIT{1,3} }
ip4_ish = { ip4_comp ~ "." ~ ip4_comp ~ "." ~ ip4_comp ~ "." ~ ip4_comp }
ip6_comp = { ASCII_HEX_DIGIT{0, 4} }
ip6_ish = { "[" ~ ip6_comp ~ (":" ~ ip6_comp){0, 7} ~ "]" }
ip = { (ip4_ish | ip6_ish) }
cidr = { ip ~ ("/" ~ ASCII_DIGIT{1, 3})? }
publish_proto = { "udp" | "tcp" }
publish_port =  { portrange | portnum }
publish_without_proto = {
	(ip ~ ("," ~ ip)* ~ ":" ~ (portrange | portnum) ~ ":" ~ portnum) |
    ((portrange | portnum) ~ ":" ~ portnum)
}
publish_with_proto = { publish_without_proto ~ ("/" ~ publish_proto ~ ("," ~ publish_proto)*)? }
iface = { ASCII_ALPHA ~ ASCII_ALPHANUMERIC* }
publish = {
    (iface ~ ("," ~ iface)* ~ "|")?
      ~ (
        (ip ~ ("," ~ ip)* ~ ":" ~ publish_port ~ ":" ~ portnum)
          | (publish_port ~ ":" ~ portnum)
    )
      ~ ("/" ~ publish_proto ~ ("," ~ publish_proto)*)?
  }
path_char = { (!(":" | "\\") ~ ANY) | "\\:" | "\\" }
path = { path_char* }
volume_mount = { path ~ ":" ~ path ~ EOI }
vnet = { iface ~ ("|" ~ cidr ~ ("," ~ cidr)*)? }
"#]
struct RuleParser;

#[derive(Debug, Clone)]
pub(crate) struct MaybeEnvPair {
    pub(crate) key: varutil::string_interpolation::Var,
    pub(crate) value: Option<String>,
}

impl FromStr for MaybeEnvPair {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once('=') {
            None => {
                let key = varutil::string_interpolation::Var::from_str(s)?;
                Ok(MaybeEnvPair { key, value: None })
            }
            Some((key, value)) => {
                let key = varutil::string_interpolation::Var::from_str(key)?;
                Ok(MaybeEnvPair {
                    key,
                    value: Some(value.to_string()),
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EnvPair {
    pub(crate) key: String,
    pub(crate) value: String,
}

impl FromStr for EnvPair {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (key, value) = s.split_once('=').unwrap();
        Ok(EnvPair {
            key: key.to_string(),
            value: value.to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IpWant(pub(crate) IpAssign);

impl std::str::FromStr for IpWant {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parsed = RuleParser::parse(Rule::vnet, input)?;
        let root = parsed.into_iter().next().unwrap();
        let mut tokens = root.into_inner();
        let mut addresses = Vec::new();
        let interface = tokens.next().unwrap().as_str().to_string();
        for token in tokens {
            addresses.push(token.as_str().parse()?);
        }
        Ok(IpWant(IpAssign {
            interface,
            addresses,
            network: None,
        }))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BindMount {
    pub(crate) source: String,
    pub(crate) destination: String,
}

impl std::str::FromStr for BindMount {
    type Err = std::io::Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parsed = RuleParser::parse(Rule::volume_mount, input).map_err(|err| {
            std::io::Error::new(std::io::ErrorKind::Other, err.to_string().as_str())
        })?;
        let root = parsed.into_iter().next().unwrap();
        let mut iinner = root.into_inner();
        let mut inner = iinner.next().unwrap();
        let source = inner.as_str().to_string();
        inner = iinner.next().unwrap();
        let destination = inner.as_str().to_string();
        Ok(BindMount {
            source,
            destination,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PublishSpec(PortRedirection);

impl PublishSpec {
    pub(crate) fn to_host_spec(&self) -> PortRedirection {
        self.0.clone()
    }
}

impl FromStr for PublishSpec {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parsed = RuleParser::parse(Rule::publish, input)?;
        let root = parsed.into_iter().next().unwrap();

        let mut ifaces = Vec::new();
        let mut addresses = Vec::new();
        let mut iinner = root.into_inner();
        let mut inner = iinner.next().unwrap();

        //.next().unwrap();
        let mut protos = Vec::new();
        // try parse interfaces
        {
            while inner.as_rule() == Rule::iface {
                ifaces.push(inner.as_str().to_string());
                inner = iinner.next().unwrap();
            }
        }

        // try parse source addresses
        {
            while inner.as_rule() == Rule::ip {
                addresses.push(inner.as_str().parse::<IpCidr>()?);
                inner = iinner.next().unwrap();
            }
        }

        // parse source port and dest port , must exists
        let source_port = inner.as_str().to_string();
        inner = iinner.next().unwrap();
        let dest_port = inner.as_str().to_string();

        // publish protocols may not exists
        for proto in iinner {
            protos.push(proto.as_str().to_string());
        }

        let proto = if protos.is_empty() {
            vec![NetProto::Tcp, NetProto::Udp]
        } else {
            protos
                .iter()
                .map(|proto| match proto.as_str() {
                    "tcp" => NetProto::Tcp,
                    "udp" => NetProto::Udp,
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>()
        };

        let source_port = if let Some((start, end)) = source_port.split_once('-') {
            PortNum::Range(start.parse()?, end.parse()?)
        } else {
            PortNum::Single(source_port.parse::<u16>()?)
        };

        let dest_port = dest_port.parse::<u16>()?;

        let pr = PortRedirection {
            ifaces: if ifaces.is_empty() {
                None
            } else {
                Some(ifaces)
            },
            proto,
            origin: Vec::new(),
            addresses: if addresses.is_empty() {
                None
            } else {
                Some(addresses)
            },
            source_port,
            dest_port,
            dest_addr: None,
        };

        Ok(Self(pr))
    }
}

const GB: usize = 1 << 30;
const MB: usize = 1 << 20;
const KB: usize = 1 << 10;

const GB_F64: f64 = GB as f64;
const MB_F64: f64 = MB as f64;
const KB_F64: f64 = KB as f64;

pub fn format_capacity(size: usize) -> String {
    let bytes = size as f64;
    if size > GB {
        format!("{:.2} GB", bytes / GB_F64)
    } else if size > MB {
        format!("{:.2} MB", bytes / MB_F64)
    } else if size > KB {
        format!("{:.2} KB", bytes / KB_F64)
    } else {
        format!("{:.2} B", bytes)
    }
}

pub fn format_bandwidth(size: usize, secs: u64) -> String {
    let bits = (size * 8) as f64;
    let ss = secs as f64;
    if size > GB {
        format!("{:.2} gbps", bits / GB_F64 / ss)
    } else if size > MB {
        format!("{:.2} mbps", bits / MB_F64 / ss)
    } else if size > KB {
        format!("{:.2} kbps", bits / KB_F64 / ss)
    } else {
        format!("{:.2} bps", bits / ss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use ipcidr::IpCidr;
    use xc::models::network::{NetProto, PortNum};
    #[test]
    fn test_parse_vnet_interface_only() -> Result<()> {
        let input = "vtnet0";
        let vnet = input.parse::<IpWant>()?;
        assert_eq!(vnet.0.addresses, Vec::new());
        assert_eq!(vnet.0.interface, input.to_string());
        Ok(())
    }

    #[test]
    fn test_parse_vnet_single_address_no_mask() -> Result<()> {
        let input = "vtnet0|192.168.0.1";
        let vnet = input.parse::<IpWant>()?;
        assert_eq!(vnet.0.addresses, vec!["192.168.0.1/32".parse()?]);
        assert_eq!(vnet.0.interface, "vtnet0".to_string());
        Ok(())
    }

    #[test]
    fn test_parse_vnet_single_addresses_mixed_masks() -> Result<()> {
        let input = "vtnet0|192.168.0.1,[dead:beef::]/120";
        let vnet = input.parse::<IpWant>()?;
        assert_eq!(
            vnet.0.addresses,
            vec!["192.168.0.1/32".parse()?, "[dead:beef::]/120".parse()?]
        );
        assert_eq!(vnet.0.interface, "vtnet0".to_string());
        Ok(())
    }
    #[test]
    fn test_publish_with_addr_no_proto() -> Result<()> {
        let input = "192.168.1.1:443:80";
        let spec = input.parse::<PublishSpec>()?;
        assert_eq!(spec.0.source_port, PortNum::Single(443u16));
        assert_eq!(spec.0.dest_port, 80u16);
        assert_eq!(
            spec.0.addresses,
            Some(vec!["192.168.1.1".parse::<IpCidr>().unwrap()])
        );
        assert_eq!(spec.0.proto, vec![NetProto::Tcp, NetProto::Udp]);

        Ok(())
    }
    #[test]
    fn test_publish_without_addr_no_proto() -> Result<()> {
        let input = "443:80";
        let spec = input.parse::<PublishSpec>()?;
        assert_eq!(spec.0.source_port, PortNum::Single(443u16));
        assert_eq!(spec.0.dest_port, 80u16);
        assert_eq!(spec.0.addresses, None);
        assert_eq!(spec.0.proto, vec![NetProto::Tcp, NetProto::Udp]);

        Ok(())
    }
    #[test]
    fn test_publish_ipv6_no_proto() -> Result<()> {
        let input = "[dead:beef::]:443:80";
        let spec = input.parse::<PublishSpec>()?;
        assert_eq!(spec.0.source_port, PortNum::Single(443u16));
        assert_eq!(spec.0.dest_port, 80u16);
        assert_eq!(
            spec.0.addresses,
            Some(vec!["dead:beef::".parse::<IpCidr>().unwrap()])
        );
        assert_eq!(spec.0.proto, vec![NetProto::Tcp, NetProto::Udp]);

        Ok(())
    }
    #[test]
    fn test_publish_ipv6() -> Result<()> {
        let input = "[dead:beef::]:443:80/tcp";
        let spec = input.parse::<PublishSpec>()?;
        assert_eq!(spec.0.source_port, PortNum::Single(443u16));
        assert_eq!(spec.0.dest_port, 80u16);
        assert_eq!(
            spec.0.addresses,
            Some(vec!["dead:beef::".parse::<IpCidr>().unwrap()])
        );
        assert_eq!(spec.0.proto, vec![NetProto::Tcp]);
        Ok(())
    }

    #[test]
    fn test_publish_very_complex() -> Result<()> {
        let input = "igb0,ixl3|192.168.2.1,[dead:beef::]:443-643:443/udp,tcp";
        let spec = input.parse::<PublishSpec>()?;
        assert_eq!(spec.0.source_port, PortNum::Range(443, 643));
        assert_eq!(
            spec.0.ifaces,
            Some(vec!["igb0".to_string(), "ixl3".to_string()])
        );
        assert_eq!(spec.0.dest_port, 443);
        assert_eq!(
            spec.0.addresses,
            Some(vec![
                "192.168.2.1".parse::<IpCidr>().unwrap(),
                "dead:beef::".parse::<IpCidr>().unwrap()
            ])
        );
        assert_eq!(spec.0.proto, vec![NetProto::Udp, NetProto::Tcp]);
        Ok(())
    }
}
