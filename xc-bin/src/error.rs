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
use ipc::proto::IpcError;
use ipc::transport::ChannelError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("{0:#?}")]
    Channel(ChannelError<IpcError>),
    #[error("io-error: {0:#?}")]
    Io(std::io::Error),
    #[error("{0:#?}")]
    Anyhow(anyhow::Error),
}

impl From<ChannelError<IpcError>> for ActionError {
    fn from(value: ChannelError<IpcError>) -> ActionError {
        ActionError::Channel(value)
    }
}

impl From<std::io::Error> for ActionError {
    fn from(value: std::io::Error) -> ActionError {
        ActionError::Io(value)
    }
}

impl From<anyhow::Error> for ActionError {
    fn from(value: anyhow::Error) -> ActionError {
        ActionError::Anyhow(value)
    }
}
