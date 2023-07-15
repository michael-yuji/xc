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
use crate::jailfile::parse::Action;

use anyhow::Result;
use clap::Parser;
use varutil::string_interpolation::Var;
use xc::models::EnvSpec;

#[derive(Parser)]
pub(crate) struct AddEnvDirective {
    #[clap(long = "require", action)]
    require: bool,
    #[clap(short = 'd', long = "description")]
    description: Option<String>,
    variable: Var,
}

impl Directive for AddEnvDirective {
    fn up_to_date(&self) -> bool {
        true
    }
    fn from_action(action: &Action) -> Result<AddEnvDirective> {
        let mut args = vec!["dummy".to_string()];
        args.extend(action.args.clone());
        let directive = AddEnvDirective::parse_from(args);
        Ok(directive)
    }
    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        let mount_spec = EnvSpec {
            description: self.description.clone(),
            required: self.require,
        };
        context
            .config_mods
            .push(ConfigMod::AddEnv(self.variable.clone(), mount_spec));
        Ok(())
    }
}
