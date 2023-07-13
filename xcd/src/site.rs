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
use crate::context::instantiate::InstantiateBlueprint;
use anyhow::{anyhow, bail, Context};
use freebsd::event::{EventFdNotify, Notify};
use freebsd::fs::zfs::ZfsHandle;
use ipc::packet::Packet;
use ipc::proto::Request;
use ipc::transport::PacketTransport;
use std::ffi::OsString;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use tokio::sync::watch::Receiver;
use tracing::{error, info};
use xc::config::XcConfig;
use xc::container::effect::UndoStack;
use xc::container::{Container, ContainerManifest};
use xc::models::exec::Jexec;
use xc::models::jail_image::JailImage;

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
    config: Receiver<XcConfig>,
    zfs: ZfsHandle,
    root: Option<OsString>,
    zfs_origin: Option<String>,
    container: Option<Receiver<ContainerManifest>>,
    notify: Arc<Notify>,
    pub main_notify: Option<Arc<EventFdNotify>>,
    pub container_notify: Option<Arc<EventFdNotify>>,
    ctl_channel: Option<i32>,
    state: SiteState,

    control_stream: Option<UnixStream>,

    // clients who interested when the main process started
    main_started_interests: Vec<EventFdNotify>,
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
    pub fn new(id: &str, config: Receiver<XcConfig>) -> Site {
        Site {
            id: id.to_string(),
            undo: UndoStack::new(),
            config,
            zfs: ZfsHandle::default(),
            root: None,
            zfs_origin: None,
            container: None,
            notify: Arc::new(Notify::new()),
            main_notify: None,
            container_notify: None,
            ctl_channel: None,
            state: SiteState::Empty,
            main_started_interests: Vec::new(),
            control_stream: None,
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

    pub fn unwind(&mut self) -> anyhow::Result<()> {
        self.undo.pop_all().context("failure on undo")?;
        self.state = SiteState::Terminated;
        self.notify.notify_waiters();
        Ok(())
    }

    pub fn exec(&mut self, jexec: Jexec) {
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
        if let Some(stream) = self.control_stream.as_mut() {
            let _result = stream.send_packet(&packet);
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

                let container = Container {
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
                    linux_no_create_sys_dir: false,
                    linux_no_create_proc_dir: false,
                    zfs_origin,
                    dns: blueprint.dns,
                    origin_image: blueprint.origin_image,
                    allowing: blueprint.allowing,
                    image_reference: blueprint.image_reference,
                    copies: blueprint.copies,
                    default_router: blueprint.default_router,
                };

                let running_container = container
                    .start_transactionally(&mut self.undo)
                    .context("fail to start container")?;
                let container_notify = running_container.notify.clone();
                let main_started_notify = running_container.main_started_notify.clone();

                let (kq, recv) = xc::container::runner::run(running_container, sock_b);
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
                let (root, zfs_origin) = self
                    .create_rootfs(oci_config)
                    .context("cannot create root file system")?;
                self.root = Some(root);
                self.zfs_origin = zfs_origin;
                self.state = SiteState::RootFsOnly;
                Ok(())
            })
        } else {
            bail!("Site is non-empty");
        }
    }

    fn create_rootfs(&mut self, image: &JailImage) -> anyhow::Result<(OsString, Option<String>)> {
        let config = self.config.borrow().clone();
        let image_dataset = config.image_dataset;
        let container_dataset = config.container_dataset;
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
        Ok((mount_point, zfs_origin))
    }
}
