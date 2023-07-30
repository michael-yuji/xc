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

use crate::container::ProcessStat;
use crate::container::error::ExecError;

use freebsd::event::EventFdNotify;
use std::sync::Arc;
use tokio::sync::watch::Sender;

#[derive(Debug)]
pub struct ProcessRunnerStat {
    pub(super) id: String,
    pub(super) pid: u32,
    pub(super) process_stat: Sender<ProcessStat>,
    pub(super) exit_notify: Option<Arc<EventFdNotify>>,
    pub(super) notify: Option<Arc<EventFdNotify>>,
}

impl ProcessRunnerStat {
    pub(super) fn pid(&self) -> u32 {
        self.pid
    }
    pub(super) fn id(&self) -> &str {
        self.id.as_str()
    }

    pub(super) fn set_error(&mut self, error: ExecError) {
        let err = format!("{error:#?}");
        self.process_stat.send_if_modified(|status| {
            status.exec_error = Some(err);
            true
        });
    }

    pub(super) fn set_exited(&mut self, exit_code: i32) {
        self.process_stat.send_if_modified(|status| {
            status.set_exited(exit_code);
            true
        });
        if let Some(notify) = &self.exit_notify {
            notify
                .clone()
                .notify_waiters_with_value(exit_code as u64 + 1);
        }
    }

    pub(super) fn set_tree_exited(&mut self) {
        self.process_stat.send_if_modified(|status| {
            status.set_tree_exited();
            true
        });
        if let Some(notify) = &self.notify {
            let exit_code = self
                .process_stat
                .borrow()
                .exit_code
                .expect("The entire tree exited but not the process itself?!");
            notify
                .clone()
                .notify_waiters_with_value(exit_code as u64 + 1);
        }
    }
}
