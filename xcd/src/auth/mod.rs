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
use crate::ipc::Variables;
use freebsd::net::UnixCredential;
use ipc::service::ConnectionContext;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;

/// The credential of a connection
#[derive(Clone, Debug)]
pub struct Credential {
    /// Represents the unix user/groups which will be used to check if files/directories should be
    /// accessible.
    unix_credential: UnixCredential,
    /// Reserved. The intention is to allow administrators to grant extra permission / capabilities
    /// to users as tokens, allowing on-demand permission grants
    #[allow(dead_code)]
    grants: Vec<String>,
}

impl Credential {
    pub(crate) fn from_conn_ctx(cctx: &ConnectionContext<Variables>) -> Credential {
        let unix_credential = cctx.unix_credential.clone().unwrap();
        Credential {
            unix_credential,
            grants: Vec::new(),
        }
    }

    #[inline]
    pub(crate) fn can_read(&self, metadata: &Metadata) -> bool {
        let mode = metadata.mode();
        mode & 0o004 > 0
            || mode & 0o400 > 0 && self.unix_credential.uid == metadata.uid()
            || mode & 0o040 > 0 && self.unix_credential.gids.contains(&metadata.gid())
    }

    #[inline]
    pub(crate) fn can_write(&self, metadata: &Metadata) -> bool {
        let mode = metadata.mode();
        mode & 0o002 > 0
            || mode & 0o200 > 0 && self.unix_credential.uid == metadata.uid()
            || mode & 0o020 > 0 && self.unix_credential.gids.contains(&metadata.gid())
    }

    #[inline]
    pub(crate) fn can_exec(&self, metadata: &Metadata) -> bool {
        let mode = metadata.mode();
        mode & 0o001 > 0
            || mode & 0o100 > 0 && self.unix_credential.uid == metadata.uid()
            || mode & 0o010 > 0 && self.unix_credential.gids.contains(&metadata.gid())
    }

    pub(crate) fn can_mount(&self, metadata: &Metadata, readonly: bool) -> bool {
        self.can_exec(metadata) && self.can_read(metadata) && (readonly || self.can_write(metadata))
    }
}
