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
use thiserror::Error;
#[macro_export]
macro_rules! precondition_failure {
    ($errno:expr, $($t:tt)*) => {
        return Err(xc::container::error::PreconditionFailure::new($errno, anyhow::anyhow!($($t)*)).into())
    }
}

#[derive(Error, Debug)]
pub enum ExecError {
    #[error("Executable not found in container")]
    ExecutableNotFound,
    #[error("Cannot rebrand executable")]
    BrandELFFailed(std::io::Error),
    #[error("Cannot open log file at {0}: {1}")]
    CannotOpenLogFile(String, std::io::Error),
    #[error("Cannot bind to socket {0}")]
    CannotBindUnixSocket(std::io::Error),
    #[error("Cannot spawn executable: {0}")]
    CannotSpawn(std::io::Error),
    #[error("Linux ABI kernel module not loaded")]
    MissingLinuxKmod,
}

#[derive(Error, Debug)]
pub struct PreconditionFailure {
    errno: i32,
    source: anyhow::Error,
}

impl std::fmt::Display for PreconditionFailure {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.source)
    }
}

impl PreconditionFailure {
    pub fn new(errno: i32, source: anyhow::Error) -> PreconditionFailure {
        PreconditionFailure { errno, source }
    }

    pub fn errno(&self) -> i32 {
        self.errno
    }

    pub fn error_message(&self) -> String {
        format!("{:#}", self.source)
    }
}
