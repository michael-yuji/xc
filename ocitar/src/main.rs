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
mod tar;
mod util;

use crate::util::*;
use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use util::hex;
use zstd::{Decoder as ZstdDecoder, Encoder as ZstdEncoder};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(short, parse(from_occurrences))]
    verbosity: usize,
    #[clap(long = "dry-run", default_value_t, action)]
    dry_run: bool,
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[clap(short_flag = 'c')]
    Create(CreateArgs),
    #[clap(short_flag = 't')]
    List(ListArgs),
    #[clap(short_flag = 'x')]
    Extract(ExtractArgs),
}

#[derive(Debug)]
enum CompressionType {
    Auto,
    None,
    Zstd,
    Gzip,
}

impl std::str::FromStr for CompressionType {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "none" => Ok(Self::None),
            "zstd" => Ok(Self::Zstd),
            "gzip" => Ok(Self::Gzip),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "unknown value",
            )),
        }
    }
}

#[derive(Parser, Debug)]
pub struct CreateArgs {
    /// path to the output file, or '-' for stdout
    #[clap(short = 'f', long)]
    file: String,

    /// paths to whiteout from the parent layers
    #[clap(long, multiple_occurrences = true)]
    remove: Vec<String>,

    /// paths to include in the layer archive, when "--zfs-diff" is set, these define
    /// the 2 ZFS snapshot / dataset to be diff, in the order zfs-diff(8) accepts
    #[clap(multiple = true)]
    paths: Vec<String>,

    /// Types of compression to be use, available compressions are zstd and gzip
    #[clap(long, default_value = "auto")]
    compression: CompressionType,

    /// If this flag is set, the utility create an archive with the difference between the two zfs
    /// datasets
    #[clap(long = "zfs-diff", action)]
    zfs_diff: bool,

    /// Do not create OCI whiteout files. Use this program's custom tar extension only
    #[clap(long = "no-oci")]
    without_oci: bool,

    /// Include this program's custom tar extension to the archive
    #[clap(long = "no-ext")]
    with_ext: bool,
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// path to the tar file, or '-' for stdin
    #[clap(short)]
    file: String,

    /// specify the compression type the archive is compressed in, if set to auto, which is the
    /// default, the type of compression will be guessed by the first 4 bytes of the file
    #[clap(long, default_value = "auto")]
    compression: CompressionType,
}

#[derive(Parser, Debug)]
pub struct ExtractArgs {
    #[clap(long, default_value = "auto")]
    compression: CompressionType,

    #[clap(long = "print-input-digest", action)]
    print_input_digest: bool,

    /// change directory to the location before extracting
    #[clap(short = 'C')]
    chdir: Option<String>,

    #[clap(short)]
    /// path to the archive file or '-' for stdin
    file: String,
}

fn prepare_compressed_stream_reader(
    mut input: Box<dyn Read>,
    hint: CompressionType,
) -> Result<Box<dyn Read>, std::io::Error> {
    match hint {
        CompressionType::Auto => {
            let mut check_magic = [0u8; 4];
            input.read_exact(&mut check_magic)?;
            if check_magic == ZSTD_MAGIC {
                Ok(Box::new(ZstdDecoder::new(PrebufferedSource::new(
                    &check_magic,
                    input,
                ))?))
            } else if check_magic[0..2] == GZIP_MAGIC {
                Ok(Box::new(flate2::read::GzDecoder::new(
                    PrebufferedSource::new(&check_magic, input),
                )))
            } else {
                Ok(Box::new(PrebufferedSource::new(&check_magic, input)))
            }
        }
        CompressionType::None => Ok(input),
        CompressionType::Zstd => Ok(Box::new(ZstdDecoder::new(input)?)),
        CompressionType::Gzip => Ok(Box::new(flate2::read::GzDecoder::new(input))),
    }
}

pub fn do_list(args: ListArgs) -> Result<(), std::io::Error> {
    let mut input: Box<dyn Read> = match args.file.as_str() {
        "-" => Box::new(std::io::stdin()),
        path => Box::new(File::open(path)?),
    };

    input = prepare_compressed_stream_reader(input, args.compression)?;

    let summary = tar::list_tar(&mut input)?;

    for whiteout in summary.whiteouts.iter() {
        println!("-\t{whiteout}");
    }

    for file in summary.files.iter() {
        println!("+\t{file}");
    }

    Ok(())
}

fn zfs_dataset_get_mountpoint(dataset: &String) -> Result<Option<String>, std::io::Error> {
    let dataset = match dataset.split_once(if dataset.contains('#') { '#' } else { '@' }) {
        None => dataset.to_string(),
        Some((origin, _)) => origin.to_string(),
    };
    let out = std::process::Command::new("zfs")
        .arg("get")
        .arg("-Ho")
        .arg("value")
        .arg("mountpoint")
        .arg(dataset)
        .output()?
        .stdout;
    let mountpoint = std::str::from_utf8(&out).unwrap().trim().to_string();
    Ok(if mountpoint == "-" {
        None
    } else {
        Some(mountpoint)
    })
}

pub fn do_create(args: CreateArgs) -> Result<(), std::io::Error> {
    let mut output: Box<dyn Write> = match args.file.as_str() {
        "-" => Box::new(std::io::stdout()),
        path => Box::new(File::create(path)?),
    };

    output = match args.compression {
        CompressionType::Zstd => Box::new(ZstdEncoder::new(output, 3)?.auto_finish()),
        CompressionType::Gzip => Box::new(flate2::write::GzEncoder::new(
            output,
            flate2::Compression::default(),
        )),
        _ => output,
    };

    if args.zfs_diff {
        if args.paths.len() != 2 {
            panic!("zfs diff accepts two and only two arguments")
        }

        let mut adding = Vec::new();
        let mut removing = Vec::new();

        let root = format!("{}/", zfs_dataset_get_mountpoint(&args.paths[1])?.unwrap());

        let out = std::process::Command::new("zfs")
            .arg("diff")
            .arg("-H")
            .arg(&args.paths[0])
            .arg(&args.paths[1])
            .output()?;

        if !out.status.success() {
            let mut err_msg = String::new();

            // ZFS error output tends to create line breaking error messages, in order
            // to print it out without ruining the formatting, we need to re-construct
            // the actual error message
            for line in std::str::from_utf8(&out.stderr).unwrap().lines() {
                if err_msg.ends_with(|c: char| c.is_alphanumeric())
                    && line.starts_with(|c: char| c.is_alphanumeric())
                {
                    err_msg.push(' ');
                    err_msg.push_str(line);
                } else {
                    err_msg.push_str(line);
                }
            }

            return Err(std::io::Error::new(std::io::ErrorKind::Other, err_msg));
        }

        let stdout = std::str::from_utf8(&out.stdout).unwrap();

        for line in stdout.lines() {
            eprintln!("{line}");
            let mut columns = line.split('\t'); //.collect::<Vec<_>>();
            let flag = columns.next().expect("Expect flag");
            let path = columns
                .next()
                .expect("Expect path")
                .to_string()
                .replacen(&root, "", 1);
            match flag {
                "-" => removing.push(path),
                "+" => adding.push(path),
                "M" => adding.push(path),
                "R" => {
                    let new = columns.next().expect("expect new path").to_string();
                    removing.push(path.to_string());
                    adding.push(new.replacen(&path, "", 1))
                }
                _ => unreachable!(),
            }
        }

        create_tar(
            args.without_oci,
            !args.with_ext,
            &["-C".to_string(), root],
            &adding,
            &removing,
            &mut output,
        )
    } else {
        create_tar(
            args.without_oci,
            !args.with_ext,
            &[],
            &args.paths,
            &args.remove,
            &mut output,
        )
    }
}

pub fn do_extract(args: ExtractArgs) -> Result<(), std::io::Error> {
    let mut input: Box<dyn Read> = match args.file.as_str() {
        "-" => Box::new(std::io::stdin()),
        path => Box::new(File::open(path)?),
    };

    let digest_input = std::rc::Rc::new(std::cell::RefCell::new(
        DigestReader::<Box<dyn Read>>::new(input),
    ));

    let handle = DigestReaderHandle(digest_input.clone());

    input = prepare_compressed_stream_reader(Box::new(handle), args.compression)?;

    if let Some(dir) = args.chdir {
        std::env::set_current_dir(dir)?;
    }
    extract(&mut input)?;
    let digest = digest_input.borrow().consume();
    if args.print_input_digest {
        println!("sha256:{}", hex(digest));
    }
    Ok(())
}

fn extract<R: Read>(reader: &mut R) -> Result<(), std::io::Error> {
    let mut child = Command::new("tar")
        .arg("-xf-")
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    let tar_stdin = child.stdin.as_mut().unwrap();

    let digest = tar::tap_extract_tar(reader, tar_stdin)?;

    println!("sha256:{}", hex(digest));

    match child.wait()?.code() {
        Some(ec) if ec != 0 => {
            err!("tar return non-zero exit code")
        }
        _ => Ok(()),
    }
}

fn create_tar<W: Write>(
    without_oci: bool,
    without_ext: bool,
    tar_options: &[String],
    paths: &[String],
    whiteouts: &[String],
    output: &mut W,
) -> Result<(), std::io::Error> {
    let paths_input = paths.join("\n");

    let mut child = Command::new("tar")
        .args(tar_options)
        .arg("-cf-")
        .arg("-T-") //.args(paths)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let mut child_stdin = child.stdin.take().unwrap();

    std::thread::spawn(move || {
        _ = child_stdin.write_all(paths_input.as_bytes());
        drop(child_stdin);
    });

    let tar_stdout = child.stdout.as_mut().unwrap();

    let digest = tar::tap_create_tar(without_oci, without_ext, whiteouts, tar_stdout, output)?;

    println!("sha256:{}", hex(digest));

    match child.wait()?.code() {
        Some(code) if code != 0 => {
            err!("tar returns non-zero exit code")
        }
        _ => Ok(()),
    }
}

fn main() -> Result<(), std::io::Error> {
    // to not ignore SIGPIPE so cli can run properly when piped
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args = Args::parse();

    if args.dry_run {
        eprintln!("{args:#?}");
        return Ok(());
    }

    // setup logging facility
    stderrlog::new()
        .module(module_path!())
        .verbosity(args.verbosity)
        .init()
        .unwrap();

    log::debug!("main args: {args:?}");

    match args.command {
        Commands::Create(c) => do_create(c)?,
        Commands::List(c) => do_list(c)?,
        Commands::Extract(c) => do_extract(c)?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn test_extraction<F: FnOnce(&str) -> R + std::panic::UnwindSafe, R>(ident: &str, f: F) {
        // git cannot contain true empty directory, hence we need to store the test directories
        // in tarball and extract them if they are not extracted yet
        let before_path = std::path::PathBuf::from("test-materials/stage-before");
        let expected_path = std::path::PathBuf::from("test-materials/stage-expected");
        if !before_path.exists() {
            std::process::Command::new("tar")
                .arg("xf")
                .arg("test-materials/stage-before.tar")
                .arg("-C")
                .arg("test-materials")
                .output()
                .unwrap();
        }
        if !expected_path.exists() {
            std::process::Command::new("tar")
                .arg("xf")
                .arg("test-materials/stage-expected.tar")
                .arg("-C")
                .arg("test-materials")
                .output()
                .unwrap();
        }
        let current_dir = std::env::current_dir().unwrap();
        let stage_dir = format!("test-materials/stage-{ident}");
        // ensure we have a clean environment
        _ = std::fs::remove_dir_all(&stage_dir);
        std::process::Command::new("cp")
            .arg("-r")
            .arg("test-materials/stage-before")
            .arg(&stage_dir)
            .output()
            .unwrap();

        let result = std::panic::catch_unwind(|| {
            f(&stage_dir);
            _ = std::env::set_current_dir(current_dir);
            let output = std::process::Command::new("diff")
                .arg("-r")
                .arg(&stage_dir)
                .arg("test-materials/stage-expected")
                .output()
                .unwrap();
            println!("stdout: {:?}", std::str::from_utf8(&output.stdout));
            println!("stderr: {:?}", std::str::from_utf8(&output.stderr));

            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        });
        _ = std::fs::remove_dir_all(&stage_dir);
        assert!(result.is_ok())
    }

    #[test]
    #[serial]
    fn test_extract_auto_zstd() {
        test_extraction("zstd", |dir| {
            let extract_arg = ExtractArgs {
                chdir: Some(dir.to_string()),
                file: "test-materials/base.tar.zst".to_string(),
                compression: CompressionType::Auto,
            };
            do_extract(extract_arg).unwrap();
        });
    }

    #[test]
    #[serial]
    fn test_extract() {
        test_extraction("normal", |dir| {
            let extract_arg = ExtractArgs {
                chdir: Some(dir.to_string()),
                file: "test-materials/base.tar".to_string(),
                compression: CompressionType::Auto,
            };
            do_extract(extract_arg).unwrap();
        });
    }
}
