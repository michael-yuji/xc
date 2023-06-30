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
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::path::Path;

pub const ELF_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const E_IDENT_LEN: usize = 16;
pub const OS_ABI_OFFSET: usize = 7;

macro_rules! impl_elf_brands {
    ($($brand:ident -> $value:expr),*) => {
        pub fn os_abi(v: u8) -> ElfBrand {
            match v {
                $($value => ElfBrand::$brand),*,
                _ => ElfBrand::Unknown
            }
        }

        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        pub enum ElfBrand {
            $($brand),*,
            Unknown
        }

        impl ElfBrand {
            pub fn byte_value(&self) -> Option<u8> {
                match self {
                    $(ElfBrand::$brand => Some($value)),*,
                    ElfBrand::Unknown => None
                }
            }

            pub fn is_supported(&self) -> bool {
                match self {
                    ElfBrand::FreeBSD | ElfBrand::Solaris => true,
                    _ => false
                }
            }
        }
    }
}

impl_elf_brands!(
    SVR4 -> 0,
    HPUX -> 1,
    NetBSD -> 2,
    Linux -> 3,
    Hurd -> 4,
    Solaris -> 6,
    AIX -> 7,
    IRIX -> 8,
    FreeBSD -> 9,
    Tru64 -> 10,
    Modesto -> 11,
    OpenBSD -> 12,
    OpenVMS -> 13,
    Nsk -> 14,
    Aros -> 15,
    FenixOS -> 16,
    CloudABI -> 17,
    OpenVOS -> 18,
    ArmEABI -> 64,
    Arm -> 97,
    Standalone -> 255
);

pub fn which_elf(path: impl AsRef<Path>) -> Result<Option<ElfBrand>, std::io::Error> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < E_IDENT_LEN as u64 {
        Ok(None)
    } else {
        let mut e_ident = [0u8; 16];
        file.read_exact(&mut e_ident)?;
        if e_ident[..4] != ELF_MAG {
            Ok(None)
        } else {
            Ok(Some(os_abi(e_ident[OS_ABI_OFFSET])))
        }
    }
}

pub fn brand_elf_if_unsupported(
    path: impl AsRef<Path>,
    brand: ElfBrand,
) -> Result<Option<ElfBrand>, std::io::Error> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    if file.metadata()?.len() < E_IDENT_LEN as u64 {
        Ok(None)
    } else {
        let mut e_ident = [0u8; 16];
        file.read_exact(&mut e_ident)?;
        if e_ident[..4] != ELF_MAG {
            Ok(None)
        } else {
            let old_abi = os_abi(e_ident[OS_ABI_OFFSET]);
            if old_abi.is_supported() {
                Ok(Some(old_abi))
            } else {
                file.write_at(&[brand.byte_value().unwrap()], OS_ABI_OFFSET as u64)?;
                Ok(Some(brand))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brand_elf() -> anyhow::Result<()> {
        let file_content: [u8; 16] = [
            0x7f, b'E', b'L', b'F', 255, 255, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        let branded: [u8; 16] = [
            0x7f, b'E', b'L', b'F', 255, 255, 255, 9, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        let temp_path = "test_brand_elf";

        std::fs::write(&temp_path, &file_content)?;
        let res = brand_elf_if_unsupported(&temp_path, ElfBrand::FreeBSD)?;
        assert_eq!(res, Some(ElfBrand::FreeBSD));
        let bytes = std::fs::read(&temp_path)?;
        assert_eq!(&bytes, &branded);

        std::fs::remove_file(&temp_path)?;

        Ok(())
    }
}
