//! Parser for original Xbox executables (XBE).
//!
//! The parser borrows the file bytes and resolves everything relative to the
//! virtual address space the image is loaded into.

use std::fmt;

const MAGIC: &[u8; 4] = b"XBEH";
const SECTION_HEADER_SIZE: usize = 56;
const LIBRARY_VERSION_SIZE: usize = 16;

/// XOR keys for the entry point and kernel thunk address, per build kind.
const KEYS: [(BuildKind, u32, u32); 3] = [
    (BuildKind::Retail, 0xA8FC_57AB, 0x5B6D_40B6),
    (BuildKind::Debug, 0x9485_9D4B, 0xEFB1_F152),
    (BuildKind::Chihiro, 0x40B5_C16E, 0x2290_059D),
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("not an XBE (bad magic)")]
    BadMagic,
    #[error("truncated: needed {len} bytes at file offset {offset:#x}")]
    Truncated { offset: usize, len: usize },
    #[error("address {0:#010x} is not backed by the file")]
    Unmapped(u32),
    #[error("entry point does not decode with any known key")]
    UnknownKey,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildKind {
    Retail,
    Debug,
    Chihiro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionFlags(pub u32);

impl SectionFlags {
    pub const WRITABLE: u32 = 0x01;
    pub const PRELOAD: u32 = 0x02;
    pub const EXECUTABLE: u32 = 0x04;
    pub const INSERTED_FILE: u32 = 0x08;

    pub fn writable(self) -> bool {
        self.0 & Self::WRITABLE != 0
    }
    pub fn preload(self) -> bool {
        self.0 & Self::PRELOAD != 0
    }
    pub fn executable(self) -> bool {
        self.0 & Self::EXECUTABLE != 0
    }
}

impl fmt::Display for SectionFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = |on, ch| if on { ch } else { '-' };
        write!(
            f,
            "{}{}{}",
            c(self.preload(), 'p'),
            c(self.writable(), 'w'),
            c(self.executable(), 'x')
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub flags: SectionFlags,
    pub va: u32,
    pub vsize: u32,
    pub raw_addr: u32,
    pub raw_size: u32,
}

impl Section {
    pub fn contains(&self, va: u32) -> bool {
        va >= self.va && va < self.va.wrapping_add(self.vsize)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub flags: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub title_id: u32,
    pub title_name: String,
    pub game_regions: u32,
    pub version: u32,
}

/// One slot in the kernel thunk table: the game calls `[slot_va]`, which the
/// loader fills with the address of kernel export `ordinal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelImport {
    pub slot_va: u32,
    pub ordinal: u32,
}

pub struct Xbe<'a> {
    data: &'a [u8],
    pub base: u32,
    pub size_of_headers: u32,
    pub size_of_image: u32,
    pub kind: BuildKind,
    pub entry: u32,
    pub kernel_thunk: u32,
    pub tls_addr: u32,
    pub certificate: Certificate,
    pub sections: Vec<Section>,
    pub libraries: Vec<Library>,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated { offset: o, len: 2 })
}

fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(Error::Truncated { offset: o, len: 4 })
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

impl<'a> Xbe<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.get(..4) != Some(MAGIC) {
            return Err(Error::BadMagic);
        }
        let base = u32_at(data, 0x104)?;
        let size_of_headers = u32_at(data, 0x108)?;
        let size_of_image = u32_at(data, 0x10C)?;
        let cert_addr = u32_at(data, 0x118)?;
        let section_count = u32_at(data, 0x11C)? as usize;
        let section_addr = u32_at(data, 0x120)?;
        let entry_enc = u32_at(data, 0x128)?;
        let tls_addr = u32_at(data, 0x12C)?;
        let thunk_enc = u32_at(data, 0x158)?;
        let library_count = u32_at(data, 0x160)? as usize;
        let library_addr = u32_at(data, 0x164)?;

        let mut xbe = Xbe {
            data,
            base,
            size_of_headers,
            size_of_image,
            kind: BuildKind::Retail,
            entry: 0,
            kernel_thunk: 0,
            tls_addr,
            certificate: Certificate {
                title_id: 0,
                title_name: String::new(),
                game_regions: 0,
                version: 0,
            },
            sections: Vec::new(),
            libraries: Vec::new(),
        };

        // Headers are mapped at `base`, so header pointers resolve directly.
        let header = |va: u32| -> Result<usize> {
            va.checked_sub(base)
                .filter(|o| *o < size_of_headers)
                .map(|o| o as usize)
                .ok_or(Error::Unmapped(va))
        };

        for i in 0..section_count {
            let o = header(section_addr)? + i * SECTION_HEADER_SIZE;
            let name_addr = u32_at(data, o + 20)?;
            let name_off = header(name_addr)?;
            xbe.sections.push(Section {
                flags: SectionFlags(u32_at(data, o)?),
                va: u32_at(data, o + 4)?,
                vsize: u32_at(data, o + 8)?,
                raw_addr: u32_at(data, o + 12)?,
                raw_size: u32_at(data, o + 16)?,
                name: cstr(&data[name_off..(name_off + 16).min(data.len())]),
            });
        }

        let c = header(cert_addr)?;
        let title = data
            .get(c + 0xC..c + 0xC + 80)
            .ok_or(Error::Truncated { offset: c + 0xC, len: 80 })?;
        let units: Vec<u16> = title
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|&u| u != 0)
            .collect();
        xbe.certificate = Certificate {
            title_id: u32_at(data, c + 8)?,
            title_name: String::from_utf16_lossy(&units),
            game_regions: u32_at(data, c + 0xA0)?,
            version: u32_at(data, c + 0xAC)?,
        };

        if library_count > 0 {
            let l = header(library_addr)?;
            for i in 0..library_count {
                let o = l + i * LIBRARY_VERSION_SIZE;
                let name = data.get(o..o + 8).ok_or(Error::Truncated { offset: o, len: 8 })?;
                xbe.libraries.push(Library {
                    name: cstr(name),
                    major: u16_at(data, o + 8)?,
                    minor: u16_at(data, o + 10)?,
                    build: u16_at(data, o + 12)?,
                    flags: u16_at(data, o + 14)?,
                });
            }
        }

        // The right key is the one that puts the entry point inside an
        // executable section.
        let (kind, entry, thunk) = KEYS
            .iter()
            .map(|&(k, ek, tk)| (k, entry_enc ^ ek, thunk_enc ^ tk))
            .find(|&(_, e, _)| xbe.sections.iter().any(|s| s.flags.executable() && s.contains(e)))
            .ok_or(Error::UnknownKey)?;
        xbe.kind = kind;
        xbe.entry = entry;
        xbe.kernel_thunk = thunk;
        Ok(xbe)
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn section_containing(&self, va: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains(va))
    }

    /// File bytes backing `[va, va + len)`. Fails for the zero-filled tail of
    /// a section (vsize > raw_size), which has no file backing.
    pub fn bytes_at(&self, va: u32, len: usize) -> Result<&'a [u8]> {
        if let Some(o) = va.checked_sub(self.base).filter(|o| *o < self.size_of_headers) {
            let o = o as usize;
            return self.data.get(o..o + len).ok_or(Error::Unmapped(va));
        }
        let s = self.section_containing(va).ok_or(Error::Unmapped(va))?;
        let off = (va - s.va) as usize;
        if off + len > s.raw_size as usize {
            return Err(Error::Unmapped(va));
        }
        let start = s.raw_addr as usize + off;
        self.data.get(start..start + len).ok_or(Error::Unmapped(va))
    }

    /// The file-backed bytes of a section.
    pub fn section_bytes(&self, s: &Section) -> Result<&'a [u8]> {
        let start = s.raw_addr as usize;
        self.data
            .get(start..start + s.raw_size as usize)
            .ok_or(Error::Truncated { offset: start, len: s.raw_size as usize })
    }

    pub fn read_u32(&self, va: u32) -> Result<u32> {
        let b = self.bytes_at(va, 4)?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }

    /// Kernel thunk table, terminated by a zero slot.
    pub fn kernel_imports(&self) -> Result<Vec<KernelImport>> {
        let mut out = Vec::new();
        let mut slot = self.kernel_thunk;
        loop {
            let v = self.read_u32(slot)?;
            if v == 0 {
                return Ok(out);
            }
            out.push(KernelImport { slot_va: slot, ordinal: v & 0x7FFF_FFFF });
            slot += 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny retail XBE: headers at 0x10000, one `.text` section at
    /// 0x11000, entry at 0x11000, a two-entry kernel thunk table at 0x11010.
    fn sample() -> Vec<u8> {
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1100];
        let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        d[..4].copy_from_slice(MAGIC);
        put(&mut d, 0x104, base);
        put(&mut d, 0x108, 0x1000); // size of headers
        put(&mut d, 0x10C, 0x2000);
        put(&mut d, 0x118, base + 0x400); // certificate
        put(&mut d, 0x11C, 1);
        put(&mut d, 0x120, base + 0x200); // section headers
        put(&mut d, 0x128, 0x11000 ^ 0xA8FC_57AB);
        put(&mut d, 0x158, 0x11010 ^ 0x5B6D_40B6);
        put(&mut d, 0x160, 1);
        put(&mut d, 0x164, base + 0x300);
        // section header
        put(&mut d, 0x200, SectionFlags::EXECUTABLE | SectionFlags::PRELOAD);
        put(&mut d, 0x204, 0x11000);
        put(&mut d, 0x208, 0x100);
        put(&mut d, 0x20C, 0x1000);
        put(&mut d, 0x210, 0x100);
        put(&mut d, 0x214, base + 0x280);
        d[0x280..0x286].copy_from_slice(b".text\0");
        // library
        d[0x300..0x303].copy_from_slice(b"D3D");
        d[0x308..0x30A].copy_from_slice(&1u16.to_le_bytes());
        d[0x30C..0x30E].copy_from_slice(&5849u16.to_le_bytes());
        // certificate
        put(&mut d, 0x408, 0x4D53_0064);
        for (i, ch) in "Halo 2".encode_utf16().enumerate() {
            d[0x40C + i * 2..0x40E + i * 2].copy_from_slice(&ch.to_le_bytes());
        }
        // section data: thunk table at 0x11010
        put(&mut d, 0x1010, 0x8000_0001);
        put(&mut d, 0x1014, 0x8000_00BB);
        d
    }

    #[test]
    fn parses_header() {
        let d = sample();
        let x = Xbe::parse(&d).unwrap();
        assert_eq!(x.kind, BuildKind::Retail);
        assert_eq!(x.entry, 0x11000);
        assert_eq!(x.certificate.title_id, 0x4D53_0064);
        assert_eq!(x.certificate.title_name, "Halo 2");
        assert_eq!(x.sections[0].name, ".text");
        assert!(x.sections[0].flags.executable());
        assert_eq!(x.libraries[0].name, "D3D");
        assert_eq!(x.libraries[0].build, 5849);
    }

    #[test]
    fn reads_kernel_imports() {
        let d = sample();
        let x = Xbe::parse(&d).unwrap();
        let k = x.kernel_imports().unwrap();
        assert_eq!(
            k,
            [
                KernelImport { slot_va: 0x11010, ordinal: 1 },
                KernelImport { slot_va: 0x11014, ordinal: 0xBB },
            ]
        );
    }

    #[test]
    fn bytes_past_raw_size_are_unmapped() {
        let d = sample();
        let x = Xbe::parse(&d).unwrap();
        assert_eq!(x.bytes_at(0x110FE, 4), Err(Error::Unmapped(0x110FE)));
        assert_eq!(x.bytes_at(0x12000, 1), Err(Error::Unmapped(0x12000)));
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(Xbe::parse(b"MZ\0\0").err(), Some(Error::BadMagic));
    }
}
