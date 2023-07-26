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

mod patch;

use crate::image::patch::PatchActions;
use anyhow::Context;
use clap::Parser;
use oci_util::image_reference::ImageReference;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use xc::models::jail_image::JailConfig;
use xcd::ipc::*;

#[derive(Parser, Debug)]
pub(crate) enum ImageAction {
    Import {
        path: String,
        image_id: ImageReference,
        /// Optionally import a configuration file as skeleton
        #[arg(long = "config-file", short = 'f')]
        config_file: Option<PathBuf>,

        #[arg(trailing_var_arg = true)]
        subcommands: Vec<String>,
    },
    List,
    Show {
        image_id: String,
    },
    Describe {
        image_id: ImageReference,
    },
    GetConfig {
        #[arg(short = 'f', default_value = "yaml")]
        format: String,
        image_id: ImageReference,
    },
    Remove {
        image_id: ImageReference,
    },
    SetConfig {
        image_id: String,
        config_file: String,
    },
    Patch {
        #[command(subcommand)]
        action: PatchActions,
        image_reference: ImageReference,
    },
}

pub(crate) fn patch_image<F>(
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
        ImageAction::Import {
            path,
            image_id,
            config_file,
            subcommands,
        } => {
            let commands = subcommands.split(|s| s == "--");
            let phantom_cmd = "<cmds...>".to_string();
            let patches = commands.map(|command| {
                let mut cv = std::collections::VecDeque::from_iter(command);
                cv.push_front(&phantom_cmd);
                PatchActions::parse_from(cv)
            });

            let mut config = match config_file {
                None => JailConfig::default(),
                Some(path) => {
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .open(path)
                        .context("cannot open config file")?;
                    serde_yaml::from_reader(file).context("cannot parse config file")?
                }
            };

            for patch in patches {
                patch.do_patch(&mut config);
            }

            let file = std::fs::OpenOptions::new()
                .read(true)
                .open(path)
                .context("cannot open archive")?;
            let fd = ipc::packet::codec::Fd(file.as_raw_fd());
            let request = FdImport {
                fd,
                config,
                image_reference: image_id,
            };

            let response = do_fd_import(conn, request);
            eprintln!("{response:#?}");
        }
        ImageAction::Patch {
            action,
            image_reference,
        } => {
            patch_image(conn, &image_reference, |c| action.do_patch(c))?;
        }
        ImageAction::List => {
            let reqt = ListManifestsRequest {};
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
                Err(e) => eprintln!("{e:#?}"),
                Ok(res) => {
                    let json = serde_json::to_string_pretty(&res).unwrap();
                    println!("{json}");
                }
            }
        }
        ImageAction::Describe { image_id } => {
            let image_name = image_id.name.to_string();
            let tag = image_id.tag.to_string();
            let reqt = DescribeImageRequest { image_name, tag };
            let res = do_describe_image(conn, reqt)?;
            match res {
                Err(e) => eprintln!("{e:#?}"),
                Ok(res) => {
                    let image = &res.jail_image;
                    let config = image.jail_config();
                    println!("\n{image_id}");
                    println!("    Envs:");
                    for (key, value) in config.envs.iter() {
                        println!("        {key}:");
                        println!("            Required: {}", value.required);
                        println!(
                            "            Description: {}",
                            value.description.clone().unwrap_or_default()
                        );
                    }
                    println!("    Ports:");
                    for (port, value) in config.ports.iter() {
                        println!("        {port}: {value}");
                    }
                    println!("    Volumes:");
                    for (name, spec) in config.mounts.iter() {
                        println!("        {name}:");
                        println!("            Mount Point: {}", spec.destination);
                        println!("            Required: {}", spec.required);
                        println!("            Read-Only: {}", spec.read_only);
                        if !spec.volume_hints.is_empty() {
                            println!("                  Hints:");
                            for (key, value) in spec.volume_hints.iter() {
                                let desc = serde_json::to_string(value).unwrap();
                                println!("            {key}: {desc}");
                            }
                        }
                    }
                }
            }
        }
        ImageAction::GetConfig { format, image_id } => {
            let reqt = DescribeImageRequest {
                image_name: image_id.name.to_string(),
                tag: image_id.tag.to_string(),
            };
            let res = do_describe_image(conn, reqt)?;
            match res {
                //                Err(DescribeImageError::ImageReferenceNotFound) => eprintln!("Image reference not found"),
                Err(e) => eprintln!("{e:#?}"),
                Ok(res) => {
                    let config = res.jail_image.jail_config();
                    let output = match format.to_lowercase().as_str() {
                        "yaml" => serde_yaml::to_string(&config).unwrap(),
                        "json" => serde_json::to_string_pretty(&config).unwrap(),
                        _ => Err(anyhow::anyhow!("Unknown format"))?,
                    };
                    println!("{output}");
                }
            }
        }
        ImageAction::Remove { image_id } => {
            _ = do_remove_image(conn, image_id)?;
        }
        ImageAction::SetConfig {
            image_id,
            config_file,
        } => {
            let (name, tag) = image_id.rsplit_once(':').expect("invalid image id");

            let input: Box<dyn std::io::Read> = if config_file == *"-" {
                Box::new(std::io::stdin())
            } else {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .open(&config_file)
                    .context("cannot open config file")?;
                Box::new(file)
            };

            let config: JailConfig =
                serde_yaml::from_reader(input).context("cannot parse input")?;

            let req = SetConfigRequest {
                name: name.to_string(),
                tag: tag.to_string(),
                config,
            };
            let manifest = do_replace_meta(conn, req)?;
            eprintln!("{manifest:#?}");
        }
    }
    Ok(())
}
