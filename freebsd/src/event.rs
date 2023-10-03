//! kqueue/eventfd related routines and extension

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

use nix::errno::Errno;
use nix::libc::{close, intptr_t, EFD_NONBLOCK};
use nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent};
use nix::unistd::dup;
use std::os::fd::{AsRawFd, RawFd};
use tokio::io::unix::AsyncFd;

extern "C" {
    pub fn eventfd(initval: std::os::raw::c_int, flags: std::os::raw::c_int)
        -> std::os::raw::c_int;
    pub fn eventfd_read(fd: std::os::fd::RawFd, value: *mut u64) -> std::os::raw::c_int;
    pub fn eventfd_write(fd: std::os::fd::RawFd, value: u64) -> std::os::raw::c_int;
}

pub type Notify = tokio::sync::Notify;

/// Notify like construct but backed by non-blocking eventfd(2). The fd is than closed when this
/// struct dropped.
///
/// There are few reason to use this instead of tokio::sync::Notify, one being since this is
/// eventfd based, it can be send across processes. It can also use synchronously by calling
/// `notified_sync`.
#[derive(Debug)]
pub struct EventFdNotify {
    fd: RawFd,
}

impl Drop for EventFdNotify {
    fn drop(&mut self) {
        unsafe { close(self.fd) };
    }
}

impl AsRawFd for EventFdNotify {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Default for EventFdNotify {
    fn default() -> Self {
        Self::new()
    }
}

impl EventFdNotify {
    pub fn from_fd(fd: RawFd) -> EventFdNotify {
        EventFdNotify { fd }
    }

    pub fn notify_waiters_with_value(&self, value: u64) {
        unsafe { eventfd_write(self.fd, value) };
    }

    pub fn notify_waiters(&self) {
        self.notify_waiters_with_value(1);
    }

    pub fn as_async_fd(&self) -> Result<AsyncFd<RawFd>, std::io::Error> {
        let new_fd = dup(self.fd).unwrap();
        AsyncFd::new(new_fd)
    }

    pub async fn notified_take_value(&self) -> std::io::Result<u64> {
        _ = self.as_async_fd()?.readable().await?;
        unsafe {
            let mut v = 0u64;
            if eventfd_read(self.fd, &mut v) != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(v)
            }
        }
    }

    pub async fn notified(&self) -> std::io::Result<()> {
        _ = self.as_async_fd()?.readable().await?;
        Ok(())
    }

    pub fn notified_sync(&self) {
        let kevent = KEvent::from_read(self.fd);
        let kq = nix::sys::event::Kqueue::new().unwrap();
        let out = KEvent::zero();
        kq.wait_events(&[kevent], &mut [out]);
    }

    pub fn notified_sync_take_value(&self) -> std::io::Result<u64> {
        let kevent = KEvent::from_read(self.fd);
        let kq = nix::sys::event::Kqueue::new().unwrap();
        let out = KEvent::zero();
        kq.wait_events(&[kevent], &mut [out]);

        unsafe {
            let mut v = 0u64;
            if eventfd_read(self.fd, &mut v) != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(v)
            }
        }
    }

    pub fn new() -> EventFdNotify {
        let fd = unsafe { eventfd(0, EFD_NONBLOCK) };
        EventFdNotify { fd }
    }
}

pub struct EventFdNotified<'a>(&'a EventFdNotify);

impl std::future::Future for EventFdNotified<'_> {
    type Output = ();
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        let mut v = 0u64;
        let xx = unsafe { eventfd_read(self.0.fd, &mut v) };
        if xx == 0 {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    }
}

pub trait KqueueExt {
    fn wait_events(&self, changelist: &[KEvent], eventlist: &mut [KEvent]) -> nix::Result<usize>;
}

pub fn kevent_classic(
    kq: i32,
    changelist: &[KEvent],
    eventlist: &mut [KEvent],
) -> nix::Result<usize> {
    loop {
        let res = unsafe {
            nix::libc::kevent(
                kq,
                changelist.as_ptr() as *const nix::libc::kevent,
                changelist.len() as i32,
                eventlist.as_mut_ptr() as *mut nix::libc::kevent,
                eventlist.len() as i32,
                std::ptr::null(),
            )
        };

        let errno = nix::errno::errno();

        match res {
            -1 => {
                if errno != nix::libc::EINTR {
                    break Err(nix::errno::Errno::from_i32(errno))
                }
            }
            size => break Ok(size as usize),
        }
    }
}

impl KqueueExt for nix::sys::event::Kqueue {
    fn wait_events(&self, changelist: &[KEvent], eventlist: &mut [KEvent]) -> nix::Result<usize> {
        loop {
            match self.kevent(changelist, eventlist, None) {
                Ok(size) => break Ok(size),
                Err(errno) if errno != Errno::EINTR => break Err(errno),
                _ => continue
            }
        }
    }
}

pub trait KEventExt {
    fn zero() -> KEvent {
        unsafe { std::mem::zeroed() }
    }

    fn from_read(fd: i32) -> KEvent {
        KEvent::new(
            fd as usize,
            EventFilter::EVFILT_READ,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::empty(),
            0 as intptr_t,
            0 as intptr_t,
        )
    }
    fn from_write(fd: i32) -> KEvent {
        KEvent::new(
            fd as usize,
            EventFilter::EVFILT_WRITE,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::empty(),
            0 as intptr_t,
            0 as intptr_t,
        )
    }
    fn from_wait_pid(pid: u32) -> KEvent {
        KEvent::new(
            pid as usize,
            EventFilter::EVFILT_PROC,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::NOTE_EXIT,
            0 as intptr_t,
            0 as intptr_t,
        )
    }
    fn from_wait_pfd(pfd: i32) -> KEvent {
        KEvent::new(
            pfd as usize,
            EventFilter::EVFILT_PROCDESC,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::NOTE_EXIT,
            0 as intptr_t,
            0 as intptr_t,
        )
    }
    fn from_trace_pid(pid: u32, evs: FilterFlag) -> KEvent {
        KEvent::new(
            pid as usize,
            EventFilter::EVFILT_PROC,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE,
            FilterFlag::NOTE_TRACK | evs,
            0 as intptr_t,
            0 as intptr_t,
        )
    }
    fn from_timer_seconds_oneshot(ident: usize, seconds: usize) -> KEvent {
        KEvent::new(
            ident,
            EventFilter::EVFILT_TIMER,
            EventFlag::EV_ADD | EventFlag::EV_ENABLE | EventFlag::EV_ONESHOT,
            FilterFlag::NOTE_SECONDS,
            seconds as intptr_t,
            0 as intptr_t,
        )
    }
    fn set_flags(&self, flag: EventFlag) -> KEvent;
}

impl KEventExt for KEvent {
    fn set_flags(&self, flag: EventFlag) -> KEvent {
        KEvent::new(
            self.ident(),
            self.filter().unwrap(),
            flag,
            self.fflags(),
            self.data(),
            self.udata(),
        )
    }
}
