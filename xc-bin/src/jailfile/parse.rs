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
use std::collections::HashMap;

#[derive(Parser)]
#[grammar = "jailfile/jailfile.pest"]
struct JailfileParser;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Action {
    pub(crate) directive_name: String,
    pub(crate) directive_args: HashMap<String, String>,
    pub(crate) args: Vec<String>,
    pub(crate) heredoc: Option<String>,
}

pub(crate) fn parse_jailfile(input: &str) -> Result<Vec<Action>, anyhow::Error> {
    let parsed = JailfileParser::parse(Rule::rules, input)?;
    let actions = parsed
        .into_iter()
        .next()
        .unwrap()
        .into_inner()
        .map(|action| {
            //    let actions = parsed.into_iter().map(|action| {
            let mut iterator = action.into_inner().into_iter();
            let directive_tokens = iterator.next().unwrap();
            let mut args = Vec::new();
            let mut heredoc = None;
            let (directive_name, directive_args) = {
                let mut directive_inner = directive_tokens.into_inner();
                let directive_name = directive_inner.next().unwrap();
                let mut directive_args = HashMap::new();
                for arg_token in directive_inner {
                    let mut arg_token_inner = arg_token.into_inner();
                    let key = arg_token_inner.next().unwrap();
                    let value = arg_token_inner.next().unwrap();
                    directive_args.insert(key.as_str().to_string(), value.as_str().to_string());
                }
                (directive_name.as_str().to_string(), directive_args)
            };

            for value in iterator {
                if value.as_rule() == Rule::heredoc {
                    let mut heredoc_tokens = value.into_inner();
                    let _heredoc_tag = heredoc_tokens.next().unwrap();
                    heredoc = Some(heredoc_tokens.next().unwrap().as_str().to_string());
                    break;
                } else {
                    args.push(value.as_str().to_string());
                }
            }

            Action {
                directive_name,
                directive_args,
                args,
                heredoc,
            }
        })
        .collect::<Vec<_>>();
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_jailfile() {
        let input = r#"
        FROM node:18-alpine
        WORKDIR /app
        COPY . .
        RUN yarn install --production
        RUN <<EOF
        This is some
        funny string
        EOF
        "#;

        let parsed = super::parse_jailfile(input).expect("cannot parse input");
        assert_eq!(
            parsed[0],
            Action {
                directive_name: "FROM".to_string(),
                directive_args: HashMap::new(),
                args: vec!["node:18-alpine".to_string()],
                heredoc: None
            }
        );
        assert_eq!(
            parsed[1],
            Action {
                directive_name: "WORKDIR".to_string(),
                directive_args: HashMap::new(),
                args: vec!["/app".to_string()],
                heredoc: None
            }
        );
        assert_eq!(
            parsed[2],
            Action {
                directive_name: "COPY".to_string(),
                directive_args: HashMap::new(),
                args: vec![".".to_string(), ".".to_string()],
                heredoc: None
            }
        );
        assert_eq!(
            parsed[3],
            Action {
                directive_name: "RUN".to_string(),
                directive_args: HashMap::new(),
                args: vec![
                    "yarn".to_string(),
                    "install".to_string(),
                    "--production".to_string()
                ],
                heredoc: None
            }
        );
        assert_eq!(
            parsed[4],
            Action {
                directive_name: "RUN".to_string(),
                directive_args: HashMap::new(),
                args: Vec::new(),
                heredoc: Some("\n        This is some\n        funny string\n        ".to_string())
            }
        );

        /*
        eprintln!("{parsed:?}");
        assert!(false);
        */
    }
}
