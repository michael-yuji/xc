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
/*
use serde::{de::Deserializer, de::Visitor, Serializer};
use serde::{Deserialize, Serialize};
use varutil::string_interpolation::InterpolatedString;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmdArg {
    All,
    Positional(u32),
    Var(InterpolatedString),
}

struct CmdArgVisitor;

impl<'de> Visitor<'de> for CmdArgVisitor {
    type Value = CmdArg;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("^[a-zA-Z_][a-zA-Z0-9_]*$")
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value == "$@" {
            Ok(CmdArg::All)
        } else if value == "$1" {
            Ok(CmdArg::Positional(1))
        } else if value == "$2" {
            Ok(CmdArg::Positional(2))
        } else if value == "$3" {
            Ok(CmdArg::Positional(3))
        } else if value == "$4" {
            Ok(CmdArg::Positional(4))
        } else if value == "$5" {
            Ok(CmdArg::Positional(5))
        } else if value == "$6" {
            Ok(CmdArg::Positional(6))
        } else if value == "$7" {
            Ok(CmdArg::Positional(7))
        } else {
            InterpolatedString::new(value)
                .map(CmdArg::Var)
                .ok_or_else(|| {
                    serde::de::Error::invalid_value(serde::de::Unexpected::Str(value), &self)
                })
        }
    }
}

impl<'de> Deserialize<'de> for CmdArg {
    fn deserialize<D>(deserializer: D) -> Result<CmdArg, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(CmdArgVisitor)
    }
}

impl Serialize for CmdArg {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            CmdArg::All => serializer.serialize_str("$@"),
            CmdArg::Positional(i) => serializer.serialize_str(format!("${i}").as_str()),
            CmdArg::Var(istr) => serializer.serialize_str(&istr.to_string()),
        }
    }
}
*/
