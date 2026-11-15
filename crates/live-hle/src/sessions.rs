//! Local session backend and the public match ABI. The owned session/attribute
//! types are the seam for a future HTTP worker; no server request happens here.
//! Searches (including FindFromID) deliberately return an empty discovery view.
use crate::{
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::BTreeMap;

const MAX_ATTRIBUTES: u32 = 64;
const MAX_SESSIONS: usize = 32;
const MAX_RESULTS: u32 = 1000;
const MAX_RESULTS_BYTES: u32 = 16 * 1024 * 1024;
const TYPE_MASK: u32 = 0x00f00000;
const STRING: u32 = 0x00100000;
const BLOB: u32 = 0x00200000;
const NULL: u32 = 0x00f00000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAttributeValue {
    Integer(u64),
    String(String),
    Blob(Vec<u8>),
    Null,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAttribute {
    pub id: u32,
    pub value: SessionAttributeValue,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSlots {
    pub public_filled: u32,
    pub public_open: u32,
    pub private_filled: u32,
    pub private_open: u32,
}
impl SessionSlots {
    fn from_args(a: &[u32]) -> Self {
        Self {
            public_filled: a[0],
            public_open: a[1],
            private_filled: a[2],
            private_open: a[3],
        }
    }
}
/// Snapshot of an advertised local session. Exchange keys stay private to the
/// backend/task and are never included in debug output or future event data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSession {
    pub id: [u8; 8],
    pub host_address: [u8; 36],
    pub host_users: Vec<u64>,
    pub slots: SessionSlots,
    pub attributes: BTreeMap<u32, SessionAttributeValue>,
    pub revision: u64,
}
struct Record {
    session: LocalSession,
    key: [u8; 16],
}
enum Task {
    Session { id: [u8; 8], key: [u8; 16] },
    Done,
    EmptySearch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Create,
    Update,
    Info,
    Delete,
    Find,
    Search,
    Results,
    Parse,
    ResultsLen,
}
impl Op {
    pub fn starts_task(self) -> bool {
        matches!(
            self,
            Self::Create | Self::Update | Self::Delete | Self::Find | Self::Search
        )
    }
}
#[derive(Default)]
pub(crate) struct Sessions {
    records: BTreeMap<[u8; 8], Record>,
    tasks: BTreeMap<u32, Task>,
}
enum Failure {
    Fault(GuestFault),
    Hr(u32),
}
impl From<GuestFault> for Failure {
    fn from(f: GuestFault) -> Self {
        Self::Fault(f)
    }
}
type Result<T> = std::result::Result<T, Failure>;
fn hr<T>(value: u32) -> Result<T> {
    Err(Failure::Hr(value))
}
impl Sessions {
    pub fn snapshot(&self) -> Vec<LocalSession> {
        self.records.values().map(|r| r.session.clone()).collect()
    }
    pub fn clear(&mut self) {
        self.records.clear();
        self.tasks.clear();
    }
    pub fn poll(&self, h: u32) -> Option<u32> {
        self.tasks.contains_key(&h).then_some(TASK_SUCCESS)
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        host: [u8; 36],
        users: &[u64; 4],
    ) -> std::result::Result<u32, Error> {
        match self.checked(op, a, m, storage, host, users) {
            Ok(v) => Ok(v),
            Err(Failure::Hr(v)) => Ok(v),
            Err(Failure::Fault(f)) => Err(Error::Fault(f)),
        }
    }
    fn checked(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        host: [u8; 36],
        users: &[u64; 4],
    ) -> Result<u32> {
        match op {
            Op::Create | Op::Update => {
                let offset = usize::from(op == Op::Update) * 2;
                let p = &a[offset..];
                work_event(p[6])?;
                m.check(p[7], 4, Access::Write, 1)?;
                // Marshall everything before publishing a task or changing state.
                let attributes = attributes(m, p[5], p[4], op == Op::Create)?;
                for i in 0..p[4] {
                    m.check(p[5] + i * 16 + 4, 4, Access::Write, 1)?;
                }
                let slots = SessionSlots::from_args(p);
                let (id, key, mut session) = if op == Op::Create {
                    if self.records.len() >= MAX_SESSIONS {
                        return hr(OUT_OF_MEMORY);
                    }
                    let (id, key) = self.identity()?;
                    (
                        id,
                        key,
                        LocalSession {
                            id,
                            host_address: host,
                            host_users: users.iter().copied().filter(|u| *u != 0).collect(),
                            slots,
                            attributes: BTreeMap::new(),
                            revision: 0,
                        },
                    )
                } else {
                    let id = id(a[0], a[1]);
                    let Some(record) = self.records.get(&id) else {
                        return hr(MATCH_INVALID_SESSION);
                    };
                    (id, record.key, record.session.clone())
                };
                session.slots = slots;
                for attr in attributes {
                    // Attribute identity excludes its datatype. A changed NULL
                    // clears the value; unchanged entries never dereference data.
                    session
                        .attributes
                        .retain(|old, _| old & !TYPE_MASK != attr.id & !TYPE_MASK);
                    if attr.value != SessionAttributeValue::Null {
                        session.attributes.insert(attr.id, attr.value);
                    }
                }
                if session.attributes.len() > MAX_ATTRIBUTES as usize {
                    return hr(BUFFER_TOO_SMALL);
                }
                let changed = self
                    .records
                    .get(&id)
                    .is_none_or(|old| old.session != session);
                if changed {
                    session.revision = session.revision.saturating_add(1);
                }
                let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                // Retail consumes dirty flags after serializing the request.
                for i in 0..p[4] {
                    m.set_u32(p[5] + i * 16 + 4, 0)?;
                }
                m.set_u32(p[7], h)?;
                if changed {
                    debug("advertise", &session);
                }
                self.records.insert(id, Record { session, key });
                self.tasks.insert(h, Task::Session { id, key });
                Ok(0)
            }
            Op::Info => {
                let Some(Task::Session { id, key }) = self.tasks.get(&a[0]) else {
                    return hr(INVALID_ARGUMENT);
                };
                m.check(a[1], 8, Access::Write, 1)?;
                m.check(a[2], 16, Access::Write, 1)?;
                m.write_bytes(a[1], id, 1)?;
                m.write_bytes(a[2], key, 1)?;
                Ok(0)
            }
            Op::Delete => {
                work_event(a[2])?;
                m.check(a[3], 4, Access::Write, 1)?;
                let id = id(a[0], a[1]);
                if !self.records.contains_key(&id) {
                    return hr(MATCH_INVALID_SESSION);
                }
                let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                m.set_u32(a[3], h)?;
                let record = self.records.remove(&id).unwrap();
                debug("delete", &record.session);
                self.tasks.insert(h, Task::Done);
                Ok(0)
            }
            Op::Search => {
                work_event(a[5])?;
                m.check(a[6], 4, Access::Write, 1)?;
                if a[1] > MAX_RESULTS || a[4] < 0x5c || a[4] > MAX_RESULTS_BYTES {
                    return hr(MATCH_INVALID_SEARCH);
                }
                let params = attributes(m, a[3], a[2], true)?;
                if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
                    eprintln!(
                        "live match search debug: procedure={} limit={} params={params:?}; local discovery is empty",
                        a[0], a[1]
                    );
                }
                self.empty_search(m, storage, a[6])
            }
            Op::Find => {
                work_event(a[2])?;
                m.check(a[3], 4, Access::Write, 1)?;
                self.empty_search(m, storage, a[3])
            }
            Op::Results => {
                if !matches!(self.tasks.get(&a[0]), Some(Task::EmptySearch)) {
                    return hr(INVALID_ARGUMENT);
                }
                m.check(a[1], 4, Access::Write, 1)?;
                m.check(a[2], 4, Access::Write, 1)?;
                m.set_u32(a[1], 0)?;
                m.set_u32(a[2], 0)?;
                Ok(0)
            }
            Op::ResultsLen => results_len(m, a),
            Op::Parse => {
                if a[0] == 0 {
                    return hr(INVALID_ARGUMENT);
                }
                m.check(a[0], 0x54, Access::Read, 1)?;
                if m.u32(a[0] + 0x50)? != 0 {
                    return hr(MATCH_INVALID_RESULT_COLUMN);
                }
                // The retail parser returns immediately for an empty record,
                // without touching the specs or destination. Searches issue no
                // records, so the title normally never calls this path at all.
                Ok(0)
            }
        }
    }
    fn empty_search(&mut self, m: &mut Guest, storage: &mut Storage, out: u32) -> Result<u32> {
        let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        m.set_u32(out, h)?;
        self.tasks.insert(h, Task::EmptySearch);
        Ok(0)
    }
    fn identity(&self) -> Result<([u8; 8], [u8; 16])> {
        loop {
            let mut data = crate::network::random(24).map_err(|_| Failure::Hr(OUT_OF_MEMORY))?;
            data[0] = (data[0] & 0x0f) | 0x80; // XNET_XNKID_ONLINE_PEER
            let id: [u8; 8] = data[..8].try_into().unwrap();
            if !self.records.contains_key(&id) {
                return Ok((id, data[8..].try_into().unwrap()));
            }
        }
    }
}
fn debug(action: &str, session: &LocalSession) {
    if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
        eprintln!("live session {action} debug: {session:?}");
    }
}
fn id(lo: u32, hi: u32) -> [u8; 8] {
    (u64::from(lo) | u64::from(hi) << 32).to_le_bytes()
}
fn work_event(event: u32) -> Result<()> {
    if event == 0 {
        Ok(())
    } else {
        hr(INVALID_ARGUMENT)
    }
}
fn attributes(m: &Guest, p: u32, count: u32, all: bool) -> Result<Vec<SessionAttribute>> {
    if count > MAX_ATTRIBUTES {
        return hr(BUFFER_TOO_SMALL);
    }
    m.check(p, count as usize * 16, Access::Read, 1)?;
    let mut output = Vec::new();
    for i in 0..count {
        let at = p + i * 16;
        if !all && m.u32(at + 4)? == 0 {
            continue;
        }
        let id = m.u32(at)?;
        let value = match id & TYPE_MASK {
            0 => SessionAttributeValue::Integer(m.u64(at + 8)?),
            STRING => SessionAttributeValue::String(string(m, m.u32(at + 8)?)?),
            BLOB => {
                let n = m.u32(at + 12)?;
                if n > 800 {
                    return hr(MATCH_BLOB_TOO_LONG);
                }
                let mut bytes = vec![0; n as usize];
                m.read_bytes(m.u32(at + 8)?, &mut bytes, 1)?;
                SessionAttributeValue::Blob(bytes)
            }
            NULL => SessionAttributeValue::Null,
            _ => return hr(MATCH_INVALID_DATA_TYPE),
        };
        output.push(SessionAttribute { id, value });
    }
    Ok(output)
}
fn string(m: &Guest, p: u32) -> Result<String> {
    let mut units = Vec::new();
    for i in 0..=400 {
        let value = m.u16(p.wrapping_add(i * 2))?;
        if value == 0 {
            return String::from_utf16(&units).map_err(|_| Failure::Hr(MATCH_INVALID_STRING));
        }
        units.push(value);
    }
    hr(MATCH_STRING_TOO_LONG)
}
fn results_len(m: &Guest, a: &[u32]) -> Result<u32> {
    // DWORD return: refuse unsafe/overflowing capacities with zero rather than
    // turning a validation HRESULT into a giant guest allocation.
    if a[0] > MAX_RESULTS || a[1] > MAX_ATTRIBUTES {
        return Ok(0);
    }
    m.check(a[2], a[1] as usize * 8, Access::Read, 1)?;
    let mut per = 0x54u64;
    for i in 0..a[1] {
        let at = a[2] + i * 8;
        let ty = m.u32(at)?;
        let len = m.u32(at + 4)?;
        let extra = match ty {
            0 => 8,
            STRING if len <= 400 => u64::from(len) * 2 + 2,
            BLOB if len <= 800 => u64::from(len) + 2,
            NULL => 0,
            _ => return Ok(0),
        };
        per += 8 + extra;
    }
    let total = 0x5c + u64::from(a[0]) * per;
    Ok(if total > u64::from(MAX_RESULTS_BYTES) {
        0
    } else {
        total as u32
    })
}
