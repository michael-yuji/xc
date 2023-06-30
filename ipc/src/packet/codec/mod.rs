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
pub mod json;

use crate::packet::TypedPacket;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::os::fd::{AsRawFd, RawFd};

#[derive(Deserialize, Serialize)]
pub struct FdRef(usize);

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Fd(pub RawFd);

impl FromPacket for Fd {
    type Dual = FdRef;
    fn decode_from_dual(value: Self::Dual, fds: &[RawFd]) -> Self {
        Fd(fds[value.0])
    }
    fn encode_to_dual(self, fds: &mut Vec<RawFd>) -> Self::Dual {
        let r = FdRef(fds.len());
        fds.push(self.0);
        r
    }
}

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

pub trait FromPacket {
    type Dual: Serialize + DeserializeOwned;
    fn decode_from_dual(value: Self::Dual, fds: &[RawFd]) -> Self;
    fn encode_to_dual(self, fds: &mut Vec<RawFd>) -> Self::Dual;

    fn from_packet<A, F>(packet: TypedPacket<A>, deserialize: F) -> Self
    where
        Self: Sized,
        F: Fn(&A) -> Self::Dual,
    {
        let data: Self::Dual = deserialize(&packet.data);
        let fds = packet.fds;
        Self::decode_from_dual(data, &fds)
    }

    fn to_packet<A, F>(self, serialize: F) -> TypedPacket<A>
    where
        Self: Sized,
        F: Fn(Self::Dual) -> A,
    {
        let mut fds = Vec::new();
        let dual = self.encode_to_dual(&mut fds);
        let data = serialize(dual);
        TypedPacket { data, fds }
    }

    fn from_packet_failable<A, E, F>(packet: TypedPacket<A>, deserialize: F) -> Result<Self, E>
    where
        Self: Sized,
        F: Fn(&A) -> Result<Self::Dual, E>,
    {
        deserialize(&packet.data).map(|data| {
            let fds = packet.fds;
            Self::decode_from_dual(data, &fds)
        })
    }

    fn to_packet_failable<A, E, F>(self, serialize: F) -> Result<TypedPacket<A>, E>
    where
        Self: Sized,
        F: Fn(Self::Dual) -> Result<A, E>,
    {
        let mut fds = Vec::new();
        let dual = self.encode_to_dual(&mut fds);
        serialize(dual).map(|data| TypedPacket { data, fds })
    }
}

/// Use in-place Vec<T> but without Serialize/Deserialize trait
#[derive(Clone)]
pub struct List<T: FromPacket>(Vec<T>);

impl<T: FromPacket> List<T> {
    pub fn new() -> List<T> {
        List(Vec::new())
    }
    pub fn to_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T: FromPacket> Default for List<T> {
    fn default() -> List<T> {
        List::new()
    }
}

impl<T: FromPacket> FromIterator<T> for List<T> {
    fn from_iter<A>(iter: A) -> Self
    where
        A: IntoIterator<Item = T>,
    {
        Self(Vec::from_iter(iter))
    }
}

impl<T: FromPacket> FromPacket for List<T> {
    type Dual = Vec<T::Dual>;
    fn decode_from_dual(value: Self::Dual, fds: &[RawFd]) -> Self {
        Self::from_iter(value.into_iter().map(|v| T::decode_from_dual(v, fds)))
    }
    fn encode_to_dual(self, fds: &mut Vec<RawFd>) -> Self::Dual {
        self.0.into_iter().map(|v| v.encode_to_dual(fds)).collect()
    }
}

/// Like Option<T> but without Serialize/Deserialize trait
pub enum Maybe<T: FromPacket> {
    Some(T),
    None,
}
impl<T: FromPacket + Clone> Clone for Maybe<T> {
    fn clone(&self) -> Maybe<T> {
        match self {
            Maybe::Some(t) => Maybe::Some(t.clone()),
            Maybe::None => Maybe::None,
        }
    }
}
impl<T: FromPacket> Maybe<T> {
    pub fn from_option(option: Option<T>) -> Self {
        match option {
            None => Self::None,
            Some(value) => Self::Some(value),
        }
    }
    pub fn to_option(self) -> Option<T> {
        match self {
            Self::None => None,
            Self::Some(v) => Some(v),
        }
    }
}

impl<T: FromPacket> FromPacket for Maybe<T> {
    type Dual = Option<T::Dual>;
    fn decode_from_dual(value: Self::Dual, fds: &[RawFd]) -> Self {
        Self::from_option(value.map(|v| T::decode_from_dual(v, fds)))
    }
    fn encode_to_dual(self, fds: &mut Vec<RawFd>) -> Self::Dual {
        self.to_option().map(|v| v.encode_to_dual(fds))
    }
}

impl<T> FromPacket for T
where
    T: Serialize + DeserializeOwned,
{
    type Dual = Self;
    fn decode_from_dual(value: Self::Dual, _fds: &[RawFd]) -> Self {
        value
    }
    fn encode_to_dual(self, _fds: &mut Vec<RawFd>) -> Self::Dual {
        self
    }
}
