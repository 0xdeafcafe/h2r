use crate::{
    client::{self, Pending, Reply, Service, Session},
    config::Account,
    entries::Operation,
    messages::Messages,
    status::*,
};
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
    Logon {
        users: Box<[u8; USERS_SIZE]>,
        pending: Option<Pending>,
        session: Option<Session>,
        status: u32,
    },
    Change {
        users: Box<[u8; USERS_SIZE]>,
        committed: bool,
        pending: Option<Pending>,
        status: u32,
    },
    Presence {
        controller: u32,
        pending: Option<Pending>,
        status: u32,
    },
    Empty {
        kind: EmptyKind,
        pumped: bool,
    },
}

/// A single mapped allocation in the shared HLE window, using Guest's physical
/// allocator/claim machinery. Tokens advance without reuse, including across
/// cleanup, so a stale handle can never alias a new task. Exhaustion fails.
pub(crate) struct Storage {
    base: Option<u32>,
    cursor: u32,
    size: u32,
}
impl Default for Storage {
    fn default() -> Self {
        Self::with_size(STORAGE_SIZE)
    }
}
impl Storage {
    pub(crate) fn with_size(size: u32) -> Self {
        Self {
            base: None,
            cursor: 0,
            size,
        }
    }
    fn ensure(&mut self, m: &mut Guest) -> std::result::Result<u32, MapError> {
        if let Some(base) = self.base {
            return Ok(base);
        }
        let mut base = 0xa000_0000u32;
        loop {
            let end = base
                .checked_add(self.size)
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
            m.map_zeroed(base, self.size, Permissions::RW)?;
            // No overlapping claim or mapping remains after the search above.
            m.claim_virtual(base, self.size, "Live HLE")?;
            self.base = Some(base);
            self.cursor = FIRST_TOKEN;
            return Ok(base);
        }
    }
    pub(crate) fn alloc(&mut self, m: &mut Guest, bytes: u32) -> Option<u32> {
        let base = self.ensure(m).ok()?;
        let end = self.cursor.checked_add(bytes)?.checked_add(7)? & !7;
        if end > self.size {
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
    service: Option<Service>,
    messages: Messages,
    menu: crate::ui::Menu,
    user_ids: [u64; 4],
    state_flags: [u32; 4],
    game_id: Option<String>,
}
impl State {
    pub fn new(accounts: Vec<Account>, address: SocketAddrV4, service: Option<Service>) -> Self {
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
            service,
            messages: Messages::default(),
            menu: crate::ui::Menu::default(),
            user_ids: [0; 4],
            state_flags: [1; 4],
            game_id: None,
        }
    }
    fn task(&mut self, m: &mut Guest, out: u32, task: Task) -> Result<u32> {
        writable(m, out, 4)?;
        let Some(handle) = self.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        self.task_at(m, out, handle, task)
    }
    fn task_at(&mut self, m: &mut Guest, out: u32, handle: u32, task: Task) -> Result<u32> {
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
        if let (Some(service), Some(session)) = (&self.service, self.session()) {
            service.cancel(&session);
        }
        self.user_ids = [0; 4];
        self.state_flags = [1; 4];
        self.game_id = None;
        self.messages.clear();
        self.menu.clear();
        if let Some(base) = self.storage.base {
            m.write_bytes(base, &[0; USERS_SIZE], 1)?;
        }
        self.logon = None;
        self.services.clear();
        Ok(())
    }
    fn accounts_for(&self, users: &[u8; USERS_SIZE]) -> Vec<Account> {
        let ids: BTreeSet<_> = users
            .as_chunks::<USER_SIZE>()
            .0
            .iter()
            .map(|u| u64::from_le_bytes(u[..8].try_into().unwrap()))
            .filter(|u| *u != 0)
            .collect();
        self.accounts
            .iter()
            .filter(|a| ids.contains(&a.xuid))
            .cloned()
            .collect()
    }
    fn publish_users(&mut self, m: &mut Guest, users: &[u8; USERS_SIZE]) -> Result<()> {
        m.write_bytes(self.storage.base.unwrap(), users, 1)?;
        for (id, user) in self
            .user_ids
            .iter_mut()
            .zip(users.as_chunks::<USER_SIZE>().0.iter())
        {
            *id = u64::from_le_bytes(user[..8].try_into().unwrap());
        }
        Ok(())
    }
    fn connected(&self) -> bool {
        matches!(
            self.logon.and_then(|h| self.tasks.get(&h)),
            Some(Task::Logon {
                status: CONNECTED,
                session,
                ..
            }) if session.as_ref().is_none_or(|s| s.status() == CONNECTED)
        )
    }
    fn session(&self) -> Option<Session> {
        match self.logon.and_then(|h| self.tasks.get(&h)) {
            Some(Task::Logon { session, .. }) => session.clone(),
            _ => None,
        }
    }
    fn poll_logon(&mut self, handle: u32, m: &mut Guest) -> Result<u32> {
        let Some(Task::Logon {
            mut users,
            mut pending,
            session,
            mut status,
        }) = self.tasks.get(&handle).cloned()
        else {
            return Ok(INVALID_ARGUMENT);
        };
        if let Some(request) = &pending
            && let Some(reply) = request.take()
        {
            status = match reply {
                Ok(Reply::Logon { files, files_error }) => {
                    self.publish_users(m, &users)?;
                    self.menu
                        .messages(self.user_ids, files.iter().map(|f| f.id).collect());
                    self.messages.set_files(files, files_error);
                    CONNECTED
                }
                Err(e) => {
                    for user in users.as_chunks_mut::<USER_SIZE>().0.iter_mut() {
                        if user[..8].iter().any(|b| *b != 0) {
                            user[108..112].copy_from_slice(&e.to_le_bytes());
                        }
                    }
                    e
                }
                _ => INVALID_ARGUMENT,
            };
            pending = None;
        }
        if status == CONNECTED
            && let Some(session) = &session
            && session.status() & 0x80000000 != 0
        {
            status = session.status();
        }
        self.tasks.insert(
            handle,
            Task::Logon {
                users,
                pending,
                session,
                status,
            },
        );
        Ok(status)
    }
    fn desired_presence(&self, controller: u32) -> client::Presence {
        if self.state_flags[controller as usize] & 1 == 0 {
            client::Presence {
                state: "away",
                game_id: None,
            }
        } else if let Some(id) = &self.game_id {
            client::Presence {
                state: "playing",
                game_id: Some(id.clone()),
            }
        } else {
            client::Presence::default()
        }
    }
    fn presence(&self, controller: u32) -> std::result::Result<Option<Pending>, u32> {
        if controller >= 4 || self.user_ids[controller as usize] == 0 {
            return Err(INVALID_USER);
        }
        match (&self.service, self.session()) {
            (Some(service), Some(session)) => service
                .presence(
                    &session,
                    self.user_ids[controller as usize],
                    self.desired_presence(controller),
                )
                .map(Some),
            _ => Ok(None),
        }
    }
    pub fn set_game_id(&mut self, id: Option<&str>) -> std::result::Result<(), u32> {
        if id.is_some_and(|id| !live_proto::game::valid_id(id)) {
            return Err(INVALID_ARGUMENT);
        }
        self.game_id = id.map(str::to_owned);
        if self.connected() {
            for controller in 0..4 {
                if self.user_ids[controller as usize] != 0 {
                    self.presence(controller)?;
                }
            }
        }
        Ok(())
    }
    pub fn server_status(&self) -> client::ServerStatus {
        self.service
            .as_ref()
            .map(Service::status)
            .unwrap_or_default()
    }
    pub fn invoke(&mut self, op: Operation, a: &[u32], m: &mut Guest) -> Result<u32> {
        use Operation::*;
        if let Ui(op) = op {
            let connected = self.connected();
            return self.menu.invoke(
                op,
                a,
                m,
                &mut self.storage,
                self.online_refs != 0,
                connected,
            );
        }
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
                    self.clear_logon(m)?;
                    self.tasks.clear();
                }
                return Ok(OK);
            }
            WsaCleanup => {
                if self.wsa_refs == 0 {
                    return Ok(WSA_NOT_INITIALIZED);
                }
                self.wsa_refs -= 1;
                if self.wsa_refs == 0 {
                    self.clear_logon(m)?;
                    self.tasks.clear();
                }
                return Ok(OK);
            }
            TitleAddress => {
                writable(m, a[0], 36)?;
                let address = if self.net_refs != 0 {
                    crate::network::xnaddr(self.address)
                } else {
                    [0; 36]
                };
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
            AcceptedInvite => {
                // Public XONLINE_ACCEPTED_GAMEINVITE is 156 bytes. Retail
                // 0x003b49f7 zeroes it; the no-launch-invite path returns E_FAIL.
                // Game 0x0006d218 accepts every nonnegative HRESULT as an invite.
                m.write_bytes(a[0], &[0; 156], 1)?;
                Ok(NO_ACCEPTED_INVITE)
            }
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
                let Some(handle) = self.storage.alloc(m, 8) else {
                    return Ok(OUT_OF_MEMORY);
                };
                let session = self.service.as_ref().map(|_| Session::new(handle));
                let pending = if let (Some(service), Some(session)) = (&self.service, &session) {
                    match service.logon(session, self.accounts_for(&users)) {
                        Ok(p) => Some(p),
                        Err(e) => return Ok(e),
                    }
                } else {
                    None
                };
                let status = if pending.is_some() { 0 } else { CONNECTED };
                if status == CONNECTED {
                    self.publish_users(m, &users)?;
                }
                m.set_u32(handle, 0)?;
                m.set_u32(a[4], handle)?;
                self.tasks.insert(
                    handle,
                    Task::Logon {
                        users,
                        pending,
                        session,
                        status,
                    },
                );
                self.logon = Some(handle);
                self.services = services;
                Ok(OK)
            }
            Continue => {
                if let Some(status) = self.menu.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.messages.poll(a[0], m)? {
                    return Ok(status);
                }
                let Some(task) = self.tasks.get(&a[0]).cloned() else {
                    return Ok(INVALID_ARGUMENT);
                };
                let (result, next) = match task {
                    Task::Logon { .. } => return self.poll_logon(a[0], m),
                    Task::Change {
                        mut users,
                        committed,
                        mut pending,
                        mut status,
                    } => {
                        if status & 0x80000000 != 0 {
                            return Ok(status);
                        }
                        if let Some(request) = &pending {
                            let Some(reply) = request.take() else {
                                return Ok(0);
                            };
                            pending = None;
                            status = match reply {
                                Ok(Reply::Done) => 0,
                                Err(e) => {
                                    for user in users.as_chunks_mut::<USER_SIZE>().0.iter_mut() {
                                        if user[..8].iter().any(|b| *b != 0) {
                                            user[108..112].copy_from_slice(&e.to_le_bytes());
                                        }
                                    }
                                    USER_CHANGE_FAILED
                                }
                                _ => INVALID_ARGUMENT,
                            };
                        }
                        if status & 0x80000000 != 0 {
                            (
                                status,
                                Task::Change {
                                    users,
                                    committed,
                                    pending,
                                    status,
                                },
                            )
                        } else if committed {
                            (
                                USER_CHANGE_COMPLETE,
                                Task::Change {
                                    users,
                                    committed,
                                    pending,
                                    status,
                                },
                            )
                        } else {
                            self.publish_users(m, &users)?;
                            (
                                USER_CHANGE_COMMIT,
                                Task::Change {
                                    users,
                                    committed: true,
                                    pending,
                                    status,
                                },
                            )
                        }
                    }
                    Task::Presence {
                        controller,
                        mut pending,
                        mut status,
                    } => {
                        if let Some(request) = &pending
                            && let Some(reply) = request.take()
                        {
                            status = match reply {
                                Ok(Reply::Done) => TASK_SUCCESS,
                                Err(e) => e,
                                _ => INVALID_ARGUMENT,
                            };
                            pending = None;
                        }
                        (
                            status,
                            Task::Presence {
                                controller,
                                pending,
                                status,
                            },
                        )
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
            LogonResults => self.poll_logon(a[0], m),
            LogonUsers => Ok(if self.connected() {
                self.storage.base.unwrap_or(0)
            } else {
                0
            }),
            ServiceInfo => {
                if !self.connected() {
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
                if self.menu.close(a[0]) {
                    return Ok(OK);
                }
                if self.messages.close(a[0]) {
                    return Ok(OK);
                }
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
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                if self.tasks.values().any(|t| {
                    matches!(
                        t,
                        Task::Change {
                            committed: false,
                            status: 0,
                            ..
                        }
                    )
                }) {
                    return Ok(TASK_BUSY);
                }
                work_event(a[1])?;
                let Some(users) = self.users(m, a[0])? else {
                    return Ok(INVALID_USER);
                };
                writable(m, a[2], 4)?;
                let Some(handle) = self.storage.alloc(m, 8) else {
                    return Ok(OUT_OF_MEMORY);
                };
                let pending =
                    if let (Some(service), Some(session)) = (&self.service, self.session()) {
                        match service.change(&session, self.accounts_for(&users)) {
                            Ok(p) => Some(p),
                            Err(e) => return Ok(e),
                        }
                    } else {
                        None
                    };
                self.task_at(
                    m,
                    a[2],
                    handle,
                    Task::Change {
                        users,
                        committed: false,
                        pending,
                        status: 0,
                    },
                )
            }
            ChangeResults => {
                let Some(Task::Change {
                    users,
                    pending,
                    status,
                    ..
                }) = self.tasks.get(&a[0])
                else {
                    return Ok(INVALID_ARGUMENT);
                };
                if pending.is_some() {
                    return Ok(0);
                }
                let mut results = [0; 16];
                if *status == USER_CHANGE_FAILED {
                    for (result, user) in results
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(users.as_chunks::<USER_SIZE>().0.iter())
                    {
                        result.copy_from_slice(&user[108..112]);
                    }
                } else if *status & 0x80000000 != 0 {
                    return Ok(*status);
                }
                m.write_bytes(a[1], &results, 1)?;
                Ok(OK)
            }
            Notification => {
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                if a[0] >= 4 {
                    return Ok(INVALID_ARGUMENT);
                }
                if a[4] > 1024 {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[5], a[4] as usize, Access::Read, 1)?;
                if self.user_ids[a[0] as usize] == 0 {
                    return Ok(INVALID_USER);
                }
                self.state_flags[a[0] as usize] = a[1];
                Ok(match self.presence(a[0]) {
                    Ok(_) => OK,
                    Err(e) => e,
                })
            }
            FriendsStartup | Offering | PresenceInit | MuteStartup | MuteGet => {
                if !self.connected() {
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
                if op == PresenceInit && self.service.is_some() {
                    writable(m, output, 4)?;
                    let Some(handle) = self.storage.alloc(m, 8) else {
                        return Ok(OUT_OF_MEMORY);
                    };
                    let pending = match self.presence(a[0]) {
                        Ok(p) => p,
                        Err(e) => return Ok(e),
                    };
                    let result = self.task_at(
                        m,
                        output,
                        handle,
                        Task::Presence {
                            controller: a[0],
                            pending,
                            status: 0,
                        },
                    )?;
                    if result == 0 {
                        self.menu.presence_init(m.u32(output)?);
                    }
                    return Ok(result);
                }
                let result = self.task(
                    m,
                    output,
                    Task::Empty {
                        kind,
                        pumped: false,
                    },
                )?;
                if op == PresenceInit && result == 0 {
                    self.menu.presence_init(m.u32(output)?);
                }
                if op == MuteGet && result == OK {
                    m.set_u32(a[5], 0)?;
                }
                Ok(result)
            }
            PresenceAdd | PresenceSubmit => {
                let Some(task) = self.tasks.get(&a[0]).cloned() else {
                    return Ok(INVALID_ARGUMENT);
                };
                if !matches!(
                    task,
                    Task::Presence { .. }
                        | Task::Empty {
                            kind: EmptyKind::Presence,
                            ..
                        }
                ) {
                    return Ok(INVALID_ARGUMENT);
                }
                if op == PresenceAdd {
                    if a[2] > 1024 {
                        return Ok(INVALID_ARGUMENT);
                    }
                    return self.menu.presence_add(m, a[0], a[1], a[2], a[3]);
                } else if let Task::Presence { controller, .. } = task {
                    let pending = match self.presence(controller) {
                        Ok(p) => p,
                        Err(e) => return Ok(e),
                    };
                    self.tasks.insert(
                        a[0],
                        Task::Presence {
                            controller,
                            pending,
                            status: 0,
                        },
                    );
                }
                Ok(OK)
            }
            PresenceClear => Ok(self.menu.presence_clear(a[0])),
            PresenceLatest => self.menu.presence_latest(m, a),
            SaveLogonState => {
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                let mut state = [0; 516];
                state[0] = 0x4c;
                state[1] = 1;
                state[2..4].copy_from_slice(&516u16.to_le_bytes());
                m.read_bytes(self.storage.base.unwrap(), &mut state[4..452], 1)?;
                for (dst, id) in state[452..].as_chunks_mut::<4>().0.iter_mut().zip(
                    self.services
                        .iter()
                        .filter(|id| **id != 0 && **id != 1 && **id != 14),
                ) {
                    *dst = id.to_le_bytes();
                }
                m.write_bytes(a[0], &state, 1)?;
                Ok(0)
            }
            MessageEnumerate
            | MessageDetails
            | MessageProperty
            | MessageDownload
            | MessageDownloadResults
            | MessageDelete => {
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                if matches!(op, MessageEnumerate | MessageDetails | MessageDelete)
                    && (a[0] >= 4 || self.user_ids[a[0] as usize] == 0)
                {
                    return Ok(INVALID_USER);
                }
                let session = self.session();
                let result = self.messages.invoke(
                    op,
                    a,
                    m,
                    &mut self.storage,
                    self.service.as_ref(),
                    session.as_ref(),
                )?;
                if op == MessageDelete && result == 0 {
                    self.menu.remove_message(a[0], a[1]);
                }
                Ok(result)
            }
            _ => unreachable!("registration routes only implemented operations here"),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let (Some(service), Some(session)) = (&self.service, self.session()) {
            service.cancel(&session);
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
