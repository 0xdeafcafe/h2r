//! The title's MOTD/hopper inbox. All task state and buffers here are guest-side;
//! the service worker returns owned file bytes and never accesses guest memory.
use crate::{
    client::{File, Pending, Reply, Service, Session},
    entries::Operation,
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::{BTreeMap, BTreeSet};
pub const SUMMARY_SIZE: usize = 64;
pub const ATTACHMENT_CAP: u32 = 0x499a;
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone)]
enum Kind {
    Details { user: u32, id: u32 },
    Download { buffer: u32, capacity: u32 },
}
#[derive(Clone)]
struct Task {
    kind: Kind,
    pending: Option<Pending>,
    file: Option<File>,
    status: u32,
}
#[derive(Default)]
pub(crate) struct Messages {
    files: Vec<File>,
    error: u32,
    deleted: BTreeSet<(u32, u32)>,
    flags: BTreeMap<(u32, u32), u32>,
    tasks: BTreeMap<u32, Task>,
}
impl Messages {
    pub fn set_files(&mut self, files: Vec<File>, error: u32) {
        self.files = files;
        self.error = error;
        self.deleted.clear();
        self.flags.clear();
    }
    pub fn clear(&mut self) {
        self.files.clear();
        self.error = 0;
        self.deleted.clear();
        self.flags.clear();
        self.tasks.clear();
    }
    pub fn close(&mut self, handle: u32) -> bool {
        self.tasks.remove(&handle).is_some()
    }
    fn file(&self, user: u32, id: u32) -> Option<File> {
        self.files
            .iter()
            .find(|f| f.id == id && !self.deleted.contains(&(user, id)))
            .cloned()
    }
    pub fn poll(
        &mut self,
        handle: u32,
        m: &mut Guest,
    ) -> std::result::Result<Option<u32>, GuestFault> {
        let Some(task) = self.tasks.get_mut(&handle) else {
            return Ok(None);
        };
        if task.status != 0 {
            return Ok(Some(task.status));
        }
        if let Kind::Download { buffer, capacity } = task.kind {
            m.check(
                buffer,
                capacity.min(ATTACHMENT_CAP) as usize,
                Access::Write,
                1,
            )?;
        }
        let Some(reply) = task.pending.as_ref().and_then(Pending::take) else {
            return Ok(Some(0));
        };
        match reply {
            Err(e) => task.status = e,
            Ok(Reply::File(file)) => {
                match task.kind {
                    Kind::Download { buffer, capacity } => {
                        if file.bytes.len() > capacity.min(ATTACHMENT_CAP) as usize {
                            task.status = BUFFER_TOO_SMALL;
                        } else {
                            m.write_bytes(buffer, &file.bytes, 1)?;
                            task.status = TASK_SUCCESS;
                        }
                    }
                    Kind::Details { .. } => task.status = TASK_SUCCESS,
                }
                task.file = Some(file);
            }
            _ => task.status = INVALID_ARGUMENT,
        }
        task.pending = None;
        Ok(Some(task.status))
    }
    fn task(
        &mut self,
        m: &mut Guest,
        storage: &mut Storage,
        out: u32,
        kind: Kind,
        fetch: impl FnOnce() -> std::result::Result<Pending, u32>,
    ) -> Result<u32> {
        let Some(handle) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        let pending = match fetch() {
            Ok(p) => p,
            Err(e) => return Ok(e),
        };
        m.set_u32(out, handle)?;
        self.tasks.insert(
            handle,
            Task {
                kind,
                pending: Some(pending),
                file: None,
                status: 0,
            },
        );
        Ok(0)
    }
    pub fn invoke(
        &mut self,
        op: Operation,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        service: Option<&Service>,
        session: Option<&Session>,
    ) -> Result<u32> {
        use Operation::*;
        match op {
            MessageEnumerate => {
                m.check(a[2], 4, Access::Write, 1)?;
                if self.error != 0 {
                    return Ok(self.error);
                }
                let files: Vec<_> = self
                    .files
                    .iter()
                    .filter(|f| !self.deleted.contains(&(a[0], f.id)))
                    .collect();
                m.check(a[1], files.len() * SUMMARY_SIZE, Access::Write, 1)?;
                for (i, file) in files.iter().enumerate() {
                    let mut summary = [0; SUMMARY_SIZE];
                    // XUID is 12 bytes (pack 4), then BYTE, pad to the 8-byte context.
                    summary[12] = 1;
                    summary[16..24].copy_from_slice(&file.version().to_le_bytes());
                    summary[24..32].copy_from_slice(&file.sent_time.to_le_bytes());
                    summary[32..36].copy_from_slice(&file.id.to_le_bytes());
                    let flags = *self.flags.get(&(a[0], file.id)).unwrap_or(&1);
                    summary[36..40].copy_from_slice(&flags.to_le_bytes());
                    summary[40..44].copy_from_slice(&0x4d530064u32.to_le_bytes());
                    summary[46..48].copy_from_slice(&256u16.to_le_bytes());
                    summary[48..60].copy_from_slice(b"Live service");
                    m.write_bytes(a[1] + i as u32 * SUMMARY_SIZE as u32, &summary, 1)?;
                }
                m.set_u32(a[2], files.len() as u32)?;
                Ok(0)
            }
            MessageDetails => {
                work_event(a[4])?;
                m.check(a[5], 4, Access::Write, 1)?;
                if (a[2] | a[3]) & !0xff0001ff != 0 {
                    return Err(Error::Unverified("message flag mask"));
                }
                let Some(file) = self.file(a[0], a[1]) else {
                    return Ok(MESSAGE_INVALID_ID);
                };
                let (Some(service), Some(session)) = (service, session) else {
                    return Ok(NO_SESSION);
                };
                let result = self.task(
                    m,
                    storage,
                    a[5],
                    Kind::Details {
                        user: a[0],
                        id: a[1],
                    },
                    || service.file(session, file.name, file.id),
                )?;
                if result == 0 {
                    let flags = self.flags.entry((a[0], a[1])).or_insert(1);
                    *flags = (*flags | a[2]) & !a[3];
                }
                Ok(result)
            }
            MessageProperty => self.property(a, m),
            MessageDownload => {
                work_event(a[4])?;
                m.check(a[5], 4, Access::Write, 1)?;
                let Some(status) = self.poll(a[0], m)? else {
                    return Ok(INVALID_ARGUMENT);
                };
                if status == 0 {
                    return Ok(TASK_BUSY);
                }
                if status & 0x80000000 != 0 {
                    return Ok(status);
                }
                let task = self.tasks.get(&a[0]).unwrap();
                let Kind::Details { user, id } = task.kind else {
                    return Ok(INVALID_ARGUMENT);
                };
                if a[1] & 0xffff != 0x9c1 {
                    return Ok(MESSAGE_PROPERTY_NOT_FOUND);
                }
                if self.deleted.contains(&(user, id)) {
                    return Ok(MESSAGE_INVALID_ID);
                }
                let file = task.file.as_ref().unwrap().clone();
                if file.bytes.len() > a[3].min(ATTACHMENT_CAP) as usize {
                    return Ok(BUFFER_TOO_SMALL);
                }
                m.check(a[2], a[3].min(ATTACHMENT_CAP) as usize, Access::Write, 1)?;
                let (Some(service), Some(session)) = (service, session) else {
                    return Ok(NO_SESSION);
                };
                self.task(
                    m,
                    storage,
                    a[5],
                    Kind::Download {
                        buffer: a[2],
                        capacity: a[3],
                    },
                    || service.file(session, file.name, file.id),
                )
            }
            MessageDownloadResults => {
                for p in &a[1..4] {
                    if *p != 0 {
                        m.check(*p, 4, Access::Write, 1)?;
                    }
                }
                let Some(status) = self.poll(a[0], m)? else {
                    return Ok(INVALID_ARGUMENT);
                };
                if status == 0 || status & 0x80000000 != 0 {
                    return Ok(status);
                }
                let task = self.tasks.get(&a[0]).unwrap();
                let Kind::Download { buffer, .. } = task.kind else {
                    return Ok(INVALID_ARGUMENT);
                };
                let len = task.file.as_ref().unwrap().bytes.len() as u32;
                for (p, v) in [(a[1], buffer), (a[2], len), (a[3], len)] {
                    if p != 0 {
                        m.set_u32(p, v)?;
                    }
                }
                Ok(0)
            }
            MessageDelete => {
                if self.file(a[0], a[1]).is_none() {
                    return Ok(MESSAGE_INVALID_ID);
                }
                if a[2] > 1 {
                    return Ok(INVALID_ARGUMENT);
                }
                // These messages have a title-service sender, not an account to block.
                self.deleted.insert((a[0], a[1]));
                Ok(0)
            }
            _ => unreachable!(),
        }
    }
    fn property(&mut self, a: &[u32], m: &mut Guest) -> Result<u32> {
        for p in &a[4..6] {
            if *p != 0 {
                m.check(*p, 4, Access::Write, 1)?;
            }
        }
        let Some(status) = self.poll(a[0], m)? else {
            return Ok(INVALID_ARGUMENT);
        };
        if status == 0 || status & 0x80000000 != 0 {
            return Ok(status);
        }
        let task = self.tasks.get(&a[0]).unwrap();
        if !matches!(task.kind, Kind::Details { .. }) {
            return Ok(INVALID_ARGUMENT);
        }
        let file = task.file.as_ref().unwrap();
        if a[1] & 0xffff == 0x9c1 {
            if a[4] != 0 {
                m.set_u32(a[4], file.bytes.len() as u32)?;
            }
            if a[5] != 0 {
                m.set_u32(a[5], 0)?;
            }
            return Ok(MESSAGE_DOWNLOAD_REQUIRED);
        }
        let value = match a[1] & 0xffff {
            0x3c2 => 1u16.to_le_bytes().to_vec(),
            0x4c3 => 1u32.to_le_bytes().to_vec(),
            0x6c4 => file
                .name
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect(),
            0x4c5 => file.id.to_le_bytes().to_vec(),
            0x581 => file.version().to_le_bytes().to_vec(),
            _ => return Ok(MESSAGE_PROPERTY_NOT_FOUND),
        };
        if a[4] != 0 {
            m.set_u32(a[4], value.len() as u32)?;
        }
        if a[5] != 0 {
            m.set_u32(a[5], 0)?;
        }
        if a[3] == 0 || a[2] < value.len() as u32 {
            return Ok(BUFFER_TOO_SMALL);
        }
        m.write_bytes(a[3], &value, 1)?;
        Ok(0)
    }
}
fn work_event(event: u32) -> Result<()> {
    if event == 0 {
        Ok(())
    } else {
        Err(Error::Unverified("nonzero work event"))
    }
}
