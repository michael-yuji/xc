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
use oci_util::distribution::client::{BasicAuth, Registry};
use oci_util::image_reference::ImageReference;
use oci_util::models::ImageManifest;

#[derive(Parser)]
struct Arg {
    #[clap(short = 'u', long = "username")]
    username: Option<String>,
    #[clap(short = 'p', long = "password")]
    password: Option<String>,
    #[clap(long, action)]
    http: bool,
    reference: ImageReference,
    config: String,
    layers: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arg = Arg::parse();
    let reference = arg.reference;
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

    let name = reference.name;
    let registry = Registry::new(base_url, basic_auth);
    let mut session = registry.new_session(name);
    let tag = reference.tag.as_ref();

    let mut layers = Vec::new();

    for path in arg.layers.iter() {
        let typ = if path.ends_with(".zstd") || path.ends_with(".zst") || path.ends_with(".tzst") {
            "application/vnd.oci.image.layer.v1.tar+zstd"
        } else if path.ends_with(".gz") || path.ends_with(".tgz") {
            "application/vnd.oci.image.layer.v1.tar+gzip"
        } else {
            "application/vnd.oci.image.layer.v1.tar"
        };
        eprintln!("Upload file at {path} as content type {typ}");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .expect("cannot open file at path");
        let descriptor = session.upload_content(None, typ.to_string(), file).await?;
        layers.push(descriptor);
    }

    let config = std::fs::OpenOptions::new().read(true).open(&arg.config)?;
    eprintln!("Uploading config file at {}", &arg.config);
    let config = session
        .upload_content(
            None,
            "application/vnd.oci.image.config.v1+json".to_string(),
            config,
        )
        .await?;

    let manifest = ImageManifest {
        schema_version: 2,
        media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
        config,
        layers,
    };

    session.register_manifest(tag, &manifest).await?;
    Ok(())
}
