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

use super::Directive;
use crate::jailfile::parse::Action;
use crate::jailfile::JailContext;

use anyhow::{bail, Result};
use ipc::packet::codec::Maybe;
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use xc::util::gen_id;
use xcd::ipc::*;

pub(crate) struct FromDirective {
    image_reference: ImageReference,
    alias: Option<String>,
}

impl Directive for FromDirective {
    fn up_to_date(&self) -> bool {
        // XXX: Technically, we should check against the image manifest digest to verify the same
        // tag is still pointing to the same image, but hey, I'm lazy!
        true
    }
    fn from_action(action: &Action) -> Result<FromDirective> {
        if action.directive_name != "FROM" {
            bail!("directive_name is not FROM");
        }
        let image = action.args.first().expect("no image specified");
        let image_reference: ImageReference = image.parse().expect("invalid image reference");
        if action.args.len() > 1 {
            let Some("as") = action.args.get(1).map(|s| s.as_str()) else { bail!("unexpected ariable") };
            let alias = action.args.get(2).expect("expected alias");
            Ok(FromDirective {
                image_reference,
                alias: Some(alias.to_string()),
            })
        } else {
            Ok(FromDirective {
                image_reference,
                alias: None,
            })
        }
    }

    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        if let Some(container_id) = &context.container_id {
            let tagged_containers = context.containers.values().collect::<Vec<_>>();
            if !tagged_containers.contains(&container_id) {
                bail!("cannot switch to another container when previous one isn't tagged");
            }
        }
        let name = format!("build-{}", gen_id());

        /* create container */
        let req = InstantiateRequest {
            name: Some(name.to_string()),
            dns: context.dns.clone(),
            image_reference: self.image_reference.clone(),
            main_norun: true,
            init_norun: true,
            deinit_norun: true,
            persist: true,
            main_started_notify: Maybe::None,
            entry_point: Some(EntryPointSpec {
                entry_point: None,
                entry_point_args: Vec::new(),
            }),
            envs: HashMap::new(),
            ipreq: context.network.clone(),
            ..InstantiateRequest::default()
        };

        eprintln!("before instantiate");
        match do_instantiate(&mut context.conn, req)? {
            Ok(response) => {
                eprintln!("instantiate resspones: {response:?}");
                if let Some(alias) = &self.alias {
                    context
                        .containers
                        .insert(alias.to_string(), name.to_string());
                }
                context.container_id = Some(name);
                Ok(())
            }
            Err(err) => {
                bail!("instantiation failure: {err:?}")
            }
        }
    }
}
