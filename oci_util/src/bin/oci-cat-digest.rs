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
use anyhow::Result;
use clap::Parser;
use oci_util::digest::OciDigest;
use oci_util::distribution::client::{BasicAuth, Registry};
use oci_util::image_reference::ImageReference;
use std::io::Write;
use std::str::FromStr;

#[derive(Parser)]
struct Arg {
    #[clap(short = 'u', long = "username")]
    username: Option<String>,
    #[clap(short = 'p', long = "password")]
    password: Option<String>,
    #[clap(long, action)]
    no_trace: bool,
    #[clap(long, action)]
    http: bool,
    reference: String,
    digest: OciDigest,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arg = Arg::parse();
    let reference = arg.reference;
    let reference = ImageReference::from_str(format!("{reference}:latest").as_str())?;
    let hostname = reference.hostname.expect("reference missing hostname part");
    let base_url = if arg.http {
        format!("http://{hostname}")
    } else {
        format!("https://{hostname}")
    };

    let basic_auth = arg.username.and_then(|username| {
        arg.password
            .map(|password| BasicAuth::new(username, password))
    });

    let registry = Registry::new(base_url, basic_auth);
    let mut session = registry.new_session(reference.name);
    let mut response = session.fetch_blob(&arg.digest).await?;
    while let Ok(Some(chunk)) = response.chunk().await {
        std::io::stdout().write_all(&chunk)?;
    }

    Ok(())
}
