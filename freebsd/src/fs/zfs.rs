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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::BufRead;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use thiserror::Error;

pub const ZFS_CMD: &str = crate::env_or_default!("XC_ZFS_CMD", "/sbin/zfs");

#[derive(Error, Debug)]
pub enum ZfsError {
    #[error("fail to spawn zfs process: {0}")]
    SpawnError(std::io::Error),
    #[error("ZFS command fail with non-zero exit code: {0}, stderr: {1}")]
    Generic(ExitStatus, String),
}

impl ZfsError {
    pub fn normalized(self) -> std::io::Error {
        match self {
            Self::SpawnError(error) => error,
            Self::Generic(_, m) => Error::new(ErrorKind::Other, m.as_str()),
        }
    }
}

type Result<T> = std::result::Result<T, ZfsError>;

/// ZFS functionality are implemented by running commands, this struct defines certain behaviours
/// when the commands execute
#[allow(dead_code)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZfsHandle {
    /// The command binary, by default it is "zfs" which will yield whichever zfs binary visible in
    /// $PATH
    executable: String,
    /// If the stdout and stderr should be piped for commands that does not rely on stdout/stderr
    /// to function correctly
    inherit_stdout: bool,
    inherit_stderr: bool,
}

pub struct ZfsCreate {
    dataset: PathBuf,
    properties: HashMap<String, String>,
    create_ancestors: bool,
    no_mount: bool,
}

impl ZfsCreate {
    /// Create a ZFS create template
    ///
    /// # Arguments
    ///
    /// * `dataset` - The dataset to be created
    /// * `create_ancestors` - Also create intermediate datasets
    /// * `no_mount` - Do no mount the created dataset
    pub fn new(dataset: impl AsRef<Path>, create_ancestors: bool, no_mount: bool) -> ZfsCreate {
        ZfsCreate {
            dataset: dataset.as_ref().to_path_buf(),
            create_ancestors,
            no_mount,
            properties: HashMap::new(),
        }
    }

    pub fn set_props(&mut self, props: HashMap<String, String>) {
        self.properties = props;
    }

    pub fn insert_pop(&mut self, key: &str, value: &str) {
        self.properties.insert(key.to_string(), value.to_string());
    }
}

pub struct ZfsClone {
    dataset: String,
    dest: String,
    tag: String,
    recursive: bool,
    properties: HashMap<String, String>,
}

impl ZfsClone {
    pub fn new(dataset: &str, tag: &str, dest: &str) -> ZfsClone {
        ZfsClone {
            dataset: dataset.to_string(),
            tag: tag.to_string(),
            dest: dest.to_string(),
            recursive: false,
            properties: HashMap::new(),
        }
    }

    pub fn set_recursive(&mut self, recursive: bool) {
        self.recursive = recursive;
    }

    pub fn add_prop(&mut self, key: &str, value: &str) -> &mut ZfsClone {
        self.properties.insert(key.to_string(), value.to_string());
        self
    }

    pub fn execute(self, handle: &ZfsHandle) -> Result<()> {
        handle.use_command(|c| {
            c.arg("clone");
            if self.recursive {
                c.arg("-p");
            }
            for (key, value) in self.properties.iter() {
                c.arg("-o");
                c.arg(format!("{key}={value}"));
            }
            c.arg(format!("{}@{}", self.dataset, self.tag));
            c.arg(self.dest);
        })
    }
}

pub struct ZfsSnapshot {
    dataset: String,
    tag: String,
    recursive: bool,
    properties: HashMap<String, String>,
}

impl ZfsSnapshot {
    pub fn new(dataset: &str, tag: &str) -> ZfsSnapshot {
        ZfsSnapshot {
            dataset: dataset.to_string(),
            tag: tag.to_string(),
            recursive: false,
            properties: HashMap::new(),
        }
    }

    pub fn set_recursive(&mut self, recursive: bool) {
        self.recursive = recursive;
    }

    pub fn add_prop(&mut self, key: &str, value: &str) -> &mut ZfsSnapshot {
        self.properties.insert(key.to_string(), value.to_string());
        self
    }

    pub fn execute(self, handle: &ZfsHandle) -> Result<()> {
        handle.use_command(|c| {
            c.arg("snapshot");
            if self.recursive {
                c.arg("-r");
            }
            for (key, value) in self.properties.iter() {
                c.arg("-o");
                c.arg(format!("{key}={value}"));
            }
            c.arg(format!("{}@{}", self.dataset, self.tag));
        })
    }
}

impl Default for ZfsHandle {
    fn default() -> ZfsHandle {
        ZfsHandle {
            executable: ZFS_CMD.to_string(),
            inherit_stdout: false,
            inherit_stderr: false,
        }
    }
}

impl ZfsHandle {
    pub fn new(executable: &str, inherit_stdout: bool, inherit_stderr: bool) -> ZfsHandle {
        ZfsHandle {
            executable: executable.to_string(),
            inherit_stdout,
            inherit_stderr,
        }
    }

    fn use_command<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Command),
    {
        let mut command = Command::new(&self.executable);
        command.stdout(Stdio::null());
        f(&mut command);
        let output = command.output().map_err(ZfsError::SpawnError)?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr =
                std::str::from_utf8(&output.stderr).expect("ZFS stderr output non utf8 bytes");
            Err(ZfsError::Generic(output.status, stderr.to_string()))
        }
    }

    fn use_command_with_output<F>(&self, f: F) -> Result<Vec<u8>>
    where
        F: FnOnce(&mut Command),
    {
        let mut command = Command::new(&self.executable);
        f(&mut command);
        let output = command.output().map_err(ZfsError::SpawnError)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let stderr =
                std::str::from_utf8(&output.stderr).expect("ZFS stderr outputs non utf8 bytes");
            Err(ZfsError::Generic(output.status, stderr.to_string()))
        }
    }
    pub fn list_snapshots(&self, dataset: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        let output = self.use_command_with_output(|cmd| {
            cmd.arg("list")
                .arg("-H")
                .arg("-t")
                .arg("snap")
                .arg("-o")
                .arg("name")
                .arg("-d")
                .arg("1")
                .arg(dataset.as_ref());
        })?;

        let mut bufs = Vec::new();

        for line in output.lines().flatten() {
            bufs.push(Path::new(&line).to_path_buf());
        }

        Ok(bufs)
    }
    pub fn list_direct_children(&self, dataset: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        let output = self.use_command_with_output(|cmd| {
            cmd.arg("list")
                .arg("-H")
                .arg("-o")
                .arg("name")
                .arg("-d")
                .arg("1")
                .arg(dataset.as_ref());
        })?;

        let mut bufs = Vec::new();

        for line in output.lines().flatten() {
            bufs.push(Path::new(&line).to_path_buf());
        }

        Ok(bufs)
    }
    /// Create a new ZFS dataset
    pub fn create2(
        &self,
        dataset: impl AsRef<Path>,
        create_ancestors: bool,
        no_mount: bool,
    ) -> Result<()> {
        self.use_command(|c| {
            c.arg("create");
            if no_mount {
                c.arg("-u");
            }
            if create_ancestors {
                c.arg("-p");
            }
            c.arg(dataset.as_ref());
        })
    }

    pub fn create(&self, arg: ZfsCreate) -> Result<()> {
        self.use_command(|c| {
            c.arg("create");
            if arg.no_mount {
                c.arg("-u");
            }
            if arg.create_ancestors {
                c.arg("-p");
            }
            if !arg.properties.is_empty() {
                for (key, value) in arg.properties.iter() {
                    c.arg("-o");
                    c.arg(format!("{}={}", key, value));
                }
            }
            c.arg(arg.dataset);
        })
    }

    pub fn jail(&self, jail: &str, dataset: impl AsRef<Path>) -> Result<()> {
        self.use_command(|c| {
            c.arg("jail");
            c.arg(jail);
            c.arg(dataset.as_ref());
        })
    }

    pub fn unjail(&self, jail: &str, dataset: impl AsRef<Path>) -> Result<()> {
        self.use_command(|c| {
            c.arg("unjail");
            c.arg(jail);
            c.arg(dataset.as_ref());
        })
    }

    /// # Arguments
    /// * `dataset`
    /// * `resursive`
    /// * `remove_dependents`
    /// * `not_very_nicely`: I mean forcefully
    pub fn destroy(
        &self,
        dataset: impl AsRef<Path>,
        recursive: bool,
        remove_dependents: bool,
        not_very_nicely: bool,
    ) -> Result<()> {
        self.use_command(|c| {
            c.arg("destroy");
            if recursive {
                c.arg("-r");
            }
            if remove_dependents {
                c.arg("-R");
            }
            if not_very_nicely {
                c.arg("-f");
            }
            c.arg(dataset.as_ref());
        })
    }

    pub fn exists(&self, dataset: impl AsRef<Path>) -> bool {
        Command::new(&self.executable)
            .arg("list")
            .arg(dataset.as_ref().to_string_lossy().to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or_else(|_| false)
    }

    pub fn clone2(
        &self,
        src: impl AsRef<Path>,
        snapshot: &str,
        dst: impl AsRef<Path>,
    ) -> Result<()> {
        self.use_command(|c| {
            c.arg("clone")
                .arg(format!("{}@{snapshot}", src.as_ref().to_string_lossy()))
                .arg(dst.as_ref());
        })
    }

    pub fn set_prop(&self, dataset: impl AsRef<Path>, prop: &str, value: &str) -> Result<()> {
        self.use_command(|c| {
            c.arg("set")
                .arg(format!("{prop}={value}"))
                .arg(dataset.as_ref());
        })
    }

    pub fn promote(&self, dataset: impl AsRef<Path>) -> Result<()> {
        self.use_command(|c| {
            c.arg("promote").arg(dataset.as_ref());
        })
    }

    pub fn snapshot2(&self, dataset: impl AsRef<Path>, snapshot: &str) -> Result<()> {
        self.use_command(|c| {
            c.arg("snapshot")
                .arg(format!("{}@{snapshot}", dataset.as_ref().to_string_lossy()));
        })
    }

    pub fn rename(
        &self,
        from_dataset: impl AsRef<Path>,
        to_dataset: impl AsRef<Path>,
    ) -> Result<()> {
        self.use_command(|c| {
            c.arg("rename")
                .arg(from_dataset.as_ref())
                .arg(to_dataset.as_ref());
        })
    }

    pub fn get_props(&self, src: impl AsRef<Path>) -> Result<HashMap<String, Option<String>>> {
        let output = self.use_command_with_output(|command| {
            command
                .arg("get")
                .arg("-Ho")
                .arg("property,value")
                .arg("all")
                .arg(src.as_ref());
        })?;

        let stdout = std::str::from_utf8(&output).unwrap();
        let trimmed = stdout.trim();

        if trimmed.is_empty() {
            Ok(HashMap::new())
        } else {
            let props = stdout
                .lines()
                .map(|line| match line.trim().split_once('\t') {
                    Some((key, value)) => (key.to_string(), Some(value.to_string())),
                    None => (line.trim().to_string(), None),
                });

            Ok(HashMap::from_iter(props))
        }
    }

    pub fn mount_point(&self, dataset: impl AsRef<Path>) -> Result<Option<PathBuf>> {
        let output = self.use_command_with_output(|c| {
            c.arg("list")
                .arg("-Ho")
                .arg("mountpoint")
                .arg(dataset.as_ref());
        })?;

        let stdout = std::str::from_utf8(&output).unwrap();
        let trimed = stdout.trim();

        if trimed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimed.into()))
        }
    }
}
