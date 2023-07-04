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
mod attach;
mod channel;
mod error;
mod format;
mod image;
mod network;

use crate::channel::{use_channel_action, ChannelAction};
use crate::error::ActionError;
use crate::format::{BindMount, EnvPair, IpWant, PublishSpec};
use crate::image::{use_image_action, ImageAction};
use crate::network::{use_network_action, NetworkAction};

use clap::Parser;
use freebsd::event::{eventfd, EventFdNotify};
use freebsd::procdesc::{pd_fork, pdwait, PdForkResult};
use ipc::packet::codec::{Fd, List, Maybe};
use oci_util::digest::OciDigest;
use oci_util::image_reference::ImageReference;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use term_table::homogeneous::{TableLayout, TableSource, Title};
use term_table::{ColumnLayout, Pos};
use tracing::debug;
use xc::container::request::{MountReq, NetworkAllocRequest};
use xc::models::jail_image::JailConfig;
use xc::models::network::DnsSetting;
use xc::tasks::{ImportImageState, ImportImageStatus};
use xcd::ipc::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PushImageStatus {
    pub layers: Vec<OciDigest>,
    pub current_upload: Option<usize>,
    pub push_config: bool,
    pub push_manifest: bool,
    pub done: bool,
    pub fault: Option<String>,
}

#[derive(Parser, Debug)]
struct Args {
    #[clap(short = 'd', action)]
    print_args: bool,
    #[clap(short = 's', long = "socket-path")]
    socket_path: Option<String>,
    #[clap(subcommand)]
    action: Action,
}

#[allow(clippy::large_enum_variant)]
#[derive(Parser, Debug)]
enum Action {
    Attach {
        name: String,
    },
    #[clap(subcommand)]
    Channel(ChannelAction),
    Commit {
        #[clap(long)]
        name: String,
        #[clap(long)]
        tag: String,
        container_name: String,
    },
    #[clap(subcommand)]
    Image(ImageAction),
    Info,
    Kill {
        name: String,
    },
    Link {
        name: String,
    },
    #[clap(subcommand)]
    Network(NetworkAction),
    Ps {
        #[clap(short = 'H', action)]
        no_print_header: bool,
        format: Option<String>,
    },
    Pull {
        image_id: ImageReference,
    },

    Push {
        image_reference: ImageReference,
        new_image_reference: ImageReference,
    },
    Rdr {
        #[clap(long = "publish", short = 'p', multiple_occurrences = true)]
        publish: Vec<PublishSpec>,
        name: String,
    },
    Run {
        #[clap(long, default_value_t, action)]
        no_clean: bool,
        #[clap(long, default_value_t, action)]
        persist: bool,
        image_reference: ImageReference,
        entry_point: Option<String>,
        entry_point_args: Vec<String>,
        #[clap(long = "link", action)]
        link: bool,
        #[clap(long = "publish", short = 'p', multiple_occurrences = true)]
        publish: Vec<PublishSpec>,
        #[clap(long = "empty-dns", action)]
        empty_dns: bool,
        #[clap(long = "dns", multiple_occurrences = true)]
        dns_servers: Vec<String>,
        #[clap(long = "dns-search", multiple_occurrences = true)]
        dns_searchs: Vec<String>,
        #[clap(long = "detach", short = 'd', action)]
        detach: bool,
        #[clap(long = "network", multiple_occurrences = true)]
        networks: Vec<NetworkAllocRequest>,
        #[clap(short = 'v', multiple_occurrences = true)]
        mounts: Vec<BindMount>,
        #[clap(long = "env", short = 'e', multiple_occurrences = true)]
        envs: Vec<EnvPair>,
        #[clap(long = "name")]
        name: Option<String>,
        #[clap(long = "hostname")]
        hostname: Option<String>,
        #[clap(long = "vnet", action)]
        vnet: bool,
        #[clap(long = "ip", action)]
        ips: Vec<IpWant>,
        #[clap(long = "copy", multiple_occurrences = true)]
        copy: Vec<BindMount>,
    },

    Show {
        id: String,
    },

    Template {
        output: String,
    },

    Trace {
        name: String,
        args: Vec<String>,
    },
}

fn main() -> Result<(), ActionError> {
    tracing_subscriber::fmt::init();

    let arg = Args::parse();
    let path = arg
        .socket_path
        .clone()
        .unwrap_or_else(|| "/var/run/xc.sock".to_string());

    if arg.print_args {
        eprintln!("{arg:#?}");
        return Ok(());
    }

    let mut conn = UnixStream::connect(path)?;

    match arg.action {
        Action::Attach { name } => {
            let request = ShowContainerRequest { id: name };
            if let Ok(response) = do_show_container(&mut conn, request)? {
                let id = response.running_container.id;
                let path = format!("/var/run/xc.{id}.main");
                let path = std::path::Path::new(&path);
                if path.exists() {
                    _ = attach::run(path);
                } else {
                    eprintln!("cannot attach to container");
                }
            }
        }
        Action::Channel(action) => {
            use_channel_action(&mut conn, action)?;
        }
        Action::Commit {
            name,
            tag,
            container_name,
        } => {
            let req = CommitRequest {
                name,
                tag,
                container_name,
            };
            let response = do_commit_container(&mut conn, req)?.unwrap();
            //            let response: CommitResponse = request(&mut conn, "commit", req)?;
            eprintln!("{response:#?}");
        }
        Action::Image(action) => {
            use_image_action(&mut conn, action)?;
        }
        Action::Info => {
            let res = do_info(&mut conn, InfoRequest {})?;
            eprintln!("{res:#?}");
        }
        Action::Kill { name } => {
            let req = DestroyContainerRequest { name };
            let res = do_destroy_container(&mut conn, req)?.unwrap();
            eprintln!("{res:#?}");
        }
        Action::Link { name } => {
            let fork_result = unsafe { pd_fork().unwrap() };
            match fork_result {
                PdForkResult::Child => {
                    drop(conn);
                    let duration = std::time::Duration::from_secs(999999999999);
                    loop {
                        std::thread::sleep(duration);
                    }
                }
                PdForkResult::Parent { child, .. } => {
                    let req = LinkContainerRequest {
                        name,
                        fd: ipc::packet::codec::Fd(child.as_raw_fd()),
                    };
                    if do_link_container(&mut conn, req)?.is_ok() {
                        _ = pdwait(child.as_raw_fd());
                    }
                }
            }
        }
        Action::Network(action) => {
            _ = use_network_action(&mut conn, action);
        }
        Action::Ps {
            no_print_header,
            format,
        } => {
            let res: Vec<xc::container::ContainerManifest> =
                do_list_containers(&mut conn, ())?.unwrap();
            let fmt = format.unwrap_or_else(|| "JID,ID,IMAGE,MAIN,IPS,NAME,OS".to_string());
            display_containers(no_print_header, fmt, &res);
        }
        Action::Pull { image_id } => {
            let reqt = PullImageRequest {
                image_reference: image_id.clone(),
            };
            let res = do_pull_image(&mut conn, reqt)?;
            debug!("do_pull_image: {res:#?}");
            let mut lines_count = 0;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if lines_count > 0 {
                    eprint!("{}\x1B[0J", "\x1B[F".repeat(lines_count));
                }

                let reqt = DownloadStat {
                    image_reference: image_id.clone(),
                };

                let res = do_download_stat(&mut conn, reqt)?.unwrap();
                debug!("do_download_stat: {res:#?}");
                match res.state {
                    xc::tasks::ImportImageState::Done => {
                        eprintln!("done");
                        break;
                    }
                    _ => {
                        lines_count = render_import_status(&res);
                    }
                }
            }
        }

        Action::Push {
            image_reference,
            new_image_reference,
        } => {
            let req = PushImageRequest {
                image_reference: image_reference.clone(),
                remote_reference: new_image_reference.clone(),
            };
            if let Ok(_res) = do_push_image(&mut conn, req)? {
                let mut lines_count = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if lines_count > 0 {
                        eprint!("{}\x1B[0J", "\x1B[F".repeat(lines_count));
                    }

                    let reqt = UploadStat {
                        image_reference: image_reference.clone(),
                        remote_reference: new_image_reference.clone(),
                    };
                    let res = do_upload_stat(&mut conn, reqt)?.unwrap();
                    if let Some(error) = res.fault {
                        eprintln!("{error}");
                        return Ok(());
                    } else if res.layers.is_empty() {
                        lines_count = 1;
                        eprintln!("initializing");
                    } else if res.done {
                        eprintln!("Completed");
                        return Ok(());
                    } else {
                        lines_count = res.layers.len() + 2;
                        let x = res.current_upload.unwrap_or(0);
                        for (i, digest) in res.layers.iter().enumerate() {
                            match i.cmp(&x) {
                                Ordering::Less => eprintln!("{digest} ... done"),
                                Ordering::Equal => {
                                    let speed = res.duration_secs.and_then(|secs| {
                                        res.bytes.map(|bytes| (bytes * 8) as f64 / secs as f64)
                                    });
                                    let uploaded = res
                                        .bytes
                                        .map(|bytes| {
                                            let bytes = bytes as f64;
                                            if bytes > 1000000000.0 {
                                                format!("{:.2} GB", bytes / 1000000000.0)
                                            } else if bytes > 1000000.0 {
                                                format!("{:.2} MB", bytes / 1000000.0)
                                            } else if bytes > 1000.0 {
                                                format!("{:.2} KB", bytes / 1000.0)
                                            } else {
                                                format!("{:.2} B", bytes)
                                            }
                                        })
                                        .unwrap_or_else(|| "".to_string());
                                    let label = match speed {
                                        None => "".to_string(),
                                        Some(speed) => {
                                            if speed > 1000000000.0 {
                                                format!("{:.2} gbps", speed / 1000000000.0)
                                            } else if speed > 1000000.0 {
                                                format!("{:.2} mbps", speed / 1000000.0)
                                            } else if speed > 1000.0 {
                                                format!("{:.2} kbps", speed / 1000.0)
                                            } else {
                                                format!("{:.2} bps", speed)
                                            }
                                        }
                                    };
                                    eprintln!("{digest} ... uploading {uploaded} @ {label}");
                                }
                                Ordering::Greater => eprintln!("{digest}"),
                            };
                        }
                        if res.push_config {
                            eprintln!("Image config ... done");
                        } else {
                            eprintln!("Image config");
                        }
                        if res.push_manifest {
                            eprintln!("Image manifest ... done")
                        } else {
                            eprintln!("Image manifest")
                        }
                    }
                }
            }
        }

        Action::Rdr { name, publish } => {
            for expose in publish.iter() {
                let redirection = expose.to_host_spec();
                let request = DoRdr {
                    name: name.clone(),
                    redirection,
                };
                if let Ok(response) = do_rdr_container(&mut conn, request)? {
                    eprintln!("{response:#?}");
                }
            }
        }
        Action::Run {
            image_reference,
            detach,
            entry_point,
            entry_point_args,
            no_clean,
            persist,
            networks,
            mounts,
            envs,
            name,
            vnet,
            empty_dns,
            dns_servers,
            dns_searchs,
            hostname,
            copy,
            publish,
            link,
            ips,
        } => {
            if detach && link {
                panic!("detach and link flags are mutually exclusive");
            }

            let (res, notify) = {
                if image_reference.hostname.is_some() {
                    return Err(anyhow::anyhow!("non local image"))?;
                }

                let envs = {
                    let mut map = std::collections::HashMap::new();
                    for env in envs.into_iter() {
                        map.insert(env.key, env.value);
                    }
                    map
                };

                let dns = if empty_dns {
                    DnsSetting::Specified {
                        servers: Vec::new(),
                        search_domains: Vec::new(),
                    }
                } else if dns_servers.is_empty() && dns_searchs.is_empty() {
                    DnsSetting::Inherit
                } else {
                    DnsSetting::Specified {
                        servers: dns_servers,
                        search_domains: dns_searchs,
                    }
                };

                let hostname = hostname.or_else(|| name.clone());

                let mount_req = mounts
                    .iter()
                    .map(|mount| {
                        let source = std::fs::canonicalize(mount.source.clone())
                            .unwrap()
                            .to_string_lossy()
                            .to_string();
                        MountReq {
                            source,
                            dest: mount.destination.clone(),
                        }
                    })
                    .collect::<Vec<_>>();

                let copies: List<CopyFile> = copy
                    .into_iter()
                    .map(|bind| {
                        let file = std::fs::OpenOptions::new()
                            .read(true)
                            .open(bind.source)
                            .expect("cannot open file for reading");
                        let source = Fd(file.into_raw_fd());
                        CopyFile {
                            source,
                            destination: bind.destination,
                        }
                    })
                    .collect();

                let main_started_notify = if detach {
                    Maybe::None
                } else {
                    let fd = unsafe { eventfd(0, nix::libc::EFD_NONBLOCK) };
                    Maybe::Some(Fd(fd))
                };

                let reqt = InstantiateRequest {
                    alt_root: None,
                    name,
                    hostname,
                    copies,
                    envs,
                    vnet,
                    ipreq: networks,
                    mount_req,
                    entry_point: entry_point.unwrap_or_else(|| "main".to_string()),
                    entry_point_args,
                    no_clean,
                    main_norun: false,
                    init_norun: false,
                    deinit_norun: false,
                    persist,
                    dns,
                    image_reference,
                    ips: ips.into_iter().map(|v| v.0).collect(),
                    main_started_notify: main_started_notify.clone(),
                };

                let res = do_instantiate(&mut conn, reqt)?;
                (res, main_started_notify)
            };

            if let Ok(res) = res {
                for publish in publish.iter() {
                    let redirection = publish.to_host_spec();
                    let req = DoRdr {
                        name: res.id.clone(),
                        redirection,
                    };
                    let _res = do_rdr_container(&mut conn, req)?.unwrap();
                }

                if !detach {
                    if let Maybe::Some(notify) = notify {
                        EventFdNotify::from_fd(notify.as_raw_fd()).notified_sync();
                    }
                    //                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let id = res.id;
                    let path = format!("/var/run/xc.{id}.main");
                    let path = std::path::Path::new(&path);
                    if path.exists() {
                        _ = attach::run(path);
                    }
                }
            } else {
                eprintln!("{res:#?}");
            }
        }

        Action::Show { id } => {
            let req = ShowContainerRequest { id };
            let res: ShowContainerResponse = do_show_container(&mut conn, req)?.unwrap();
            display_container(&res.running_container);
        }
        Action::Template { output } => {
            use std::io::Write;
            let template = JailConfig::default();
            let encoded = serde_json::to_vec_pretty(&template).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&output)
                .unwrap_or_else(|_| panic!("cannot open {output} for writing"));
            file.write_all(&encoded).expect("cannot write to file");
        }
        Action::Trace { name, args } => {
            let request = ShowContainerRequest { id: name };
            if let Ok(response) = do_show_container(&mut conn, request)? {
                let jid = response.running_container.jid;
                let args = if args.is_empty() { vec!["-F".to_string(), "syscall".to_string()] } else { args };
                let mut process = std::process::Command::new("dwatch")
                    .arg("-j")
                    .arg(jid.to_string())
                    .args(args)
                    .spawn()?;
                process.wait()?;
            } else {
                eprintln!("no such container");
            }
        }
    };

    Ok(())
}

struct PrintManifest<'a>(&'a xc::container::ContainerManifest);

impl<'a> TableSource for PrintManifest<'a> {
    fn value_for_column(&self, column: &str) -> Option<String> {
        match column {
            "JID" => Some(self.0.jid.to_string()),
            "ID" => Some(self.0.id.to_string()),
            "IMAGE" => self.0.image_reference.clone().map(|i| i.to_string()),
            "MAIN" => self.0.processes.get("main").map(|proc| {
                let mut args = proc.exec.arg0.to_string();
                for arg in proc.exec.args.iter() {
                    args.push(' ');
                    args.push_str(arg.as_str());
                }
                args
            }),
            "VNET" => Some(self.0.vnet.to_string()),
            "ROOT" => Some(self.0.root.to_string()),
            "NAME" => Some(self.0.name.to_string()),
            "ALLOW" => Some(self.0.allowing.join(",")),
            "OS" => Some(if self.0.linux {
                "Linux".to_string()
            } else {
                "FreeBSD".to_string()
            }),
            "IPS" => {
                if self.0.ip_alloc.is_empty() {
                    None
                } else {
                    Some(
                        self.0
                            .ip_alloc
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                }
            }
            _ => None,
        }
    }
}

fn display_containers(
    no_print_header: bool,
    format: String,
    manifests: &[xc::container::ContainerManifest],
) {
    fn make_standard_column() -> ColumnLayout {
        ColumnLayout::align(Pos::Left, ' ')
    }

    let title = format
        .split(',')
        .map(|title| {
            let title = Title::new(&title.to_uppercase(), &title.to_uppercase());
            (title, make_standard_column())
        })
        .collect::<Vec<_>>();

    let mut layout = TableLayout::new(" ", !no_print_header, title);

    for manifest in manifests {
        layout.append_data(PrintManifest(manifest));
    }
    println!("{}", layout.flush());
}

fn display_container(manifest: &xc::container::ContainerManifest) {
    let name = &manifest.name;
    let root = &manifest.root;
    let id = &manifest.id;
    let vnet = &manifest.vnet;
    let linux = &manifest.linux;
    let no_clean = &manifest.no_clean;

    println!(
        "
name: {name}
root: {root}
id: {id}
vnet: {vnet}, linux: {linux}, no_clean: {no_clean}
networks:"
    );

    for assign in manifest.ip_alloc.iter() {
        for address in assign.addresses.iter() {
            match &assign.network {
                None => println!("    {}", address),
                Some(network) => println!("    ({network}) {}", address),
            }
        }
    }

    println!("processes:");

    for (label, process) in manifest.processes.iter() {
        let arg0 = &process.exec.arg0;
        println!("    {label}:");
        println!(
            "         pid: {}",
            if process.pid.is_none() {
                "none".to_string()
            } else {
                process.pid.unwrap().to_string()
            }
        );
        println!("        arg0: {arg0}");
        println!("        args: {:?}", process.exec.args);
        println!(
            "     started: {}",
            if process.started.is_none() {
                "none".to_string()
            } else {
                process.started.unwrap().to_string()
            }
        );
        println!(
            "      exited: {}",
            if process.exited.is_none() {
                "none".to_string()
            } else {
                process.exited.unwrap().to_string()
            }
        );
    }
}

fn render_import_status(status: &ImportImageStatus) -> usize {
    match status.state {
        ImportImageState::Unavailable => {
            println!("unavailable");
            1
        }
        ImportImageState::Faulted => {
            println!("error occured");
            1
        }
        ImportImageState::DownloadManifest => {
            println!("downloading manifest");
            1
        }
        ImportImageState::DownloadConfig => {
            println!("downloading config");
            1
        }
        ImportImageState::DownloadLayers => {
            let mut lines = 0usize;
            for layer in status.layers.clone().unwrap().iter() {
                if layer.total.is_none() {
                    println!("{}: waiting", layer.digest);
                    lines += 1;
                } else if layer.total.unwrap() == layer.downloaded {
                    println!("{}: completed", layer.digest);
                    lines += 1;
                } else {
                    println!(
                        "{}: downloading {}/{}",
                        layer.digest,
                        layer.downloaded,
                        layer.total.unwrap()
                    );
                    lines += 1;
                }
            }
            lines
        }
        ImportImageState::ExtractLayers => {
            println!("extracting layers...");
            1
        }
        ImportImageState::Done => {
            println!("done!");
            1
        }
    }
}
