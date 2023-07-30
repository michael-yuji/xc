mod format;
mod jailfile;

use crate::jailfile::directives::*;

use clap::Parser;
use oci_util::image_reference::ImageReference;
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
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
    workdir: PathBuf,
}

struct BuildContext {
    stages: Vec<BuildStage>,
}

struct BuildStage {
    dependencies: Vec<String>,
    directives: Vec<jailfile::parse::Action>,
}

fn main() {
    let Args {
        network,
        dns_servers,
        dns_searchs,
        empty_dns,
        output_inplace,
        image_reference,
        mut workdir,
    } = Args::parse();

    workdir.push("Jailfile");

    let jailfile = std::fs::read_to_string(workdir).expect("cannot open Jailfile");
}
