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

use oci_util::distribution::client::{BasicAuth, Registry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Seek, Write};
use std::path::Path;

pub trait RegistriesProvider {
    fn default_registry(&self) -> Option<Registry>;
    fn get_registry_by_name(&self, name: &str) -> Option<Registry>;
    fn insert_registry(&mut self, name: &str, registry: &Registry);
}

pub struct JsonRegistryProvider {
    file: std::fs::File,
    data: RegistriesJsonScheme,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Auth {
    username: String,
    password: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct RegistryScheme {
    base_url: String,
    basic_auth: Option<Auth>,
}

impl RegistryScheme {
    fn without_auth(base_url: &str) -> RegistryScheme {
        RegistryScheme {
            base_url: base_url.to_string(),
            basic_auth: None,
        }
    }

    fn to_reg(&self) -> Registry {
        let basic_auth = self.basic_auth.as_ref().map(|auth| BasicAuth {
            username: auth.username.clone(),
            password: auth.password.clone(),
        });

        Registry::new(self.base_url.clone(), basic_auth)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct RegistriesJsonScheme {
    default: Option<String>,
    // this is probably a bad idea for us to store all the credentials in plain text in a single
    // file
    registries: HashMap<String, RegistryScheme>,
}

impl RegistriesJsonScheme {
    pub fn get_registry_by_name(&self, name: &str) -> Option<RegistryScheme> {
        self.registries.get(&name.to_string()).cloned()
    }
    pub fn default_registry(&self) -> Option<RegistryScheme> {
        self.default
            .clone()
            .and_then(|name| self.get_registry_by_name(&name))
    }
}

impl JsonRegistryProvider {
    pub fn from_path(path: impl AsRef<Path>) -> Result<JsonRegistryProvider, std::io::Error> {
        let file = if !path.as_ref().exists() {
            let mut default = RegistriesJsonScheme {
                default: Some("index.docker.io".to_string()),
                ..RegistriesJsonScheme::default()
            };
            default.registries.insert(
                "index.docker.io".to_string(),
                RegistryScheme::without_auth("https://index.docker.io"),
            );

            let json = serde_json::to_vec_pretty(&default).unwrap();
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(path)?;
            file.write_all(&json)?;
            file.rewind().unwrap();
            file
        } else {
            OpenOptions::new().read(true).write(true).open(path)?
        };
        let regs: RegistriesJsonScheme = serde_json::from_reader(&file).unwrap();
        Ok(JsonRegistryProvider { file, data: regs })
    }
}
impl RegistriesProvider for JsonRegistryProvider {
    fn default_registry(&self) -> Option<Registry> {
        self.data.default_registry().map(|r| r.to_reg())
    }

    fn get_registry_by_name(&self, name: &str) -> Option<Registry> {
        self.data.get_registry_by_name(name).map(|r| r.to_reg())
    }

    fn insert_registry(&mut self, name: &str, registry: &Registry) {
        let mut copy = self.data.clone();
        let basic_auth = registry.basic_auth.clone().map(|auth| Auth {
            username: auth.username.to_string(),
            password: auth.password,
        });
        let reg = RegistryScheme {
            base_url: registry.base_url.clone(),
            basic_auth,
        };
        copy.registries.insert(name.to_string(), reg);
        self.data = copy;
        let json = serde_json::to_vec_pretty(&self.data).unwrap();
        self.file.write_all(&json).unwrap();
    }
}
