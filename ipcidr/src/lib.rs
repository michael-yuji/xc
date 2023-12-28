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

use paste::paste;
use serde::{de, de::Deserializer, Deserialize, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::io::{Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum IpCidr {
    V4(Ipv4Cidr),
    V6(Ipv6Cidr),
}

impl Display for IpCidr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::V4(cidr) => write!(f, "{cidr}"),
            Self::V6(cidr) => write!(f, "{cidr}"),
        }
    }
}

fn __err() -> std::io::Error {
    Error::new(ErrorKind::Other, "incorrect network format")
}

// allow ip address to be quoted by square bracket
fn parse_ip_extended(input: &str) -> Result<IpAddr, std::io::Error> {
    match input.strip_prefix('[') {
        None => IpAddr::from_str(input).map_err(|_| __err()),
        Some(maybe_quoted) => maybe_quoted
            .strip_suffix(']')
            .ok_or(__err())
            .and_then(|i| IpAddr::from_str(i).map_err(|_| __err())),
    }
}

impl FromStr for IpCidr {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, std::io::Error> {
        match s.split_once('/') {
            None => {
                let net = parse_ip_extended(s)?;
                match net {
                    IpAddr::V4(address) => Ok(IpCidr::V4(Ipv4Cidr { address, mask: 32 })),
                    IpAddr::V6(address) => Ok(IpCidr::V6(Ipv6Cidr { address, mask: 128 })),
                }
            }
            Some((net, mask)) => {
                let net = parse_ip_extended(net)?;
                let mask = mask
                    .parse::<u8>()
                    .map_err(|_| Error::new(ErrorKind::Other, "invalid subnet mask format"))?;
                match net {
                    IpAddr::V4(_) if mask > 32 => Err(Error::new(
                        ErrorKind::Other,
                        "ipv4 netmask cannot greater than 32",
                    )),
                    IpAddr::V6(_) if mask > 128 => Err(Error::new(
                        ErrorKind::Other,
                        "ipv6 netmask cannot greater than 128",
                    )),
                    IpAddr::V4(address) => Ok(IpCidr::V4(Ipv4Cidr { address, mask })),
                    IpAddr::V6(address) => Ok(IpCidr::V6(Ipv6Cidr { address, mask })),
                }
            }
        }
    }
}

impl IpCidr {
    pub fn mask(&self) -> u8 {
        match self {
            Self::V4(cidr) => cidr.mask,
            Self::V6(cidr) => cidr.mask,
        }
    }

    pub fn to_singleton(&self) -> IpCidr {
        match self {
            Self::V4(cidr) => Self::V4(cidr.to_singleton()),
            Self::V6(cidr) => Self::V6(cidr.to_singleton()),
        }
    }

    pub fn from_singleton(address: IpAddr) -> IpCidr {
        match address {
            IpAddr::V4(address) => Self::V4(Ipv4Cidr { address, mask: 32 }),
            IpAddr::V6(address) => Self::V6(Ipv6Cidr { address, mask: 128 }),
        }
    }
    pub fn from_addr(addr: IpAddr, mask: u8) -> Option<Self> {
        match addr {
            IpAddr::V4(addr) if mask <= 32 => Some(Self::V4(Ipv4Cidr {
                address: addr,
                mask,
            })),
            IpAddr::V6(addr) if mask <= 128 => Some(Self::V6(Ipv6Cidr {
                address: addr,
                mask,
            })),
            _ => None,
        }
    }
    pub fn min_addr(&self) -> Option<IpAddr> {
        match self {
            Self::V4(cidr) => cidr.min_addr().map(IpAddr::V4),
            Self::V6(cidr) => cidr.min_addr().map(IpAddr::V6),
        }
    }
    pub fn max_addr(&self) -> Option<IpAddr> {
        match self {
            Self::V4(cidr) => cidr.max_addr().map(IpAddr::V4),
            Self::V6(cidr) => cidr.max_addr().map(IpAddr::V6),
        }
    }
    pub fn addr(&self) -> IpAddr {
        match self {
            Self::V4(cidr) => IpAddr::V4(cidr.address),
            Self::V6(cidr) => IpAddr::V6(cidr.address),
        }
    }
    pub fn network_addr(&self) -> IpAddr {
        match self {
            Self::V4(cidr) => IpAddr::V4(cidr.network_addr()),
            Self::V6(cidr) => IpAddr::V6(cidr.network_addr()),
        }
    }
    pub fn broadcast_addr(&self) -> IpAddr {
        match self {
            Self::V4(cidr) => IpAddr::V4(cidr.broadcast_addr()),
            Self::V6(cidr) => IpAddr::V6(cidr.broadcast_addr()),
        }
    }
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self, ip) {
            (Self::V4(cidr), IpAddr::V4(ip)) => cidr.contains(ip),
            (Self::V6(cidr), IpAddr::V6(ip)) => cidr.contains(ip),
            _ => false,
        }
    }
}

macro_rules! masked_ip {
    ($version:expr, $bits:expr) => {
        paste! {
            #[derive(Clone, Debug, PartialEq, Eq)]
            pub struct [<Ipv $version Cidr>] {
                pub address: [<Ipv $version Addr>],
                pub mask: u8
            }

            impl [<Ipv $version Cidr>] {
                pub fn addr_raw(&self) -> [<u $bits>] {
                    <[<u $bits>]>::from(self.address)
                }

                pub fn netmask_addr_raw(&self) -> [<u $bits>] {
                    <[<u $bits>]>::MAX << ($bits - (self.mask as [<u $bits>]))
                }

                pub fn network_addr_raw(&self) -> [<u $bits>] {
                    self.addr_raw() & self.netmask_addr_raw()
                }

                pub fn broadcast_addr_raw(&self) -> [<u $bits>] {
                    self.addr_raw() | (<[<u $bits>]>::MAX & !self.netmask_addr_raw())
                }

                pub fn addr(&self) -> [<Ipv $version Addr>] {
                    self.address
                }

                pub fn to_singleton(&self) -> [<Ipv $version Cidr>] {
                    [<Ipv $version Cidr>] { address: self.address, mask: $bits }
                }

                pub fn netmask_addr(&self) -> [<Ipv $version Addr>] {
                    [<Ipv $version Addr>]::from(self.netmask_addr_raw())
                }

                pub fn network_addr(&self)  -> [<Ipv $version Addr>] {
                    [<Ipv $version Addr>]::from(self.network_addr_raw())
                }

                pub fn broadcast_addr(&self) -> [<Ipv $version Addr>] {
                    [<Ipv $version Addr>]::from(self.broadcast_addr_raw())
                }

                pub fn min_addr(&self) -> Option<[<Ipv $version Addr>]> {
                    if self.mask == $bits || self.mask == 0 {
                        None
                    } else {
                        Some(<[<Ipv $version Addr>]>::from(self.network_addr_raw() + 1))
                    }
                }

                pub fn max_addr(&self) -> Option<[<Ipv $version Addr>]> {
                    if self.mask == $bits || self.mask == 0 {
                        None
                    } else {
                        Some(<[<Ipv $version Addr>]>::from(self.broadcast_addr_raw() - 1))
                    }
                }

                pub fn contains(&self, addr: &[<Ipv $version Addr>]) -> bool {
                    let value = [<u $bits>]::from(addr.clone());
                    value > self.addr_raw() && value < self.broadcast_addr_raw()
                }
            }

            impl Display for [<Ipv $version Cidr>] {
                fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}/{}", self.addr(), self.mask)
                }
            }

            impl FromStr for [<Ipv $version Cidr>] {
                type Err = std::io::Error;
                fn from_str(s: &str) -> Result<Self, std::io::Error> {

                    match s.split_once('/') {
                        None => {
                            let net = [<Ipv $version Addr>]::from_str(s)
                                .map_err(|_| Error::new(ErrorKind::Other, "incorrect network format"))?;
                            Ok([<Ipv $version Cidr>] { address: net, mask: $bits })
                         }
                        Some((net, mask)) => {
                            let net = [<Ipv $version Addr>]::from_str(net)
                                .map_err(|_| Error::new(ErrorKind::Other, "incorrect network format"))?;
                            let mask = mask.parse::<u8>().map_err(|_| Error::new(ErrorKind::Other, "invalid subnet mask format"))?;
                            if mask > $bits {
                                Err(Error::new(ErrorKind::Other, "invalid subnet mask"))
                            } else {
                                Ok([<Ipv $version Cidr>] { address: net, mask })
                            }
                        }
                    }
                }
            }
            impl<'de> Deserialize<'de> for [<Ipv $version Cidr>] {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where D: Deserializer<'de>
                {
                    FromStr::from_str(&String::deserialize(deserializer)?).map_err(de::Error::custom)
                }
            }
            impl Serialize for [<Ipv $version Cidr>] {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
                    serializer.serialize_str(self.to_string().as_str())
                }
            }
        }
    }
}

masked_ip!(4, 32);
masked_ip!(6, 128);

#[derive(Clone)]
pub struct Cidr {
    pub net: IpAddr,
    pub mask: u8,
}

macro_rules! implement_subnet {
    ($f:ident) => {
        paste! {
            pub fn $f(&self) -> IpAddr {
                match self.net {
                    IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from(self.[<$f _u32>]())),
                    IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(self.[<$f _u128>]()))
                }
            }
        }
    };
}

impl Cidr {
    implement_subnet!(netmask);
    implement_subnet!(network_addr);
    implement_subnet!(broadcast_addr);

    pub fn addr_u32(&self) -> u32 {
        if let IpAddr::V4(addr) = self.net {
            return u32::from(addr);
        }
        panic!("")
    }
    pub fn addr_u128(&self) -> u128 {
        let IpAddr::V6(addr) = self.net else {
            panic!("")
        };
        u128::from(addr)
    }
    pub fn netmask_u32(&self) -> u32 {
        u32::MAX << (32 - self.mask)
    }
    pub fn netmask_u128(&self) -> u128 {
        u128::MAX << (128 - self.mask)
    }
    pub fn network_addr_u32(&self) -> u32 {
        let IpAddr::V4(addr) = self.net else {
            panic!("")
        };
        u32::from(addr) & self.netmask_u32()
    }
    pub fn network_addr_u128(&self) -> u128 {
        let IpAddr::V6(addr) = self.net else {
            panic!("")
        };
        u128::from(addr) & self.netmask_u128()
    }
    pub fn broadcast_addr_u32(&self) -> u32 {
        self.network_addr_u32() | (u32::MAX & !self.netmask_u32())
    }
    pub fn broadcast_addr_u128(&self) -> u128 {
        self.network_addr_u128() | (u128::MAX & !self.netmask_u128())
    }
    pub fn first_addr(&self) -> Option<IpAddr> {
        match self.net {
            IpAddr::V4(_) => {
                if self.mask == 32 || self.mask == 0 {
                    None
                } else {
                    Some(IpAddr::V4(Ipv4Addr::from(self.broadcast_addr_u32() - 1)))
                }
            }
            IpAddr::V6(_) => {
                if self.mask == 128 || self.mask == 0 {
                    None
                } else {
                    Some(IpAddr::V6(Ipv6Addr::from(self.broadcast_addr_u128() - 1)))
                }
            }
        }
    }
    pub fn last_addr(&self) -> Option<IpAddr> {
        match self.net {
            IpAddr::V4(_) => {
                if self.mask == 32 || self.mask == 0 {
                    None
                } else {
                    Some(IpAddr::V4(Ipv4Addr::from(self.network_addr_u32() + 1)))
                }
            }
            IpAddr::V6(_) => {
                if self.mask == 128 || self.mask == 0 {
                    None
                } else {
                    Some(IpAddr::V6(Ipv6Addr::from(self.network_addr_u128() + 1)))
                }
            }
        }
    }
    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.net, addr) {
            (IpAddr::V4(_), IpAddr::V4(addr)) => {
                let addr_u32 = u32::from(addr);
                addr_u32 > self.addr_u32() && addr_u32 < self.broadcast_addr_u32()
            }
            (IpAddr::V6(_), IpAddr::V6(addr)) => {
                let addr_u128 = u128::from(addr);
                addr_u128 > self.addr_u128() && addr_u128 < self.broadcast_addr_u128()
            }
            _ => false,
        }
    }
}

impl Display for Cidr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.net, self.mask)
    }
}

impl FromStr for Cidr {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, std::io::Error> {
        match s.split_once('/') {
            None => {
                let net = IpAddr::from_str(s)
                    .map_err(|_| Error::new(ErrorKind::Other, "incorrect network format"))?;
                Ok(Cidr {
                    net,
                    mask: if net.is_ipv4() { 32 } else { 128 },
                })
            }
            Some((net, mask)) => {
                let net = IpAddr::from_str(net)
                    .map_err(|_| Error::new(ErrorKind::Other, "incorrect network format"))?;
                let mask = mask
                    .parse::<u8>()
                    .map_err(|_| Error::new(ErrorKind::Other, "invalid subnet mask format"))?;
                match net {
                    IpAddr::V4(_) if mask > 32 => Err(Error::new(
                        ErrorKind::Other,
                        "ipv4 netmask cannot greater than 32",
                    )),
                    IpAddr::V6(_) if mask > 128 => Err(Error::new(
                        ErrorKind::Other,
                        "ipv6 netmask cannot greater than 128",
                    )),
                    _ => Ok(Cidr { net, mask }),
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        FromStr::from_str(&String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl Serialize for Cidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}
