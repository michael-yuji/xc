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

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info};
use xcd::ipc::*;

#[derive(Parser, Debug)]
pub(crate) struct CopyDirective {
    #[clap(long = "from")]
    from: Option<String>,
    #[clap(long = "to")]
    to: Option<String>,
    source_path: String,
    dest_path: String,
}

impl Directive for CopyDirective {
    fn from_action(action: &Action) -> Result<CopyDirective> {
        if action.directive_name != "COPY" {
            bail!("directive_name is not COPY")
        }
        let mut args = vec!["dummy".to_string()];
        args.extend(action.args.clone());
        let directive = CopyDirective::parse_from(args);

        if unsafe { freebsd::libc::geteuid() } != 0 {
            error!("Cannot run COPY as non-root for now");
            bail!("Cannot run COPY as non-root for now");
        }

        Ok(directive)
    }

    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        let dest_path = PathBuf::from(&self.dest_path);
        let source_path = PathBuf::from(&self.source_path);

        let name = self
            .to
            .as_ref()
            .or(context.container_id.as_ref())
            .expect("cannot determine destination container");
        let request = ShowContainerRequest {
            id: name.to_string(),
        };
        let response = do_show_container(&mut context.conn, request)?
            .expect("cannot determine destination container root");

        let mut dest = std::path::PathBuf::from(response.running_container.root);
        assert!(dest_path.is_absolute());
        for component in dest_path.components() {
            if component == std::path::Component::RootDir {
                continue;
            } else {
                dest.push(component);
            }
        }

        let mut source = match &self.from {
            None => source_path.as_os_str().to_os_string(),
            Some(container) => {
                let container = context
                    .containers
                    .get(container)
                    .ok_or(anyhow!("no such container"))?;
                // copy from the source container
                let request = ShowContainerRequest {
                    id: container.to_string(),
                };
                let response = do_show_container(&mut context.conn, request)?
                    .expect("cannot determine source container root");
                let mut source = std::path::PathBuf::from(response.running_container.root);
                for component in source_path.components() {
                    if component == std::path::Component::RootDir {
                        continue;
                    } else {
                        source.push(component);
                    }
                }
                source.as_os_str().to_os_string()
            }
        };

        if self.source_path.ends_with('/') || self.source_path == "." {
            source.push("/");
        }

        info!("cp -a {source:?} -> {dest:?}");

        let exit_status = std::process::Command::new("cp")
            .arg("-a")
            .arg(source)
            .arg(dest)
            .status()
            .context("cannot execute rsync")?;

        if !exit_status.success() {
            bail!("rsync return unsuccessful exit code: {exit_status}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        let input = ["oo", "--from", "abcde", "--to", "fgh", ".", "."];
        let parsed = CopyDirective::parse_from(input);
        println!("{parsed:?}");
        assert_eq!(parsed.from, Some("abcde".to_string()));
        assert_eq!(parsed.to, Some("fgh".to_string()));
        assert_eq!(parsed.source_path, ".".to_string());
        assert_eq!(parsed.dest_path, ".".to_string());
    }
}
