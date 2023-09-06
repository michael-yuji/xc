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

use super::{ConfigMod, Directive, JailContext};
use crate::{format::EnvPair, jailfile::parse::Action};

use anyhow::{bail, Result};
use clap::Parser;
use std::{ffi::OsString, path::PathBuf};
use xc::models::MountSpec;

#[derive(Parser, Debug)]
pub(crate) struct VolumeDirective {
    #[clap(long = "hint")]
    hints: Vec<EnvPair>,
    #[clap(long = "required", action)]
    required: bool,
    #[clap(long = "description", short = 'd', default_value_t)]
    description: String,
    #[clap(long = "ro", action)]
    read_only: bool,
    destination: PathBuf,
    name: Option<OsString>,
}
impl Directive for VolumeDirective {
    fn up_to_date(&self) -> bool {
        true
    }
    fn from_action(action: &Action) -> Result<VolumeDirective> {
        if action.directive_name != "VOLUME" {
            bail!("directive_name is not VOLUME")
        }
        let mut args = vec!["dummy".to_string()];
        args.extend(action.args.clone());
        let directive = VolumeDirective::parse_from(args);

        Ok(directive)
    }

    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        let mut volume_hints = std::collections::HashMap::new();

        for pair in self.hints.iter() {
            volume_hints.insert(
                pair.key.to_string(),
                serde_json::Value::String(pair.value.to_string()),
            );
        }

        let mount_spec = MountSpec {
            read_only: self.read_only,
            destination: self.destination.clone(),
            required: self.required,
            volume_hints,
            description: self.description.to_string(),
        };

        let name = self
            .name
            .clone()
            .unwrap_or_else(|| self.destination.as_os_str().to_os_string());

        context
            .config_mods
            .push(ConfigMod::Volume(name, mount_spec));
        Ok(())
    }
}
