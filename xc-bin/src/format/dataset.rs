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
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::str::FromStr;
use varutil::string_interpolation::Var;

#[derive(Clone, Debug)]
pub struct DatasetParam {
    pub key: Option<Var>,
    pub dataset: PathBuf,
}

impl FromStr for DatasetParam {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            None => {
                if s.starts_with('/') {
                    return Err(Error::new(ErrorKind::Other, "invalid dataset path"));
                }
                let dataset = s
                    .parse::<PathBuf>()
                    .map_err(|_| Error::new(ErrorKind::Other, "invalid dataset path"))?;
                Ok(DatasetParam { key: None, dataset })
            }
            Some((env, dataset)) => {
                if dataset.starts_with('/') {
                    return Err(Error::new(ErrorKind::Other, "invalid dataset path"));
                }
                let key = varutil::string_interpolation::Var::from_str(env)?;
                Ok(DatasetParam {
                    key: Some(key),
                    dataset: dataset.into(),
                })
            }
        }
    }
}
