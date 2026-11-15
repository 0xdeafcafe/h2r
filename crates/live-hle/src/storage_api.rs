//! Checked storage path ABI and asynchronous per-title resource downloads.
//! Only the verified title paths reach the HTTP worker; none reach the host FS.
use crate::{
    client::{FILES, File, Pending, Reply, Service, Session},
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
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
struct Receive {
    buffer: u32,
    capacity: u32,
    id: u32,
}
struct Task {
    kind: Kind,
    status: u32,
    receive: Option<Receive>,
    pending: Option<Pending>,
    received: u32,
}
#[derive(Default)]
pub(crate) struct Files {
    tasks: BTreeMap<u32, Task>,
}
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
}
type Result<T> = std::result::Result<T, Error>;
impl Files {
    pub fn clear(&mut self) {
        self.tasks.clear();
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }
    pub fn poll(&mut self, h: u32, m: &mut Guest) -> std::result::Result<Option<u32>, GuestFault> {
        let Some(task) = self.tasks.get_mut(&h) else {
            return Ok(None);
        };
        if task.status != 0 {
            return Ok(Some(task.status));
        }
        // Revalidate before consuming the completion, so a guest fault cannot
        // discard it or partially copy the file. No worker sees guest memory.
        let receive = task.receive.as_ref().expect("pending memory download");
        m.check(receive.buffer, receive.capacity as usize, Access::Write, 1)?;
        let Some(reply) = task.pending.as_ref().and_then(Pending::take) else {
            return Ok(Some(0));
        };
        task.status = match reply {
            Ok(Reply::File(File { id, bytes })) if id == receive.id => {
                if bytes.len() > receive.capacity as usize {
                    BUFFER_TOO_SMALL
                } else {
                    m.write_bytes(receive.buffer, &bytes, 1)?;
                    task.received = bytes.len() as u32;
                    TASK_SUCCESS
                }
            }
            Err(e) => e,
            _ => INVALID_ARGUMENT,
        };
        task.pending = None;
        Ok(Some(task.status))
    }
    pub fn invoke(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        use Op::*;
        match op {
            CreatePath => return create_path(a, m),
            DownloadResults => {
                if !self
                    .tasks
                    .get(&a[0])
                    .is_some_and(|t| t.kind == Kind::DownloadMemory)
                {
                    return Ok(INVALID_ARGUMENT);
                }
                // Optional outputs: buffer, received, total, owner PUID, FILETIME.
                for (p, size) in [(a[1], 4), (a[2], 4), (a[3], 4), (a[4], 8), (a[5], 8)] {
                    if p != 0 {
                        m.check(p, size, Access::Write, 1)?;
                    }
                }
                let status = self.poll(a[0], m)?.unwrap();
                if status == 0 {
                    return Ok(TASK_BUSY);
                }
                if status & 0x80000000 != 0 {
                    return Ok(status);
                }
                let task = &self.tasks[&a[0]];
                let buffer = task.receive.as_ref().unwrap().buffer;
                for (p, value) in [(a[1], buffer), (a[2], task.received), (a[3], task.received)] {
                    if p != 0 {
                        m.set_u32(p, value)?;
                    }
                }
                // Shared title storage has no account owner. The file endpoint
                // supplies no creation timestamp; return unknown (zero).
                for p in [a[4], a[5]] {
                    if p != 0 {
                        m.write_bytes(p, &[0; 8], 1)?;
                    }
                }
                return Ok(0);
            }
            Progress => {
                if !self.tasks.contains_key(&a[0]) {
                    return Ok(INVALID_ARGUMENT);
                }
                for (p, size) in [(a[1], 4), (a[2], 8), (a[3], 8)] {
                    if p != 0 {
                        m.check(p, size, Access::Write, 1)?;
                    }
                }
                let status = self.poll(a[0], m)?.unwrap();
                let bytes = u64::from(self.tasks[&a[0]].received);
                if a[1] != 0 {
                    m.set_u32(a[1], if status == TASK_SUCCESS { 100 } else { 0 })?;
                }
                for p in [a[2], a[3]] {
                    if p != 0 {
                        m.write_bytes(p, &bytes.to_le_bytes(), 1)?;
                    }
                }
                return Ok(status);
            }
            _ => {}
        }
        if !(1..=5).contains(&a[0]) {
            return Ok(STORAGE_INVALID_FACILITY);
        }
        let Some(path) = wide(m, a[2])? else {
            return Ok(STORAGE_INVALID_PATH);
        };
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
        let Some(handle) = ctx.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        let mut task = Task {
            kind,
            status: if kind == Kind::Upload {
                STORAGE_ACCESS_DENIED
            } else {
                STORAGE_FILE_NOT_FOUND
            },
            receive: None,
            pending: None,
            received: 0,
        };
        if op == DownloadMemory
            && let Some((id, name)) = title_file(a[0], &path)
            && let (Some(service), Some(session)) = (ctx.service, ctx.session)
        {
            task.pending = Some(match service.file(session, name, id) {
                Ok(pending) => pending,
                Err(e) => return Ok(e),
            });
            task.receive = Some(Receive {
                buffer: a[3],
                capacity: a[4],
                id,
            });
            task.status = 0;
        }
        m.set_u32(out, handle)?;
        self.tasks.insert(handle, task);
        Ok(0)
    }
}
fn title_file(facility: u32, path: &[u16]) -> Option<(u32, &'static str)> {
    if facility != 4 {
        return None;
    }
    let text = String::from_utf16(path).ok()?;
    let relative = text.strip_prefix("//title.4d530064/t:4d530064/")?;
    let name = relative
        .strip_prefix("9809/")
        .or_else(|| relative.strip_prefix("test/"))?;
    FILES.into_iter().find(|(_, file)| *file == name)
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
