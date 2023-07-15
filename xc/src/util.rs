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
use serde::{Deserialize, Deserializer};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use sysctl::{Ctl, Sysctl};

pub fn default_on_missing<'de, D, T: Default + serde::Deserialize<'de>>(
    deserializer: D,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

pub fn sha256_hex_file_r_bytes(path: impl AsRef<Path>) -> Result<[u8; 32], anyhow::Error> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let stat = std::fs::metadata(path.as_ref())?;
    let mut file = std::fs::OpenOptions::new().read(true).open(path.as_ref())?;
    let mut buf = [0u8; 4096];
    let mut hasher = Sha256::new();
    let mut remaining = stat.len() as usize;
    while remaining > 0 {
        let nread = file.read(&mut buf)?;
        hasher.update(&buf[..(4096.min(nread))]);
        remaining -= nread;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(digest)
}

#[derive(Debug, Clone)]
pub enum PathComp {
    RootDir,
    CurDir,
    ParentDir,
    Normal(OsString),
}

impl PathComp {
    fn as_os_str(&self) -> &OsStr {
        match self {
            Self::RootDir => OsStr::new("/"),
            Self::CurDir => OsStr::new("."),
            Self::ParentDir => OsStr::new(".."),
            Self::Normal(path) => path.as_ref(),
        }
    }
}

impl AsRef<OsStr> for PathComp {
    fn as_ref(&self) -> &OsStr {
        match self {
            Self::RootDir => OsStr::new("/"),
            Self::CurDir => OsStr::new("."),
            Self::ParentDir => OsStr::new(".."),
            Self::Normal(path) => path.as_ref(),
        }
    }
}

impl AsRef<Path> for PathComp {
    fn as_ref(&self) -> &Path {
        self.as_os_str().as_ref()
    }
}
impl<'a> From<Component<'a>> for PathComp {
    fn from(c: Component<'a>) -> PathComp {
        match c {
            Component::RootDir => Self::RootDir,
            Component::CurDir => Self::CurDir,
            Component::ParentDir => Self::ParentDir,
            Component::Normal(osstr) => Self::Normal(osstr.to_os_string()),
            _ => unreachable!(),
        }
    }
}
/// Given an absolute path, and another path as its root directory, resolve the final path that
/// in absolute in the host's system.
///
/// This function also resolve symlinks up to the specified amount of times
///
/// # Parameters
/// * root: The alternate root location
/// * path: Path in that root location
/// * max_redirect: maximum number of redirection for symlink resolution
pub fn realpath(root: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<PathBuf, std::io::Error> {
    _realpath(root, path, 256).and_then(|path| {
        path.ok_or(std::io::Error::new(
            std::io::ErrorKind::Other,
            "invalid path",
        ))
    })
}

/// Given an absolute path, and another path as its root directory, resolve the final path that
/// in absolute in the host's system.
///
/// This function also resolve symlinks up to the specified amount of times
///
/// # Parameters
/// * root: The alternate root location
/// * path: Path in that root location
/// * max_redirect: maximum number of redirection for symlink resolution
#[inline(always)]
fn _realpath(
    root: impl AsRef<Path>,
    path: impl AsRef<Path>,
    max_redirect: usize,
) -> Result<Option<PathBuf>, std::io::Error> {
    let root = root.as_ref().to_path_buf();
    if path.as_ref().is_relative() {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "path connot be relative path",
        ))?;
    }

    let mut path = path.as_ref().to_path_buf();
    let mut current = PathBuf::new();
    let mut real_path = root.clone();
    let mut directed = 0;

    'p: while max_redirect > directed {
        let mut components: VecDeque<_> = path.components().map(PathComp::from).collect();

        while let Some(head) = components.pop_front() {
            if max_redirect <= directed {
                return Ok(None);
            }
            match head {
                PathComp::RootDir => {
                    current.push(head);
                    real_path = root.clone();
                }
                PathComp::CurDir => continue,
                PathComp::ParentDir => {
                    if !current.pop() {
                        return Ok(None);
                    }
                    real_path.pop();
                }
                PathComp::Normal(ent) => {
                    current.push(&ent);
                    real_path.push(&ent);
                }
            }

            if real_path.is_symlink() {
                directed += 1;
                let link = real_path.read_link()?;
                let link_components = link.components();
                if link.is_relative() {
                    for component in link_components.rev() {
                        components.push_front(PathComp::from(component));
                    }
                } else {
                    path = link.to_path_buf();
                    current = PathBuf::new();
                    real_path = root.clone();
                    continue 'p;
                }
            } else {
                continue;
            }
        }

        break;
    }
    Ok(Some(real_path))
}

/// Given a file system subtree as root, check if there exists an executable at `path`, with
/// a maximum number of softlink redirection allowed. Return Ok(Some(path)) if path exists and is
/// and executable, Err(..) if error on read link, and Ok(None) if exceeded max read_link count
///
/// # Parameters
/// * root: The path acting as root
/// * path: The path relative to `root`
/// * max_redirect: Maximum number of symlink redirection allowed
pub fn exists_exec(
    root: impl AsRef<Path>,
    path: impl AsRef<Path>,
    max_redirect: usize,
) -> Result<Option<PathBuf>, std::io::Error> {
    let root = root.as_ref();
    let mut path = path.as_ref().to_path_buf();
    let mut redirected = 0usize;

    if path.is_relative() {
        panic!("path cannot be relative path");
    }

    while path.is_symlink() {
        if max_redirect == redirected {
            break;
        }
        let link = path.read_link()?;
        if link.is_relative() {
            let mut old_path = path.to_path_buf();
            for component in link.components() {
                old_path.push(component);
            }
            path = old_path;
        } else {
            let mut old_path = root.to_path_buf();
            for component in link.components() {
                if component != Component::RootDir {
                    old_path.push(component);
                }
            }
            path = old_path;
        }
        redirected += 1;
    }

    if path.exists() && path.is_file() && path.starts_with(root) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

pub enum CompressionFormat {
    Other,
    Zstd,
    Gzip,
}

pub trait CompressionFormatExt {
    fn compression_format(&self) -> Result<CompressionFormat, std::io::Error>;
}

impl CompressionFormatExt for std::fs::File {
    /// Read the first 4 bytes in the file to determine it's compression type
    fn compression_format(&self) -> Result<CompressionFormat, std::io::Error> {
        let fd = self.as_raw_fd();
        let mut magic = [0u8; 4];
        if unsafe { freebsd::libc::pread(fd, magic.as_mut_ptr().cast(), 4, 0) } == -1 {
            Err(std::io::Error::last_os_error())
        } else if magic[..2] == [0x1f, 0x8b] {
            Ok(CompressionFormat::Gzip)
        } else if magic == [0x28, 0xb5, 0x2f, 0xfd] {
            Ok(CompressionFormat::Zstd)
        } else {
            Ok(CompressionFormat::Other)
        }
    }
}

pub fn jail_allowables() -> Vec<String> {
    Ctl::new("security.jail.param.allow")
        .expect("cannot sysctl")
        .into_iter()
        .map(|entry| {
            entry
                .and_then(|e| e.name())
                .ok()
                .and_then(|s| {
                    s.strip_prefix("security.jail.param.allow.")
                        .map(|s| s.to_string())
                })
                .expect("cannot get name from sysctl")
        })
        .collect()
}

pub fn get_current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        otherwise => otherwise,
    }
}

pub fn gen_id() -> String {
    // I'm lazy
    let uuid = uuid::Uuid::new_v4().to_string();
    let (_, id) = uuid.rsplit_once('-').unwrap();
    id.to_string()
}

pub fn mk_string(
    v: &[impl AsRef<str>],
    begin: impl AsRef<str>,
    sep: impl AsRef<str>,
    end: impl AsRef<str>,
) -> String {
    let mut ret = begin.as_ref().to_string();
    let mut once = false;
    for s in v {
        if once {
            ret.push_str(sep.as_ref());
        } else {
            once = true;
        }
        ret.push_str(s.as_ref());
    }
    ret.push_str(end.as_ref());
    ret
}

#[cfg(test)]
mod tests {
    use super::{mk_string, realpath};
    /*
    #[test]
    fn test_to_hex() {
        let input = [0x01, 0x02, 0x33, 0xfe, 0x6f];
        let output = super::hex(&input);
        assert_eq!(output, "010233fe6f");
    }
    #[test]
    fn test_to_hex_empty() {
        let input = [];
        let output = super::hex(&input);
        assert_eq!(output, "");
    }
    */
    #[test]
    fn test_mk_string() {
        assert_eq!(
            mk_string(&["a", "b", "c"], "[", ",", "]"),
            "[a,b,c]".to_string()
        );
        assert_eq!(mk_string(&["a"], "[", ",", "]"), "[a]".to_string());
        let empty = Vec::<String>::new();
        assert_eq!(mk_string(&empty, "[", ",", "]"), "[]".to_string());
    }

    #[test]
    fn test_jail_parameters() {
        let parameters = super::jail_allowables();
        eprintln!("{parameters:#?}");
        assert!(false)
    }

    #[test]
    fn test_real_path() {
        let root = "/usr/home/yuuji/test_xc_realpath/root";
        let path = "/a/b/c";
        let expected = "/usr/home/yuuji/test_xc_realpath/root/usr";
        let result = realpath(root, path);
        eprintln!("result: {result:#?}");
        assert!(false);
        //        assert_eq!(result, Ok(Some(std::path::PathBuf::from(expected))))
    }
}
