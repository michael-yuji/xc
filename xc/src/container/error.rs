// Copyright (c), , Yan Ka, Chiu.
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
use freebsd::libc::*;
use std::io::ErrorKind;
use thiserror::Error;

#[macro_export]
macro_rules! errx {
    ($errno:expr, $($t:tt)*) => {
        return Err(xc::container::error::Error::new($errno, anyhow::anyhow!($($t)*)).into())
    }
}
#[macro_export]
macro_rules! err {
    ($errno:expr, $($t:tt)*) => {
        xc::container::error::Error::new($errno, anyhow::anyhow!($($t)*))
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
    #[error("User {0} not found")]
    NotSuchUser(String),
    #[error("Group {0} not found")]
    NotSuchGroup(String),
}

#[derive(Error, Debug)]
pub struct Error {
    errno: i32,
    source: anyhow::Error,
}

impl std::fmt::Display for Error {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.source)
    }
}

impl Error {
    pub fn new(errno: i32, source: anyhow::Error) -> Error {
        Error { errno, source }
    }

    pub fn errno(&self) -> i32 {
        self.errno
    }

    pub fn with_errno(self, errno: i32) -> Error {
        Self { errno, ..self }
    }

    pub fn error_message(&self) -> String {
        format!("{:#}", self.source)
    }
}

pub trait WithErrno {
    fn with_errno(self, errno: i32) -> Error;
}

impl<E: std::error::Error + Send + Sync + 'static> WithErrno for E {
    fn with_errno(self, errno: i32) -> Error {
        Error {
            errno,
            source: anyhow::Error::from(self),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Error {
        Error {
            errno: 128,
            source: error,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Error {
        let errno = match error.kind() {
            ErrorKind::NotFound => ENOENT,
            ErrorKind::OutOfMemory => ENOMEM,
            ErrorKind::Interrupted => EINTR,
            ErrorKind::Other => ETIMEDOUT,
            #[cfg(feature = "io_error_more")]
            ErrorKind::Deadlock => EDEADLK,
            ErrorKind::AddrInUse => EADDRINUSE,
            ErrorKind::BrokenPipe => EPIPE,
            ErrorKind::WouldBlock => EWOULDBLOCK,
            #[cfg(feature = "io_error_more")]
            ErrorKind::NetworkDown => ENETDOWN,
            #[cfg(feature = "io_error_more")]
            ErrorKind::InvalidData => EINVAL,
            #[cfg(feature = "io_error_more")]
            ErrorKind::StorageFull => ENOSPC,
            #[cfg(feature = "io_error_more")]
            ErrorKind::NotSeekable => ENXIO,
            #[cfg(feature = "io_error_more")]
            ErrorKind::Unsupported => ENOTSUP,
            ErrorKind::NotConnected => ENOTCONN,
            #[cfg(feature = "io_error_more")]
            ErrorKind::IsADirectory => EISDIR,
            #[cfg(feature = "io_error_more")]
            ErrorKind::InvalidInput => EINVAL,
            #[cfg(feature = "io_error_more")]
            ErrorKind::FileTooLarge => EFBIG,
            #[cfg(feature = "io_error_more")]
            ErrorKind::ResourceBusy => EBUSY,
            #[cfg(feature = "io_error_more")]
            ErrorKind::TooManyLinks => ELOOP,
            ErrorKind::AlreadyExists => EEXIST,
            ErrorKind::TimedOut => ETIMEDOUT,
            #[cfg(feature = "io_error_more")]
            ErrorKind::NotADirectory => ENOTDIR,
            #[cfg(feature = "io_error_more")]
            ErrorKind::FilesystemLoop => ELOOP,
            #[cfg(feature = "io_error_more")]
            ErrorKind::CrossesDevices => ENOTSUP,
            ErrorKind::ConnectionReset => ECONNRESET,
            #[cfg(feature = "io_error_more")]
            ErrorKind::HostUnreachable => EHOSTUNREACH,
            #[cfg(feature = "io_error_more")]
            ErrorKind::InvalidFilename => ENAMETOOLONG,
            ErrorKind::PermissionDenied => EPERM,
            ErrorKind::AddrNotAvailable => EADDRNOTAVAIL,
            ErrorKind::ConnectionRefused => ECONNREFUSED,
            ErrorKind::ConnectionAborted => ECONNABORTED,
            #[cfg(feature = "io_error_more")]
            ErrorKind::DirectoryNotEmpty => ENOTEMPTY,
            #[cfg(feature = "io_error_more")]
            ErrorKind::NetworkUnreachable => ENETUNREACH,
            #[cfg(feature = "io_error_more")]
            ErrorKind::ReadOnlyFilesystem => EROFS,
            #[cfg(feature = "io_error_more")]
            ErrorKind::ExecutableFileBusy => ETXTBSY,
            #[cfg(feature = "io_error_more")]
            ErrorKind::ArgumentListTooLong => E2BIG,
            #[cfg(feature = "io_error_more")]
            ErrorKind::StaleNetworkFileHandle => ESTALE,
            #[cfg(feature = "io_error_more")]
            ErrorKind::FilesystemQuotaExceeded => EDQUOT,
            _ => 128,
        };

        Error {
            errno,
            source: error.into(),
        }
    }
}

pub mod errno {

    macro_rules! export {
        ($($name:ident, $desc:expr),*) => {
            $(pub const $name: i32 = freebsd::libc::$name;)*

            pub fn errno_desc(v: i32) -> &'static str {
                #[allow(unreachable_patterns)]
                match v {
                    $($name => $desc,)*
                    _ => "unknown"
                }
            }
        }
    }
    export!(
        EPERM,
        "Operation not permitted",
        ENOENT,
        "No such file or directory",
        ESRCH,
        "No such process",
        EINTR,
        "Interrupted system call",
        EIO,
        "Input/output error",
        ENXIO,
        "Device not configured",
        E2BIG,
        "Argument list too long",
        ENOEXEC,
        "Exec format error",
        EBADF,
        "Bad file descriptor",
        ECHILD,
        "No child processes",
        EDEADLK,
        "Resource deadlock avoided",
        ENOMEM,
        "Cannot allocate memory",
        EACCES,
        "Permission denied",
        EFAULT,
        "Bad address",
        ENOTBLK,
        "Block device required",
        EBUSY,
        "Device busy",
        EEXIST,
        "File exists",
        EXDEV,
        "Cross-device link",
        ENODEV,
        "Operation not supported by device",
        ENOTDIR,
        "Not a directory",
        EISDIR,
        "Is a directory",
        EINVAL,
        "Invalid argument",
        ENFILE,
        "Too many open files in system",
        EMFILE,
        "Too many open files",
        ENOTTY,
        "Inappropriate ioctl for device",
        ETXTBSY,
        "Text file busy",
        EFBIG,
        "File too large",
        ENOSPC,
        "No space left on device",
        ESPIPE,
        "Illegal seek",
        EROFS,
        "Read-only filesystem",
        EMLINK,
        "Too many links",
        EPIPE,
        "Broken pipe",
        EDOM,
        "Numerical argument out of domain",
        ERANGE,
        "Result too large",
        EAGAIN,
        "Resource temporarily unavailable",
        EWOULDBLOCK,
        "Operation would block",
        EINPROGRESS,
        "Operation now in progress",
        EALREADY,
        "Operation already in progress",
        ENOTSOCK,
        "Socket operation on non-socket",
        EDESTADDRREQ,
        "Destination address required",
        EMSGSIZE,
        "Message too long",
        EPROTOTYPE,
        "Protocol wrong type for socket",
        ENOPROTOOPT,
        "Protocol not available",
        EPROTONOSUPPORT,
        "Protocol not supported",
        ESOCKTNOSUPPORT,
        "Socket type not supported",
        EOPNOTSUPP,
        "Operation not supported",
        ENOTSUP,
        "Operation not supported",
        EPFNOSUPPORT,
        "Protocol family not supported",
        EAFNOSUPPORT,
        "Address family not supported by protocol family",
        EADDRINUSE,
        "Address already in use",
        EADDRNOTAVAIL,
        "Can't assign requested address",
        ENETDOWN,
        "Network is down",
        ENETUNREACH,
        "Network is unreachable",
        ENETRESET,
        "Network dropped connection on reset",
        ECONNABORTED,
        "Software caused connection abort",
        ECONNRESET,
        "Connection reset by peer",
        ENOBUFS,
        "No buffer space available",
        EISCONN,
        "Socket is already connected",
        ENOTCONN,
        "Socket is not connected",
        ESHUTDOWN,
        "Can't send after socket shutdown",
        ETOOMANYREFS,
        "Too many references: can't splice",
        ETIMEDOUT,
        "Operation timed out",
        ECONNREFUSED,
        "Connection refused",
        ELOOP,
        "Too many levels of symbolic links",
        ENAMETOOLONG,
        "File name too long",
        EHOSTDOWN,
        "Host is down",
        EHOSTUNREACH,
        "No route to host",
        ENOTEMPTY,
        "Directory not empty",
        EPROCLIM,
        "Too many processes",
        EUSERS,
        "Too many users",
        EDQUOT,
        "Disc quota exceeded",
        ESTALE,
        "Stale NFS file handle",
        EREMOTE,
        "Too many levels of remote in path",
        EBADRPC,
        "RPC struct is bad",
        ERPCMISMATCH,
        "RPC version wrong",
        EPROGUNAVAIL,
        "RPC prog. not avail",
        EPROGMISMATCH,
        "Program version wrong",
        EPROCUNAVAIL,
        "Bad procedure for program",
        ENOLCK,
        "No locks available",
        ENOSYS,
        "Function not implemented",
        EFTYPE,
        "Inappropriate file type or format",
        EAUTH,
        "Authentication error",
        ENEEDAUTH,
        "Need authenticator",
        EIDRM,
        "Identifier removed",
        ENOMSG,
        "No message of desired type",
        EOVERFLOW,
        "Value too large to be stored in data type",
        ECANCELED,
        "Operation canceled",
        EILSEQ,
        "Illegal byte sequence",
        ENOATTR,
        "Attribute not found",
        EDOOFUS,
        "Programming error",
        EBADMSG,
        "Bad message",
        EMULTIHOP,
        "Multihop attempted",
        ENOLINK,
        "Link has been severed",
        EPROTO,
        "Protocol error",
        ENOTCAPABLE,
        "Capabilities insufficient",
        ECAPMODE,
        "Not permitted in capability mode",
        ENOTRECOVERABLE,
        "State not recoverable",
        EOWNERDEAD,
        "Previous owner died",
        EINTEGRITY,
        "Integrity check failed"
    );
}
