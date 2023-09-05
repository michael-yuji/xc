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
use freebsd::fs::zfs::ZfsHandle;
use freebsd::net::ifconfig::{
    add_to_bridge, create_alias, interface_up, move_to_jail, remove_alias, remove_from_bridge,
    remove_from_jail,
};
use ipcidr::IpCidr;
use paste::paste;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::PathBuf;
use tracing::{debug, error};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UndoStack {
    pub undos: Vec<Undo>,
}

impl UndoStack {
    pub fn new() -> UndoStack {
        UndoStack::default()
    }

    pub fn pop(&mut self) -> Result<(), anyhow::Error> {
        if let Some(undo) = self.undos.pop() {
            undo.run()
        } else {
            Ok(())
        }
    }

    pub fn pop_all(&mut self) -> Result<()> {
        while let Some(undo) = self.undos.pop() {
            undo.run()?;
        }
        Ok(())
    }
}

macro_rules! impl_undos {
   ($($name:ident($($arg:ident: $argtpe:ty $([$adoc:expr])?),*) $(-> $rt:ty)?
       { $doc:expr, $run_code: expr, $unwind_code:expr });*) => {

        macro_rules! unwrap_parens {
            () => {
                ()
            };
            ($inner:ty) => {
                $inner
            };
        }

        #[derive(Serialize, Deserialize, Clone, Debug)]
        pub enum Undo {
            #[allow(unused_parens)]
            $($name { result: unwrap_parens!($($rt)?), $($arg: $argtpe),* }),*
        }

        #[allow(dead_code, unused)]
        impl Undo {
            fn run(&self) -> Result<()> {
                match self {
                    $(Undo::$name { result, $($arg),* } => {
                        debug!("cleaning up {}", stringify!($name));
                        #[allow(clippy::redundant_closure_call)]
                        {
                            $unwind_code(result)
                        }?;
                    }),*
                }
                Ok(())
            }
        }

        #[allow(dead_code, unused)]
        impl UndoStack {
        $(
            paste! {
                #[doc = $doc]
                #[doc = ""]
                #[doc = "# Arguments"]
                $(
                    #[doc = "* `" $arg "` - " $($adoc)? ""]
                )*
                pub fn [<$name:snake>](&mut self, $($arg: $argtpe),*) -> Result<($($rt)?)> {
                    let res = { $run_code };
                    match res {
                        Ok(r) => {
                            self.undos.push(Undo::$name { result: r.clone(), $($arg: $arg.clone()),* });
                            Ok(r)
                        },
                        Err(e) => {
                            error!("panic at {}: {e:#?}", stringify!($name));
                            panic!()
                        }
                    }
                }
            }
        )*
        }
    }
}

extern "C" {
    fn pdgetpid(fd: std::os::fd::RawFd, pid: *mut u32) -> i32;
    fn kill(pid: u32, signum: i32) -> i32;
}
impl_undos! {

    DupFd(fd: std::os::fd::RawFd [""])
    {
        "",
        Ok::<(), anyhow::Error>(()),
        |_| {
            let i = unsafe {
                let mut pid = 0;
                let mut r = 0;
                r = pdgetpid(*fd, &mut pid);
                if r != 0 {
                    r
                } else {
                    kill(pid, 9)
                }
            };
            eprintln!("kill: {i}");
            Ok::<(), anyhow::Error>(())
        }
    };

    ZfsCreate(
        handle: ZfsHandle [""],
        dataset: String ["dataset to create"])
    {
        "Create a zfs dataset",
        handle.create2(&dataset, false, false),
//        create_dataset(&dataset),
        |_| {
            handle.destroy(dataset, true, true, true)
        }
    };

    ZfsClone(
        handle: ZfsHandle [""],
        src: String  ["source dataset"],
        tag: String  ["tag of the snapshot"],
        dest: String ["replica dataset"])
    {
        "Clone a ZFS dataset from `src` to `dest`",
        {
            eprintln!("zfs clone2 {src}@{tag} -> {dest}");
            handle.clone2(&src, &tag, &dest).map(|_| ())
        },
        |result| {
            handle.destroy(dest, true, true, true)
        }
    };

    ZfsSnap(
        handle: ZfsHandle [""],
        src: String ["source dataset"],
        tag: String ["tag of the snapshot"])
    {
        "Take a ZFS snapshot named `tag`",
        handle.snapshot2(&src, &tag),
        |_| {
            handle.destroy(format!("{src}@{tag}"), true, true, true)
        }
    };

    MoveIf(
        interface: String ["iface to move"],
        jid: i32 ["jail id"])
    {
        "Move a network interface `interface` from host vnet to `jid`",
        move_to_jail(&interface, jid),
        |result| {
            remove_from_jail(interface, *jid)
        }
    };

    IfaceCreateAlias(
        interface: String ["Network interface the ip alias will be created on"],
        address: IpCidr   ["The IP address alias with a netmask"])
    {
        "Create an IP alias on interface `interface` with cidr `address`",
        create_alias(&interface, &address),
        |result| remove_alias(interface, address)
    };

    IfaceUp(interface: String ["Interface to be set up"])
    {
        "Apply ifconfig up on the selected interface",
        interface_up(&interface),
        |_| { Ok::<(), anyhow::Error>(()) }
    };

    BridgeAddIface(
        bridge: String    ["The bridge network interface the interface adding to"],
        interface: String ["The name of thenetwork interface that will be added"])
    {
        "Add a network interface `interface` to a bridge interface `bridge`",
        add_to_bridge(&bridge, &interface),
        |result| {
            remove_from_bridge(bridge, interface)
        }
    };

    Mount(
        fs_type: String      ["Type of the filesystem"],
        options: Vec<String> ["Mount options to use during mount"],
        source:    OsString ["The source location / origin of the filesystem"],
        mountpoint: PathBuf ["The directory the filesystem mount on"])
    {
        "Mount a filesystem with type `fs_type` of/at `source` to `mountpoint` with options `options`",
        freebsd::fs::mount(&fs_type, &source, &mountpoint, &options),
        |result| freebsd::fs::umount(mountpoint)
    };

    CreateEpair() -> (String, String) {
        "",
        freebsd::net::ifconfig::create_epair(),
        |result: &(String, String)| {
            let (epair_a, epair_b) = result.clone();
            freebsd::net::ifconfig::destroy_interface(epair_a)
        }
    };

    PfTableAddAddress(anchor: Option<String>, table: String, address: IpCidr) {
        "add an address to a pf table",
        freebsd::net::pf::table_add_address(anchor.clone(), &table, &address),
        |_| {
            freebsd::net::pf::table_del_address(anchor.clone(), table, address)
        }
    };

    PfCreateAnchor(anchor: String) {
        "create a new pf anchor",
        freebsd::net::pf::set_rules(Some(anchor.to_string()), &["\n"]),
        |_| {
            freebsd::net::pf::set_rules(Some(anchor.to_string()), &["\n"])
        }
    };

    JailDataset(zfs_handle: ZfsHandle, jail: String, dataset: PathBuf) {
        "jail a zfs dataset to `jail`",
        zfs_handle.jail(jail.as_str(), &dataset),
        |_| {
            // by the time the undo stack rewind, the jail has already been destroyed and therefore
            // unjail will not work.
            Ok::<(), anyhow::Error>(())
        }
    }
}
