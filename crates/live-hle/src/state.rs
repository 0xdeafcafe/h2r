use crate::{
    client::{self, Pending, Reply, Service, Session},
    config::Account,
    entries::Operation,
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
#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
    pub(crate) fn ensure(&mut self, m: &mut Guest) -> std::result::Result<u32, MapError> {
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
    pub(crate) fn used_range(
        &self,
        claims: &[(u32, u32)],
        size: u32,
    ) -> std::result::Result<Option<(u32, u32)>, String> {
        if self.size != size {
            return Err("unexpected Live arena size".into());
        }
        match self.base {
            None if self.cursor == 0 => Ok(None),
            Some(base)
                if claims.contains(&(base, size))
                    && (FIRST_TOKEN..=size).contains(&self.cursor)
                    && self.cursor.is_multiple_of(8) =>
            {
                Ok(Some((
                    base.checked_add(FIRST_TOKEN).ok_or("invalid arena base")?,
                    base.checked_add(self.cursor).ok_or("invalid arena end")?,
                )))
            }
            _ => Err("invalid Live arena cursor".into()),
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
    menu: crate::ui::Menu,
    feedback: crate::feedback::Feedback,
    mutes: crate::mutes::Mutes,
    invites: crate::invites::Invites,
    files: crate::storage_api::Files,
    stats: crate::stats::Stats,
    pub(crate) arbitration: crate::arbitration::Arbiter,
    outbox: crate::outbox::Outbox,
    sessions: crate::sessions::Sessions,
    teams: crate::teams::Teams,
    user_ids: [u64; 4],
    user_flags: [u32; 4],
    state_flags: [u32; 4],
    game_id: Option<String>,
    /// Where accounts come from and which server they are for: a sign-in
    /// that needs a code saves there and reads the accounts again.
    config: crate::config::LiveConfig,
    /// The sign-in waiting for a code, if any ([`crate::signin`]).
    signin: Option<crate::signin::Signin>,
    activity: Option<String>,
    /// Co-op test automation (`H2_COOP_AUTO_*`, [`crate::auto`]).
    auto: crate::auto::Auto,
    /// The guest's system time for the call in progress, when it is the
    /// accepted-invitation read ([`State::set_guest_now`]).
    guest_now: Option<u64>,
    /// The network's registered keys for the stat read in progress
    /// ([`State::set_read_keys`]): they name the session joined.
    read_keys: Option<Vec<[u8; 8]>>,
    /// The bot accounts the server leased to this console for this logon
    /// (docs/game/bots.md §9.5): every bot of each roster it answered
    /// [`State::roster_bots`] with, by xuid, with its gamertag. A local
    /// Clanker signed in as one of these passes a user change without a
    /// key; no other xuid outside the accounts file does. Cleared when the
    /// logon ends.
    leased: Leased,
    /// The clanker hoppers the server served last (docs/game/bots.md §8),
    /// for the Clankermatch row's playlists, the search and the lobby fill.
    clankers: crate::clankermatch::Marks,
}

/// The bot accounts leased to this console ([`State::roster_bots`]).
pub(crate) type Leased = std::sync::Arc<std::sync::Mutex<BTreeMap<u64, String>>>;

impl State {
    pub fn new(
        accounts: Vec<Account>,
        address: SocketAddrV4,
        service: Option<Service>,
        config: crate::config::LiveConfig,
    ) -> Self {
        Self {
            config,
            signin: None,
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
            menu: crate::ui::Menu::default(),
            feedback: Default::default(),
            mutes: Default::default(),
            invites: Default::default(),
            files: crate::storage_api::Files::default(),
            stats: crate::stats::Stats::default(),
            arbitration: crate::arbitration::Arbiter::default(),
            outbox: crate::outbox::Outbox::default(),
            sessions: crate::sessions::Sessions::default(),
            teams: crate::teams::Teams::default(),
            user_ids: [0; 4],
            user_flags: [0; 4],
            state_flags: [1; 4],
            game_id: None,
            activity: None,
            auto: crate::auto::Auto::from_env(),
            guest_now: None,
            read_keys: None,
            leased: Leased::default(),
            clankers: Default::default(),
        }
    }
    pub fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
    }
    pub(crate) fn signed_in_account(&self, controller: u32) -> Option<(live_proto::Xuid, String)> {
        let xuid = *self.user_ids.get(controller as usize)?;
        let account = self
            .accounts
            .iter()
            .find(|account| account.xuid == xuid && account.key.is_some())?;
        Some((live_proto::Xuid(xuid), account.gamertag.clone()))
    }

    /// The sign-in prompt now, if a sign-in is waiting for one.
    pub fn signin(&mut self) -> Option<crate::signin::Prompt> {
        self.signin.as_mut().map(|s| s.prompt())
    }
    /// The controller whose prompt should open now (once per sign-in).
    pub fn take_signin_open(&mut self) -> Option<u32> {
        self.signin.as_mut()?.take_open()
    }
    pub fn signin_submit(&mut self, code: &str) -> bool {
        self.signin.as_mut().is_some_and(|s| s.submit(code))
    }
    pub fn signin_create(&mut self) -> bool {
        self.signin.as_mut().is_some_and(|s| s.create())
    }
    pub fn signin_typed(&mut self, text: Option<&str>) -> bool {
        self.signin.as_mut().is_some_and(|s| s.typed(text))
    }
    pub fn signin_cancel(&mut self) {
        let Some(s) = self.signin.as_mut() else {
            return;
        };
        s.cancel();
        // A gamertag found taken was saved as waiting for its code: list it
        // now, so a later sign-in can pick it.
        match self.config.accounts() {
            Ok(accounts) => self.accounts = accounts,
            Err(e) => eprintln!("live: reading the accounts after sign-in: {e}"),
        }
    }
    /// The prompt closed: forget a sign-in that has nothing more to ask
    /// and that the logon has taken in (or that was cancelled).
    pub fn signin_dismiss(&mut self) {
        let done = self.signin.as_mut().is_some_and(|s| {
            let p = s.prompt();
            p.phase.finished() && (!s.has_verified() || !self.tasks.contains_key(&s.handle))
        });
        if done {
            self.signin = None;
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
            if let Some(account) = self.accounts.iter().find(|a| a.xuid == id) {
                dst.copy_from_slice(&record(account));
            } else if let Some(tag) = self.leased_bot(id) {
                dst.copy_from_slice(&bot_record(id, &tag));
            } else {
                return Ok(None);
            }
            dst[8..12].copy_from_slice(&src[8..12]); // Includes the guest slot number.
        }
        Ok(Some(users))
    }
    /// The gamertag of the bot account `xuid`, when the server leased it
    /// to this console in a roster this logon (docs/game/bots.md §9.5).
    fn leased_bot(&self, xuid: u64) -> Option<String> {
        if !live_proto::bots::is_server_xuid(live_proto::Xuid(xuid)) {
            return None;
        }
        self.leased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&xuid)
            .cloned()
    }
    fn clear_logon(&mut self, m: &mut Guest) -> Result<()> {
        if let (Some(service), Some(session)) = (&self.service, self.session()) {
            service.cancel(&session);
        }
        self.leased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.user_ids = [0; 4];
        self.user_flags = [0; 4];
        self.state_flags = [1; 4];
        self.game_id = None;
        self.menu.clear();
        self.feedback.clear();
        self.mutes.clear();
        self.invites.clear();
        self.files.clear();
        self.stats.clear_tasks();
        self.arbitration.clear_tasks();
        self.outbox.clear_session();
        self.sessions.clear();
        self.teams.clear_tasks();
        if let Some(base) = self.storage.base {
            m.write_bytes(base, &[0; USERS_SIZE], 1)?;
        }
        self.logon = None;
        self.services.clear();
        self.signin = None;
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
        for (flag, user) in self
            .user_flags
            .iter_mut()
            .zip(users.as_chunks::<USER_SIZE>().0.iter())
        {
            *flag = u32::from_le_bytes(user[8..12].try_into().unwrap());
        }
        self.invites.users_changed(&self.user_ids);
        Ok(())
    }
    fn machine(&self) -> bool {
        self.service.as_ref().is_some_and(|service| service.machine)
    }

    // Retail need not visit the controller sign-in screen on a playerless
    // host. Queue machine logon before the first retail advertisement.
    fn machine_logon(&mut self, m: &mut Guest) -> Result<u32> {
        if self.logon.is_some() {
            return Ok(OK);
        }
        let Some(handle) = self.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        let session = Session::new(handle);
        let service = self.service.as_ref().expect("machine service");
        let pending = match service.logon(&session, Vec::new()) {
            Ok(pending) => pending,
            Err(error) => return Ok(error),
        };
        m.set_u32(handle, 0)?;
        self.tasks.insert(
            handle,
            Task::Logon {
                users: Box::new([0; USERS_SIZE]),
                pending: Some(pending),
                session: Some(session),
                status: 0,
            },
        );
        self.logon = Some(handle);
        Ok(OK)
    }

    fn machine_connected(&self) -> bool {
        self.machine() && self.session().is_some_and(|s| s.status() == CONNECTED)
    }

    // Player services require a player logon. The machine logon only drives
    // the retail session writer and its own logon task.
    fn connected(&self) -> bool {
        !self.machine() && matches!(
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
        if let Some(waited) = self.signin_step(handle, &mut users, &mut pending, &session) {
            status = waited;
        }

        if let Some(request) = &pending
            && let Some(reply) = request.take()
        {
            status = match reply {
                Ok(Reply::Logon) => {
                    self.publish_users(m, &users)?;
                    // The playlist screens filter by them (docs/game/bots.md
                    // §8.3): ask now, so the first list already has them.
                    if !self.machine()
                        && let Err(e) = self.refresh_clanker_hoppers()
                    {
                        eprintln!("live: clankermatch: clanker hoppers not asked for: {e}");
                    }
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
    /// A logon waiting on an in-game code ([`crate::signin`]): `Some`
    /// status while it waits (`0`) or when it ends here; `None` once the
    /// verified account is in and the service logon runs as usual.
    fn signin_step(
        &mut self,
        handle: u32,
        users: &mut [u8; USERS_SIZE],
        pending: &mut Option<Pending>,
        session: &Option<Session>,
    ) -> Option<u32> {
        let s = self.signin.as_mut().filter(|s| s.handle == handle)?;
        let slot = s.slot;
        let fail = |users: &mut [u8; USERS_SIZE]| {
            users[slot * USER_SIZE + 108..(slot + 1) * USER_SIZE]
                .copy_from_slice(&INVALID_USER.to_le_bytes());
            Some(INVALID_USER)
        };
        if s.cancelled() {
            return fail(users);
        }
        let Some(entry) = s.take_verified() else {
            return Some(0);
        };
        // The logon has it now; the prompt may still show it.
        s.handle = 0;

        match self.config.accounts() {
            Ok(accounts) => self.accounts = accounts,
            Err(e) => {
                eprintln!("live: reading the accounts after sign-in: {e}");
                return fail(users);
            }
        }
        let Some(fresh) = self
            .accounts
            .iter()
            .find(|a| a.xuid == entry.xuid.0 && a.key.is_some() && !a.needs_code)
            .map(record)
        else {
            eprintln!(
                "live: {} was saved but is not in the accounts file",
                entry.gamertag
            );
            return fail(users);
        };
        let user = &mut users[slot * USER_SIZE..(slot + 1) * USER_SIZE];
        let flags: [u8; 4] = user[8..12].try_into().unwrap();
        user.copy_from_slice(&fresh);
        user[8..12].copy_from_slice(&flags);
        let accounts = self.accounts_for(users);
        match (&self.service, session) {
            (Some(service), Some(session)) => match service.logon(session, accounts) {
                Ok(p) => {
                    *pending = Some(p);
                    None
                }
                Err(e) => Some(e),
            },
            _ => fail(users),
        }
    }
    fn desired_presence(&self, controller: u32) -> client::Presence {
        if self.state_flags[controller as usize] & 1 == 0 {
            client::Presence {
                state: "away",
                game_id: None,
                activity: None,
            }
        } else if let Some(id) = &self.game_id {
            client::Presence {
                state: "playing",
                game_id: Some(id.clone()),
                activity: self.activity.clone(),
            }
        } else {
            client::Presence {
                activity: self.activity.clone(),
                ..client::Presence::default()
            }
        }
    }
    /// Controller `c`'s user when it is one of the accounts file's (a
    /// human or its guest), else 0: a leased bot account has no key, so
    /// the server is never asked anything on its behalf.
    fn service_user(&self, c: usize) -> u64 {
        let id = self.user_ids[c];
        if self.accounts.iter().any(|a| a.xuid == id) {
            id
        } else {
            0
        }
    }
    fn presence(&self, controller: u32) -> std::result::Result<Option<Pending>, u32> {
        if controller >= 4 || self.user_ids[controller as usize] == 0 {
            return Err(INVALID_USER);
        }
        if self.service_user(controller as usize) == 0 {
            // A leased bot: no presence of its own (docs/game/bots.md §9.5).
            return Ok(None);
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
    /// What presence says the players are doing, in words ("Co-op:
    /// Outskirts"), sent to the server for every signed-in player when it
    /// changes. None clears it. A bad text is refused.
    pub fn set_activity(&mut self, activity: Option<&str>) -> std::result::Result<(), u32> {
        if activity.is_some_and(|a| !live_proto::valid_activity(a)) {
            return Err(INVALID_ARGUMENT);
        }
        if self.activity.as_deref() == activity {
            return Ok(());
        }
        self.activity = activity.map(str::to_owned);
        if self.connected() {
            for controller in 0..4 {
                if self.user_ids[controller as usize] != 0 {
                    self.presence(controller)?;
                }
            }
        }
        Ok(())
    }
    pub fn activity(&self) -> Option<&str> {
        self.activity.as_deref()
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
    pub fn stat_writes(&self) -> Vec<crate::stats::StatWrite> {
        self.stats.snapshot()
    }
    pub fn drain_stat_writes(&mut self) -> Vec<crate::stats::StatWrite> {
        self.stats.drain()
    }
    pub fn message_sends(&self) -> Vec<crate::outbox::MessageSend> {
        self.outbox.snapshot()
    }
    pub fn drain_message_sends(&mut self) -> Vec<crate::outbox::MessageSend> {
        self.outbox.drain()
    }
    pub fn local_sessions(&self) -> Vec<crate::sessions::LocalSession> {
        self.sessions.snapshot()
    }
    pub fn set_session_visibility(&mut self, v: crate::sessions::Visibility) {
        let session = self.session();
        self.sessions
            .set_visibility(v, self.service.as_ref(), session.as_ref());
    }
    pub fn set_session_skulls(&mut self, skulls: Option<Vec<u8>>) {
        let session = self.session();
        self.sessions
            .set_skulls(skulls, self.service.as_ref(), session.as_ref());
    }
    pub fn set_session_variant_ext(&mut self, ext: Option<live_proto::session::VariantExt>) {
        let session = self.session();
        self.sessions
            .set_variant_ext(ext, self.service.as_ref(), session.as_ref());
    }
    /// The bots in this console's hosted game, rostered for `session`
    /// (hex): that session is told of them, and every hosted session of
    /// its signed-in users, as players.
    pub fn set_session_players(&mut self, session: Option<&str>, bots: Vec<live_proto::Xuid>) {
        let id = session
            .and_then(|s| u64::from_str_radix(s, 16).ok())
            .map(u64::to_be_bytes);
        let connection = self.session();
        self.sessions
            .set_players(id, &bots, self.service.as_ref(), connection.as_ref());
    }
    /// Arms (Some) or leaves (None) a co-op campaign for this console's
    /// lobby advertisements, and says so in presence ("Co-op: <level>").
    pub fn set_coop(
        &mut self,
        campaign: Option<live_proto::session::Campaign>,
        skulls: Option<Vec<u8>>,
    ) {
        let session = self.session();
        let activity = campaign.as_ref().map(|c| coop_activity(c.label()));
        self.sessions
            .set_coop(campaign, skulls, self.service.as_ref(), session.as_ref());
        let _ = self.set_activity(activity.as_deref());
    }
    pub fn set_coop_ping(&mut self, ms: Option<u16>) {
        let session = self.session();
        self.sessions
            .set_coop_ping(ms, self.service.as_ref(), session.as_ref());
    }
    pub fn coop(&self) -> Option<live_proto::session::Campaign> {
        self.sessions.coop().cloned()
    }
    /// Controller `controller`'s friends, as last read; asks for a fresh
    /// read (answered on a later call).
    pub fn friend_list(&mut self, controller: usize) -> Vec<crate::Friend> {
        if controller >= 4 {
            return Vec::new();
        }
        let session = self.session();
        self.invites.refresh(
            controller,
            self.user_ids[controller],
            self.service.as_ref(),
            session.as_ref(),
        );
        self.invites.friend_list(controller)
    }
    /// Co-op test automation ([`crate::auto`]), for the first signed-in
    /// player: keeps the friends view fresh, invites the named friends to
    /// this console's advertised lobby, and accepts the named host's
    /// invitation. Nothing when it is off or not signed in.
    pub fn set_auto(&mut self, auto: crate::auto::Auto) {
        self.auto = auto;
    }
    /// The guest's KeQuerySystemTime for the next accepted-invitation read
    /// (read before any lock: the kernel's clock comes first).
    pub fn set_guest_now(&mut self, now: Option<u64>) {
        self.guest_now = now;
    }
    /// The registered keys for the next stat read (read before this lock,
    /// as [`crate::Hle::matchmade`] does), so it can name the session this
    /// console is in.
    pub fn set_read_keys(&mut self, keys: Option<Vec<[u8; 8]>>) {
        self.read_keys = keys;
    }
    /// The Live session (hex) this console advertises, else the one it
    /// joined.
    fn game_session(&self, keys: &[[u8; 8]]) -> Option<String> {
        self.sessions
            .current_id()
            .map(|id| live_proto::sign::hex(&id))
            .or_else(|| self.joined_session(keys))
    }
    /// The title's read of the accepted invitation, said in the log when
    /// one was there.
    fn accepted_invite(&mut self, m: &mut Guest, out: u32) -> std::result::Result<u32, Error> {
        let result = self.invites.accepted(m, out, self.guest_now.take());
        if let Some(v) = self.invites.take_read() {
            if self.auto.is_on() {
                self.auto
                    .taken(&v, time::OffsetDateTime::now_utc().unix_timestamp());
                eprintln!(
                    "coop-auto: joining {}'s session {} (the game took the invitation)",
                    v.gamertag, v.session_id
                );
            } else {
                eprintln!(
                    "live: join: the game took {}'s invitation to session {}",
                    v.gamertag, v.session_id
                );
            }
        }
        result
    }
    fn auto_step(&mut self) {
        if !self.auto.is_on() || self.online_refs == 0 || !self.connected() {
            return;
        }
        let Some(c) = self.user_ids.iter().position(|&u| u != 0) else {
            return;
        };
        let user = self.user_ids[c];
        let session = self.session();
        self.invites
            .refresh(c, user, self.service.as_ref(), session.as_ref());
        if let Some(v) = self.auto.accept_now(
            &self.invites.pending(c),
            time::OffsetDateTime::now_utc().unix_timestamp(),
        ) {
            match self
                .invites
                .accept(c, user, &v, self.service.as_ref(), session.as_ref())
            {
                Ok(()) => eprintln!(
                    "coop-auto: accepting {}'s invitation to session {}",
                    v.gamertag, v.session_id
                ),
                Err(e) => {
                    self.auto.accept_failed(v.id);
                    eprintln!(
                        "coop-auto: could not accept {}'s invitation ({e:#010x})",
                        v.gamertag
                    );
                }
            }
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        for (v, queued) in self.auto.unread_too_long(&self.invites.unread(), now) {
            if queued {
                eprintln!(
                    "coop-auto: {}'s invitation to session {} is accepted but the game hasn't \
                     taken it after {}s (its reader runs from one menu screen; \
                     docs/live/live-coop.md 12.1)",
                    v.gamertag,
                    v.session_id,
                    crate::auto::UNREAD_AFTER
                );
            } else {
                eprintln!(
                    "coop-auto: {}'s invitation to session {}: the server never confirmed the \
                     acceptance within {}s",
                    v.gamertag,
                    v.session_id,
                    crate::auto::UNREAD_AFTER
                );
            }
        }
        let looked_up = self
            .auto
            .taken_session()
            .and_then(|id| u64::from_str_radix(id, 16).ok())
            .is_some_and(|id| self.sessions.looked_up(&id.to_be_bytes()));
        if let Some((v, n)) = self.auto.offer_again(now, looked_up)
            && self.invites.offer_again(v.id)
        {
            eprintln!(
                "coop-auto: the game took {}'s invitation to session {} but never looked the \
                 session up; offering it again ({n} of {})",
                v.gamertag,
                v.session_id,
                crate::auto::OFFERS
            );
        }
        let Some(own) = self.sessions.lobby_id() else {
            return;
        };
        let id = live_proto::sign::hex(&own);
        for f in self.auto.invite_now(&id, &self.invites.friend_list(c)) {
            match self.invites.send(
                c,
                user,
                f.xuid,
                id.clone(),
                self.service.as_ref(),
                session.as_ref(),
            ) {
                Ok(()) => eprintln!("coop-auto: invited {} to session {id}", f.gamertag),
                Err(e) => {
                    self.auto.invite_failed(&id, f.xuid);
                    eprintln!("coop-auto: could not invite {} ({e:#010x})", f.gamertag);
                }
            }
        }
    }
    /// Invites `to` to this console's advertised lobby, from controller
    /// `controller`'s player.
    pub fn invite_to_lobby(
        &mut self,
        controller: usize,
        to: live_proto::Xuid,
    ) -> std::result::Result<String, u32> {
        if controller >= 4 || self.user_ids[controller] == 0 {
            return Err(INVALID_USER);
        }
        let Some(id) = self.sessions.lobby_id() else {
            return Err(MATCH_INVALID_SESSION);
        };
        let session_id = live_proto::sign::hex(&id);
        let session = self.session();
        self.invites.send(
            controller,
            self.user_ids[controller],
            to,
            session_id.clone(),
            self.service.as_ref(),
            session.as_ref(),
        )?;
        Ok(session_id)
    }
    /// The signed-in player of controller `controller` (0 for none).
    pub fn user(&self, controller: usize) -> u64 {
        self.user_ids.get(controller).copied().unwrap_or(0)
    }
    /// The session this console joined: a registered key (`keys`, from
    /// `XNetRegisterKey`) that is not one of its own advertisements.
    pub fn joined_session(&self, keys: &[[u8; 8]]) -> Option<String> {
        if let Some((_, id)) = self.sessions.dedicated_match(keys) {
            return Some(id);
        }
        let own = self.sessions.own_ids();
        keys.iter()
            .find(|k| !own.contains(k))
            .map(|k| live_proto::sign::hex(k))
    }
    /// The playlist and session (hex) of the game about to start: the
    /// registered dedicated match, this console's advertisement, or the
    /// joined session as last read, in that order.
    /// No request is made.
    pub fn matchmade(&self, keys: &[[u8; 8]]) -> Option<(u32, String)> {
        if let Some(dedicated) = self.sessions.dedicated_match(keys) {
            return Some(dedicated);
        }
        if let (Some(p), Some(id)) = (self.sessions.current_playlist(), self.sessions.current_id())
        {
            return Some((p, live_proto::sign::hex(&id)));
        }
        let id = self.joined_session(keys)?;
        let seen = self
            .service
            .as_ref()?
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let p = live_proto::optimatch::playlist(&seen.get(&id)?.advertisement.attributes)?;
        Some((p, id))
    }
    pub fn matchmaking_status(&self) -> Option<&str> {
        self.sessions.matchmaking_status()
    }
    pub fn frames_from_host(&self, session_id: &str) -> bool {
        self.sessions.frames_from_host(session_id)
    }
    pub fn end_match_join(&mut self, id: [u8; 8]) -> bool {
        self.sessions.end_match_join(id)
    }
    pub fn has_match_tunnel(&self) -> bool {
        self.sessions.has_match_tunnel()
    }
    /// The Live session this console hosts (16 lowercase hex digits):
    /// the advertisement it made last, while it still stands.
    pub fn hosted_session(&self) -> Option<String> {
        self.sessions
            .current_id()
            .map(|id| live_proto::sign::hex(&id))
    }
    /// The clanker hoppers the server served last (none before the first
    /// answer of [`State::refresh_clanker_hoppers`]).
    pub fn clanker_hoppers(&self) -> crate::clankermatch::ClankerHoppers {
        self.clankers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// The hosted session (hex) and its hopper's mark, while this console
    /// hosts a lobby of a clanker hopper (docs/game/bots.md §8.4).
    pub fn clanker_hosted(&self) -> Option<(String, crate::clankermatch::ClankerHopper)> {
        let id = self.sessions.current_id()?;
        let hopper = self.sessions.current_playlist()?;
        let mark = *self
            .clankers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(hopper)?;
        Some((live_proto::sign::hex(&id), mark))
    }
    /// Asks the server for its clanker hoppers (`GET /v1/hoppers/clankers`);
    /// the answer replaces the ones kept, on the Live worker. Never waits.
    /// A refusal keeps the old ones and is said.
    pub fn refresh_clanker_hoppers(&self) -> std::result::Result<(), String> {
        let service = self.service.as_ref().ok_or("Live is not connected")?;
        let session = self.session().ok_or("not signed in to Live")?;
        let marks = self.clankers.clone();
        service
            .clanker_hoppers(
                &session,
                Box::new(move |r| match r {
                    Ok(Reply::ClankerHoppers(h)) => {
                        let mut m = marks.lock().unwrap_or_else(|e| e.into_inner());
                        if *m != h {
                            let ids: Vec<u32> = h.hoppers.iter().map(|h| h.hopper_id).collect();
                            eprintln!("live: clankermatch: clanker hoppers {ids:?}");
                        }
                        *m = h;
                    }
                    Ok(_) => eprintln!("live: clankermatch: the server sent no clanker hoppers"),
                    Err(code) => eprintln!(
                        "live: clankermatch: no clanker hoppers from the server ({code:#010x})"
                    ),
                }),
            )
            .map_err(|code| format!("Live is busy ({code:#010x})"))
    }
    /// Asks the server for a bot roster (`POST /games/bots`,
    /// docs/game/bots.md §9.1); `done` gets the checked roster, or why
    /// there is none, on the Live worker. Never waits.
    pub fn roster_bots(
        &self,
        request: live_proto::bots::RosterRequest,
        done: Box<dyn FnOnce(std::result::Result<live_proto::bots::Roster, String>) + Send>,
    ) -> std::result::Result<(), String> {
        let service = self.service.as_ref().ok_or("Live is not connected")?;
        let session = self.session().ok_or("not signed in to Live")?;
        let leased = self.leased.clone();
        service
            .bots(
                &session,
                request,
                Box::new(move |r| {
                    done(match r {
                        Ok(Reply::Roster(roster)) => roster
                            .check()
                            .map(|()| {
                                // The roster's bots are leased to this
                                // console: its Clankers may sign in as them.
                                let mut l = leased.lock().unwrap_or_else(|e| e.into_inner());
                                for b in &roster.bots {
                                    l.insert(b.xuid.0, b.gamertag.as_str().to_owned());
                                }
                                drop(l);
                                roster
                            })
                            .map_err(|e| format!("the server's roster: {e}")),
                        Ok(_) => Err("not a roster".into()),
                        Err(code) => Err(format!("the server refused: {code:#010x}")),
                    })
                }),
            )
            .map_err(|code| format!("Live is busy ({code:#010x})"))
    }
    pub fn bot_card(
        &self,
        xuid: live_proto::Xuid,
        done: Box<dyn FnOnce(live_proto::bots::PlayerCard) + Send>,
    ) {
        if let (Some(service), Some(session)) = (&self.service, self.session()) {
            let _ = service.bot_card(
                &session,
                xuid,
                Box::new(move |r| {
                    if let Ok(Reply::BotCard(card)) = r
                        && card.xuid == xuid
                    {
                        done(card);
                    }
                }),
            );
        }
    }
    fn mod_list_session(&self) -> std::result::Result<(&Service, Session), crate::mods::ModsError> {
        use crate::mods::ModsError;
        let service = self.service.as_ref().ok_or(ModsError::Offline)?;
        if !self.connected() || !(0..4).any(|c| self.service_user(c) != 0) {
            return Err(ModsError::NotSignedIn);
        }
        let session = self.session().ok_or(ModsError::NotSignedIn)?;
        Ok((service, session))
    }
    /// Whether a player can ask for the mod repository, without making a request.
    pub fn check_mod_list(&self) -> std::result::Result<(), crate::mods::ModsError> {
        self.mod_list_session().map(|_| ())
    }
    /// A page of the mod repository ([`crate::Hle::mod_list`]).
    pub fn mod_list(
        &self,
        limit: u32,
        cursor: Option<String>,
        done: crate::mods::ListDone,
    ) -> std::result::Result<(), crate::mods::ModsError> {
        use crate::mods::ModsError;
        let (service, session) = self.mod_list_session()?;
        service
            .mods(
                &session,
                limit,
                cursor,
                Box::new(move |r| {
                    done(match r {
                        Ok(Reply::Mods(page)) => Ok(page),
                        Ok(_) => Err(ModsError::Server("not a list of maps".into())),
                        Err(code) => Err(crate::mods::server_error(code)),
                    })
                }),
            )
            .map_err(|_| ModsError::Busy)
    }
    /// Installs map `id` from the mod repository ([`crate::Hle::install_mod`]).
    pub fn install_mod(
        &self,
        id: content::MapId,
        progress: crate::mods::Progress,
        done: crate::mods::InstallDone,
    ) -> std::result::Result<(), crate::mods::ModsError> {
        use crate::mods::ModsError;
        let service = self.service.as_ref().ok_or(ModsError::Offline)?;
        let session = self.session().ok_or(ModsError::NotSignedIn)?;
        service
            .content_api(
                &session,
                Box::new(move |r| match r {
                    Ok(Reply::Content {
                        api,
                        gate: Some(gate),
                        reader,
                    }) => crate::mods::spawn_install(api, gate, reader, id, progress, done),
                    Ok(Reply::Content { gate: None, .. }) => done(Err(ModsError::Offline)),
                    Ok(_) => done(Err(ModsError::Server("no content API".into()))),
                    Err(code) => done(Err(crate::mods::server_error(code))),
                }),
            )
            .map_err(|_| ModsError::Busy)
    }
    /// Asks the server for session `id` again (its answer lands in the
    /// worker's [`crate::client::Seen`]); the request, when one could be
    /// sent, and where the answer goes.
    pub fn refresh_session(
        &self,
        id: &str,
    ) -> (Option<crate::client::Pending>, Option<crate::client::Seen>) {
        let Some(service) = self.service.as_ref() else {
            return (None, None);
        };
        let pending = self.session().and_then(|c| {
            service
                .match_request(&c, crate::client::MatchRequest::Find(id.to_owned()))
                .ok()
        });
        (pending, Some(service.seen.clone()))
    }
    /// Asks the server for everyone's presence; the request, when one
    /// could be sent (signed in to a server).
    pub fn read_presence(&self) -> Option<crate::client::Pending> {
        let service = self.service.as_ref()?;
        let session = self.session()?;
        service.read_presence(&session).ok()
    }
    pub fn set_visibility_probe(&mut self, probe: Option<crate::sessions::VisibilityProbe>) {
        self.sessions.set_probe(probe);
    }
    pub fn set_map_probe(&mut self, probe: Option<crate::sessions::MapProbe>) {
        self.sessions.set_map_probe(probe);
    }
    pub fn lobby_map_changed(&mut self, guest: &Guest) {
        let connection = self.session();
        self.sessions
            .lobby_map_changed(guest, self.service.as_ref(), connection.as_ref());
    }
    pub fn set_skulls_probe(&mut self, probe: Option<crate::sessions::SkullsProbe>) {
        self.sessions.set_skulls_probe(probe);
    }
    pub fn session_visibility(&self) -> crate::sessions::Visibility {
        self.sessions.visibility()
    }
    pub fn local_teams(&self) -> live_proto::team::Store {
        self.teams.store.clone()
    }
    pub fn local_team_action(
        &mut self,
        actor: u64,
        action: &live_proto::team::Action,
    ) -> std::result::Result<(), live_proto::team::TeamError> {
        if self.service.is_some() {
            return Err(live_proto::team::TeamError::Invalid);
        }
        let account = self
            .accounts
            .iter()
            .find(|a| a.xuid == actor)
            .ok_or(live_proto::team::TeamError::UserNotFound)?;
        let actor = live_proto::team::Identity {
            xuid: live_proto::Xuid(actor),
            gamertag: account.gamertag.clone(),
        };
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        self.teams.store.apply(&actor, action, &now)
    }
    pub fn invoke(&mut self, op: Operation, a: &[u32], m: &mut Guest, hle: &crate::Hle) -> Result<u32> {
        use Operation::*;
        if let Teams(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if op.starts() && !self.connected() {
                return Ok(NO_SESSION);
            }
            let known: Vec<_> = self
                .accounts
                .iter()
                .map(|a| live_proto::team::Identity {
                    xuid: live_proto::Xuid(a.xuid),
                    gamertag: a.gamertag.clone(),
                })
                .collect();
            let actor = if op.starts() {
                if a[0] >= 4 {
                    return Ok(INVALID_ARGUMENT);
                }
                known
                    .iter()
                    .find(|v| v.xuid.0 == self.user_ids[a[0] as usize])
                    .cloned()
            } else {
                None
            };
            let session = self.session();
            return self.teams.invoke(
                op,
                a,
                m,
                crate::teams::Context {
                    storage: &mut self.storage,
                    outbox: &mut self.outbox,
                    actor,
                    known: &known,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                },
            );
        }
        if let Sessions(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if self.machine() {
                if matches!(op, crate::sessions::Op::Find | crate::sessions::Op::Search) {
                    return Ok(NO_USER);
                }
                if op.starts_task() && self.logon.is_none() {
                    return Ok(NO_SESSION);
                }
            } else if op.starts_task() && !self.connected() {
                return Ok(NO_SESSION);
            }
            let session = self.session();
            let clankers = self.clanker_hoppers();
            return self.sessions.invoke(
                op,
                a,
                m,
                crate::sessions::Context {
                    hle,
                    storage: &mut self.storage,
                    host: crate::network::xnaddr(self.address),
                    users: &self.user_ids,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                    clankers: &clankers,
                },
            );
        }
        self.invites.pump(&self.user_ids, &self.user_flags);
        self.auto_step();
        if let Invites(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if !self.connected() {
                return Ok(NO_SESSION);
            }
            let session = self.session();
            return self.invites.invoke(
                op,
                m,
                a,
                crate::invites::Context {
                    storage: &mut self.storage,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                    users: &self.user_ids,
                },
            );
        }
        if let Outbox(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            let mut sender = 0;
            if op.sends() {
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                if a[0] >= 4 {
                    return Ok(INVALID_ARGUMENT);
                }
                sender = self.user_ids[a[0] as usize];
                if sender == 0 {
                    return Ok(NO_USER);
                }
            }
            let result = self.outbox.invoke(op, a, m, &mut self.storage, sender)?;
            if result == 0
                && op == crate::outbox::Op::Invite
                && let (Some(service), Some(session)) = (&self.service, self.session())
            {
                let capture = self.outbox.last().unwrap();
                let crate::outbox::MessageSendKind::GameInvite { session_id, .. } = &capture.kind
                else {
                    unreachable!()
                };
                let to = capture
                    .recipients
                    .iter()
                    .filter_map(|r| match r {
                        crate::outbox::MessageRecipient::User(u) => Some(live_proto::Xuid(u.xuid)),
                        _ => None,
                    })
                    .collect();
                let pending = service.social(
                    &session,
                    sender,
                    client::SocialRequest::Send {
                        to,
                        session_id: live_proto::sign::hex(&session_id.to_le_bytes()),
                    },
                );
                self.outbox.attach(m.u32(a[8])?, pending);
            }
            return Ok(result);
        }
        if let Arbitration(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if op != crate::arbitration::Op::CreateRoundId && !self.connected() {
                return Ok(NO_SESSION);
            }
            let session = self.session();
            let advertising = self.sessions.current_id();
            return self.arbitration.invoke(
                op,
                a,
                m,
                crate::arbitration::Context {
                    storage: &mut self.storage,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                    advertising,
                },
            );
        }
        if let Stats(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if matches!(op, crate::stats::Op::Read | crate::stats::Op::Write) && !self.connected() {
                return Ok(NO_SESSION);
            }
            let keys = self.read_keys.take().unwrap_or_default();
            let game_session = self.game_session(&keys);
            let session = self.session();
            return self.stats.invoke(
                op,
                a,
                m,
                crate::stats::Context {
                    storage: &mut self.storage,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                    game_session,
                },
            );
        }
        if let Storage(op) = op {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if op.starts_task() {
                if !self.connected() {
                    return Ok(NO_SESSION);
                }
                if a[1] >= 4 {
                    return Ok(INVALID_ARGUMENT);
                }
                if self.user_ids[a[1] as usize] == 0 {
                    return Ok(NO_USER);
                }
            }
            let session = self.session();
            return self.files.invoke(
                op,
                a,
                m,
                crate::storage_api::Context {
                    storage: &mut self.storage,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                },
            );
        }
        if matches!(op, MuteStartup | MuteGet | MuteAdd | MuteRemove) {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if !self.connected() {
                return Ok(NO_SESSION);
            }
            let session = self.session();
            let ctx = crate::mutes::Context {
                storage: &mut self.storage,
                service: self.service.as_ref(),
                session: session.as_ref(),
                users: &self.user_ids,
            };
            return match op {
                MuteStartup => self.mutes.startup(a, m, ctx),
                MuteGet => self.mutes.get(a, m, ctx),
                MuteAdd => Ok(self.mutes.change(a, ctx, true)),
                _ => Ok(self.mutes.change(a, ctx, false)),
            };
        }
        if op == Feedback {
            if self.online_refs == 0 {
                return Ok(NOT_INITIALIZED);
            }
            if !self.connected() {
                return Ok(NO_SESSION);
            }
            let session = self.session();
            return self.feedback.invoke(
                a,
                m,
                crate::feedback::Context {
                    storage: &mut self.storage,
                    service: self.service.as_ref(),
                    session: session.as_ref(),
                    users: &self.user_ids,
                    game_id: self.game_id.as_deref(),
                },
            );
        }
        if let Ui(op) = op {
            if self.online_refs != 0
                && matches!(
                    op,
                    crate::ui::Op::FriendsLatest | crate::ui::Op::Notification
                )
            {
                if a[0] >= 4 {
                    return Ok(0);
                }
                let session = self.session();
                self.invites.refresh(
                    a[0] as usize,
                    self.user_ids[a[0] as usize],
                    self.service.as_ref(),
                    session.as_ref(),
                );
                if op == crate::ui::Op::Notification {
                    return self.invites.notification(a[0] as usize, m, a);
                }
                if a[1] > 100 {
                    return Ok(0);
                }
                let rows = self.invites.friends(a[0] as usize);
                let count = rows.len().min(a[1] as usize);
                m.check(
                    a[2],
                    count * crate::invites::FRIEND_SIZE,
                    runtime::guest::Access::Write,
                    1,
                )?;
                for (i, row) in rows.iter().take(count).enumerate() {
                    m.write_bytes(a[2] + i as u32 * crate::invites::FRIEND_SIZE as u32, row, 1)?;
                }
                return Ok(count as u32);
            }
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
                if self.machine() {
                    let status = self.machine_logon(m)?;
                    if status != OK {
                        return Ok(status);
                    }
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
            AcceptedInvite => self.accepted_invite(m, a[0]),
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
                if !self.machine() && self.logon.is_some() {
                    return Ok(TASK_BUSY);
                }
                if a[2] > 16 {
                    return Ok(INVALID_ARGUMENT);
                }
                if self.machine() {
                    if a[0] != 0
                        && self
                            .users(m, a[0])?
                            .is_none_or(|users| users.iter().any(|b| *b != 0))
                    {
                        return Ok(INVALID_USER);
                    }
                    m.check(a[1], a[2] as usize * 4, Access::Read, 1)?;
                    self.services = (0..a[2])
                        .map(|i| m.u32(a[1] + i * 4))
                        .collect::<std::result::Result<_, _>>()?;
                    let status = self.machine_logon(m)?;
                    if status == OK {
                        m.set_u32(a[4], self.logon.expect("machine logon installed"))?;
                    }
                    return Ok(status);
                }
                let Some(users) = self.users(m, a[0])? else {
                    return Ok(INVALID_USER);
                };
                if users.iter().all(|v| *v == 0) {
                    return Ok(NO_USER);
                }
                // An identity waiting for a code: the logon waits for the
                // prompt (crate::signin). One per logon, and only with a server.
                let waiting: Vec<(usize, u64, String)> = users
                    .as_chunks::<USER_SIZE>()
                    .0
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, u)| {
                        let id = u64::from_le_bytes(u[..8].try_into().unwrap());
                        self.accounts
                            .iter()
                            .find(|a| id != 0 && a.xuid == id && a.needs_code)
                            .map(|a| (slot, a.xuid, a.gamertag.clone()))
                    })
                    .collect();
                let server_url = self.config.server_url.clone();
                let waiting = match (waiting.as_slice(), &server_url) {
                    ([], _) => None,
                    ([one], Some(url)) if self.service.is_some() => {
                        Some((one.clone(), url.clone()))
                    }
                    _ => return Ok(INVALID_USER),
                };
                m.check(a[1], a[2] as usize * 4, Access::Read, 1)?;
                let services = (0..a[2])
                    .map(|i| m.u32(a[1] + i * 4))
                    .collect::<std::result::Result<BTreeSet<_>, _>>()?;
                let Some(handle) = self.storage.alloc(m, 8) else {
                    return Ok(OUT_OF_MEMORY);
                };
                let session = self.service.as_ref().map(|_| Session::new(handle));
                if let Some(((slot, xuid, tag), url)) = waiting {
                    let files = crate::signin::files(self.config.accounts_path.as_deref());
                    self.signin = Some(if xuid == crate::config::NEW_ACCOUNT_XUID {
                        eprintln!("live: {tag}: asking in game for a gamertag on {url}");
                        crate::signin::Signin::adding(handle, slot, &url, files)
                    } else {
                        eprintln!("live: {tag} needs a code to sign in on {url}; asking in game");
                        crate::signin::Signin::new(handle, slot, &tag, &url, files)
                    });
                    m.set_u32(handle, 0)?;
                    m.set_u32(a[4], handle)?;
                    self.tasks.insert(
                        handle,
                        Task::Logon {
                            users,
                            pending: None,
                            session,
                            status: 0,
                        },
                    );
                    self.logon = Some(handle);
                    self.services = services;
                    return Ok(OK);
                }
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
                if let Some(status) = self
                    .invites
                    .poll(a[0], &self.user_ids, &self.user_flags)
                    .or_else(|| self.invites.details_status(a[0]))
                {
                    return Ok(status);
                }
                if let Some(status) = self.teams.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.sessions.poll(a[0], m, &mut self.storage, hle)? {
                    return Ok(status);
                }
                if let Some(status) = self.outbox.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.stats.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.arbitration.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.files.poll(a[0], m)? {
                    return Ok(status);
                }
                if let Some(status) = self.menu.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.feedback.poll(a[0]) {
                    return Ok(status);
                }
                if let Some(status) = self.mutes.poll(a[0], m)? {
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
                        if kind == EmptyKind::Friends {
                            let session = self.session();
                            for i in 0..4 {
                                self.invites.refresh(
                                    i,
                                    self.service_user(i),
                                    self.service.as_ref(),
                                    session.as_ref(),
                                );
                            }
                        }
                        let result = match kind {
                            EmptyKind::Friends if self.invites.take_changed() => {
                                TASK_RESULTS_AVAILABLE
                            }
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
            LogonUsers => Ok(if self.connected() || self.machine_connected() {
                self.storage.base.unwrap_or(0)
            } else {
                0
            }),
            ServiceInfo => {
                if !self.connected() && !self.machine_connected() {
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
                if self.invites.close(a[0]) {
                    return Ok(OK);
                }
                if self.teams.close(a[0], &mut self.outbox) {
                    return Ok(OK);
                }
                if self.sessions.close(a[0]) {
                    return Ok(OK);
                }
                if self.outbox.close(a[0]) {
                    return Ok(OK);
                }
                if self.stats.close(a[0]) {
                    return Ok(OK);
                }
                if self.arbitration.close(a[0]) {
                    return Ok(OK);
                }
                if self.files.close(a[0]) {
                    return Ok(OK);
                }
                if self.menu.close(a[0]) {
                    return Ok(OK);
                }
                if self.feedback.close(a[0]) {
                    return Ok(OK);
                }
                if self.mutes.close(a[0]) {
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
            FriendsStartup | Offering | PresenceInit => {
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
            PresenceLatest => {
                let result = self.menu.presence_latest(m, a)?;
                if result == 0
                    && let Some(Task::Presence { controller, .. }) = self.tasks.get(&a[0])
                {
                    let controller = *controller as usize;
                    let session = self.session();
                    self.invites.refresh(
                        controller,
                        self.user_ids[controller],
                        self.service.as_ref(),
                        session.as_ref(),
                    );
                    self.invites.presence(controller, m, a)?;
                }
                Ok(result)
            }
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
                if matches!(op, MessageEnumerate | MessageDetails | MessageDelete) {
                    self.invites.refresh(
                        a[0] as usize,
                        self.user_ids[a[0] as usize],
                        self.service.as_ref(),
                        session.as_ref(),
                    );
                }
                match op {
                    MessageEnumerate => self.invites.enumerate(a[0] as usize, m, a),
                    MessageDetails => self.invites.details(m, a, &mut self.storage),
                    MessageProperty => self.invites.property(m, a),
                    MessageDelete => Ok(self.invites.delete(
                        a[0] as usize,
                        a[1],
                        a[2] != 0,
                        self.service.as_ref(),
                        session.as_ref(),
                    )),
                    _ => crate::messages::invoke(op, a, m),
                }
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
/// A leased bot account's record: its xuid and gamertag, as an account's
/// own player ([`State::leased_bot`]). It has no key, so no service logon
/// is made for it.
fn bot_record(xuid: u64, gamertag: &str) -> [u8; USER_SIZE] {
    let mut r = [0; USER_SIZE];
    r[..8].copy_from_slice(&xuid.to_le_bytes());
    let tag = &gamertag.as_bytes()[..gamertag.len().min(15)];
    r[12..12 + tag.len()].copy_from_slice(tag);
    r
}

/// Presence's words for a co-op campaign on `level` (cut to fit).
pub fn coop_activity(level: &str) -> String {
    let mut a = format!("Co-op: {level}");
    if a.chars().count() > live_proto::PRESENCE_ACTIVITY_MAX {
        a = a.chars().take(live_proto::PRESENCE_ACTIVITY_MAX).collect();
    }
    a
}
