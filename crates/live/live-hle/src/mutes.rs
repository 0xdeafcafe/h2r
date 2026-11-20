//! The mute list (docs/live/live-voice.md §1.5 and §3.6): the XOnline calls
//! the game builds its voice mute matrix from, backed by the server's
//! per-account list (docs/live/live-api.md "Mute list").
//!
//! - `XOnlineMutelistStartup(event, &task)` (`0x003a0841`, from `0x6c1a0`
//!   after sign-in) loads every signed-in player's list; its task completes
//!   when they are in.
//! - `XOnlineMutelistGet(controller, max, event, &task, buffer, &count)`
//!   (`0x003a0832`, at most 250) writes the controller's list, 16 bytes an
//!   entry (the 8-byte XUID, its 4-byte flags, 4 unused), and the count.
//!   A loaded list is written at once; otherwise when it loads.
//! - `XOnlineMutelistAdd` and `Remove(controller, xuid lo, xuid hi, flags)`
//!   (`0x003a084c`, `0x003a086e`) change the list at once and send the
//!   change to the server behind it; the game waits on no task. A change
//!   the server did not take is sent again before the next load.
//!
//! Signed in without a server, the lists start empty and live on this
//! machine until sign-out.
//!
//! The game compares the 12 bytes byte for byte (`0x541a0`), so Get hands
//! back the flags Add was given. Entries loaded from the server that this
//! machine never added get flags 0, which is what every h2 live account's
//! XUID carries.
use crate::{
    client::{Pending, Reply, Service, Session},
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, guest::Access};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Error>;

/// The game's list size, and the server's cap.
pub(crate) const MAX: usize = live_proto::mutes::MAX;
/// Bytes an entry takes in the game's buffer.
const ENTRY: u32 = 16;

/// Where a Get writes its answer.
#[derive(Clone, Copy)]
struct GetOut {
    user: u64,
    buffer: u32,
    max: u32,
    count: u32,
}

struct Task {
    /// The players whose lists the task waits for.
    users: Vec<u64>,
    get: Option<GetOut>,
    status: u32,
}

/// A change sent to the server and not yet answered.
struct Write {
    user: u64,
    target: u64,
    mute: bool,
    pending: Pending,
}

/// What a call needs from the session.
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    /// The signed-in players' XUIDs, by controller.
    pub users: &'a [u64; 4],
}

#[derive(Default)]
pub(crate) struct Mutes {
    /// Loaded lists by player, oldest first: XUID and flags.
    lists: BTreeMap<u64, Vec<(u64, u32)>>,
    /// Lists being fetched, and the error of a fetch that failed.
    loading: BTreeMap<u64, Pending>,
    failed: BTreeMap<u64, u32>,
    writes: Vec<Write>,
    /// Changes the server has not taken, by player and target: mute or not.
    unsynced: BTreeMap<u64, BTreeMap<u64, bool>>,
    /// Changes made while a player's list was loading, oldest first: the
    /// server's answer may predate them.
    since_load: BTreeMap<u64, Vec<(u64, bool)>>,
    /// The flags the game gave for each XUID it muted.
    flags: BTreeMap<u64, u32>,
    tasks: BTreeMap<u32, Task>,
}

impl Mutes {
    /// Signed out: lists load again at the next sign-in. Changes the server
    /// has not taken are kept for then.
    pub fn clear(&mut self) {
        self.harvest_writes();
        for w in self.writes.drain(..) {
            self.unsynced
                .entry(w.user)
                .or_default()
                .insert(w.target, w.mute);
        }
        self.lists.clear();
        self.loading.clear();
        self.since_load.clear();
        self.failed.clear();
        self.tasks.clear();
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }

    /// The task's status (0 while lists are loading), writing a Get's
    /// answer once its list is in.
    pub fn poll(&mut self, h: u32, m: &mut Guest) -> Result<Option<u32>> {
        if !self.tasks.contains_key(&h) {
            return Ok(None);
        }
        self.harvest();
        let t = &self.tasks[&h];
        if t.status != 0 {
            return Ok(Some(t.status));
        }
        if t.users.iter().any(|u| self.loading.contains_key(u)) {
            return Ok(Some(0));
        }
        let failed = t.users.iter().find_map(|u| self.failed.get(u)).copied();
        let get = t.get;
        let status = match (failed, get) {
            (Some(e), _) => e,
            (None, Some(g)) => {
                self.write_list(m, g)?;
                TASK_SUCCESS
            }
            (None, None) => TASK_SUCCESS,
        };
        self.tasks.get_mut(&h).unwrap().status = status;
        Ok(Some(status))
    }

    /// `XOnlineMutelistStartup(event, &task)`.
    pub fn startup(&mut self, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        if a[0] != 0 {
            return Err(Error::Unverified("nonzero work event"));
        }
        m.check(a[1], 4, Access::Write, 1)?;
        let users: Vec<u64> = ctx.users.iter().copied().filter(|u| *u != 0).collect();
        for u in &users {
            if let Err(e) = self.load(*u, &ctx) {
                return Ok(e);
            }
        }
        self.task(m, a[1], ctx.storage, users, None)
    }

    /// `XOnlineMutelistGet(controller, max, event, &task, buffer, &count)`.
    pub fn get(&mut self, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        if a[0] >= 4 || a[1] as usize > MAX {
            return Ok(INVALID_ARGUMENT);
        }
        if a[2] != 0 {
            return Err(Error::Unverified("nonzero work event"));
        }
        m.check(a[3], 4, Access::Write, 1)?;
        m.check(a[5], 4, Access::Write, 1)?;
        m.check(a[4], (a[1] * ENTRY) as usize, Access::Write, 1)?;
        let user = ctx.users[a[0] as usize];
        if user == 0 {
            return Ok(NO_USER);
        }
        let out = GetOut {
            user,
            buffer: a[4],
            max: a[1],
            count: a[5],
        };
        self.harvest();
        if let Err(e) = self.load(user, &ctx) {
            return Ok(e);
        }
        // Loaded already (or kept here, without a server): written now.
        if self.lists.contains_key(&user) {
            self.write_list(m, out)?;
        }
        self.task(m, a[3], ctx.storage, vec![user], Some(out))
    }

    /// `XOnlineMutelistAdd` (`mute`) or `Remove(controller, xuid lo, xuid
    /// hi, flags)`.
    pub fn change(&mut self, a: &[u32], ctx: Context<'_>, mute: bool) -> u32 {
        if a[0] >= 4 {
            return INVALID_ARGUMENT;
        }
        let user = ctx.users[a[0] as usize];
        if user == 0 {
            return NO_USER;
        }
        let target = u64::from(a[1]) | (u64::from(a[2]) << 32);
        if target == 0 || target == user {
            return INVALID_ARGUMENT;
        }
        self.harvest();
        if let Some(list) = self.lists.get_mut(&user) {
            let present = list.iter().position(|(x, _)| *x == target);
            match (mute, present) {
                (true, Some(i)) => list[i].1 = a[3],
                (true, None) if list.len() >= MAX => return INVALID_ARGUMENT,
                (true, None) => list.push((target, a[3])),
                (false, Some(i)) => {
                    list.remove(i);
                }
                (false, None) => {}
            }
        }
        if mute {
            self.flags.insert(target, a[3]);
        }
        if self.loading.contains_key(&user) {
            self.since_load
                .entry(user)
                .or_default()
                .push((target, mute));
        }
        let (Some(service), Some(session)) = (ctx.service, ctx.session) else {
            // Signed in without a server: the list lives here only.
            return OK;
        };
        match service.mute(session, user, target, mute) {
            Ok(pending) => self.writes.push(Write {
                user,
                target,
                mute,
                pending,
            }),
            // The worker's queue is full: send it with the next load.
            Err(_) => {
                self.unsynced.entry(user).or_default().insert(target, mute);
            }
        }
        OK
    }

    /// Starts fetching `user`'s list unless it is in or on its way, sending
    /// the changes the server has not taken first (the worker runs them in
    /// order, so the list comes back with them).
    fn load(&mut self, user: u64, ctx: &Context<'_>) -> std::result::Result<(), u32> {
        if self.lists.contains_key(&user) || self.loading.contains_key(&user) {
            return Ok(());
        }
        let (Some(service), Some(session)) = (ctx.service, ctx.session) else {
            // Signed in without a server: an empty list, kept here.
            self.lists.insert(user, Vec::new());
            return Ok(());
        };
        self.failed.remove(&user);
        for (target, mute) in self.unsynced.remove(&user).unwrap_or_default() {
            match service.mute(session, user, target, mute) {
                Ok(pending) => self.writes.push(Write {
                    user,
                    target,
                    mute,
                    pending,
                }),
                Err(_) => {
                    self.unsynced.entry(user).or_default().insert(target, mute);
                }
            }
        }
        let pending = service.mute_list(session, user)?;
        self.loading.insert(user, pending);
        self.since_load.remove(&user);
        Ok(())
    }

    fn harvest(&mut self) {
        self.harvest_writes();
        let done: Vec<(u64, Result2)> = self
            .loading
            .iter()
            .filter_map(|(u, p)| p.take().map(|r| (*u, r)))
            .collect();
        for (user, reply) in done {
            self.loading.remove(&user);
            match reply {
                Ok(Reply::Mutes(xuids)) => {
                    let mut list: Vec<(u64, u32)> = xuids
                        .into_iter()
                        .take(MAX)
                        .map(|x| (x, self.flags.get(&x).copied().unwrap_or(0)))
                        .collect();
                    // Changes the server has not taken, then those made
                    // while it loaded.
                    let changes: Vec<(u64, bool)> = self
                        .unsynced
                        .get(&user)
                        .into_iter()
                        .flatten()
                        .map(|(t, m)| (*t, *m))
                        .chain(self.since_load.remove(&user).unwrap_or_default())
                        .collect();
                    for (target, mute) in changes {
                        list.retain(|(x, _)| *x != target);
                        if mute && list.len() < MAX {
                            list.push((target, self.flags.get(&target).copied().unwrap_or(0)));
                        }
                    }
                    self.lists.insert(user, list);
                }
                Ok(_) => {
                    self.since_load.remove(&user);
                    self.failed.insert(user, CANNOT_ACCESS_SERVICE);
                }
                Err(e) => {
                    self.since_load.remove(&user);
                    self.failed.insert(user, e);
                }
            }
        }
    }

    fn harvest_writes(&mut self) {
        let mut i = 0;
        while i < self.writes.len() {
            let Some(reply) = self.writes[i].pending.take() else {
                i += 1;
                continue;
            };
            let w = self.writes.remove(i);
            let unsynced = self.unsynced.entry(w.user).or_default();
            match reply {
                Ok(_) => {
                    if unsynced.get(&w.target) == Some(&w.mute) {
                        unsynced.remove(&w.target);
                    }
                }
                Err(_) => {
                    unsynced.insert(w.target, w.mute);
                }
            }
        }
        self.unsynced.retain(|_, m| !m.is_empty());
    }

    fn write_list(&self, m: &mut Guest, g: GetOut) -> Result<()> {
        let list = self.lists.get(&g.user).map(Vec::as_slice).unwrap_or(&[]);
        let n = list.len().min(g.max as usize);
        for (i, (xuid, flags)) in list[..n].iter().enumerate() {
            let p = g.buffer + i as u32 * ENTRY;
            let mut e = [0u8; ENTRY as usize];
            e[..8].copy_from_slice(&xuid.to_le_bytes());
            e[8..12].copy_from_slice(&flags.to_le_bytes());
            m.write_bytes(p, &e, 1)?;
        }
        m.set_u32(g.count, n as u32)?;
        Ok(())
    }

    fn task(
        &mut self,
        m: &mut Guest,
        out: u32,
        storage: &mut Storage,
        users: Vec<u64>,
        get: Option<GetOut>,
    ) -> Result<u32> {
        let Some(h) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, h)?;
        self.tasks.insert(
            h,
            Task {
                users,
                get,
                status: 0,
            },
        );
        Ok(OK)
    }
}

type Result2 = std::result::Result<Reply, u32>;
