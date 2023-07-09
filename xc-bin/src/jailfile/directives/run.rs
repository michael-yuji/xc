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
//

use crate::jailfile::JailContext;
use crate::jailfile::parse::Action;
use super::Directive;

use anyhow::{bail, Result};
use freebsd::event::EventFdNotify;
use ipc::packet::codec::{Maybe, Fd};
use nix::unistd::pipe;
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use xcd::ipc::*;

enum Input {
    File(File),
    Content(String),
    None
}

pub(crate) struct RunDirective {
    arg0: String,
    args: Vec<String>,
    envs: HashMap<String, String>,
    input: Input
}

impl Directive for RunDirective {
    fn from_action(action: &Action) -> Result<RunDirective> {
        let mut args_iter = action.args.iter();
        let mut envs = HashMap::new();
        let mut args = Vec::new();

        let mut curr = args_iter.next();

        while let Some((key, value)) = curr.and_then(|c| c.split_once('=')) {
            envs.insert(key.to_string(), value.to_string());
            curr = args_iter.next();
        }

        let Some(arg0) = curr else {
            bail!("cannot determine arg0");
        };

        for arg in args_iter {
            args.push(arg.to_string());
        }

        let input = match &action.heredoc {
            Some(value) => Input::Content(value.to_string()),
            None => Input::None
        };

        Ok(RunDirective {
            arg0: arg0.to_string(),
            args,
            envs,
            input
        })
    }

    fn run_in_context(&self, context: &mut JailContext) -> Result<()> {
        let notify = EventFdNotify::new();
        /*
        let (stdout_a, stdout_b) = pipe()?;
        let (stderr_a, stderr_b) = pipe()?;
        let (stdin_a, stdin_b) = pipe()?;
        */
        eprintln!("arg0: {}, args: {:?}", self.arg0, self.args);
        let request = ExecCommandRequest {
            name: context.container_id.clone().expect("container not set"),
            arg0: self.arg0.clone(),
            args: self.args.clone(),
            envs: self.envs.clone(),
            stdin: Maybe::None,//Maybe::Some(Fd(stdin_b)),
            stdout:Maybe::None,// Maybe::Some(Fd(stdout_b)),
            stderr:Maybe::None,// Maybe::Some(Fd(stderr_b)),
            uid: 0,
            notify: Maybe::Some(Fd(notify.as_raw_fd()))
        };
        eprintln!("before do_exec");
        match do_exec(&mut context.conn, request)? {
            Ok(_) => {
                eprintln!("before wait sync");
                notify.notified_sync();
                eprintln!("after wait sync");
                Ok(())
            },
            Err(err) => bail!("exec failure: {err:?}")
        }
    }
}
