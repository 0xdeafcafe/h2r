use crate::{config::Account, entries::Operation, status::*};
use runtime::{
    Guest, GuestFault,
    guest::{Access, MapError, Permissions},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddrV4,
};

pub const USER_SIZE: usize = 112;
const USERS_SIZE: usize = USER_SIZE * 4;
const STORAGE_SIZE: u32 = 0x1_0000;
const FIRST_TOKEN: u32 = 0x1000;

pub(crate) enum Error {
    Fault(GuestFault),
    Unverified(&'static str),
}
impl From<GuestFault> for Error {
    fn from(e: GuestFault) -> Self {
        Self::Fault(e)
    }
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EmptyKind {
    Friends,
    Offering,
    Presence,
    Mute,
}
#[derive(Clone)]
enum Task {
    Logon,
    Change {
        users: Box<[u8; USERS_SIZE]>,
        committed: bool,
    },
    Empty {
        kind: EmptyKind,
        pumped: bool,
    },
}

/// A single mapped allocation in the shared HLE window, using Guest's physical
/// allocator/claim machinery. Tokens advance without reuse, including across
/// cleanup, so a stale handle can never alias a new task. Exhaustion fails.
#[derive(Default)]
struct Storage {
    base: Option<u32>,
    cursor: u32,
}
impl Storage {
    fn ensure(&mut self, m: &mut Guest) -> std::result::Result<u32, MapError> {
        if let Some(base) = self.base {
            return Ok(base);
        }
        let mut base = 0xa000_0000u32;
        loop {
            let end = base
                .checked_add(STORAGE_SIZE)
                .filter(|e| *e <= 0xa100_0000)
                .ok_or(MapError::OutOfMemory)?;
            let occupied = m
                .mappings()
                .iter()
                .map(|x| (x.virtual_start, x.len))
                .chain(m.virtual_claims().iter().map(|x| (x.virtual_start, x.len)))
                .find(|&(va, len)| (base as u64) < va as u64 + len as u64 && end > va);
            if let Some((va, len)) = occupied {
                base = va
                    .checked_add(len)
                    .and_then(|v| v.checked_add(0xfff))
                    .ok_or(MapError::OutOfMemory)?
                    & !0xfff;
                continue;
            }
            m.map_zeroed(base, STORAGE_SIZE, Permissions::RW)?;
            // No overlapping claim or mapping remains after the search above.
            m.claim_virtual(base, STORAGE_SIZE, "Live HLE")?;
            self.base = Some(base);
            self.cursor = FIRST_TOKEN;
            return Ok(base);
        }
    }
    fn alloc(&mut self, m: &mut Guest, bytes: u32) -> Option<u32> {
        let base = self.ensure(m).ok()?;
        let end = self.cursor.checked_add(bytes)?.checked_add(7)? & !7;
        if end > STORAGE_SIZE {
            return None;
        }
        let p = base + self.cursor;
        self.cursor = end;
        Some(p)
    }
}

pub(crate) struct State {
    accounts: Vec<Account>,
    address: SocketAddrV4,
    net_refs: u32,
    wsa_refs: u32,
    online_refs: u32,
    storage: Storage,
    tasks: BTreeMap<u32, Task>,
    qos: BTreeSet<u32>,
    logon: Option<u32>,
    services: BTreeSet<u32>,
}
impl State {
    pub fn new(accounts: Vec<Account>, address: SocketAddrV4) -> Self {
        Self {
            accounts,
            address,
            net_refs: 0,
            wsa_refs: 0,
            online_refs: 0,
            storage: Storage::default(),
            tasks: BTreeMap::new(),
            qos: BTreeSet::new(),
            logon: None,
            services: BTreeSet::new(),
        }
    }
    fn task(&mut self, m: &mut Guest, out: u32, task: Task) -> Result<u32> {
        writable(m, out, 4)?;
        let Some(handle) = self.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(handle, 0)?;
        m.set_u32(out, handle)?;
        self.tasks.insert(handle, task);
        Ok(OK)
    }
    fn users(&self, m: &Guest, ptr: u32) -> Result<Option<Box<[u8; USERS_SIZE]>>> {
        let mut input = [0; USERS_SIZE];
        m.read_bytes(ptr, &mut input, 1)?;
        let mut users = Box::new([0; USERS_SIZE]);
        for (src, dst) in input
            .as_chunks::<USER_SIZE>()
            .0
            .iter()
            .zip(users.as_chunks_mut::<USER_SIZE>().0.iter_mut())
        {
            let id = u64::from_le_bytes(src[..8].try_into().unwrap());
            if id == 0 {
                continue;
            }
            let Some(account) = self.accounts.iter().find(|a| a.xuid == id) else {
                return Ok(None);
            };
            dst.copy_from_slice(&record(account));
            dst[8..12].copy_from_slice(&src[8..12]); // Includes the guest slot number.
        }
        Ok(Some(users))
    }
    fn clear_logon(&mut self, m: &mut Guest) -> Result<()> {
        if let Some(base) = self.storage.base {
            m.write_bytes(base, &[0; USERS_SIZE], 1)?;
        }
        self.logon = None;
        self.services.clear();
        Ok(())
    }
    pub fn invoke(&mut self, op: Operation, a: &[u32], m: &mut Guest) -> Result<u32> {
        use Operation::*;
        match op {
            Link => return Ok(crate::PHY_LINK),
            NetStartup => {
                if a[0] != 0 {
                    m.check(a[0], 12, Access::Read, 1)?;
                    if m.u8(a[0])? != 12 {
                        return Ok(WSA_INVALID_ARGUMENT);
                    }
                }
                self.net_refs = self.net_refs.saturating_add(1);
                return Ok(OK);
            }
            WsaStartup => {
                if self.net_refs == 0 {
                    return Ok(WSA_NOT_INITIALIZED);
                }
                if a[0] & 0xffff != 2 && a[0] & 0xffff != 0x202 {
                    return Ok(WSA_VERSION);
                }
                writable(m, a[1], 400)?;
                let mut data = [0; 400];
                data[..2].copy_from_slice(&(a[0] as u16).to_le_bytes());
                data[2..4].copy_from_slice(&0x202u16.to_le_bytes());
                m.write_bytes(a[1], &data, 1)?;
                self.wsa_refs = self.wsa_refs.saturating_add(1);
                return Ok(OK);
            }
            NetCleanup => {
                if self.net_refs == 0 {
                    return Ok(WSA_NOT_INITIALIZED);
                }
                self.net_refs -= 1;
                if self.net_refs == 0 {
                    self.qos.clear();
                }
                return Ok(OK);
            }
            WsaCleanup => {
                if self.wsa_refs == 0 {
                    return Ok(WSA_NOT_INITIALIZED);
                }
                self.wsa_refs -= 1;
                return Ok(OK);
            }
            TitleAddress => {
                writable(m, a[0], 36)?;
                let mut address = [0; 36];
                if self.net_refs != 0 {
                    // XNADDR: ina[0..4], inaOnline[4..8], wPortOnline[8..10].
                    address[..4].copy_from_slice(&self.address.ip().octets());
                    address[4..8].copy_from_slice(&self.address.ip().octets());
                    address[8..10].copy_from_slice(&self.address.port().to_be_bytes());
                    // Locally administered synthetic Ethernet identity, not a console secret.
                    address[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
                }
                m.write_bytes(a[0], &address, 1)?;
                return Ok(if self.net_refs == 0 { 1 } else { 4 });
            }
            QosLookup => {
                if self.net_refs == 0 {
                    return Ok(WSA_NOT_INITIALIZED);
                }
                work_event(a[1])?;
                if a[0] != 0 {
                    return Err(Error::Unverified("QoS service lookup flags"));
                }
                writable(m, a[2], 4)?;
                let Some(p) = self.storage.alloc(m, 32) else {
                    return Ok(WSA_NO_BUFFERS);
                };
                // XNQOS header plus one 24-byte XNQOSINFO. Halo reads entry 0
                // unconditionally at 0x0007b806..0x0007b816, even with count 0.
                // COMPLETE (1), without TARGET_CONTACTED (2): no latency/data.
                let mut qos = [0; 32];
                qos[..4].copy_from_slice(&1u32.to_le_bytes());
                qos[8] = 1;
                m.write_bytes(p, &qos, 1)?;
                m.set_u32(a[2], p)?;
                self.qos.insert(p);
                return Ok(OK);
            }
            QosRelease => {
                return Ok(if self.qos.remove(&a[0]) {
                    OK
                } else {
                    WSA_INVALID_ARGUMENT
                });
            }
            OnlineStartup => {
                if self.net_refs == 0 || self.wsa_refs == 0 {
                    return Ok(NOT_INITIALIZED);
                }
                if a[0] != 0 {
                    m.check(a[0], 4, Access::Read, 1)?;
                }
                if self.storage.ensure(m).is_err() {
                    return Ok(OUT_OF_MEMORY);
                }
                self.online_refs = self.online_refs.saturating_add(1);
                return Ok(OK);
            }
            _ => {}
        }
        if self.online_refs == 0 {
            return Ok(if op == LogonUsers { 0 } else { NOT_INITIALIZED });
        }
        match op {
            OnlineCleanup => {
                self.online_refs -= 1;
                if self.online_refs == 0 {
                    self.clear_logon(m)?;
                    self.tasks.clear();
                }
                Ok(OK)
            }
            GetUsers => {
                writable(m, a[0], USER_SIZE * 16)?;
                writable(m, a[1], 4)?;
                let mut users = [0; USER_SIZE * 16];
                for (dst, account) in users
                    .as_chunks_mut::<USER_SIZE>()
                    .0
                    .iter_mut()
                    .zip(&self.accounts)
                {
                    dst.copy_from_slice(&record(account));
                }
                m.write_bytes(a[0], &users, 1)?;
                m.set_u32(a[1], self.accounts.len() as u32)?;
                Ok(OK)
            }
            Logon => {
                work_event(a[3])?;
                writable(m, a[4], 4)?;
                if self.logon.is_some() {
                    return Ok(TASK_BUSY);
                }
                if a[2] > 16 {
                    return Ok(INVALID_ARGUMENT);
                }
                let Some(users) = self.users(m, a[0])? else {
                    return Ok(INVALID_USER);
                };
                if users.iter().all(|v| *v == 0) {
                    return Ok(NO_USER);
                }
                m.check(a[1], a[2] as usize * 4, Access::Read, 1)?;
                let services = (0..a[2])
                    .map(|i| m.u32(a[1] + i * 4))
                    .collect::<std::result::Result<BTreeSet<_>, _>>()?;
                let result = self.task(m, a[4], Task::Logon)?;
                if result == OK {
                    m.write_bytes(self.storage.base.unwrap(), &users[..], 1)?;
                    self.logon = Some(m.u32(a[4])?);
                    self.services = services;
                }
                Ok(result)
            }
            Continue => {
                let Some(task) = self.tasks.get(&a[0]).cloned() else {
                    return Ok(INVALID_ARGUMENT);
                };
                let (result, next) = match task {
                    Task::Logon => (CONNECTED, Task::Logon),
                    Task::Change { users, committed } => {
                        if committed {
                            (USER_CHANGE_COMPLETE, Task::Change { users, committed })
                        } else {
                            m.write_bytes(self.storage.base.unwrap(), &users[..], 1)?;
                            (
                                USER_CHANGE_COMMIT,
                                Task::Change {
                                    users,
                                    committed: true,
                                },
                            )
                        }
                    }
                    Task::Empty { kind, pumped } => {
                        let result = match kind {
                            EmptyKind::Friends if pumped => RUNNING_IDLE,
                            EmptyKind::Offering => NO_NEW_CONTENT,
                            _ => TASK_SUCCESS,
                        };
                        (result, Task::Empty { kind, pumped: true })
                    }
                };
                self.tasks.insert(a[0], next);
                Ok(result)
            }
            LogonResults => Ok(if matches!(self.tasks.get(&a[0]), Some(Task::Logon)) {
                CONNECTED
            } else {
                INVALID_ARGUMENT
            }),
            LogonUsers => Ok(self.logon.and(self.storage.base).unwrap_or(0)),
            ServiceInfo => {
                if self.logon.is_none() {
                    return Ok(NO_SESSION);
                }
                if !self.services.contains(&a[0]) {
                    return Ok(SERVICE_NOT_REQUESTED);
                }
                if a[1] != 0 {
                    let mut info = [0; 12];
                    info[..4].copy_from_slice(&a[0].to_le_bytes());
                    info[4..8].copy_from_slice(&self.address.ip().octets());
                    info[8..10].copy_from_slice(&self.address.port().to_be_bytes());
                    m.write_bytes(a[1], &info, 1)?;
                }
                Ok(OK)
            }
            Close => {
                if !self.tasks.contains_key(&a[0]) {
                    return Ok(INVALID_ARGUMENT);
                }
                if self.logon == Some(a[0]) {
                    self.clear_logon(m)?;
                    self.tasks.clear();
                } else {
                    self.tasks.remove(&a[0]);
                }
                Ok(OK)
            }
            ChangeUsers => {
                if self.logon.is_none() {
                    return Ok(NO_SESSION);
                }
                work_event(a[1])?;
                let Some(users) = self.users(m, a[0])? else {
                    return Ok(INVALID_USER);
                };
                self.task(
                    m,
                    a[2],
                    Task::Change {
                        users,
                        committed: false,
                    },
                )
            }
            ChangeResults => {
                if !matches!(self.tasks.get(&a[0]), Some(Task::Change { .. })) {
                    return Ok(INVALID_ARGUMENT);
                }
                m.write_bytes(a[1], &[0; 16], 1)?;
                Ok(OK)
            }
            Notification => {
                if self.logon.is_none() {
                    return Ok(NO_SESSION);
                }
                if a[0] >= 4 {
                    return Ok(INVALID_ARGUMENT);
                }
                if a[4] > 1024 {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[5], a[4] as usize, Access::Read, 1)?;
                Ok(OK)
            }
            FriendsStartup | Offering | PresenceInit | MuteStartup | MuteGet => {
                if self.logon.is_none() {
                    return Ok(NO_SESSION);
                }
                let (event, output, kind) = match op {
                    FriendsStartup => (a[0], a[1], EmptyKind::Friends),
                    Offering => (a[1], a[2], EmptyKind::Offering),
                    PresenceInit => {
                        if a[0] >= 4 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        (a[1], a[2], EmptyKind::Presence)
                    }
                    MuteStartup => (a[0], a[1], EmptyKind::Mute),
                    MuteGet => {
                        if a[0] >= 4 || a[1] > 250 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        writable(m, a[5], 4)?;
                        writable(m, a[4], a[1] as usize * 16)?;
                        (a[2], a[3], EmptyKind::Mute)
                    }
                    _ => unreachable!(),
                };
                work_event(event)?;
                let result = self.task(
                    m,
                    output,
                    Task::Empty {
                        kind,
                        pumped: false,
                    },
                )?;
                if op == MuteGet && result == OK {
                    m.set_u32(a[5], 0)?;
                }
                Ok(result)
            }
            PresenceAdd | PresenceSubmit => {
                if !matches!(
                    self.tasks.get(&a[0]),
                    Some(Task::Empty {
                        kind: EmptyKind::Presence,
                        ..
                    })
                ) {
                    return Ok(INVALID_ARGUMENT);
                }
                if op == PresenceAdd {
                    if a[2] > 1024 {
                        return Ok(INVALID_ARGUMENT);
                    }
                    m.check(a[3], a[2] as usize * 12, Access::Read, 1)?;
                }
                Ok(OK)
            }
            _ => unreachable!("registration routes only implemented operations here"),
        }
    }
}
fn writable(m: &Guest, p: u32, size: usize) -> std::result::Result<(), GuestFault> {
    m.check(p, size, Access::Write, 1)
}
fn work_event(event: u32) -> Result<()> {
    if event == 0 {
        Ok(())
    } else {
        Err(Error::Unverified("nonzero work event"))
    }
}
fn record(account: &Account) -> [u8; USER_SIZE] {
    let mut r = [0; USER_SIZE];
    r[..8].copy_from_slice(&account.xuid.to_le_bytes());
    r[12..12 + account.gamertag.len()].copy_from_slice(account.gamertag.as_bytes());
    r
}
