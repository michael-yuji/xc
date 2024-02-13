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

mod control_stream;
mod process_stat;

use self::control_stream::{ControlStream, Readiness};
use self::process_stat::ProcessRunnerStat;

use crate::container::error::ExecError;
use crate::container::process::*;
use crate::container::running::RunningContainer;
use crate::container::{ContainerManifest, ProcessStat};
use crate::elf::{brand_elf_if_unsupported, ElfBrand};
use crate::models::exec::{IpcJexec, Jexec, StdioMode};
use crate::models::network::HostEntry;
use crate::util::{epoch_now_nano, exists_exec};

use anyhow::Context;
use freebsd::event::{kevent_classic, EventFdNotify, KEventExt};
use freebsd::nix::libc::intptr_t;
use freebsd::nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent};
use freebsd::FreeBSDCommandExt;
use ipc::packet::codec::json::JsonPacket;
use ipc::packet::codec::FromPacket;
use jail::process::Jailed;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch::{channel, Receiver};
use tracing::{debug, error, info, trace, warn};

#[usdt::provider]
mod container_runner {
    use crate::models::exec::Jexec;
    fn spawn_process(jid: i32, id: &str, exec: &Jexec) {}
    fn cleanup_enter(jid: i32) {}
}

#[derive(Debug)]
pub struct ProcessRunner {
    kq: i32,
    named_process: Vec<ProcessRunnerStat>,
    parent_map: HashMap<u32, u32>,

    // mapping from a children to its descentdant
    descentdant_map: HashMap<u32, Vec<u32>>,

    control_streams: HashMap<i32, ControlStream>,

    /// If `auto_start` is true, the container executes its init routine automatically after
    /// create
    auto_start: bool,

    container: RunningContainer,

    main_exited: bool,

    // a queue containing the processes to be spawn by the end of event loop
    spawn_queue: VecDeque<(String, Jexec)>,

    inits: SerialExec,

    deinits: SerialExec,

    should_kill: bool,
}

/// Processes that should start synchronously; that the next process should start if and only if
/// the previous process (but not all the descentdant of previous proces) exited
#[derive(Debug)]
struct SerialExec {
    base_id: String,
    idx: usize,
    execs: VecDeque<Jexec>,
    last_spawn: Option<String>,
    activated: bool,
}

impl SerialExec {
    fn new(base_id: &str, execs: Vec<Jexec>, activated: bool) -> SerialExec {
        SerialExec {
            base_id: base_id.to_string(),
            execs: VecDeque::from(execs),
            idx: 0,
            last_spawn: None,
            activated,
        }
    }

    fn activate(&mut self) {
        self.activated = true;
    }

    fn is_empty(&self) -> bool {
        self.execs.is_empty()
    }

    fn pop_front(&mut self) -> Option<(String, Jexec)> {
        let exec_id = format!("{}.{}", self.base_id, self.idx);
        let exec = self.execs.pop_front()?;
        self.last_spawn = Some(exec_id.to_string());
        self.idx += 1;
        Some((exec_id, exec))
    }

    fn try_drain_proc_queue(
        &mut self,
        id: &str,
        next_processes: &mut VecDeque<(String, Jexec)>,
    ) -> bool {
        if self.activated {
            match &self.last_spawn {
                Some(last_spawn) if last_spawn != id => false,
                Some(last_spawn) if last_spawn == id && self.is_empty() => {
                    info!("{} drained", self.base_id);
                    true
                }
                _ => {
                    if !self.is_empty() {
                        let exec_id = format!("{}.{}", self.base_id, self.idx);
                        let exec = self.execs.pop_front().unwrap();
                        self.last_spawn = Some(exec_id.to_string());
                        next_processes.push_back((exec_id, exec));
                        self.idx += 1;
                    }
                    false
                }
            }
        } else {
            false
        }
    }
}

impl ProcessRunner {
    fn add_control_stream(&mut self, control_stream: ControlStream) {
        debug!("adding control stream");
        let fd = control_stream.socket_fd();
        self.control_streams.insert(fd, control_stream);
        let read_event = KEvent::from_read(fd);
        _ = kevent_classic(self.kq, &[read_event], &mut []);
    }

    fn find_exec(&self, env_path: &str, exec: &str) -> Option<PathBuf> {
        let root = Path::new(&self.container.root).to_path_buf();
        let exec_path = Path::new(&exec);

        if exec_path.is_absolute() {
            exists_exec(root, exec_path, 64).unwrap()
        } else {
            env_path
                .split(':')
                .map(|s| s.to_string())
                .find_map(|search_path| {
                    let mut path = PathBuf::from(&search_path);
                    path.push(exec);
                    exists_exec(&root, path, 64).unwrap()
                })
        }
    }

    fn spawn_process(
        &mut self,
        id: &str,
        exec: &Jexec,
        exit_notify: Option<Arc<EventFdNotify>>,
        notify: Option<Arc<EventFdNotify>>,
    ) -> Result<SpawnInfo, ExecError> {
        info!("spawn: {exec:#?}");
        container_runner::spawn_process!(|| (self.container.jid, id, exec));

        let mut envs = self.container.envs.clone();

        let jail = freebsd::jail::RunningJail::from_jid_unchecked(self.container.jid);
        let paths = envs
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| "/bin:/usr/bin:/sbin:/usr/sbin".to_string());
        let path = self
            .find_exec(&paths, &exec.arg0)
            .ok_or(ExecError::ExecutableNotFound)?;

        let (tx, rx) = channel(ProcessStat::new(exec.clone()));

        if self.container.linux {
            if !freebsd::exists_kld("linux") && !freebsd::exists_kld("linux64") {
                return Err(ExecError::MissingLinuxKmod);
            }
            brand_elf_if_unsupported(path, ElfBrand::Linux).map_err(ExecError::BrandELFFailed)?;
        }

        let uid = match exec.uid {
            Some(uid) => uid,
            None => match &exec.user {
                Some(user) => unsafe {
                    if let Some(user) = freebsd::get_uid_in_jail(self.container.jid, user)
                        .ok()
                        .flatten()
                    {
                        user
                    } else {
                        return Err(ExecError::NotSuchUser(user.to_string()));
                    }
                },
                None => 0,
            },
        };

        let gid = match exec.gid {
            Some(gid) => gid,
            None => match &exec.group {
                Some(group) => unsafe {
                    if let Some(group) = freebsd::get_gid_in_jail(self.container.jid, group)
                        .ok()
                        .flatten()
                    {
                        group
                    } else {
                        return Err(ExecError::NotSuchGroup(group.to_string()));
                    }
                },
                None => 0,
            },
        };


        for (key, value) in exec.envs.iter() {
            envs.insert(key.to_string(), value.to_string());
        }

        if let Some(address) = self.container.main_address() {
            // allow faking the environ to make debugging easier
            if !envs.contains_key("XC_MAIN_IP") {
                envs.insert("XC_MAIN_IP".to_string(), address.address.to_string());
            }
            if !envs.contains_key("XC_MAIN_IFACE") {
                envs.insert("XC_MAIN_IFACE".to_string(), address.interface);
            }
        }

        let mut networks_count = 0;
        for network in self.container.networks() {
            networks_count += 1;
            let network_name = network.network.as_ref().unwrap();
            envs.insert(
                format!("XC_NETWORK_{network_name}_ADDR_COUNT"),
                network.addresses.len().to_string()
            );
            envs.insert(
                format!("XC_NETWORK_{network_name}_IFACE"),
                network.interface.to_string(),
            );
            for (i, addr) in network.addresses.iter().enumerate() {
                envs.insert(format!("XC_NETWORK_{network_name}_ADDR_{i}"), addr.to_string());
            }
        }

        envs.insert("XC_NETWORKS_COUNT".to_string(), networks_count.to_string());

        envs.insert("XC_ID".to_string(), self.container.id.to_string());


        let mut cmd = std::process::Command::new(&exec.arg0);

        cmd.env_clear()
            .args(&exec.args)
            .envs(envs)
            .jail(&jail)
            .juid(uid)
            .jgid(gid);

        let devnull = std::path::PathBuf::from("/dev/null");
        let spawn_info_result = match &exec.output_mode {
            StdioMode::Terminal => {
                let socket_path = format!("/var/run/xc.{}.{}", self.container.id, id);
                let log_path = self
                    .container
                    .log_directory
                    .clone()
                    .map(|path| {
                        let mut path = path;
                        path.push(format!("xc.{}.{id}.log", self.container.id));
                        path
                    })
                    .unwrap_or_else(|| devnull.clone());
                spawn_process_pty(cmd, log_path, socket_path)
            }
            StdioMode::Files { stdout, stderr } => spawn_process_files(&mut cmd, stdout, stderr),
            StdioMode::Inherit => {
                let (out_path, err_path) = self
                    .container
                    .log_directory
                    .clone()
                    .map(|path| {
                        let mut path = path;
                        let mut path2 = path.clone();
                        path.push(format!("xc.{}.{id}.out.log", self.container.id));
                        path2.push(format!("xc.{}.{id}.err.log", self.container.id));
                        (path, path2)
                    })
                    .unwrap_or_else(|| (devnull.clone(), devnull));
                spawn_process_files(&mut cmd, &Some(out_path), &Some(err_path))
            }
            StdioMode::Forward {
                stdin,
                stdout,
                stderr,
            } => spawn_process_forward(&mut cmd, *stdin, *stdout, *stderr),
        };

        let spawn_info = spawn_info_result.map_err(|error| {
            if let Some(n) = notify.clone() {
                n.notify_waiters();
            }
            error
        })?;

        let pid = spawn_info.pid;

        tx.send_if_modified(|status| {
            status.set_started(spawn_info.clone());
            true
        });

        let rstat = ProcessRunnerStat {
            pid,
            id: id.to_string(),
            process_stat: tx,
            exit_notify,
            tree_exit_notify: notify,
        };

        self.container.processes.insert(id.to_string(), rx);

        self.named_process.push(rstat);
        self.descentdant_map.insert(pid, vec![pid]);
        let event = KEvent::from_trace_pid(pid, FilterFlag::NOTE_EXIT);
        _ = kevent_classic(self.kq, &[event], &mut []);

        Ok(spawn_info)
    }

    fn pid_ancestor(&self, pid: u32) -> u32 {
        let mut pid = pid;
        while let Some(parent) = self.parent_map.get(&pid) {
            pid = *parent;
        }
        pid
    }

    pub fn new(kq: i32, container: RunningContainer, auto_start: bool) -> ProcessRunner {
        ProcessRunner {
            kq,
            named_process: Vec::new(),
            parent_map: HashMap::new(),
            descentdant_map: HashMap::new(),
            control_streams: HashMap::new(),
            spawn_queue: VecDeque::new(),
            inits: SerialExec::new("init", container.init_proto.clone(), !container.init_norun),
            deinits: SerialExec::new("deinit", container.deinit_proto.clone(), false),
            main_exited: false,
            container,
            auto_start,
            should_kill: false,
        }
    }

    #[inline]
    pub fn run_main(&mut self) {
        if let Some(main) = self.container.main_proto.clone() {
            self.spawn_queue.push_back(("main".to_string(), main));
        }
    }

    fn handle_control_stream_cmd(
        &mut self,
        mut fd: i32,
        method: String,
        request: JsonPacket,
    ) -> anyhow::Result<()> {
        use ipc::proto::write_response;
        use ipc::transport::PacketTransport;

        let packet = if method == "exec" {
            let jexec = IpcJexec::from_packet_failable(request, |value| {
                serde_json::from_value(value.clone())
            })
            .context("cannot deserialize jexec")?;

            let jexec = jexec.to_local();

            let notify = Arc::new(EventFdNotify::from_fd(jexec.notify.unwrap()));
            let result = self.spawn_process(&crate::util::gen_id(), &jexec, Some(notify), None);

            match result {
                Ok(spawn_info) => write_response(0, spawn_info).unwrap(),
                Err(err) => {
                    error!("exec error: {err:?}");
                    write_response(
                        freebsd::libc::EIO,
                        serde_json::json!({
                            "message": format!("failed to spawn process in container: {err:?}")
                        }),
                    )
                    .unwrap()
                }
            }
        } else if method == "run_main" {
            if let Some(main) = self.container.main_proto.clone() {
                self.spawn_queue.push_back(("main".to_string(), main));
                // XXX: implement me
                write_response(0, ()).unwrap()
            } else {
                write_response(0, ()).unwrap()
            }
        } else if method == "start" {
            self.start();
            write_response(0, ()).unwrap()
        } else if method == "kill" {
            self.should_kill = true;
            write_response(0, ()).unwrap()
        } else if method == "write_hosts" {
            let recv: Vec<HostEntry> = serde_json::from_value(request.data).unwrap();
            if let Ok(host_path) = crate::util::realpath(&self.container.root, "/etc/hosts") {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(host_path)
                {
                    _ = writeln!(&mut file, "::1 localhost");
                    _ = writeln!(&mut file, "127.0.0.1 localhost");
                    for entry in recv.iter() {
                        _ = writeln!(&mut file, "{} {}", entry.ip_addr, entry.hostname);
                    }
                }
            }
            write_response(0, ()).unwrap()
        } else if method == "query_manifest" {
            let manifest = self.container.serialized();
            write_response(0, manifest).unwrap()
        } else {
            todo!()
        };

        fd.send_packet(&packet)
            .context("failure on writing response packet for method \"{method}\"")?;

        Ok(())
    }

    fn start(&mut self) {
        if self.container.started.is_none() {
            self.container.started = Some(epoch_now_nano());
            if self.inits.is_empty() && !self.container.main_norun {
                self.run_main();
            } else if let Some((id, jexec)) = self.inits.pop_front() {
                self.inits.activate();
                _ = self.spawn_process(&id, &jexec, None, None);
            }
        } else {
            error!("self.start() is called but the container has already started!")
        }
    }

    fn handle_pid_event(&mut self, event: KEvent, last_deinit: &mut Option<String>) -> bool {
        let fflag = event.fflags();
        let pid = event.ident() as u32;
        if fflag.contains(FilterFlag::NOTE_EXIT) {
            let ancestor = self.pid_ancestor(pid);
            self.parent_map.remove(&pid);

            let descentdant = self.descentdant_map.get_mut(&ancestor).unwrap();
            trace!(
                pid,
                ancestor,
                "NOTE_EXIT: {pid} exited; ancestor: {ancestor}"
            );

            if let Some(pos) = descentdant.iter().position(|x| *x == pid) {
                descentdant.remove(pos);
            }
            let descentdant_gone = descentdant.is_empty();
            if descentdant_gone {
                debug!("all descentdant of pid {ancestor} are gone");
            }

            if ancestor == pid || descentdant_gone {
                for stat in self.named_process.iter_mut() {
                    if stat.pid() == ancestor {
                        if ancestor == pid {
                            stat.set_exited(event.data() as i32);
                            info!("exited: {}", event.data());
                            unsafe {
                                freebsd::nix::libc::waitpid(pid as i32, std::ptr::null_mut(), 0)
                            };

                            if self
                                .inits
                                .try_drain_proc_queue(stat.id(), &mut self.spawn_queue)
                                && !self.container.main_norun
                            {
                                if let Some(main) = self.container.main_proto.clone() {
                                    self.spawn_queue.push_back(("main".to_string(), main));
                                }
                            }

                            if self
                                .deinits
                                .try_drain_proc_queue(stat.id(), &mut self.spawn_queue)
                            {
                                *last_deinit = self.deinits.last_spawn.clone();
                                // allow for the last deinit action to run at most
                                // 15 seconds
                                let event = KEvent::from_timer_seconds_oneshot(1486, 15);
                                _ = kevent_classic(self.kq, &[event], &mut []);
                            }
                        }
                        if descentdant_gone {
                            stat.set_tree_exited();
                            if stat.id() == "main" {
                                self.main_exited = true;
                                self.container.finished_at = Some(epoch_now_nano());
                                if (self.container.deinit_norun || self.deinits.is_empty())
                                    && !self.container.persist
                                {
                                    return true;
                                } else {
                                    debug!("activating deinit queue");
                                    self.deinits.activate();
                                    self.deinits.try_drain_proc_queue("", &mut self.spawn_queue);
                                }
                            } else if let Some(last_deinit) = last_deinit.clone() {
                                if last_deinit == stat.id() && !self.container.persist {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        } else if fflag.contains(FilterFlag::NOTE_CHILD) {
            let parent = event.data() as u32;
            let ancestor = self.pid_ancestor(parent);
            trace!("NOTE_CHILD: parent {parent}, pid {pid}, ancestor: {ancestor}");
            self.parent_map.insert(pid, ancestor);
            let v = self
                .descentdant_map
                .get_mut(&ancestor)
                .expect("cannot find ancestor");
            v.push(pid);
        }

        false
    }

    fn run(mut self, mut sender: std::os::unix::net::UnixStream) {
        let mut events = vec![KEvent::zero(); 64];
        let kq = self.kq;
        let kill_event = KEvent::new(
            2,
            EventFilter::EVFILT_USER,
            EventFlag::EV_ONESHOT | EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::NOTE_FFNOP,
            0 as intptr_t,
            0 as intptr_t,
        );

        _ = kevent_classic(kq, &[kill_event], &mut []);

        let mut last_deinit = None;

        if self.auto_start {
            self.start();
        }

        'kq: loop {
            let mut should_update = false;
            while let Some((id, process)) = self.spawn_queue.pop_front() {
                should_update = true;
                match self.spawn_process(&id, &process, None, None) {
                    Ok(spawn_info) => {
                        debug!("{id} spawn: {spawn_info:#?}");
                        if id == "main" {
                            self.container.started = Some(epoch_now_nano());
                            self.send_update(&mut sender);
                            self.container.main_started_notify.notify_waiters();
                        }
                    }
                    Err(error) => {
                        if id == "main" {
                            self.container.fault = Some(format!("{error:#?}"));
                            self.send_update(&mut sender);
                            self.container
                                .main_started_notify
                                .notify_waiters_with_value(2);

                            self.main_exited = true;
                            self.container.finished_at = Some(epoch_now_nano());
                            if (self.container.deinit_norun || self.deinits.is_empty())
                                && !self.container.persist
                            {
                                self.should_kill = true;
                            } else {
                                debug!("activating deinit queue");
                                self.deinits.activate();
                                self.deinits.try_drain_proc_queue("", &mut self.spawn_queue);
                            }
                        }
                        error!("cannot spawn {id}: {process:#?} {error:#?}")
                    }
                }
            }
            if should_update {
                self.send_update(&mut sender);
            }

            let nevx = kevent_classic(kq, &[], &mut events);
            let nev = nevx.unwrap();

            for event in &events[..nev] {
                match event.filter().unwrap() {
                    EventFilter::EVFILT_PROC => {
                        if self.handle_pid_event(*event, &mut last_deinit) {
                            self.should_kill = true;
                        }
                    }
                    EventFilter::EVFILT_TIMER => {
                        if !self.container.persist {
                            // the only timer event is the killer event
                            warn!("deinit time out reached, proceed to kill jail");
                            break 'kq;
                        } else if let Some(id) = last_deinit.as_ref() {
                            // only kill the last deinit
                            for process in self.named_process.iter() {
                                if process.id() == id {
                                    if let Some(pids) = self.descentdant_map.get(&process.pid()) {
                                        for pid in pids.iter() {
                                            let pid =
                                                freebsd::nix::unistd::Pid::from_raw(*pid as i32);
                                            _ = freebsd::nix::sys::signal::kill(
                                                pid,
                                                freebsd::nix::sys::signal::Signal::SIGKILL,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    EventFilter::EVFILT_READ => {
                        let fd = event.ident() as i32;
                        if event.data() == 0 {
                            self.control_streams.remove(&fd);
                        } else if let Some(control_stream) = self.control_streams.get_mut(&fd) {
                            match control_stream.try_get_request(event.data() as usize) {
                                Err(_) => {
                                    self.control_streams.remove(&fd);
                                }
                                Ok(Readiness::Pending) => {}
                                Ok(Readiness::Ready((method, request))) => {
                                    let handled =
                                        self.handle_control_stream_cmd(fd, method, request);
                                    if let Err(error) = handled {
                                        error!(
                                            "closing control_stream {fd} due to error: {error:#?}"
                                        );
                                        self.control_streams.remove(&fd);
                                    }
                                }
                            }
                            if self.should_kill {
                                break 'kq;
                            }
                        }
                    }
                    EventFilter::EVFILT_USER => {
                        debug!("{event:#?}");
                        if self.container.deinit_norun || self.deinits.is_empty() {
                            break 'kq;
                        } else {
                            debug!("activating deinit queue");
                            self.deinits.activate();
                            self.deinits.try_drain_proc_queue("", &mut self.spawn_queue);
                        }
                    }
                    _ => {
                        debug!("{event:#?}");
                    }
                }
            }

            if self.should_kill {
                break 'kq;
            }
        }

        self.cleanup(&mut sender);
    }

    fn cleanup(&mut self, stream: &mut UnixStream) {
        let jid = self.container.jid;
        container_runner::cleanup_enter!(|| (jid));
        let jail = freebsd::jail::RunningJail::from_jid_unchecked(jid);

        for jailed_dataset in self.container.jailed_datasets.iter() {
            let handle = freebsd::fs::zfs::ZfsHandle::default();
            info!("unjailing dataset: {jailed_dataset:?}");
            if let Err(error) = handle.unjail(&jid.to_string(), jailed_dataset) {
                warn!("cannot unjail dataset {jailed_dataset:?} from {jid}: {error}");
            }
        }

        let kill = jail.kill().context("cannot kill jail").map_err(|e| {
            error!("cannot kill jail: {e}");
            e
        });

        info!("jail kill: {kill:#?}");

        // allow 5 seconds for the jail to be killed
        //            std::thread::sleep(std::time::Duration::from_secs(5));
        //
        let epoch = epoch_now_nano();

        info!("cleaning up at {:#?}", epoch / 1_000_000_000);
        self.container.deleted = Some(epoch);

        self.send_update(stream);
        self.container.notify.notify_waiters();
    }

    fn send_update(&self, stream: &mut UnixStream) {
        use ipc::proto::write_request;
        use ipc::transport::PacketTransport;
        let manifest = self.container.serialized();
        let packet = write_request("update", manifest).unwrap();
        _ = stream.send_packet(&packet);
    }
}

// Fork the current process, run the container supervisor in the child process, and the receiving
// loop in the current process to receive events and update from the container supervisor
pub fn run(
    container: RunningContainer,
    control_stream: UnixStream,
    auto_start: bool,
) -> (i32, Receiver<ContainerManifest>, Receiver<bool>) {
    let (tx, rx) = channel(container.serialized());
    let (ltx, lrx) = channel(true);
    let (parent, sender) = std::os::unix::net::UnixStream::pair().unwrap();

    if let Ok(fork_result) = unsafe { freebsd::nix::unistd::fork() } {
        match fork_result {
            freebsd::nix::unistd::ForkResult::Child => {
                let kq = unsafe { freebsd::nix::libc::kqueue() };
                let mut pr = ProcessRunner::new(kq, container, auto_start);
                pr.add_control_stream(ControlStream::new(control_stream));
                pr.run(sender);
                std::process::exit(0);
            }
            freebsd::nix::unistd::ForkResult::Parent { child } => {
                let kq = unsafe { freebsd::nix::libc::kqueue() };
                let mut recv_events = [
                    KEvent::from_read(parent.as_raw_fd()),
                    KEvent::from_wait_pid(child.as_raw() as u32),
                ];
                kevent_classic(kq, &recv_events, &mut []).unwrap();

                let mut control_stream = ControlStream::new(parent);
                std::thread::spawn(move || {
                    'kq: loop {
                        let nenv = kevent_classic(kq, &[], &mut recv_events).unwrap();
                        let events = &recv_events[..nenv];

                        for event in events {
                            if event.filter().unwrap() == EventFilter::EVFILT_READ {
                                let bytes = event.data() as usize;
                                if bytes == 0 {
                                    break 'kq;
                                } else {
                                    match control_stream.try_get_request(event.data() as usize) {
                                        Err(err) => {
                                            error!("main loop error: {err:?}");
                                        }
                                        Ok(Readiness::Pending) => {}
                                        Ok(Readiness::Ready((method, request))) => {
                                            if method == "update" {
                                                let manifest: ContainerManifest =
                                                    serde_json::from_value(request.data).unwrap();
                                                tx.send_if_modified(|x| {
                                                    *x = manifest;
                                                    true
                                                });
                                            } else if method == "event" {
                                            }
                                        }
                                    }
                                }
                            } else if event.filter().unwrap() == EventFilter::EVFILT_PROC {
                                break 'kq;
                            }
                        }
                    }
                    ltx.send_if_modified(|x| {
                        *x = true;
                        true
                    });
                });
                (kq, rx, lrx)
            }
        }
    } else {
        panic!()
    }
}
