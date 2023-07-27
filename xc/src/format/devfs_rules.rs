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
use pest::Parser;
use pest_derive::Parser;
use serde::{de::Visitor, Deserializer, Deserialize, Serialize, Serializer};
use std::str::FromStr;

#[derive(Parser)]
#[grammar_inline = r#"
need_escape = { "\n" | "\'" | "\"" | "\\" | WHITE_SPACE }
path_char = @{ !need_escape ~ ANY | "\\" ~ need_escape  }
path = @{ path_char+ }

Id = @{ ASCII_DIGIT+ }
Uid = @{ Id }
Gid = @{ Id }
Mode = { "0"? ~ ('0'..'7'){3} }
devtype = { "disk"|"mem"|"tape"|"tty" }
condition = { "path" ~ path | "type" ~ devtype }
action = { "hide" | "unhide" | "group" ~ Gid | "user" ~ Uid | "mode" ~ Mode }
WHITESPACE = _{ " " }
rule = _{ WHITE_SPACE* ~ ((condition+ ~ action*) | (action+ ~ condition*)) ~ WHITE_SPACE* ~ EOI }
"#]
struct RuleParser;


/// A "safer" condition that does not support glob
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Path(std::path::PathBuf),
    Type(String),
}

impl std::fmt::Display for Condition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) ->  std::fmt::Result {
        match self {
            Condition::Type(t) => write!(f, "type {t}"),
            Condition::Path(p) => write!(f, "path {}", p.to_string_lossy())
        }
    }
}

/// A "safer" subset of devfs rule action, does not allow include and user/group by name
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevfsAction {
    Hide,
    Unhide,
    User(u32),
    Group(u32),
    Mode(u32),
}
impl std::fmt::Display for DevfsAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) ->  std::fmt::Result {
        match self {
            Self::Hide => write!(f, "hide"),
            Self::Unhide => write!(f, "unhide"),
            Self::Mode(mode) => write!(f, "mode {mode:o}"),
            Self::User(uid) => write!(f, "user {uid}"),
            Self::Group(gid) => write!(f, "group {gid}"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevfsRule {
    conditions: Vec<Condition>,
    actions: Vec<DevfsAction>
}
impl std::fmt::Display for DevfsRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let space = " ".to_string();
        let c = self.conditions.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(&space);
        let a = self.actions.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(&space);

        if c.is_empty() {
            write!(f, "{a}")
        } else if a.is_empty() {
            write!(f, "{c}")
        } else {
            write!(f, "{c} {a}")
        }
    }
}

impl Serialize for DevfsRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

struct DevfsRuleVisitor;

impl<'de> Visitor<'de> for DevfsRuleVisitor {
    type Value = DevfsRule;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("devfs(8) rule")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        value.parse().map_err(|e| E::custom(format!("{e:?}")))
    }
}

impl<'de> Deserialize<'de> for DevfsRule {
    fn deserialize<D>(deserializer: D) -> Result<DevfsRule, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(DevfsRuleVisitor)
    }
}

impl FromStr for DevfsRule {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parsed = RuleParser::parse(Rule::rule, input)?;

        let mut conditions = Vec::new();
        let mut actions = Vec::new();

        for component in parsed {
            match component.as_rule() {
                Rule::condition => {
                    let part = component.into_inner().next().unwrap();
                    match part.as_rule() {
                        Rule::path => {
                            conditions.push(Condition::Path(std::path::PathBuf::from(part.as_str())));
                        },
                        Rule::devtype => {
                            conditions.push(Condition::Type(part.as_str().to_string()));
                        },
                        _ => unreachable!()
                    }
                },
                Rule::action => {
                    match component.as_str() {
                        "hide" => actions.push(DevfsAction::Hide),
                        "unhide" => actions.push(DevfsAction::Unhide),
                        _ => {
                            let n = component.into_inner().next().unwrap();
                            match n.as_rule() {
                                Rule::Gid => actions.push(DevfsAction::Group(n.as_str().parse()?)),
                                Rule::Uid => actions.push(DevfsAction::User(n.as_str().parse()?)),
                                Rule::Mode => actions.push(DevfsAction::Mode(n.as_str().parse()?)),
                                _ => unreachable!()
                            }
                        }
                    }
                },
                _ => continue
            }
        }

        Ok(DevfsRule {
            conditions,
            actions
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_devfs() -> Result<(), anyhow::Error> {
        let input = "     type disk path hello group 100 user 501";
        let rule = DevfsRule::from_str(input)?;

        println!("rule: {rule:#?}");

        assert_eq!(rule.conditions[0], Condition::Type("disk".to_string()));
        assert_eq!(rule.conditions[1], Condition::Path(std::path::PathBuf::from("hello")));
        assert_eq!(rule.actions[0], DevfsAction::Group(100));
        assert_eq!(rule.actions[1], DevfsAction::User(501));

        Ok(())
    }
}
