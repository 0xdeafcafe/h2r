//! Legacy Xbox storage path ABI and deliberately unavailable remote storage.
//! Title attachments use messages.rs instead; no guest path reaches the host FS.
use crate::{
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, guest::Access};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    CreatePath,
    Download,
    DownloadMemory,
    DownloadResults,
    Upload,
    UploadMemory,
    Progress,
}
impl Op {
    pub fn starts_task(self) -> bool {
        matches!(
            self,
            Self::Download | Self::DownloadMemory | Self::Upload | Self::UploadMemory
        )
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Download,
    DownloadMemory,
    Upload,
}
#[derive(Clone, Copy)]
struct Task {
    kind: Kind,
}
impl Task {
    fn result(self) -> u32 {
        if self.kind == Kind::Upload {
            STORAGE_ACCESS_DENIED
        } else {
            STORAGE_FILE_NOT_FOUND
        }
    }
}
#[derive(Default)]
pub(crate) struct Files {
    tasks: BTreeMap<u32, Task>,
}
type Result<T> = std::result::Result<T, Error>;
impl Files {
    pub fn clear(&mut self) {
        self.tasks.clear();
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }
    pub fn poll(&self, h: u32) -> Option<u32> {
        self.tasks.get(&h).map(|t| t.result())
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
    ) -> Result<u32> {
        use Op::*;
        match op {
            CreatePath => return create_path(a, m),
            DownloadResults => {
                return Ok(match self.tasks.get(&a[0]) {
                    Some(t) if t.kind == Kind::DownloadMemory => t.result(),
                    _ => INVALID_ARGUMENT,
                });
            }
            Progress => {
                let Some(t) = self.tasks.get(&a[0]) else {
                    return Ok(INVALID_ARGUMENT);
                };
                for (p, size) in [(a[1], 4), (a[2], 8), (a[3], 8)] {
                    if p != 0 {
                        m.check(p, size, Access::Write, 1)?;
                    }
                }
                for (p, size) in [(a[1], 4), (a[2], 8), (a[3], 8)] {
                    if p != 0 {
                        m.write_bytes(p, &[0; 8][..size], 1)?;
                    }
                }
                return Ok(t.result());
            }
            _ => {}
        }
        if !(1..=5).contains(&a[0]) {
            return Ok(STORAGE_INVALID_FACILITY);
        }
        if wide(m, a[2])?.is_none() {
            return Ok(STORAGE_INVALID_PATH);
        }
        let (event, out, kind) = match op {
            Download => (a[5], a[6], Kind::Download),
            DownloadMemory => {
                m.check(a[3], a[4] as usize, Access::Write, 1)?;
                (a[6], a[7], Kind::DownloadMemory)
            }
            Upload => (a[7], a[8], Kind::Upload),
            UploadMemory => {
                m.check(a[5], a[6] as usize, Access::Read, 1)?;
                (a[8], a[9], Kind::Upload)
            }
            _ => unreachable!(),
        };
        if event != 0 {
            return Err(Error::Unverified("nonzero work event"));
        }
        m.check(out, 4, Access::Write, 1)?;
        let Some(handle) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, handle)?;
        self.tasks.insert(handle, Task { kind });
        Ok(0)
    }
}
fn wide(m: &Guest, p: u32) -> Result<Option<Vec<u16>>> {
    let mut units = Vec::new();
    // A malformed pointer faults through Guest. Bound unterminated host work;
    // this is well above the SDK's published 256-character storage limit.
    for i in 0..65536u32 {
        let v = m.u16(p.wrapping_add(i * 2))?;
        if v == 0 {
            return Ok(Some(units));
        }
        units.push(v);
    }
    Ok(None)
}
fn encoded(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .copied()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn create_path(a: &[u32], m: &mut Guest) -> Result<u32> {
    let user = u64::from(a[1]) | (u64::from(a[2]) << 32);
    let team = u64::from(a[3]) | (u64::from(a[4]) << 32);
    let title = 0x4d530064u32;
    let prefix = match a[0] {
        3 => format!("//tteam.{title:08x}/u:{team:016x}/{title:08x}/{user:016x}/"),
        4 => format!("//title.{title:08x}/t:{title:08x}/"),
        5 => format!("//tuser.{title:08x}/u:{user:016x}/{title:08x}/"),
        _ => return Ok(STORAGE_INVALID_FACILITY),
    };
    m.check(a[7], 4, Access::Write, 1)?;
    let capacity = m.u32(a[7])? as usize;
    let prefix: Vec<_> = prefix.encode_utf16().collect();
    // Retail 0x003c79f2 formats the prefix before checking the full length.
    if a[6] != 0 && capacity > prefix.len() {
        m.write_bytes(a[6], &encoded(&prefix), 1)?;
    }
    let Some(name) = wide(m, a[5])? else {
        return Ok(STORAGE_INVALID_PATH);
    };
    let required = prefix.len() + name.len() + 1;
    if capacity < required {
        m.set_u32(a[7], required as u32)?;
        return Ok(INSUFFICIENT_BUFFER);
    }
    if a[6] != 0 {
        let mut result = prefix;
        result.extend_from_slice(&name);
        m.write_bytes(a[6], &encoded(&result), 1)?;
    }
    m.set_u32(a[7], required as u32)?;
    Ok(0)
}
