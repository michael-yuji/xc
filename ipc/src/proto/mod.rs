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
use nix::libc::{ENOENT, EPERM};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::packet::codec::FromPacket;
use crate::packet::Packet;

#[derive(Error, Debug)]
pub enum IpcError {
    #[error("serialization error: {0}")]
    Serde(serde_json::Error),
    #[error("io error: {0}")]
    Io(std::io::Error),
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    pub value: Value,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub errno: i32,
    pub value: Value,
}

impl Response {
    pub fn to_err_typed<T: DeserializeOwned>(self) -> Result<ErrResponse<T>, IpcError> {
        Ok(ErrResponse {
            errno: self.errno,
            value: serde_json::from_value(self.value).map_err(IpcError::Serde)?,
        })
    }
}

pub fn write_request<V: FromPacket>(method: &str, value: V) -> serde_json::Result<Packet> {
    value
        .to_packet_failable(|dual| serde_json::to_value(&dual))?
        .map_into_failable(|value| {
            serde_json::to_vec(&Request {
                method: method.to_string(),
                value,
            })
        })
}

pub fn write_response<V: FromPacket>(errno: i32, value: V) -> serde_json::Result<Packet> {
    let packet = value.to_packet_failable(|dual| serde_json::to_value(&dual))?;
    packet.map_into_failable(|value| serde_json::to_vec(&Response { errno, value }))
}

#[derive(Serialize, Deserialize, Error, Debug)]
pub struct ErrResponse<E> {
    pub errno: i32,
    pub value: E,
}

pub type IpcResult<T, E> = Result<T, ErrResponse<E>>;

pub type GenericResult<T> = Result<T, ErrResponse<Value>>;

pub fn ipc_err<T>(errno: i32, message: &str) -> Result<T, ErrResponse<Value>> {
    Err(ErrResponse {
        errno,
        value: json!({ "error": message }),
    })
}

pub fn enoent<T>(message: &str) -> Result<T, ErrResponse<Value>> {
    Err(ErrResponse {
        errno: ENOENT,
        value: json!({ "error": message }),
    })
}

pub fn eperm<T>(message: &str) -> Result<T, ErrResponse<Value>> {
    Err(ErrResponse {
        errno: EPERM,
        value: json!({ "error": message }),
    })
}
