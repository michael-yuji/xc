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
use crate::models::network::{NetProto, PortNum};
use pest::Parser;
use pest_derive::Parser;
use std::str::FromStr;

#[derive(Parser)]
#[grammar_inline = r#"
portnum = { ASCII_DIGIT{1,5} }
portrange = { portnum ~ "-" ~ portnum }
net_proto = { "tcp" | "udp" }
expose = { (portrange | portnum) ~ ("/" ~ net_proto)? }
"#]
struct RuleParser;

#[derive(Debug)]
pub struct Expose {
    pub port: PortNum,
    pub proto: Option<NetProto>,
}

impl FromStr for Expose {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parsed = RuleParser::parse(Rule::expose, input)?;
        let root = parsed.into_iter().next().unwrap();
        let mut iterator = root.into_inner();
        let portnum_or_portrange = iterator.next().unwrap();
        let port = if portnum_or_portrange.as_rule() == Rule::portrange {
            let mut inners = portnum_or_portrange.into_inner();
            let head_port = inners.next().unwrap();
            let tail_port = inners.next().unwrap();
            PortNum::Range(head_port.as_str().parse()?, tail_port.as_str().parse()?)
        } else {
            PortNum::Single(portnum_or_portrange.as_str().parse()?)
        };

        let proto = iterator.next().map(|token| {
            let tok = token.as_str();
            match tok {
                "tcp" => NetProto::Tcp,
                "udp" => NetProto::Udp,
                _ => unreachable!(),
            }
        });

        Ok(Expose { port, proto })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse_expose_port() {
        let input = "80-8080";
        let expose = Expose::from_str(input).unwrap();
        assert_eq!(expose.port, PortNum::Range(80, 8080));
        assert_eq!(expose.proto, None);
    }
    #[test]
    fn test_parse_expose_port_1() {
        let input = "80-8080/tcp";
        let expose = Expose::from_str(input).unwrap();
        assert_eq!(expose.port, PortNum::Range(80, 8080));
        assert_eq!(expose.proto, Some(NetProto::Tcp));
    }
    #[test]
    fn test_parse_expose_port_2() {
        let input = "80/tcp";
        let expose = Expose::from_str(input).unwrap();
        assert_eq!(expose.port, PortNum::Single(80));
        assert_eq!(expose.proto, Some(NetProto::Tcp));
    }
}
