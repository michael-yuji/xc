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
use clap::Parser;
use std::os::unix::net::UnixStream;
use xcd::ipc::*;

use crate::format::PublishSpec;

#[derive(Parser, Debug)]
pub(crate) enum RdrAction {
    Add {
        #[clap(long = "publish", short = 'p', multiple_occurrences = true)]
        publish: Vec<PublishSpec>,
        name: String,
    },
    List {
        name: String,
        #[clap(short = 'H', action)]
        without_header: bool,
    },
}

pub(crate) fn use_rdr_action(
    conn: &mut UnixStream,
    action: RdrAction,
) -> Result<(), crate::ActionError> {
    match action {
        RdrAction::Add { name, publish } => {
            for expose in publish.iter() {
                let redirection = expose.to_host_spec();
                let request = DoRdr {
                    name: name.clone(),
                    redirection,
                };
                if let Ok(response) = do_rdr_container(conn, request)? {
                    eprintln!("{response:#?}");
                }
            }
        }
        RdrAction::List {
            name,
            without_header,
        } => {
            let response = do_list_site_rdr(conn, ContainerRdrList { name })?;
            if let Ok(response) = response {
                if !without_header {
                    let count = response.len();
                    println!("{count} redirection(s)");
                }
                for rdr in response.iter() {
                    println!("{}", rdr.to_pf_rule());
                }
            }
        }
    }
    Ok(())
}