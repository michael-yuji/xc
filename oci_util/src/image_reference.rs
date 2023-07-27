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

use crate::digest::OciDigest;

use pest::Parser;
use pest_derive::Parser;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Parser)]
#[grammar_inline = r#"
alphanum = { ASCII_ALPHANUMERIC }
idchar = { (alphanum | "_") }
tag = { idchar ~ (idchar | "." | "-"){0,127} }
digest = { "sha256:" ~ (ASCII_HEX_DIGIT){64} }
hostcomponent = { alphanum ~
    (
        (alphanum | "-") ~ (alphanum | "-") |
        (alphanum ~ !(alphanum | "-"))
    )*
}
portnum = { ASCII_DIGIT{1,5} }
hostname = {
    ("localhost" | (hostcomponent ~ ("." ~ hostcomponent)+)) ~ (":" ~ portnum)?
}
separator = { ("_" | ".") | "_" ~ "_" | "-"* }
component = { alphanum ~ (separator ~ alphanum)* }
name = { component ~ ("/" ~ component)* }
source = { hostname | name }
reference = { (source ~ "/")? ~ name ~ ((":" ~ tag) | ("@" ~ digest)) }
"#]
struct ImageReferenceParser;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ImageTag {
    Tag(String),
    Digest(OciDigest),
}

impl ImageTag {
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    pub fn is_tag(&self) -> bool {
        matches!(self, ImageTag::Tag(_))
    }
}

impl ToString for ImageTag {
    fn to_string(&self) -> String {
        self.as_str().to_string()
    }
}

impl AsRef<str> for ImageTag {
    fn as_ref(&self) -> &str {
        match self {
            ImageTag::Tag(s) => s.as_str(),
            ImageTag::Digest(d) => d.as_str(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ImageReference {
    pub hostname: Option<String>,
    pub name: String,
    pub tag: ImageTag,
}

impl ImageReference {
    pub fn with_hostname(&self, hostname: String) -> ImageReference {
        ImageReference {
            hostname: Some(hostname),
            ..self.clone()
        }
    }
}

impl std::fmt::Display for ImageReference {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let without_hostname = match &self.tag {
            ImageTag::Tag(reference) => {
                format!("{}:{reference}", self.name)
            }
            ImageTag::Digest(digest) => {
                format!("{}@{digest}", self.name)
            }
        };

        match &self.hostname {
            Some(hostname) => write!(f, "{hostname}/{without_hostname}"),
            None => write!(f, "{}", without_hostname),
        }
    }
}

impl FromStr for ImageReference {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<ImageReference, Self::Err> {
        let parsed = ImageReferenceParser::parse(Rule::reference, input)?;
        let root = parsed.into_iter().next().unwrap();
        let mut inner = root.into_inner();
        let maybe_source = inner.next().unwrap();

        let hostname = if Rule::source == maybe_source.as_rule() {
            Some(maybe_source.as_str().to_string())
        } else {
            None
        };

        let name = if hostname.is_some() {
            let n = inner.next().unwrap();
            n.as_str().to_string()
        } else {
            maybe_source.as_str().to_string()
        };

        let locator = inner.next().unwrap();
        let tag_value = locator.as_str().to_string();

        let tag = if locator.as_rule() == Rule::tag {
            ImageTag::Tag(tag_value)
        } else if locator.as_rule() == Rule::digest {
            ImageTag::Digest(OciDigest::from_str(&tag_value)?)
        } else {
            unreachable!()
        };

        Ok(ImageReference {
            hostname,
            name,
            tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ImageReference;
    use std::str::FromStr;

    #[test]
    fn test_parser() {
        let input = "127.0.0.1/helloworld:1234567";
        let output = ImageReference::from_str(input).unwrap();
        eprintln!("output: {output:#?}");
        assert_eq!(output.to_string(), input.to_string())
    }
}
