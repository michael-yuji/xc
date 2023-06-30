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
use crate::parse::{ParseContext, Variable};
use serde::{de, de::Visitor, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpolatedString(Vec<Variable>);

impl FromStr for InterpolatedString {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s).ok_or(std::io::Error::new(
            std::io::ErrorKind::Other,
            "cannot parse interpolated string",
        ))
    }
}

impl InterpolatedString {
    pub fn new(s: &str) -> Option<InterpolatedString> {
        let mut context = ParseContext::new(s);
        context.take_parts().map(InterpolatedString)
    }

    pub fn apply(&self, variables: &HashMap<String, String>) -> String {
        let mut b = String::new();
        for variable in self.0.iter() {
            let applied = variable.apply(variables).unwrap();
            b.push_str(applied.as_str())
        }
        b
    }

    pub fn collect_variable_dependencies(&self, deps: &mut HashSet<String>) {
        for variable in &self.0 {
            variable.collect_variable_dependencies(deps)
        }
    }
}

impl std::fmt::Display for InterpolatedString {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|var| var.to_string())
                .collect::<Vec<_>>()
                .join("")
        )
    }
}

impl Serialize for InterpolatedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct InterpolatedStringVisitor;

impl<'de> Visitor<'de> for InterpolatedStringVisitor {
    type Value = InterpolatedString;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("String with variable denoting as $VARIABLE, or ${VARIABLE}, etc...")
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        InterpolatedString::new(value).ok_or_else(|| {
            serde::de::Error::invalid_value(serde::de::Unexpected::Str(value), &self)
        })
    }
}

impl<'de> Deserialize<'de> for InterpolatedString {
    fn deserialize<D>(deserializer: D) -> Result<InterpolatedString, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(InterpolatedStringVisitor)
    }
}

/// String value that conforms to usual variable naming rules
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub struct Var(String);

struct VarVisitor;

impl<'de> Visitor<'de> for VarVisitor {
    type Value = Var;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("^[a-zA-Z_][a-zA-Z0-9_]*$")
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Var::new(value).ok_or_else(|| {
            serde::de::Error::invalid_value(serde::de::Unexpected::Str(value), &self)
        })
    }
}

impl<'de> Deserialize<'de> for Var {
    fn deserialize<D>(deserializer: D) -> Result<Var, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(VarVisitor)
    }
}

impl Serialize for Var {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl std::fmt::Display for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Var {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    pub fn new<S: AsRef<str>>(s: S) -> Option<Var> {
        let string = s.as_ref();
        if string.is_empty() {
            None
        } else {
            let mut chars = string.chars();
            let first = chars.next().expect("Expected non empty string");
            if first.is_ascii_alphabetic() || first == '_' {
                for c in chars {
                    if !c.is_ascii_alphanumeric() && c != '_' {
                        return None;
                    }
                }
                Some(Var(string.to_string()))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_var_constraints() {
        assert_eq!(Var::new("0_helium"), None);
        assert_ne!(Var::new("hello_world"), None);
        assert_eq!(Var::new(" a b c d"), None);
    }

    #[test]
    fn test_var_serialization() {
        #[derive(Serialize, Deserialize)]
        struct TestVar {
            test: Var,
        }

        let json = r#"{"test": "_abcde"}"#;
        let deserialized: TestVar = serde_json::from_str(json).expect("serialize Var");
        assert_eq!(deserialized.test, Var::new("_abcde").unwrap());

        let object = serde_json::to_string(&deserialized).expect("should be able to serialize");
        assert_eq!(object, r#"{"test":"_abcde"}"#);
    }
}
