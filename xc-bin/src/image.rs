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
use crate::format::EnvPair;

use clap::{Parser, Subcommand};
use oci_util::image_reference::ImageReference;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use varutil::string_interpolation::Var;
use xc::models::{jail_image::{JailConfig, SpecialMount}, EnvSpec, MountSpec, SystemVPropValue};
use xcd::ipc::*;

#[derive(Subcommand, Debug)]
pub(crate) enum ImageAction {
    Import {
        path: String,
        config: String,
        image_id: ImageReference,
    },
    List,
    Show {
        image_id: String,
    },
    GetConfig {
        image_id: String,
    },
    SetConfig {
        image_id: String,
        meta_path: String,
    },
    #[clap(subcommand)]
    Patch(PatchActions),
}

#[derive(Parser, Debug)]
pub(crate) enum PatchActions {
    AddEnv {
        #[clap(long, action)]
        required: bool,
        #[clap(short = 'd', long = "description")]
        description: Option<String>,
        env: String,
        image_reference: ImageReference,
    },
    /// add a new volume spec to the image
    AddVolume {
        /// some good information about what the volume is for
        #[clap(short = 'd', long = "description")]
        description: Option<String>,
        /// hints to create this volume for intelligently, for example the right block size
        #[clap(long="hint", multiple_occurrences = true)]
        hints: Vec<EnvPair>,
        /// if the volume should be mounted as read-only
        #[clap(long="read-only", action)]
        read_only: bool,
        /// a name for the 
        #[clap(long="name")]
        name: Option<String>,
        /// mount point in the container
        mount_point: PathBuf,
        /// the image
        image_reference: ImageReference
    },
    MountFdescfs { image_reference: ImageReference },
    MountProcfs { image_reference: ImageReference },
    ModAllow {
        allows: Vec<String>,
        image_reference: ImageReference
    },
    SysvIpc {
        #[clap(long="enable", multiple_occurrences = true)]
        enable: Vec<String>,
        image_reference: ImageReference
    }
}

fn patch_image<F>(
    conn: &mut UnixStream,
    image_reference: &ImageReference,
    f: F,
) -> Result<(), crate::ActionError>
where
    F: FnOnce(&mut JailConfig),
{
    let image_name = &image_reference.name;
    let tag = &image_reference.tag;
    let reqt = DescribeImageRequest {
        image_name: image_name.to_string(),
        tag: tag.to_string(),
    };
    let res = do_describe_image(conn, reqt)?;
    match res {
        Err(e) => {
            eprintln!("{e:#?}");
        }
        Ok(res) => {
            let mut config = res.jail_image.jail_config();
            f(&mut config);
            let req = SetConfigRequest {
                name: image_name.to_string(),
                tag: tag.to_string(),
                config,
            };
            _ = do_replace_meta(conn, req)?;
        }
    }
    Ok(())
}

pub(crate) fn use_image_action(
    conn: &mut UnixStream,
    action: ImageAction,
) -> Result<(), crate::ActionError> {
    match action {
        ImageAction::Patch(patch) => match patch {
            PatchActions::AddEnv {
                required,
                description,
                env,
                image_reference,
            } => {
                patch_image(conn, &image_reference, |config| {
                    let env_var = Var::new(env).expect("invalid environment variable name");
                    config.envs.insert(
                        env_var,
                        EnvSpec {
                            description,
                            required,
                        },
                    );
                })?;
            }
            PatchActions::AddVolume { description, hints, name, mount_point, image_reference, read_only } => {
                patch_image(conn, &image_reference, |config| {
                    let destination = mount_point.to_string_lossy().to_string();
                    let description = description.unwrap_or_default();
                    let key = name.unwrap_or_else(|| destination.to_string());
                    let mut volume_hints = HashMap::new();
                    for hint in hints.into_iter() {
                        volume_hints.insert(hint.key, serde_json::Value::String(hint.value));
                    }
                    for (_, mount) in config.mounts.iter() {
                        if mount.destination == destination {
                            panic!("mounts with such mountpoint already exists");
                        }
                    }
                    let mountspec = MountSpec {
                        description,
                        read_only,
                        volume_hints,
                        destination
                    };
                    config.mounts.insert(key,mountspec);
                })?;
            }
            PatchActions::ModAllow { allows, image_reference } => {
                patch_image(conn, &image_reference, |config| {
                    for allow in allows.into_iter() {
                        if let Some(param) = allow.strip_prefix('-') {
                            for i in (0..config.allow.len()).rev() {
                                if config.allow[i] == param {
                                    config.allow.remove(i);
                                }
                            }
                        } else if !config.allow.contains(&allow) {
                            config.allow.push(allow);
                        }
                    }
                })?;
            }
            PatchActions::SysvIpc { enable, image_reference } => {
                let mut enabled = Vec::new();
                for e in enable.iter() {
                    enabled.extend(e.split(',').map(|h| h.trim()).collect::<Vec<_>>());
                }
                patch_image(conn, &image_reference, |config| {
                    for e in enabled.into_iter() {
                        match e {
                            "shm" => config.sysv_shm = SystemVPropValue::New,
                            "-shm" => config.sysv_shm = SystemVPropValue::Disable,
                            "msg" => config.sysv_msg = SystemVPropValue::New,
                            "-msg" => config.sysv_msg = SystemVPropValue::Disable,
                            "sem" => config.sysv_sem = SystemVPropValue::New,
                            "-sem" => config.sysv_sem = SystemVPropValue::Disable,
                            _ => continue
                        }
                    }
                })?;
            }
            PatchActions::MountFdescfs { image_reference } => {
                patch_image(conn, &image_reference, |config| {
                    for i in (0..config.special_mounts.len()).rev() {
                        let mount = &config.special_mounts[i];
                        if mount.mount_type.as_str() == "fdescfs"
                            && mount.mount_point.as_str() == "/dev/fd"
                        {
                            break;
                        }
                    }
                    config.special_mounts.push(
                        SpecialMount {
                            mount_type: "fdescfs".to_string(),
                            mount_point: "/dev/fd".to_string()
                        }
                    );
                })?;
            },
            PatchActions::MountProcfs { image_reference } => {
                patch_image(conn, &image_reference, |config| {
                    for i in (0..config.special_mounts.len()).rev() {
                        let mount = &config.special_mounts[i];
                        if mount.mount_type.as_str() == "procfs"
                            && mount.mount_point.as_str() == "/proc"
                        {
                            break;
                        }
                    }
                    config.special_mounts.push(
                        SpecialMount {
                            mount_type: "procfs".to_string(),
                            mount_point: "/proc".to_string()
                        }
                    );
                })?;
            },
        },
        ImageAction::Import {
            image_id,
            path,
            config,
        } => {
            use std::os::fd::AsRawFd;
            let config_file = std::fs::OpenOptions::new().read(true).open(config).unwrap();
            let config: xc::models::jail_image::JailConfig =
                serde_json::from_reader(config_file).unwrap();
            let file = std::fs::OpenOptions::new().read(true).open(path).unwrap();
            let fd = ipc::packet::codec::Fd(file.as_raw_fd());
            let request = FdImport {
                fd,
                config,
                image_reference: image_id,
            };
            let response = do_fd_import(conn, request);
            eprintln!("{response:#?}");
        }
        ImageAction::List => {
            let reqt = ListManifestsRequest {};
            //            let res: ListManifestsResponse2 = request(conn, "list_manifests", reqt)?;
            if let Ok(res) = do_list_all_images(conn, reqt)? {
                let names = res
                    .manifests
                    .iter()
                    .map(|row| format!("{}:{}", row.name, row.tag))
                    .collect::<Vec<_>>();
                println!("{names:#?}");
            }
        }
        ImageAction::Show { image_id } => {
            let (image_name, tag) = image_id.rsplit_once(':').expect("invalid image id");
            let reqt = DescribeImageRequest {
                image_name: image_name.to_string(),
                tag: tag.to_string(),
            };
            let res = do_describe_image(conn, reqt)?;
            match res {
                //                Err(DescribeImageError::ImageReferenceNotFound) => eprintln!("Image reference not found"),
                Err(e) => eprintln!("{e:#?}"),

                Ok(res) => {
                    let json = serde_json::to_string_pretty(&res).unwrap();
                    println!("{json}");
                }
            }
        }
        ImageAction::GetConfig { image_id } => {
            let (image_name, tag) = image_id.rsplit_once(':').expect("invalid image id");
            let reqt = DescribeImageRequest {
                image_name: image_name.to_string(),
                tag: tag.to_string(),
            };
            let res = do_describe_image(conn, reqt)?;
            match res {
                //                Err(DescribeImageError::ImageReferenceNotFound) => eprintln!("Image reference not found"),
                Err(e) => eprintln!("{e:#?}"),
                Ok(res) => {
                    let meta = res.jail_image.jail_config();
                    let json = serde_json::to_string_pretty(&meta).unwrap();
                    println!("{json}");
                }
            }
        }
        ImageAction::SetConfig {
            image_id,
            meta_path,
        } => {
            let (name, tag) = image_id.rsplit_once(':').expect("invalid image id");

            let config: xc::models::jail_image::JailConfig = if meta_path == *"-" {
                //let input = std::io::read_to_string(std::io::stdin())?;
                serde_json::from_reader(std::io::stdin()).unwrap()
            } else {
                let meta_file = std::fs::OpenOptions::new()
                    .read(true)
                    .open(&meta_path)
                    .unwrap();
                serde_json::from_reader(meta_file).unwrap()
            };

            let req = SetConfigRequest {
                name: name.to_string(),
                tag: tag.to_string(),
                config,
            };
            let manifest = do_replace_meta(conn, req)?;
            //            let manifest = request(conn, "replace_meta", req)?;
            eprintln!("{manifest:#?}");
        }
    }
    Ok(())
}
