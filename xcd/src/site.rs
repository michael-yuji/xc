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
use crate::config::XcConfig;
use crate::context::instantiate::InstantiateBlueprint;

use anyhow::{anyhow, bail, Context};
use freebsd::event::{EventFdNotify, Notify};
use freebsd::fs::zfs::ZfsHandle;
use ipc::packet::Packet;
use ipc::proto::{Request, Response};
use ipc::transport::PacketTransport;
use oci_util::digest::OciDigest;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::net::IpAddr;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use xc::container::effect::UndoStack;
use xc::container::{ContainerManifest, CreateContainer};
use xc::models::exec::Jexec;
use xc::models::jail_image::JailImage;
use xc::models::network::HostEntry;

enum SiteState {
    Empty,
    RootFsOnly,
    Started,
    Terminated,
}

/// Represent an rollback able context that can be use to create a jail within it
pub struct Site {
    id: String,
    undo: UndoStack,
    config: XcConfig,
    pub(crate) zfs: ZfsHandle,
    root: Option<OsString>,
    /// The dataset contains the root of the container
    pub(crate) root_dataset: Option<String>,
    /// The dataset where the root dataset cloned from
    zfs_origin: Option<String>,
    zfs_snapshots: Vec<String>,
    container: Option<Receiver<ContainerManifest>>,
    notify: Arc<Notify>,
    pub main_notify: Option<Arc<EventFdNotify>>,
    pub container_notify: Option<Arc<EventFdNotify>>,
    ctl_channel: Option<i32>,
    state: SiteState,

    control_stream: Option<UnixStream>,

    // clients who interested when the main process started
    main_started_interests: Vec<EventFdNotify>,

    hosts_cache: HashMap<String, Vec<(String, IpAddr)>>,
}

macro_rules! guard {
    ($self:expr, $e:expr) => {
        match { $e } {
            Err(e) => {
                $self.unwind()?;
                Err(e)
            }
            Ok(t) => Ok(t),
        }
    };
}

impl Site {
    pub fn new(id: &str, config: XcConfig) -> Site {
        Site {
            id: id.to_string(),
            undo: UndoStack::new(),
            config,
            zfs: ZfsHandle::default(),
            root: None,
            root_dataset: None,
            zfs_origin: None,
            zfs_snapshots: Vec::new(),
            container: None,
            notify: Arc::new(Notify::new()),
            main_notify: None,
            container_notify: None,
            ctl_channel: None,
            state: SiteState::Empty,
            main_started_interests: Vec::new(),
            control_stream: None,
            hosts_cache: HashMap::new(),
        }
    }

    pub fn id(&self) -> String {
        self.id.to_string()
    }

    pub fn notify_main_started(&self) {
        for interest in self.main_started_interests.iter() {
            interest.notify_waiters();
        }
    }

    pub fn update_host_file(&mut self, network: &str, hosts: &[(String, IpAddr)]) {
        self.hosts_cache.insert(network.to_string(), hosts.to_vec());

        let mut host_entries = Vec::new();

        for (_, entries) in self.hosts_cache.iter() {
            for (hostname, ip_addr) in entries.iter() {
                host_entries.push(HostEntry {
                    ip_addr: *ip_addr,
                    hostname: hostname.clone(),
                })
            }
        }

        let Some(stream) = self.control_stream.as_mut() else {
            return
        };

        let packet = ipc::proto::write_request("write_hosts", host_entries).unwrap();
        let Ok(_) = stream.send_packet(&packet) else { return };
        _ = stream.recv_packet();
    }

    /// Clone and promote a snapshot to a dataset, any previous snapshot will become snapshot of
    /// the promoted dataset
    ///
    /// # Arugments
    ///
    /// `tag`: The snapshot to promote
    /// `dst_dataset`: The dataset which the promoted snapshot should become
    ///
    pub fn promote_snapshot(
        &mut self,
        tag: &str,
        dst_dataset: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        if let Some(root_dataset) = &self.root_dataset {
            self.zfs_snapshots
                .iter()
                .position(|s| s == tag)
                .context("no such snapshot")?;
            self.zfs.clone2(root_dataset, tag, dst_dataset.as_ref())?;
            self.zfs.promote(dst_dataset.as_ref())?;
            self.zfs_snapshots.drain(..self.zfs_snapshots.len());
        }
        Ok(())
    }

    pub fn snapshot_with_generated_tag(&mut self) -> anyhow::Result<String> {
        let mut tag = xc::util::gen_id();
        while self.zfs_snapshots.contains(&tag) {
            tag = xc::util::gen_id();
        }
        self.snapshot(&tag)?;
        Ok(tag)
    }

    pub fn snapshot(&mut self, tag: &str) -> anyhow::Result<()> {
        if self.zfs_snapshots.contains(&String::from(tag)) {
            bail!("duplicate tag");
        }
        if let Some(root_dataset) = &self.root_dataset {
            self.zfs.snapshot2(root_dataset, tag)?;
            self.zfs_snapshots.push(tag.to_string())
        }
        Ok(())
    }

    pub fn commit_to_file(
        &mut self,
        start_tag: &str,
        end_tag: &str,
        file: File,
    ) -> anyhow::Result<(OciDigest, OciDigest)> {
        let Some(root_dataset) = &self.root_dataset else {
            bail!("container is not backed by zfs");
        };

        let mut contains_start_tag = false;
        let mut contains_end_tag = false;

        for tag in self.zfs_snapshots.iter() {
            if tag == start_tag {
                contains_start_tag = true;
            } else if tag == end_tag {
                if !contains_start_tag {
                    bail!("end tag detected before start tag");
                }
                contains_end_tag = true;
            }
        }

        if !contains_start_tag {
            bail!("no such start tag");
        }

        if !contains_end_tag {
            bail!("no such end tag");
        }

        let output = std::process::Command::new("ocitar")
            .arg("-cf-")
            .arg("--write-to-stderr")
            .stdout(file)
            .arg("--compression")
            .arg("zstd")
            .stderr(Stdio::piped())
            .arg("--zfs-diff")
            .arg(format!("{root_dataset}@{start_tag}"))
            .arg(format!("{root_dataset}@{end_tag}"))
            .output()
            .context("cannot spawn ocitar")?;

        let (diff_id, digest) = {
            let mut results = std::str::from_utf8(&output.stderr).unwrap().trim().lines();
            let diff_id = results.next().expect("unexpceted output");
            let digest = results.next().expect("unexpected output");
            (
                OciDigest::from_str(diff_id).unwrap(),
                OciDigest::from_str(digest).unwrap(),
            )
        };
        Ok((diff_id, digest))
    }

    pub fn unwind(&mut self) -> anyhow::Result<()> {
        self.undo.pop_all().context("failure on undo")?;
        self.state = SiteState::Terminated;
        self.notify.notify_waiters();
        Ok(())
    }

    pub fn exec(
        &mut self,
        jexec: Jexec,
    ) -> ipc::proto::GenericResult<xc::container::process::SpawnInfo> {
        use ipc::packet::codec::FromPacket;
        use ipc::proto::ipc_err;

        let value = serde_json::to_value(jexec).unwrap();
        let request = Request {
            method: "exec".to_string(),
            value,
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let packet = Packet {
            data: encoded,
            fds: Vec::new(),
        };

        let Some(stream) = self.control_stream.as_mut() else {
            return ipc_err(freebsd::libc::ENOENT, "no much control stream")
        };

        let _result = stream.send_packet(&packet);

        let Ok(packet) = stream.recv_packet() else {
            return ipc_err(freebsd::libc::EIO, "no response from container")
        };

        let response =
            Response::from_packet(packet, |bytes| serde_json::from_slice(bytes).unwrap());

        if response.errno == 0 {
            Ok(serde_json::from_value(response.value).unwrap())
        } else {
            ipc_err(freebsd::libc::EIO, "something went wrong")
        }
    }

    /// XXX: CHANGE ME
    pub fn run_main(&mut self, main_notify: Option<EventFdNotify>) {
        let request = Request {
            method: "run_main".to_string(),
            value: serde_json::json!({}),
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let packet = Packet {
            data: encoded,
            fds: Vec::new(),
        };
        if let Some(stream) = self.control_stream.as_mut() {
            let _result = stream.send_packet(&packet);
        }
        if let Some(interest) = main_notify {
            self.main_started_interests.push(interest);
        }
    }

    pub fn link_fd(&mut self, fd: RawFd) {
        self.undo.dup_fd(fd).unwrap();
    }

    pub fn kill_conatiner(&mut self) -> anyhow::Result<()> {
        use freebsd::nix::sys::event::{kevent_ts, EventFilter, EventFlag, FilterFlag, KEvent};
        let event = KEvent::new(
            2,
            EventFilter::EVFILT_USER,
            EventFlag::EV_ONESHOT,
            FilterFlag::NOTE_TRIGGER | FilterFlag::NOTE_FFNOP,
            0 as freebsd::libc::intptr_t,
            0 as freebsd::libc::intptr_t,
        );
        info!(id = self.id, "killing container");
        _ = kevent_ts(self.ctl_channel.unwrap(), &[event], &mut [], None);
        Ok(())
    }

    pub fn container_dump(&self) -> Option<ContainerManifest> {
        self.container.clone().map(|c| c.borrow().clone())
    }

    pub fn run_container(&mut self, blueprint: InstantiateBlueprint) -> anyhow::Result<()> {
        guard!(self, {
            let (sock_a, sock_b) = UnixStream::pair().unwrap();
            self.control_stream = Some(sock_a);

            if let SiteState::RootFsOnly = self.state {
                let root = self.root.clone().unwrap().to_string_lossy().to_string();
                let zfs_origin = self.zfs_origin.clone();

                for (i, layer_fd) in blueprint.extra_layers.iter().enumerate() {
                    let file = unsafe { std::fs::File::from_raw_fd(*layer_fd) };
                    info!("extracting extra layer: {i}");
                    let exit_status = std::process::Command::new("ocitar")
                        .arg("-xf-")
                        .arg("-C")
                        .arg(&root)
                        .stdin(file)
                        .status()
                        .with_context(|| format!("extracting extra layers at offset {i}"))?;
                    if !exit_status.success() {
                        error!(
                            "ocitar exit with {exit_status} while extract the extra layers at {i}"
                        );
                        bail!(
                            "ocitar exit with unsuccessful exit code {exit_status} at offset {i}"
                        );
                    }
                }

                if blueprint.extra_layers.is_empty() {
                    info!("no extra layers to extract");
                }

                let container = CreateContainer {
                    name: blueprint.name,
                    hostname: blueprint.hostname,
                    id: blueprint.id,
                    root,
                    devfs_ruleset_id: blueprint.devfs_ruleset_id,
                    vnet: blueprint.vnet,
                    init: blueprint.init,
                    deinit: blueprint.deinit,
                    main: blueprint.main,
                    ip_alloc: blueprint.ip_alloc,
                    mount_req: blueprint.mount_req,
                    linux: blueprint.linux,
                    deinit_norun: blueprint.deinit_norun,
                    init_norun: blueprint.init_norun,
                    main_norun: blueprint.main_norun,
                    persist: blueprint.persist,
                    no_clean: blueprint.no_clean,
                    linux_no_create_sys_dir: blueprint.linux_no_create_sys_dir,
                    linux_no_mount_sys: blueprint.linux_no_mount_sys,
                    linux_no_create_proc_dir: blueprint.linux_no_create_proc_dir,
                    linux_no_mount_proc: blueprint.linux_no_mount_proc,
                    zfs_origin,
                    origin_image: blueprint.origin_image,
                    allowing: blueprint.allowing,
                    image_reference: blueprint.image_reference,
                    default_router: blueprint.default_router,
                    log_directory: Some(std::path::PathBuf::from(&self.config.logs_dir)),
                };

                let running_container = container
                    .create_transactionally(&mut self.undo)
                    .context("fail to start container")?;

                _ = running_container.setup_resolv_conf(&blueprint.dns);

                for copy in blueprint.copies.into_iter() {
                    _ = running_container.copyin(&copy);
                }

                let container_notify = running_container.notify.clone();
                let main_started_notify = running_container.main_started_notify.clone();

                let (kq, recv) =
                    xc::container::runner::run(running_container, sock_b, !blueprint.create_only);

                self.container = Some(recv);
                self.ctl_channel = Some(kq);
                self.container_notify = Some(container_notify);
                self.main_notify = Some(main_started_notify);
                self.state = SiteState::Started;
                if let Some(interest) = blueprint.main_started_notify {
                    self.main_started_interests.push(interest);
                }
                Ok(())
            } else if let SiteState::Empty = self.state {
                Err(anyhow!("site does not contain a valid file system"))
            } else {
                Err(anyhow!("site has invalid state"))
            }
        })
    }

    pub fn stage(&mut self, oci_config: &JailImage) -> anyhow::Result<()> {
        if let SiteState::Empty = self.state {
            guard!(self, {
                self.create_rootfs(oci_config)
                    .context("cannot create root file system")?;
                self.state = SiteState::RootFsOnly;
                Ok(())
            })
        } else {
            bail!("Site is non-empty");
        }
    }

    fn create_rootfs(&mut self, image: &JailImage) -> anyhow::Result<()> {
        let config = &self.config;
        let image_dataset = &config.image_dataset;
        let container_dataset = &config.container_dataset;
        let dest_dataset = format!("{container_dataset}/{}", self.id);
        let source_dataset = image.chain_id().map(|id| format!("{image_dataset}/{id}"));
        let zfs_origin;
        match source_dataset {
            None => {
                zfs_origin = None;
                self.undo
                    .zfs_create(self.zfs.clone(), dest_dataset.clone())
                    .context("while creating dataset for container")?;
            }
            Some(source_dataset) => {
                zfs_origin = Some(source_dataset.clone());
                self.undo
                    .zfs_clone(
                        self.zfs.clone(),
                        source_dataset,
                        "xc".to_string(),
                        dest_dataset.clone(),
                    )
                    .context("while cloning dataset for container")?;
            }
        }

        let mount_point = self
            .zfs
            .mount_point(dest_dataset.clone())
            .with_context(|| format!("cannot get mount point for {dest_dataset}"))?
            .with_context(|| format!("dataset {dest_dataset} does not have a mount point"))?
            .into_os_string();
        self.root_dataset = Some(dest_dataset);
        self.root = Some(mount_point);
        self.zfs_origin = zfs_origin;

        self.snapshot("init").context("fail on initial snapshot")?;

        Ok(())
    }
}
