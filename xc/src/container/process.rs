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
use crate::container::error::ExecError;
use crate::container::Jexec;

use pty_process::kqueue_forwarder::PtyForwarder;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Statistic and information about a process spawned by the runtime in the jail
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessStat {
    pub started: Option<u64>,
    pub exited: Option<u64>,
    pub tree_exited: Option<u64>,
    pub exec: Jexec,
    pub exit_code: Option<i32>,
    pub spawn_info: Option<SpawnInfo>,
    pub description: Option<String>,
}

impl ProcessStat {
    pub fn new(exec: Jexec) -> ProcessStat {
        ProcessStat {
            started: None,
            exited: None,
            exit_code: None,
            exec,
            description: None,
            tree_exited: None,
            spawn_info: None,
        }
    }

    pub fn new_with_desc(exec: Jexec, desc: impl AsRef<str>) -> ProcessStat {
        ProcessStat {
            started: None,
            exited: None,
            exit_code: None,
            exec,
            tree_exited: None,
            description: Some(desc.as_ref().to_string()),
            spawn_info: None,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.spawn_info.as_ref().map(|info| info.pid)
    }

    pub fn set_started(&mut self, spawn_info: SpawnInfo) {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        self.started = Some(time.as_secs());
        self.spawn_info = Some(spawn_info);
    }

    pub fn set_exited(&mut self, exit_code: i32) {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        self.exited = Some(time.as_secs());
        self.exit_code = Some(exit_code);
    }

    pub fn set_tree_exited(&mut self) {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        self.tree_exited = Some(time.as_secs());
    }

    pub fn exited(&self) -> bool {
        self.exit_code.is_some()
    }

    pub fn started(&self) -> bool {
        self.started.is_some()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpawnInfo {
    pub pid: u32,
    pub stdout_log_file: Option<String>,
    pub stderr_log_file: Option<String>,
    pub terminal_socket: Option<String>,
}

pub(super) fn spawn_process_forward(
    cmd: &mut std::process::Command,
    stdin: Option<RawFd>,
    stdout: Option<RawFd>,
    stderr: Option<RawFd>,
) -> Result<SpawnInfo, ExecError> {
    unsafe {
        cmd.pre_exec(move || {
            if let Some(fd) = stdin {
                freebsd::libc::close(0);
                freebsd::libc::dup2(fd, 0);
            }
            if let Some(fd) = stdout {
                freebsd::libc::close(1);
                freebsd::libc::dup2(fd, 1);
            }
            if let Some(fd) = stderr {
                freebsd::libc::close(2);
                freebsd::libc::dup2(fd, 2);
            }
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(ExecError::CannotSpawn)?;
    let pid = child.id();
    Ok(SpawnInfo {
        pid,
        stdout_log_file: None,
        stderr_log_file: None,
        terminal_socket: None,
    })
}

pub(super) fn spawn_process_pty(
    cmd: std::process::Command,
    log_path: impl AsRef<Path>,
    socket_path: impl AsRef<Path>,
) -> Result<SpawnInfo, ExecError> {
    let log_path_str = log_path.as_ref().to_string_lossy().to_string();
    let file = File::options()
        .create(true)
        .write(true)
        .open(log_path)
        .map_err(|err| ExecError::CannotOpenLogFile(log_path_str.to_string(), err))?;
    let listener = std::os::unix::net::UnixListener::bind(socket_path.as_ref())
        .map_err(ExecError::CannotBindUnixSocket)?;
    let forwarder =
        PtyForwarder::from_command(listener, cmd, file).map_err(ExecError::CannotSpawn)?;
    let pid = forwarder.pid();
    std::thread::spawn(move || {
        // XXX
        _ = forwarder.spawn();
        unsafe { nix::libc::waitpid(pid as i32, std::ptr::null_mut(), 0) };
    });
    Ok(SpawnInfo {
        pid,
        stdout_log_file: Some(log_path_str.to_string()),
        stderr_log_file: Some(log_path_str),
        terminal_socket: Some(socket_path.as_ref().to_string_lossy().to_string()),
    })
}

pub(super) fn spawn_process_files(
    cmd: &mut std::process::Command,
    stdout: &Option<impl AsRef<Path>>,
    stderr: &Option<impl AsRef<Path>>,
) -> Result<SpawnInfo, ExecError> {
    if let Some(path) = stdout {
        let file = File::options()
            .create(true)
            .write(true)
            .open(path)
            .map_err(|err| {
                ExecError::CannotOpenLogFile(path.as_ref().to_string_lossy().to_string(), err)
            })?;
        cmd.stdout(file);
    }

    if let Some(path) = stderr {
        let file = File::options()
            .create(true)
            .write(true)
            .open(path)
            .map_err(|err| {
                ExecError::CannotOpenLogFile(path.as_ref().to_string_lossy().to_string(), err)
            })?;
        cmd.stderr(file);
    }

    let child = cmd.spawn().map_err(ExecError::CannotSpawn)?;
    let pid = child.id();
    Ok(SpawnInfo {
        pid,
        stdout_log_file: stdout
            .as_ref()
            .map(|p| p.as_ref().to_string_lossy().to_string()),
        stderr_log_file: stderr
            .as_ref()
            .map(|p| p.as_ref().to_string_lossy().to_string()),
        terminal_socket: None,
    })
}
