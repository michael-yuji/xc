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
use std::collections::HashMap;
use std::str::Chars;

#[non_exhaustive]
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Variable {
    Const(String),
    Ref(String),
    OrElse(String, Box<Variable>),
    OrPanic(String, Box<Variable>),
    AlterVal(String, Box<Variable>),
}

impl std::fmt::Display for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Const(v) => write!(f, "{v}"),
            Self::Ref(v) => write!(f, "${{{v}}}"),
            Self::OrElse(v, b) => write!(f, "${{{v}:-{b}}}"),
            Self::OrPanic(v, b) => write!(f, "${{{v}:?{b}}}"),
            Self::AlterVal(v, b) => write!(f, "${{{v}:+{b}}}"),
        }
    }
}

impl Variable {
    pub fn apply(&self, variables: &HashMap<String, String>) -> Option<String> {
        match self {
            Variable::Const(value) => Some(value.to_string()),
            Variable::Ref(key) => variables.get(key).as_ref().map(|m| m.to_string()),
            Variable::OrElse(key, othervar) => variables
                .get(key)
                .map(|v| v.to_string())
                .or_else(|| othervar.apply(variables)),
            Variable::OrPanic(key, _) => variables.get(key).map(|v| v.to_string()),
            Variable::AlterVal(key, othervar) => match variables.get(key) {
                Some(_) => Some("".to_string()),
                None => othervar.apply(variables),
            },
        }
    }

    pub fn collect_variable_dependencies(&self, deps: &mut std::collections::HashSet<String>) {
        match self {
            Variable::Const(_) => (),
            Variable::Ref(name) => {
                deps.insert(name.to_string());
            }
            Variable::OrElse(name, variable) => {
                deps.insert(name.to_string());
                variable.collect_variable_dependencies(deps);
            }
            // soft dependency
            Variable::OrPanic(name, variable) => {
                deps.insert(name.to_string());
                variable.collect_variable_dependencies(deps);
            }
            Variable::AlterVal(name, variable) => {
                deps.insert(name.to_string());
                variable.collect_variable_dependencies(deps);
            }
        }
    }
}

pub struct ParseContext<'a> {
    remaining: Chars<'a>,
    last_char: Option<char>,
}

macro_rules! take_int {
    ($t:ty) => {
        paste! {
            pub fn [<take $t>](&mut self) -> Option<$t> {
                let mut digits = String::new();

                while let Some(c) = self.last_char {
                    if c.is_ascii_digit() {
                        digits.push(c);
                        self.next();
                    }
                }

                if digits.is_empty() {
                    None
                } else {
                    digits.parse::<$t>().ok()
                }
            }
        }
    };
}

impl<'a> ParseContext<'a> {
    pub fn next(&mut self) -> Option<char> {
        self.last_char = self.remaining.next();
        self.last_char
    }

    pub fn new(input: &'a str) -> ParseContext<'a> {
        let mut chars = input.chars();
        let first = chars.next();
        ParseContext {
            remaining: chars,
            last_char: first,
        }
    }

    pub fn has_more(&self) -> bool {
        self.last_char.is_some()
    }

    pub fn take_c_ident(&mut self) -> Option<String> {
        match self.last_char {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let mut taken = c.to_string();
                loop {
                    match self.next() {
                        Some(nc) if nc.is_ascii_alphanumeric() || nc == '_' => taken.push(nc),
                        _ => return Some(taken),
                    }
                }
            }
            _ => None,
        }
    }

    pub fn take_until_token_no_consume(&mut self, end_token: char) -> String {
        let mut buffer = String::new();
        while self.last_char != Some(end_token) {
            match self.last_char {
                None => break,
                Some(c) => buffer.push(c),
            }
            self.next();
        }
        buffer
    }

    fn take_until_token(&mut self, end_token: char) -> Option<String> {
        let mut buffer = self.last_char?.to_string();
        self.next()?;
        while self.last_char != Some(end_token) {
            buffer.push(self.last_char.unwrap());
            self.next()?;
        }
        Some(buffer)
    }

    pub fn take_parts(&mut self) -> Option<Vec<Variable>> {
        let mut parts = vec![];
        loop {
            match self.last_char {
                None => break,
                Some('$') => parts.push(self.take_interpolate_args()?),
                _ => {
                    parts.push(Variable::Const(self.take_until_token_no_consume('$')));
                }
            }
        }
        Some(parts)
    }

    take_int!(u8);

    pub fn take_u32(&mut self) -> Option<u32> {
        let mut digits = String::new();

        while let Some(c) = self.last_char {
            if c.is_ascii_digit() {
                digits.push(c);
                self.next();
            }
        }

        if digits.is_empty() {
            None
        } else {
            digits.parse::<u32>().ok()
        }
    }

    pub fn take_interpolate_args(&mut self) -> Option<Variable> {
        if self.last_char == Some('$') {
            match self.next() {
                Some('{') => {
                    self.trim_spaces();
                    // cosume the start bracket
                    self.next()?;

                    let ident = self.take_c_ident()?;

                    match self.last_char {
                        Some('}') => {
                            // consume the end bracket
                            self.next();
                            Some(Variable::Ref(ident))
                        }
                        Some(':') => {
                            match self.next()? {
                                m @ ('-' | '=' | '?' | '+') => {
                                    // consume and move to next token
                                    let var = match self.next()? {
                                        '$' => Box::new(self.take_interpolate_args()?),
                                        '\"' => {
                                            // drop the leading '"'
                                            self.next()?;
                                            let value = self.take_until_token('\"')?;
                                            // need to follow by '}'
                                            self.next()?;
                                            if self.last_char != Some('}') {
                                                return None;
                                            } else {
                                                Box::new(Variable::Const(value))
                                            }
                                        }
                                        _ => {
                                            let value = self.take_until_token('}')?;
                                            Box::new(Variable::Const(value))
                                        }
                                    };

                                    match m {
                                        '-' | '=' => Some(Variable::OrElse(ident, var)),
                                        '?' => Some(Variable::OrPanic(ident, var)),
                                        '+' => Some(Variable::AlterVal(ident, var)),
                                        _ => panic!("should never happen"),
                                    }
                                }
                                _ => None,
                            }
                        }
                        // Everything except ':=', ':-', ':?', '+' after the name are bad
                        _ => None,
                    }
                }
                Some(_) => self.take_c_ident().map(Variable::Ref),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn trim_spaces(&mut self) {
        match self.last_char {
            Some(c) if c.is_ascii_whitespace() => loop {
                self.last_char = self.remaining.next();
                match self.last_char {
                    Some(nc) if nc.is_ascii_whitespace() => continue,
                    _ => break,
                }
            },
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ParseContext, Variable};

    #[test]
    fn test_take_c_ident_start_with_space() {
        let mut input = ParseContext::new("  hello world ");
        assert_eq!(input.take_c_ident(), None);

        input.trim_spaces();

        // These should be no-ops
        input.trim_spaces();
        input.trim_spaces();
        input.trim_spaces();
        input.trim_spaces();

        assert_eq!(input.take_c_ident(), Some("hello".to_string()));
        assert_eq!(input.take_c_ident(), None);
        input.trim_spaces();
        assert_eq!(input.take_c_ident(), Some("world".to_string()));
        assert_eq!(input.take_c_ident(), None);
        input.trim_spaces();
        assert_eq!(input.take_c_ident(), None);
    }

    #[test]
    fn test_take_c_ident_terminating_string() {
        let mut input = ParseContext::new("HELLO_WORLD");
        assert_eq!(input.take_c_ident(), Some("HELLO_WORLD".to_string()));
    }

    #[test]
    fn test_interpolate_args() {
        let mut input = ParseContext::new("$HELLO World");
        let arg = input.take_interpolate_args().expect("Should get $HELLO");
        input.trim_spaces();
        let remaining = input.take_c_ident().expect("should get World");
        assert_eq!(arg, Variable::Ref("HELLO".to_string()));
        assert_eq!(remaining, "World".to_string());
    }

    #[test]
    fn test_interpolate_args_quoted() {
        let mut quoted = ParseContext::new("${HELLO} World");
        let quoted_arg = quoted.take_interpolate_args().expect("Should get $HELLO");
        quoted.trim_spaces();
        let quoted_rem = quoted.take_c_ident().expect("should get World");
        assert_eq!(quoted_arg, Variable::Ref("HELLO".to_string()));
        assert_eq!(quoted_rem, "World".to_string());
    }

    #[test]
    fn test_interpolate_args_or_else() {
        let mut quoted = ParseContext::new("${HELLO:-\"World\"}");
        let var = quoted.take_interpolate_args().unwrap();
        assert_eq!(
            var,
            Variable::OrElse(
                "HELLO".to_string(),
                Box::new(Variable::Const("World".to_string()))
            )
        );
    }

    #[test]
    fn test_interpolate_args_or_else_without_end_curly_bracket() {
        let mut quoted = ParseContext::new("${HELLO:-\"World\"");
        let var = quoted.take_interpolate_args();
        assert_eq!(var, None);
    }

    #[test]
    fn test_interpolate_args_or_else_no_double_quote() {
        let mut quoted = ParseContext::new("${HELLO:-World    }");
        let var = quoted.take_interpolate_args().unwrap();
        assert_eq!(
            var,
            Variable::OrElse(
                "HELLO".to_string(),
                Box::new(Variable::Const("World    ".to_string()))
            )
        );
    }

    #[test]
    fn test_interpolate_args_nested() {
        let mut quoted = ParseContext::new("${HELLO:-${World:-hahaha   }}");
        let var = quoted.take_interpolate_args().unwrap();
        let eq = Variable::OrElse(
            "HELLO".to_string(),
            Box::new(Variable::OrElse(
                "World".to_string(),
                Box::new(Variable::Const("hahaha   ".to_string())),
            )),
        );
        assert_eq!(var, eq);
        assert_eq!(eq.to_string(), "${HELLO:-${World:-hahaha   }}".to_string());
    }

    #[test]
    fn test_take_until_no_consume() {
        let mut input = ParseContext::new("hello $world");
        let value = input.take_until_token_no_consume('$');
        assert_eq!("hello ".to_string(), value);

        let var = input.take_interpolate_args().unwrap();
        assert_eq!(var, Variable::Ref("world".to_string()));
    }

    #[test]
    fn test_interpolate_string() {
        let mut input = ParseContext::new("hello $world .");
        let parts = input.take_parts().unwrap();
        let comp = vec![
            Variable::Const("hello ".to_string()),
            Variable::Ref("world".to_string()),
            Variable::Const(" .".to_string()),
        ];
        assert_eq!(parts, comp);
    }
}
