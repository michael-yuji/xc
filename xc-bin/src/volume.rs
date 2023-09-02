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

use std::{os::unix::net::UnixStream, path::PathBuf};

use anyhow::anyhow;
use clap::Parser;
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use xcd::ipc::*;
use xcd::volume::VolumeDriverKind;

#[derive(Parser, Debug)]
pub(crate) enum VolumeAction {
    Create {
        name: String,
        #[arg(short = 'i', long = "image")]
        image_reference: Option<ImageReference>,
        #[arg(short = 'v', long = "volume")]
        volume: Option<String>,
        /// The alternative path
        #[arg(short = 's')]
        device: Option<PathBuf>,
        /// ZFS mount options
        #[arg(short = 'o', long = "zfs-option")]
        zfs_props: Vec<String>,

        driver: VolumeDriverKind,
    },
    List
}

pub(crate) fn use_volume_action(conn: &mut UnixStream, action: VolumeAction)
    -> Result<(), crate::ActionError>
{
    match action {
        VolumeAction::List => {
            if let Ok(volumes) = do_list_volumes(conn, ())? {
                println!("{volumes:#?}")
            }
        },
        VolumeAction::Create { name, image_reference, volume, device, zfs_props, driver } => {
            let template = image_reference.and_then(|ir| {
                volume.map(|v| (ir, v))
            });
            let zfs_props = {
                let mut props = HashMap::new();
                for value in zfs_props.into_iter() {
                    if let Some((key, value)) = value.split_once('=') {
                        props.insert(key.to_string(), value.to_string());
                    } else {
                        Err(anyhow!("invalid zfs option, accepted formats are $key=$value"))?;
                    }
                }
                props
            };
            let request = CreateVolumeRequest {
                name,
                template,
                device,
                zfs_props,
                kind: driver
            };
            if let Err(err) = do_create_volume(conn, request)? {
                eprintln!("error occurred: {err:#?}")
            }
        }
    }
    Ok(())
}
