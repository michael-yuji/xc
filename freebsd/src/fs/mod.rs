//! File system specific bits

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

pub mod devfs;
pub mod zfs;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

pub const MOUNT_CMD: &str = crate::env_or_default!("XC_MOUNT_CMD", "/sbin/mount");
pub const UMOUNT_CMD: &str = crate::env_or_default!("XC_UMOUNT_CMD", "/sbin/umount");

#[derive(Error, Debug)]
pub enum MountError {
    #[error("Mount point does not exist: {0}")]
    MountPointNotFound(String),
    #[error("Mount point is not a directory (or file in the case of nullfs)")]
    InvalidMountPointType,
    #[error("Mount point and source are not the same type")]
    MountPointTypeMismatch,
    #[error("{0}")]
    Other(std::io::Error),
}

/// Mount a filesystem with `options` from `source` to a given `mount_point`
///
/// # Parameters
/// * type: Type of the filesystem
/// * source: Source of the filesystem
/// * mount_point: Targetted mountpoint of this mount operation
/// * options: mount options
pub fn mount<S: AsRef<str>>(
    tpe: impl AsRef<str>,
    source: impl AsRef<OsStr>,
    mount_point: impl AsRef<Path>,
    options: impl AsRef<[S]>,
) -> Result<(), MountError> {
    if !mount_point.as_ref().exists() {
        return Err(MountError::MountPointNotFound(
            mount_point.as_ref().to_string_lossy().to_string(),
        ));
    }

    if tpe.as_ref() == "nullfs" {
        let source_type = std::fs::metadata(source.as_ref())
            .map_err(MountError::Other)?
            .file_type();
        let mount_type = mount_point
            .as_ref()
            .metadata()
            .map_err(MountError::Other)?
            .file_type();

        if source_type != mount_type {
            return Err(MountError::MountPointTypeMismatch);
        } else if !mount_type.is_dir() && !mount_type.is_file() {
            return Err(MountError::InvalidMountPointType);
        }
    } else if !mount_point.as_ref().is_dir() {
        return Err(MountError::InvalidMountPointType);
    }

    let options = options
        .as_ref()
        .iter()
        .map(|s| s.as_ref().to_string())
        .collect::<Vec<_>>();

    let mut command = Command::new(MOUNT_CMD);
    command.arg("-t");
    command.arg(tpe.as_ref());

    if !options.is_empty() {
        command.arg("-o");
        command.arg(options.join(","));
    }

    command.arg(source.as_ref());
    command.arg(mount_point.as_ref());

    command.status().map_err(MountError::Other)?;
    Ok(())
}

/// Unmount filesystem at a given mountpoint
pub fn umount(mountpoint: impl AsRef<Path>) -> Result<(), MountError> {
    let mp = mountpoint.as_ref();
    if !mp.exists() {
        Err(MountError::MountPointNotFound(
            mp.to_string_lossy().to_string(),
        ))
    } else if !mp.is_dir() {
        Err(MountError::InvalidMountPointType)
    } else {
        Command::new(UMOUNT_CMD)
            .arg("-f")
            .arg(mp)
            .status()
            .map_err(MountError::Other)
            .map(|_| ())
    }
}
