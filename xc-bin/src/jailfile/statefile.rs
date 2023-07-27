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
use anyhow::Result;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub trait Ignorer {
    fn should_ignore(&self, path: impl AsRef<Path>) -> bool;
}

/// Create a cache directory `cache_dir` that is keeping tracks of files, symlinks and directories
/// under `path`
///
/// # Arguments
///
/// `ignorer`: A checker gives information which files/directories should be ignored
/// `cache_dir`: The cache directory to be created
/// `path`: The path to cache
pub fn create_cache(
    ignorer: &impl Ignorer,
    cache_dir: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> Result<()> {
    let cache_dir = cache_dir.as_ref().to_path_buf();
    if ignorer.should_ignore(&path) {
        return Ok(());
    }

    let mut path = path.as_ref().to_path_buf();

    if path.is_file() {
        let meta = std::fs::metadata(&path)?;
        let len = meta.len();
        let digest = xc::util::sha256_hex_file_r_bytes(&path)?;

        let mut cache_path = cache_dir.clone();
        cache_path.push(&path);

        let mut cache_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(cache_path)?;
        cache_file.write_all(&len.to_le_bytes())?;
        cache_file.write_all(&digest)?;
        cache_file.set_permissions(meta.permissions())?;
    } else if path.is_dir() {
        let mut cache_path = cache_dir.clone();
        cache_path.push(&path);
        std::fs::create_dir(&cache_path)?;
    }

    let mut walk_dir = vec![path];

    loop {
        if walk_dir.is_empty() {
            break;
        } else {
            path = walk_dir.pop().unwrap();
        }

        let Ok(read_dir) = std::fs::read_dir(&path) else { continue };
        let mut known_entries = 0i64;

        for entry in read_dir {
            let Ok(entry) = entry else { continue };
            let Ok(file_type) = entry.file_type() else { continue };
            let mut p = path.clone();
            p.push(entry.file_name());
            let mut cache_path = cache_dir.clone();
            cache_path.push(&p);
            let metadata = entry.metadata()?;
            if file_type.is_file() {
                let digest = xc::util::sha256_hex_file_r_bytes(&p)?;
                let len = metadata.len();
                let mut cache_file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&cache_path)?;
                cache_file.write_all(&len.to_le_bytes())?;
                cache_file.write_all(&digest)?;
            } else if file_type.is_dir() {
                std::fs::create_dir(&cache_path)?;
                if !ignorer.should_ignore(&path) {
                    walk_dir.push(p.clone());
                }
            } else if file_type.is_symlink() {
                let link = std::fs::read_link(&p)?;
                let contents = link.as_os_str().as_bytes();
                std::fs::write(&cache_path, contents)?;
            }
            std::fs::set_permissions(&cache_path, metadata.permissions())?;
            known_entries += 1;
        }

        let mut dotdot_xc_dir_cache = cache_dir.clone();
        dotdot_xc_dir_cache.push(&path);
        dotdot_xc_dir_cache.push("..xc.dir.cache");

        std::fs::write(dotdot_xc_dir_cache, known_entries.to_le_bytes())?;
    }

    Ok(())
}

/// Check if we have previously walked the `path` under `source`, absolute path not yet supported
pub fn is_content_changed(
    cache: &Path,
    ignorer: &impl Ignorer,
    path: impl AsRef<Path>,
) -> Result<bool> {
    if ignorer.should_ignore(&path) {
        return Ok(false);
    }

    let mut path = path.as_ref().to_path_buf();

    if path.is_file() {
        return is_changed(cache, &path);
    }

    let mut walk_dir = vec![path];

    loop {
        if walk_dir.is_empty() {
            break;
        } else {
            path = walk_dir.pop().unwrap();
        }

        let Ok(read_dir) = std::fs::read_dir(&path) else {
            continue
        };

        let mut dotdot_xc_dir_cache = cache.to_path_buf();
        dotdot_xc_dir_cache.push(&path);
        dotdot_xc_dir_cache.push("..xc.dir.cache");

        let dotdot_bytes = std::fs::read(&dotdot_xc_dir_cache)?;
        if dotdot_bytes.len() != 8 {
            return Ok(true);
        }

        let mut expected_entries = i64::from_le_bytes(dotdot_bytes[..8].try_into().unwrap());

        for entry in read_dir {
            let Ok(entry) = entry else { continue };
            let Ok(file_type) = entry.file_type() else { continue };
            let mut p = path.clone();
            p.push(entry.file_name());

            if ignorer.should_ignore(&p) {
                continue;
            }

            if file_type.is_dir() {
                walk_dir.push(p.clone());
            } else {
                let Ok(false) = is_changed(cache, &p) else { return Ok(true) };
            }
            expected_entries -= 1;
        }

        if expected_entries != 0 {
            return Ok(true);
        }
    }

    Ok(false)
}

fn is_changed(cache_dir: &Path, real: &PathBuf) -> Result<bool> {
    let mut cached = cache_dir.to_path_buf();
    cached.push(real);

    if !cached.exists() {
        return Ok(true);
    }

    let cached_meta = std::fs::symlink_metadata(&cached)?;
    let real_meta = std::fs::symlink_metadata(real)?;

    if cached_meta.uid() != real_meta.uid() || cached_meta.gid() != real_meta.gid() {
        return Ok(true);
    }

    if real_meta.is_symlink() {
        let cached_link = std::fs::read(cached)?;
        let path = std::fs::read_link(real)?;
        return Ok(path.as_os_str().as_bytes() != cached_link);
    } else if real_meta.is_file() {
        if real_meta.file_type() != cached_meta.file_type() {
            return Ok(true);
        }
        let mut cache = std::fs::OpenOptions::new().read(true).open(cached)?;
        let mut len_buf = [0u8; 8];
        cache.read_exact(&mut len_buf)?;
        if u64::from_le_bytes(len_buf) != real_meta.len() {
            return Ok(true);
        }
        let mut digest = [0u8; 32];
        cache.read_exact(&mut digest)?;
        let computed_digest = xc::util::sha256_hex_file_r_bytes(real)?;
        return Ok(digest != computed_digest);
    }
    Ok(false)
}

struct NoIgnore;

impl Ignorer for NoIgnore {
    fn should_ignore(&self, _path: impl AsRef<Path>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    macro_rules! test_detech_changes {
        ($p:expr) => {
            let curdir = std::env::current_dir()?;
            _ = std::fs::remove_dir_all("test-dir");
            _ = std::fs::create_dir("test-dir");
            _ = std::env::set_current_dir("test-dir");
            let cache_dir = Path::new("cache").to_path_buf();
            let target = Path::new("this");
            // stage our test
            {
                _ = std::process::Command::new("mkdir")
                    .arg("-p")
                    .arg("this/is/a/test/directory")
                    .status();
                _ = std::fs::write("this/is/a/file", "abcdefg");
                _ = std::os::unix::fs::symlink("123456", "this/is/a/symlink");
                _ = std::process::Command::new("mkdir").arg("cache").status();
                _ = create_cache(&NoIgnore, &cache_dir, &target);
            }
            {
                let result1 = is_content_changed(&cache_dir, &NoIgnore, &target);
                assert!(result1.is_ok());
                assert!(!result1.unwrap());
            }
            {
                $p;
            }
            let result = is_content_changed(&cache_dir, &NoIgnore, &target).unwrap_or_default();
            assert!(result);
            _ = std::env::set_current_dir(curdir);
            Ok(())
        };
    }

    #[test]
    #[serial]
    fn test_changes_modify_file() -> anyhow::Result<()> {
        test_detech_changes! {
            std::fs::write("this/is/a/file", "xyz")?
        }
    }

    #[test]
    #[serial]
    fn test_changes_remove_symlink() -> anyhow::Result<()> {
        test_detech_changes! {
            std::fs::remove_file("this/is/a/symlink")?
        }
    }
    #[test]
    #[serial]
    fn test_changes_remove_file() -> anyhow::Result<()> {
        test_detech_changes! {
            std::fs::remove_file("this/is/a/file")?
        }
    }

    #[test]
    #[serial]
    fn test_changes_remove_dir() -> anyhow::Result<()> {
        let curdir = std::env::current_dir()?;

        _ = std::fs::remove_dir_all("test-dir");
        _ = std::fs::create_dir("test-dir");
        _ = std::env::set_current_dir("test-dir");

        // stage our test
        _ = std::process::Command::new("mkdir")
            .arg("-p")
            .arg("this/is/a/test/directory")
            .status();
        _ = std::fs::write("this/is/a/file", "abcdefg");
        _ = std::os::unix::fs::symlink("123456", "this/is/a/symlink");

        _ = std::process::Command::new("mkdir").arg("cache").status();

        let cache_dir = Path::new("cache").to_path_buf();
        let target = Path::new("this");

        create_cache(&NoIgnore, &cache_dir, &target);

        {
            let file_meta = std::fs::metadata("cache/this/is/a/file")?;
            assert_eq!(file_meta.len(), 40);
            let content = std::fs::read("cache/this/is/a/file")?;
            let len = u64::from_le_bytes(content[..8].try_into().unwrap());
            assert_eq!(len, 7);
            assert_eq!(
                content[8..],
                [
                    0x7d, 0x1a, 0x54, 0x12, 0x7b, 0x22, 0x25, 0x02, 0xf5, 0xb7, 0x9b, 0x5f, 0xb0,
                    0x80, 0x30, 0x61, 0x15, 0x2a, 0x44, 0xf9, 0x2b, 0x37, 0xe2, 0x3c, 0x65, 0x27,
                    0xba, 0xf6, 0x65, 0xd4, 0xda, 0x9a
                ]
            );
        }

        let result1 = is_content_changed(&cache_dir, &NoIgnore, &target);

        assert!(result1.is_ok());
        assert!(!result1.unwrap());

        std::fs::remove_dir_all("this/is/a/test")?;

        let result = is_content_changed(&cache_dir, &NoIgnore, &target).unwrap_or_default();

        assert!(result);

        std::env::set_current_dir(curdir);
        Ok(())
    }
}
