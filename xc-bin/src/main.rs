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
mod jailfile;
mod network;
mod redirect;
mod run;
mod volume;

use crate::channel::{use_channel_action, ChannelAction};
use crate::error::ActionError;
use crate::format::{format_bandwidth, format_capacity};
use crate::image::{use_image_action, ImageAction};
use crate::jailfile::directives::volume::VolumeDirective;
use crate::network::{use_network_action, NetworkAction};
use crate::redirect::{use_rdr_action, RdrAction};
use crate::run::{CreateArgs, DnsArgs, RunArg};
use crate::volume::{use_volume_action, VolumeAction};

use clap::Parser;
use freebsd::event::{eventfd, EventFdNotify};
use freebsd::libc::EXIT_FAILURE;
use freebsd::procdesc::{pd_fork, pdwait, PdForkResult};
use ipc::packet::codec::{Fd, Maybe};
use oci_util::digest::OciDigest;
use oci_util::image_reference::ImageReference;
use run::PublishArgs;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use term_table::homogeneous::{TableLayout, TableSource, Title};
use term_table::{ColumnLayout, Pos};
use tracing::{debug, error, info};
use xc::container::request::NetworkAllocRequest;
use xc::container::runner::process_stat::decode_exit_code;
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
    #[arg(short = 'd', action)]
    print_args: bool,
    #[arg(short = 's', long = "socket-path")]
    socket_path: Option<String>,
    #[command(subcommand)]
    action: Action,
}

#[allow(clippy::large_enum_variant)]
#[derive(Parser, Debug)]
enum Action {
    /// Attach to the tty of the main process of the container, if available
    Attach {
        name: String,
    },
    Build {
        #[arg(long = "network")]
        network: Option<String>,
        #[arg(long = "dns" /*, multiple_occurrences = true*/)]
        dns_servers: Vec<String>,
        #[arg(long = "dns_search", /* multiple_occurrences = true */)]
        dns_searchs: Vec<String>,
        #[arg(long = "empty-dns", action)]
        empty_dns: bool,
        #[arg(long = "output-inplace", action)]
        output_inplace: bool,
        image_reference: ImageReference,
    },
    #[command(subcommand)]
    Channel(ChannelAction),
    Commit {
        #[arg(long)]
        name: String,
        #[arg(long)]
        tag: String,
        container_name: String,
    },
    Create {
        #[command(flatten)]
        create: CreateArgs,
        #[command(flatten)]
        publish: PublishArgs,
    },
    #[command(subcommand)]
    Image(ImageAction),
    Info,
    /// Kill a container by either name, jail id, or id
    Kill {
        name: String,
    },
    Link {
        name: String,
    },
    /// Login to a container registry
    ///
    /// This command does not actually verify the username/password against the registry, but just
    /// record the credential for later use
    Login {
        /// Username
        #[arg(long = "username", short = 'u')]
        username: String,
        #[arg(long = "password", short = 'p')]
        /// Password
        password: Option<String>,
        /// Take the password from stdin
        #[arg(long = "password-stdin", action)]
        password_stdin: bool,
        /// The server uses http instead of https
        #[arg(long = "insecure", action)]
        insecure: bool,
        /// The target server
        server: String,
    },
    #[command(subcommand)]
    Network(NetworkAction),
    Ps {
        #[arg(short = 'H', action)]
        no_print_header: bool,
        format: Option<String>,
    },
    /// Remove un-referenced resources
    Purge,
    /// Pull image from registries
    Pull {
        /// The image to pull, in the format of {registry}/{repo}:{tag}, if registry is missing,
        /// assume the default registry
        image_id: ImageReference,
        /// Rename the imported image
        local_reference: Option<ImageReference>,
    },
    /// Upload a locally available image to the remote registry
    Push {
        #[arg(long = "insecure", default_value_t)]
        insecure: bool,
        /// The local image to push
        image_reference: ImageReference,
        /// Destination of the upload
        new_image_reference: ImageReference,
    },
    #[command(subcommand)]
    Rdr(RdrAction),
    Run {
        #[command(flatten)]
        create: CreateArgs,
        #[command(flatten)]
        dns: DnsArgs,
        #[command(flatten)]
        publish: PublishArgs,
        #[command(flatten)]
        args: RunArg,
    },
    RunMain {
        #[arg(long = "detach", short = 'd', action)]
        detach: bool,
        name: String,
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

    Exec {
        #[arg(short = 't', action)]
        terminal: bool,
        #[arg(long = "user", short = 'u')]
        user: Option<String>,
        #[arg(long = "group", short = 'g')]
        group: Option<String>,
        name: String,
        arg0: String,
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    #[command(subcommand)]
    Volume(VolumeAction),
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
        Action::Build {
            image_reference,
            network,
            dns_servers,
            dns_searchs,
            empty_dns,
            output_inplace,
        } => {
            use crate::jailfile::directives::add_env::*;
            use crate::jailfile::directives::copy::*;
            use crate::jailfile::directives::from::*;
            use crate::jailfile::directives::run::*;
            use crate::jailfile::directives::*;
            use crate::jailfile::parse::*;
            use crate::jailfile::*;
            let file = std::fs::read_to_string("Jailfile")?;

            let net_req = network
                .map(|network| vec![NetworkAllocRequest::Any { network }])
                .unwrap_or_default();

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

            let is_image_existed = do_describe_image(&mut conn, image_reference.clone())?;

            if is_image_existed.is_ok() {
                Err(anyhow::anyhow!("image already exist"))?;
            }

            let actions = parse_jailfile(&file)?;

            if unsafe { freebsd::libc::geteuid() } != 0 {
                for action in actions.iter() {
                    if action.directive_name == "COPY" {
                        error!(
                            "This Jailfile contains COPY directive(s) but is not running as root,"
                        );
                        error!("due to the lack of time to properly implement COPY by the developer(me)");
                        error!("This should be solved in some later release say v0.0.1");
                        std::process::exit(1)
                    }
                }
            }

            let mut context = JailContext::new(conn, dns, net_req, output_inplace);

            for action in actions.iter() {
                macro_rules! do_directive {
                    ($name:expr, $tpe:ty) => {
                        if action.directive_name == $name {
                            let directive = <$tpe>::from_action(action)?;
                            directive.run_in_context(&mut context)?;
                            continue;
                        }
                    };
                }

                do_directive!("RUN", RunDirective);
                do_directive!("COPY", CopyDirective);
                do_directive!("VOLUME", VolumeDirective);
                do_directive!("ADDENV", AddEnvDirective);

                if action.directive_name == "FROM" {
                    let directive = FromDirective::from_action(action)?;
                    directive.run_in_context(&mut context)?;
                } else if ConfigMod::implemented_directives()
                    .contains(&action.directive_name.as_str())
                {
                    let directive = ConfigMod::from_action(action)?;
                    directive.run_in_context(&mut context)?;
                }
            }
            context.release(image_reference)?;
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
                alt_out: Maybe::None,
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
            let req = KillContainerRequest { name };
            let res = do_kill_container(&mut conn, req)?.unwrap();
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
        Action::Login {
            username,
            password,
            password_stdin,
            server,
            insecure,
        } => {
            if password.is_none() && !password_stdin {
                eprintln!("at least --password <password> at --password-stdin required");
                std::process::exit(1)
            }

            let password = password.unwrap_or_else(|| {
                println!("Enter password: ");
                rpassword::read_password().unwrap()
            });

            let request = LoginRequest {
                username,
                password,
                server,
                insecure,
            };
            if let Err(err) = do_login_registry(&mut conn, request)? {
                eprintln!("error: {err:#?}");
            }
        }
        Action::Network(action) => {
            _ = use_network_action(&mut conn, action);
        }
        Action::Purge => {
            do_purge(&mut conn, ())?.unwrap();
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
        Action::Pull {
            image_id,
            local_reference,
        } => {
            let reqt = PullImageRequest {
                image_reference: image_id.clone(),
                rename_reference: local_reference,
            };
            let res = do_pull_image(&mut conn, reqt)?;

            debug!("do_pull_image: {res:#?}");
            match res {
                Err(err) => {
                    if let Some(msg) = err
                        .value
                        .as_object()
                        .and_then(|map| map.get("error"))
                        .and_then(|v| v.as_str())
                    {
                        error!("{msg}");
                    } else {
                        error!("{err:?}");
                    }
                }
                Ok(_) => {
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
            }
        }

        Action::Push {
            insecure,
            image_reference,
            new_image_reference,
        } => {
            let req = PushImageRequest {
                image_reference: image_reference.clone(),
                remote_reference: new_image_reference.clone(),
                insecure,
            };
            match do_push_image(&mut conn, req)? {
                Ok(_) => {
                    let mut lines_count = 0;
                    let mut last_pulled_ms: Option<u128> = None;
                    let mut last_pulled_bytes: std::collections::HashMap<OciDigest, usize> =
                        std::collections::HashMap::new();
                    //                    let mut last_pulled_ms: std::collections::HashMap<OciDigest, u128> = std::collections::HashMap::new();
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
                            let x = res.current_upload_idx.unwrap_or(0);
                            for (i, digest) in res.layers.iter().enumerate() {
                                match i.cmp(&x) {
                                    Ordering::Less => eprintln!("{digest} ... done"),
                                    Ordering::Equal => {
                                        let uploaded =
                                            res.bytes.map(format_capacity).unwrap_or_default();

                                        let (avg, bandwidth) = res
                                            .bytes
                                            .and_then(|bytes| {
                                                let b = bytes
                                                    - last_pulled_bytes.get(digest).unwrap_or(&0);
                                                last_pulled_bytes.insert(digest.clone(), bytes);
                                                res.duration_ms.map(|ms| {
                                                    (
                                                        format_bandwidth(bytes, ms),
                                                        format_bandwidth(
                                                            b,
                                                            ms - last_pulled_ms.unwrap_or_default(),
                                                        ),
                                                    )
                                                })
                                            })
                                            .unwrap_or_default();

                                        let total = res
                                            .current_layer_size
                                            .map(format_capacity)
                                            .unwrap_or_default();
                                        eprintln!("{digest} ... uploading {uploaded}/{total} @ {bandwidth} (avg {avg})");
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
                        if let Some(ms) = res.duration_ms {
                            last_pulled_ms = Some(ms);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("cannot push image: {err:#?}")
                }
            }
        }
        Action::Rdr(rdr) => {
            _ = use_rdr_action(&mut conn, rdr);
        }
        Action::Create { create, publish } => {
            let publish = publish.publish.clone();

            let res = {
                let reqt = create.create_request()?;
                do_instantiate(&mut conn, reqt)?
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
            } else {
                eprintln!("{res:#?}");
            }
        }
        Action::Run {
            create,
            dns,
            publish,
            args,
        } => {
            if args.detach && args.link {
                panic!("detach and link flags are mutually exclusive");
            }

            if dns.dns_nop && dns.empty_dns {
                panic!("--dns-nop and --empty-dns are mutually exclusive");
            }

            let publish = publish.publish.clone();

            let dns = dns.make();

            let (res, main_started_notify, main_exited_notify) = {
                let main_started_notify = if args.detach {
                    Maybe::None
                } else {
                    let fd = unsafe { eventfd(0, freebsd::nix::libc::EFD_NONBLOCK) };
                    Maybe::Some(Fd(fd))
                };

                let main_exited_notify = if args.detach {
                    None
                } else {
                    Some(unsafe { eventfd(0, freebsd::nix::libc::EFD_NONBLOCK) })
                };

                let stdin = if args.detach || !args.interactive {
                    Maybe::None
                } else {
                    Maybe::Some(Fd::stdin())
                };

                let mut reqt = InstantiateRequest {
                    dns,
                    create_only: false,
                    main_norun: false,
                    init_norun: false,
                    deinit_norun: false,
                    main_started_notify: main_started_notify.clone(),
                    entry_point: Some(EntryPointSpec {
                        entry_point: args.entry_point,
                        entry_point_args: args.entry_point_args,
                    }),
                    main_exited_fd: Maybe::from_option(main_exited_notify.map(Fd)),
                    port_redirections: publish.into_iter().map(|p| p.to_host_spec()).collect(),
                    use_tty: args.terminal,
                    stdin,
                    stdout: if args.detach {
                        Maybe::None
                    } else {
                        Maybe::Some(Fd::stdout())
                    },
                    stderr: if args.detach {
                        Maybe::None
                    } else {
                        Maybe::Some(Fd::stderr())
                    },
                    ..create.create_request()?
                };

                if args.terminal {
                    if let Ok(term) = std::env::var("TERM") {
                        reqt.envs.insert("TERM".to_string(), term.to_string());
                    }
                }

                let res = do_instantiate(&mut conn, reqt)?;
                let exit_notify = main_exited_notify.map(EventFdNotify::from_fd);
                (res, main_started_notify, exit_notify)
            };

            if let Ok(res) = res {
                if !res.require_clearence.is_empty() {
                    println!(
                        "this container require exposing these additional device nodes (y/n):"
                    );
                    for dev in res.require_clearence.iter() {
                        println!("    {dev}");
                    }

                    let mut s = String::new();
                    std::io::stdin()
                        .read_line(&mut s)
                        .expect("cannot read user input");
                    if s.to_lowercase().starts_with('y') {
                        let req = ContinueInstantiateRequest {
                            id: res.id.to_string(),
                            clearences: res.require_clearence.clone(),
                        };
                        _ = do_continue_instantiate(&mut conn, req)?;
                    } else {
                        std::process::exit(0)
                    }
                }

                if !args.detach {
                    if let Maybe::Some(notify) = main_started_notify {
                        EventFdNotify::from_fd(notify.as_raw_fd()).notified_sync();
                    }
                    let id = res.id;

                    if args.terminal {
                        if let Ok(container) =
                            do_show_container_nocache(&mut conn, ShowContainerRequest { id })?
                        {
                            if let Some(reason) = container.running_container.fault {
                                error!("Container faulted {reason}");
                            } else {
                                let spawn_info = container
                                    .running_container
                                    .processes
                                    .get("main")
                                    .as_ref()
                                    .and_then(|proc| proc.spawn_info.as_ref())
                                    .expect("process not started yet or not found");
                                if let Some(socket) = &spawn_info.terminal_socket {
                                    if let Ok(exit_by_user) = attach::run(socket) {
                                        if !exit_by_user {
                                            if let Some(notify) = main_exited_notify {
                                                if let Ok(exit_value) =
                                                    notify.notified_sync_take_value()
                                                {
                                                    let exit_status = decode_exit_code(exit_value);
                                                    std::process::exit(
                                                        exit_status.code().unwrap_or(EXIT_FAILURE),
                                                    )
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    info!("main process is not running with tty");
                                }
                            }
                        } else {
                            panic!("cannot find container");
                        }
                    } else if let Some(notify) = main_exited_notify {
                        if let Ok(exit_value) = notify.notified_sync_take_value() {
                            let exit_status = decode_exit_code(exit_value);
                            std::process::exit(exit_status.code().unwrap_or(EXIT_FAILURE))
                        }
                    }
                }
            } else {
                eprintln!("{res:#?}");
            }
        }
        Action::RunMain { detach, name } => {
            let notify = if detach {
                Maybe::None
            } else {
                let fd = unsafe { eventfd(0, freebsd::nix::libc::EFD_NONBLOCK) };
                Maybe::Some(Fd(fd))
            };

            let req = RunMainRequest {
                name: name.to_string(),
                notify: notify.clone(),
            };

            if let Ok(_res) = do_run_main(&mut conn, req)? {
                if !detach {
                    if let Maybe::Some(notify) = notify {
                        EventFdNotify::from_fd(notify.as_raw_fd()).notified_sync();
                        if let Ok(container) =
                            do_show_container(&mut conn, ShowContainerRequest { id: name })?
                        {
                            let spawn_info = container
                                .running_container
                                .processes
                                .get("main")
                                .as_ref()
                                .and_then(|proc| proc.spawn_info.as_ref())
                                .expect("process not started yet or not found");
                            if let Some(socket) = &spawn_info.terminal_socket {
                                _ = attach::run(socket);
                            } else {
                                info!("main process is not running with tty");
                            }
                        } else {
                            panic!("cannot find container");
                        }
                    }
                }
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
                let args = if args.is_empty() {
                    vec!["-F".to_string(), "syscall".to_string()]
                } else {
                    args
                };
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
        Action::Exec {
            terminal,
            name,
            arg0,
            args,
            user,
            group,
        } => {
            let n = EventFdNotify::new();
            let mut envs = std::collections::HashMap::new();

            if terminal {
                if let Ok(term) = std::env::var("TERM") {
                    envs.insert("TERM".to_string(), term.to_string());
                }
            }

            let request = ExecCommandRequest {
                name,
                arg0,
                args,
                envs,
                stdin: if terminal {
                    Maybe::None
                } else {
                    Maybe::Some(Fd::stdin())
                },
                stdout: if terminal {
                    Maybe::None
                } else {
                    Maybe::Some(Fd::stdout())
                },
                stderr: if terminal {
                    Maybe::None
                } else {
                    Maybe::Some(Fd::stderr())
                },
                user,
                group,
                notify: Maybe::Some(ipc::packet::codec::Fd(n.as_raw_fd())),
                use_tty: terminal,
            };

            match do_exec(&mut conn, request)? {
                Ok(response) => {
                    if let Some(socket) = response.terminal_socket {
                        _ = attach::run(socket);
                    }
                    debug!("waiting for process to exit");
                    let exit = n.notified_sync_take_value()?;
                    let exit_status = decode_exit_code(exit);
                    debug!(exit_status=exit_status.code(), "process exited");
                    std::process::exit(exit_status.code().unwrap_or(EXIT_FAILURE))
                }
                Err(err) => {
                    eprintln!("{err:?}")
                }
            }
        }
        Action::Volume(action) => {
            use_volume_action(&mut conn, action)?;
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
            if process.pid().is_none() {
                "none".to_string()
            } else {
                process.pid().unwrap().to_string()
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
