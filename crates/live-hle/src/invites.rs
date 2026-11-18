//! Menu snapshots and invitation actions. All HTTP is queued on client::Service.
use crate::{
    client::{Pending, Reply, Service, Session, SocialRequest, SocialSnapshot},
    state::{Error, Storage},
    status::*,
};
use live_proto::{
    Xuid,
    mailbox::{Answer, Answered, Invite},
};
use runtime::{Guest, guest::Access};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};
pub const FRIEND_SIZE: usize = 86;
pub const ACCEPTED_SIZE: usize = 156;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    FriendSend,
    FriendAnswer,
    Join,
    Answer,
}
#[derive(Default)]
struct Cache {
    user: u64,
    view: Option<SocialSnapshot>,
    pending: Option<Pending>,
    next: Option<Instant>,
    hidden: BTreeSet<u64>,
    notified: BTreeSet<u64>,
    changed: bool,
}
struct Task {
    pending: Option<Pending>,
    controller: usize,
    status: u32,
    invite: Option<u64>,
    actor: u64,
}
#[derive(Clone)]
struct Accepted {
    invite: Invite,
    controller: usize,
    at: u64,
    users: [u64; 4],
    flags: [u32; 4],
}
#[derive(Default)]
pub(crate) struct Invites {
    cache: [Cache; 4],
    tasks: BTreeMap<u32, Task>,
    internal: Vec<Task>,
    accepted: VecDeque<Accepted>,
    message_ids: BTreeMap<u64, u32>,
    known: BTreeMap<u32, Invite>,
    next_message: u32,
    details: BTreeMap<u32, Invite>,
    /// The accepted invitation the title read last, until taken.
    read: Option<Invite>,
    /// That invitation as queued, to offer it again.
    last: Option<Accepted>,
}
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    pub users: &'a [u64; 4],
}
impl Invites {
    pub fn clear(&mut self) {
        self.cache = Default::default();
        self.tasks.clear();
        self.internal.clear();
        self.accepted.clear();
        self.read = None;
        self.last = None;
        self.details.clear();
        self.message_ids.clear();
        self.known.clear();
    }
    pub fn users_changed(&mut self, users: &[u64; 4]) {
        self.accepted
            .retain(|v| users[v.controller] == v.users[v.controller]);
        if self
            .last
            .as_ref()
            .is_some_and(|v| users[v.controller] != v.users[v.controller])
        {
            self.last = None;
        }
        self.tasks.retain(|_, v| users[v.controller] == v.actor);
        self.internal.retain(|v| users[v.controller] == v.actor);
        self.details.retain(|_, v| users.contains(&v.to.0));
        self.known.retain(|_, v| users.contains(&v.to.0));
        for (c, user) in self.cache.iter_mut().zip(users) {
            if c.user != *user {
                *c = Cache {
                    user: *user,
                    ..Default::default()
                };
            }
        }
    }
    fn finish(&mut self, mut task: Task, users: &[u64; 4], flags: &[u32; 4]) -> Task {
        if let Some(reply) = task.pending.as_ref().and_then(Pending::take) {
            task.pending = None;
            task.status = match reply {
                Ok(Reply::InviteAccepted(invite))
                    if users[task.controller] == task.actor && invite.to.0 == task.actor =>
                {
                    self.cache[task.controller].hidden.insert(invite.id);
                    self.cache[task.controller].next = None;
                    // A newer invitation from the same host names its
                    // current session: the older ones would join one gone.
                    self.accepted.retain(|v| v.invite.from != invite.from);
                    if self.accepted.len() >= 16 {
                        self.accepted.pop_front();
                    }
                    self.accepted.push_back(Accepted {
                        invite,
                        controller: task.controller,
                        at: filetime(time::OffsetDateTime::now_utc().unix_timestamp()),
                        users: *users,
                        flags: *flags,
                    });
                    TASK_SUCCESS
                }
                Ok(Reply::Done) => TASK_SUCCESS,
                Err(e) => {
                    if let Some(id) = task.invite {
                        self.cache[task.controller].hidden.remove(&id);
                    }
                    e
                }
                _ => INVALID_ARGUMENT,
            };
        }
        task
    }
    pub fn pump(&mut self, users: &[u64; 4], flags: &[u32; 4]) {
        for task in std::mem::take(&mut self.internal) {
            let task = self.finish(task, users, flags);
            if task.pending.is_some() {
                self.internal.push(task);
            }
        }
        let handles: Vec<_> = self.tasks.keys().copied().collect();
        for h in handles {
            let task = self.tasks.remove(&h).unwrap();
            let task = self.finish(task, users, flags);
            self.tasks.insert(h, task);
        }
    }
    pub fn poll(&mut self, h: u32, users: &[u64; 4], flags: &[u32; 4]) -> Option<u32> {
        self.pump(users, flags);
        self.tasks.get(&h).map(|t| t.status)
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.details.remove(&h).is_some() || self.tasks.remove(&h).is_some()
    }
    pub fn refresh(
        &mut self,
        controller: usize,
        user: u64,
        service: Option<&Service>,
        session: Option<&Session>,
    ) {
        let c = &mut self.cache[controller];
        if c.user != user {
            *c = Cache {
                user,
                ..Default::default()
            };
        }
        if let Some(reply) = c.pending.as_ref().and_then(Pending::take) {
            c.pending = None;
            if let Ok(Reply::Social(view)) = reply {
                let changed = c.view.as_ref().is_none_or(|old| {
                    old.friends.friends != view.friends.friends
                        || old.friends.requests_in != view.friends.requests_in
                        || old.friends.requests_out != view.friends.requests_out
                        || old.invites != view.invites
                        || old.sessions != view.sessions
                });
                if changed {
                    c.changed = true;
                }
                c.view = Some(view);
            }
            c.next = Some(Instant::now() + Duration::from_millis(250));
        }
        if user != 0
            && c.pending.is_none()
            && c.next.is_none_or(|v| Instant::now() >= v)
            && let (Some(service), Some(session)) = (service, session)
        {
            if let Ok(p) = service.social(session, user, SocialRequest::Refresh) {
                c.pending = Some(p);
            } else {
                c.next = Some(Instant::now() + Duration::from_millis(250));
            }
        }
    }
    pub fn take_changed(&mut self) -> bool {
        let mut any = false;
        for c in &mut self.cache {
            any |= std::mem::take(&mut c.changed);
        }
        any
    }
    pub fn presence(&self, index: usize, m: &mut Guest, a: &[u32]) -> Result<(), Error> {
        let friends = self.friends(index);
        for i in 0..a[2] {
            let at = a[3] + i * 61;
            let uid = m.u64(at)?;
            if let Some(f) = friends
                .iter()
                .find(|f| u64::from_le_bytes(f[..8].try_into().unwrap()) == uid)
            {
                m.write_bytes(at + 12, &f[28..32], 1)?;
                m.write_bytes(at + 16, &f[40..52], 1)?;
            }
        }
        Ok(())
    }
    fn pending_invites(&self, index: usize) -> Vec<Invite> {
        self.cache[index]
            .view
            .as_ref()
            .into_iter()
            .flat_map(|v| &v.invites)
            .filter(|v| !self.cache[index].hidden.contains(&v.id))
            .cloned()
            .collect()
    }
    pub fn friends(&self, index: usize) -> Vec<[u8; FRIEND_SIZE]> {
        let Some(view) = &self.cache[index].view else {
            return Vec::new();
        };
        let mut rows = BTreeMap::new();
        for f in &view.friends.friends {
            let mut row = friend(f.xuid.0, &f.gamertag.to_string(), 0, 0, 0);
            if let Some(p) = &f.presence {
                let flags: u32 = if p.state == "playing" { 3 } else { 1 };
                row[28..32].copy_from_slice(&flags.to_le_bytes());
            }
            if let Some(s) = view.sessions.iter().find(|s| s.host_xuid == f.xuid) {
                row[40..48].copy_from_slice(&session_bytes(&s.session_id));
                row[48..52].copy_from_slice(&0x4d530064u32.to_le_bytes());
                let flags = u32::from_le_bytes(row[28..32].try_into().unwrap()) | 0x10;
                row[28..32].copy_from_slice(&flags.to_le_bytes());
            }
            rows.insert(f.xuid, row);
        }
        for (requests, flag) in [
            (&view.friends.requests_in, 0x80000000u32),
            (&view.friends.requests_out, 0x40000000),
        ] {
            for r in requests {
                rows.entry(r.xuid)
                    .or_insert_with(|| friend(r.xuid.0, &r.gamertag.to_string(), flag, 0, 0));
            }
        }
        for v in self.pending_invites(index).into_iter().rev() {
            let mut row = friend(
                v.from.0,
                &v.gamertag,
                0x08000011,
                0x4d530064,
                filetime(v.sent_at),
            );
            row[40..48].copy_from_slice(&session_bytes(&v.session_id));
            rows.insert(v.from, row);
        }
        rows.into_values().take(100).collect()
    }
    fn message_id(&mut self, invite: &Invite) -> u32 {
        let id = invite.id;
        if let Some(v) = self.message_ids.get(&id) {
            self.known.insert(*v, invite.clone());
            return *v;
        }
        self.next_message = self.next_message.saturating_add(1);
        self.message_ids.insert(id, self.next_message);
        self.known.insert(self.next_message, invite.clone());
        self.next_message
    }
    pub fn enumerate(&mut self, index: usize, m: &mut Guest, a: &[u32]) -> Result<u32, Error> {
        let invites = self.pending_invites(index);
        m.check(a[2], 4, Access::Write, 1)?;
        m.check(a[1], invites.len() * 64, Access::Write, 1)?;
        for (i, v) in invites.iter().enumerate() {
            let id = self.message_id(v);
            m.write_bytes(a[1] + i as u32 * 64, &summary(v, id), 1)?;
        }
        m.set_u32(a[2], invites.len() as u32)?;
        Ok(0)
    }
    pub fn notification(&mut self, index: usize, m: &mut Guest, a: &[u32]) -> Result<u32, Error> {
        m.check(a[1], 9, Access::Write, 1)?;
        m.check(a[2], 4, Access::Write, 1)?;
        let next = self
            .pending_invites(index)
            .into_iter()
            .find(|v| !self.cache[index].notified.contains(&v.id));
        let mut out = [0; 9];
        let mut flags = 0;
        if let Some(v) = next {
            out[0] = 3;
            out[1..5].copy_from_slice(&self.message_id(&v).to_le_bytes());
            self.cache[index].notified.insert(v.id);
            flags = u32::from(
                self.pending_invites(index)
                    .iter()
                    .any(|v| !self.cache[index].notified.contains(&v.id)),
            );
        }
        let found = u32::from(out[0] != 0);
        m.write_bytes(a[1], &out, 1)?;
        m.set_u32(a[2], flags)?;
        Ok(found)
    }
    /// The title's read of the accepted invitation. `guest_now` is the
    /// guest's system time (its KeQuerySystemTime): the reader at
    /// `0x0022f3b5` drops a record stamped at or after it, or 15 minutes
    /// or more before it, so the stamp is a second before the guest's now,
    /// whatever the host's wall clock says. Without it (no guest clock),
    /// the time of the acceptance.
    pub fn accepted(
        &mut self,
        m: &mut Guest,
        out: u32,
        guest_now: Option<u64>,
    ) -> Result<u32, Error> {
        m.check(out, ACCEPTED_SIZE, Access::Write, 1)?;
        let mut record = [0; ACCEPTED_SIZE];
        self.accepted.retain(|v| {
            filetime(time::OffsetDateTime::now_utc().unix_timestamp()).saturating_sub(v.at)
                < 900 * 10_000_000
        });
        let result = if let Some(v) = self.accepted.pop_front() {
            let mut f = friend(
                v.invite.from.0,
                &v.invite.gamertag,
                0x08000011,
                0x4d530064,
                filetime(v.invite.sent_at),
            );
            f[40..48].copy_from_slice(&session_bytes(&v.invite.session_id));
            record[..86].copy_from_slice(&f);
            record[88..96].copy_from_slice(&v.users[v.controller].to_le_bytes());
            record[96..100].copy_from_slice(&v.flags[v.controller].to_le_bytes());
            let at = guest_now.map_or(v.at, |now| now.saturating_sub(10_000_000));
            record[100..108].copy_from_slice(&at.to_le_bytes());
            for (i, user) in v.users.into_iter().enumerate() {
                record[108 + i * 12..116 + i * 12].copy_from_slice(&user.to_le_bytes());
                record[116 + i * 12..120 + i * 12].copy_from_slice(&v.flags[i].to_le_bytes());
            }
            self.read = Some(v.invite.clone());
            self.last = Some(v);
            0
        } else {
            NO_ACCEPTED_INVITE
        };
        m.write_bytes(out, &record, 1)?;
        Ok(result)
    }
    fn find_message(&self, index: usize, id: u32) -> Option<Invite> {
        self.pending_invites(index)
            .into_iter()
            .find(|v| self.message_ids.get(&v.id) == Some(&id))
    }
    pub fn details(
        &mut self,
        m: &mut Guest,
        a: &[u32],
        storage: &mut Storage,
    ) -> Result<u32, Error> {
        if a[4] != 0 {
            return Ok(INVALID_ARGUMENT);
        }
        let Some(v) = self.find_message(a[0] as usize, a[1]) else {
            return Ok(MESSAGE_INVALID_ID);
        };
        if self.details.len() >= 64 {
            return Ok(OUT_OF_MEMORY);
        }
        m.check(a[5], 4, Access::Write, 1)?;
        let Some(h) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(a[5], h)?;
        self.details.insert(h, v);
        Ok(0)
    }
    pub fn property(&self, m: &mut Guest, a: &[u32]) -> Result<u32, Error> {
        let Some(v) = self.details.get(&a[0]) else {
            return Ok(INVALID_ARGUMENT);
        };
        let bytes = match a[1] as u16 {
            0x581 => session_bytes(&v.session_id).to_vec(),
            _ => return Ok(MESSAGE_PROPERTY_NOT_FOUND),
        };
        if a[4] != 0 {
            m.check(a[4], 4, Access::Write, 1)?;
        }
        if a[5] != 0 {
            m.check(a[5], 4, Access::Write, 1)?;
        }
        if a[3] != 0 && a[2] >= bytes.len() as u32 {
            m.check(a[3], bytes.len(), Access::Write, 1)?;
            m.write_bytes(a[3], &bytes, 1)?;
        }
        if a[4] != 0 {
            m.set_u32(a[4], bytes.len() as u32)?;
        }
        if a[5] != 0 {
            m.set_u32(a[5], 0)?;
        }
        Ok(if a[3] == 0 || a[2] < bytes.len() as u32 {
            INSUFFICIENT_BUFFER
        } else {
            0
        })
    }
    pub fn details_status(&self, h: u32) -> Option<u32> {
        self.details.contains_key(&h).then_some(TASK_SUCCESS)
    }
    pub fn invoke(
        &mut self,
        op: Op,
        m: &mut Guest,
        a: &[u32],
        ctx: Context<'_>,
    ) -> Result<u32, Error> {
        let controller = a[0] as usize;
        if controller >= 4 || ctx.users[controller] == 0 {
            return Ok(INVALID_USER);
        }
        let (Some(service), Some(session)) = (ctx.service, ctx.session) else {
            return Ok(NO_SESSION);
        };
        let request = match op {
            Op::FriendSend => {
                if a[3] == 0 || a[3] > 100 {
                    return Ok(INVALID_ARGUMENT);
                }
                m.check(a[4], a[3] as usize * FRIEND_SIZE, Access::Read, 1)?;
                let mut to = Vec::new();
                for i in 0..a[3] {
                    let uid = m.u64(a[4] + i * FRIEND_SIZE as u32)?;
                    if uid == 0 {
                        return Ok(INVALID_ARGUMENT);
                    }
                    to.push(Xuid(uid));
                }
                SocialRequest::Send {
                    to,
                    session_id: live_proto::sign::hex(&pair(a[1], a[2]).to_le_bytes()),
                }
            }
            Op::FriendAnswer | Op::Answer => {
                if a[2] > 2 {
                    return Ok(INVALID_ARGUMENT);
                }
                let n = if op == Op::Answer { 48 } else { FRIEND_SIZE };
                m.check(a[1], n, Access::Read, 1)?;
                let from = m.u64(a[1])?;
                let offset = if op == Op::Answer { 32 } else { 40 };
                let kid = m.u64(a[1] + offset)?;
                let id = live_proto::sign::hex(&kid.to_le_bytes());
                let Some(invite) = self
                    .pending_invites(controller)
                    .into_iter()
                    .find(|v| v.from.0 == from && v.session_id == id)
                else {
                    return Ok(FRIEND_NOT_FOUND);
                };
                SocialRequest::Answer(Answered {
                    by: Xuid(ctx.users[controller]),
                    session_id: id,
                    invite_id: invite.id,
                    answer: match a[2] {
                        1 => Answer::Accept,
                        2 if op == Op::Answer => Answer::Block,
                        2 => Answer::Remove,
                        _ => Answer::Decline,
                    },
                })
            }
            Op::Join => {
                m.check(a[1], FRIEND_SIZE, Access::Read, 1)?;
                SocialRequest::Join {
                    from: Xuid(m.u64(a[1])?),
                    session_id: live_proto::sign::hex(&m.u64(a[1] + 40)?.to_le_bytes()),
                }
            }
        };
        let out = if op == Op::Answer {
            if a[3] != 0 {
                return Ok(INVALID_ARGUMENT);
            }
            m.check(a[4], 4, Access::Write, 1)?;
            Some(a[4])
        } else {
            None
        };
        if self.internal.len() + self.tasks.len() >= 64 {
            return Ok(TASK_BUSY);
        }
        let handle = if out.is_some() {
            let Some(h) = ctx.storage.alloc(m, 8) else {
                return Ok(OUT_OF_MEMORY);
            };
            Some(h)
        } else {
            None
        };
        let answered = match &request {
            SocialRequest::Answer(v) => Some(v.invite_id),
            _ => None,
        };
        let pending = match service.social(session, ctx.users[controller], request) {
            Ok(p) => p,
            Err(e) => return Ok(e),
        };
        if let Some(id) = answered {
            self.cache[controller].hidden.insert(id);
        }
        let task = Task {
            pending: Some(pending),
            controller,
            status: 0,
            invite: answered,
            actor: ctx.users[controller],
        };
        if let (Some(h), Some(out)) = (handle, out) {
            m.set_u32(out, h)?;
            self.tasks.insert(h, task);
        } else {
            self.internal.push(task);
        }
        Ok(0)
    }
    /// The friends of controller `index`'s player as last read (the menu's
    /// refresh keeps it current): accepted friends only, online ones first,
    /// each with what presence says.
    pub fn friend_list(&self, index: usize) -> Vec<crate::Friend> {
        let Some(view) = &self.cache[index].view else {
            return Vec::new();
        };
        let mut out: Vec<crate::Friend> = view
            .friends
            .friends
            .iter()
            .map(|f| crate::Friend {
                xuid: f.xuid,
                gamertag: f.gamertag.to_string(),
                online: f.presence.is_some(),
                playing: f.presence.as_ref().is_some_and(|p| p.state == "playing"),
                activity: f.presence.as_ref().and_then(|p| p.activity.clone()),
            })
            .collect();
        out.sort_by_key(|f| (!f.online, f.gamertag.to_ascii_lowercase()));
        out
    }
    /// Sends a game invitation for `session_id` from controller
    /// `controller`'s player (`actor`) to `to`, outside a guest task (the
    /// co-op screen's Invite). The answer is pumped like the title's own.
    pub fn send(
        &mut self,
        controller: usize,
        actor: u64,
        to: Xuid,
        session_id: String,
        service: Option<&Service>,
        session: Option<&Session>,
    ) -> Result<(), u32> {
        let (Some(service), Some(session)) = (service, session) else {
            return Err(NO_SESSION);
        };
        if controller >= 4 || actor == 0 {
            return Err(INVALID_USER);
        }
        if self.internal.len() + self.tasks.len() >= 64 {
            return Err(TASK_BUSY);
        }
        let pending = service.social(
            session,
            actor,
            SocialRequest::Send {
                to: vec![to],
                session_id,
            },
        )?;
        self.internal.push(Task {
            pending: Some(pending),
            controller,
            status: 0,
            invite: None,
            actor,
        });
        Ok(())
    }
    /// The accepted invitation the title read since the last call.
    pub fn take_read(&mut self) -> Option<Invite> {
        self.read.take()
    }
    /// Queues the invitation the title read last again, if it is `id`, as
    /// accepted now (a title that read it and never looked the session up
    /// gets another go). False if it isn't the last read.
    pub fn offer_again(&mut self, id: u64) -> bool {
        let Some(mut v) = self.last.clone().filter(|v| v.invite.id == id) else {
            return false;
        };
        v.at = filetime(time::OffsetDateTime::now_utc().unix_timestamp());
        self.accepted.retain(|a| a.invite.from != v.invite.from);
        self.accepted.push_back(v);
        true
    }
    /// Accepted invitations the title hasn't read yet, oldest first.
    pub fn unread(&self) -> Vec<Invite> {
        self.accepted.iter().map(|v| v.invite.clone()).collect()
    }
    /// The invitations waiting for controller `index`'s player, as last
    /// read.
    pub fn pending(&self, index: usize) -> Vec<Invite> {
        self.pending_invites(index)
    }
    /// Accepts `invite` for controller `controller`'s player (`actor`)
    /// outside a guest task, as the friends list's Accept does; on success
    /// the accepted invitation is queued for the title like one accepted
    /// there ([`Invites::accepted`]).
    pub fn accept(
        &mut self,
        controller: usize,
        actor: u64,
        invite: &Invite,
        service: Option<&Service>,
        session: Option<&Session>,
    ) -> Result<(), u32> {
        let (Some(service), Some(session)) = (service, session) else {
            return Err(NO_SESSION);
        };
        if controller >= 4 || actor == 0 {
            return Err(INVALID_USER);
        }
        if self.internal.len() + self.tasks.len() >= 64 {
            return Err(TASK_BUSY);
        }
        let pending = service.social(
            session,
            actor,
            SocialRequest::Answer(Answered {
                by: Xuid(actor),
                session_id: invite.session_id.clone(),
                invite_id: invite.id,
                answer: Answer::Accept,
            }),
        )?;
        self.cache[controller].hidden.insert(invite.id);
        self.internal.push(Task {
            pending: Some(pending),
            controller,
            status: 0,
            invite: Some(invite.id),
            actor,
        });
        Ok(())
    }
    pub fn delete(
        &mut self,
        index: usize,
        id: u32,
        block: bool,
        service: Option<&Service>,
        session: Option<&Session>,
    ) -> u32 {
        let Some(v) = self
            .known
            .get(&id)
            .filter(|v| v.to.0 == self.cache[index].user)
            .cloned()
        else {
            return MESSAGE_INVALID_ID;
        };
        if self.cache[index].hidden.contains(&v.id) {
            return 0;
        }
        if self.internal.len() >= 64 {
            return TASK_BUSY;
        }
        if let (Some(service), Some(session)) = (service, session) {
            let request = SocialRequest::Answer(Answered {
                by: v.to,
                session_id: v.session_id,
                invite_id: v.id,
                answer: if block { Answer::Block } else { Answer::Remove },
            });
            let pending = match service.social(session, v.to.0, request) {
                Ok(p) => p,
                Err(e) => return e,
            };
            self.internal.push(Task {
                pending: Some(pending),
                controller: index,
                status: 0,
                invite: Some(v.id),
                actor: v.to.0,
            });
        }
        self.cache[index].hidden.insert(v.id);
        0
    }
}
fn filetime(seconds: i64) -> u64 {
    ((i128::from(seconds) + 11_644_473_600) * 10_000_000).clamp(0, u64::MAX as i128) as u64
}
fn pair(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}
fn session_bytes(id: &str) -> [u8; 8] {
    u64::from_str_radix(id, 16).unwrap_or(0).to_be_bytes()
}
fn friend(uid: u64, tag: &str, flags: u32, title: u32, at: u64) -> [u8; FRIEND_SIZE] {
    let mut v = [0; FRIEND_SIZE];
    v[..8].copy_from_slice(&uid.to_le_bytes());
    let n = tag.len().min(15);
    v[12..12 + n].copy_from_slice(&tag.as_bytes()[..n]);
    v[28..32].copy_from_slice(&flags.to_le_bytes());
    v[32..40].copy_from_slice(&at.to_le_bytes());
    v[48..52].copy_from_slice(&title.to_le_bytes());
    v
}
fn summary(v: &Invite, id: u32) -> [u8; 64] {
    let mut b = [0; 64];
    b[..8].copy_from_slice(&v.from.0.to_le_bytes());
    b[12] = 3;
    b[24..32].copy_from_slice(&filetime(v.sent_at).to_le_bytes());
    b[32..36].copy_from_slice(&id.to_le_bytes());
    b[46..48].copy_from_slice(&16u16.to_le_bytes());
    b[40..44].copy_from_slice(&0x4d530064u32.to_le_bytes());
    let n = v.gamertag.len().min(15);
    b[48..48 + n].copy_from_slice(&v.gamertag.as_bytes()[..n]);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;
    fn invite() -> Invite {
        Invite {
            id: 9,
            from: Xuid(0x0009000000000001),
            to: Xuid(0x0009000000000002),
            gamertag: "Host".into(),
            session_id: "8001020304050607".into(),
            sent_at: 1_700_000_000,
            answer: None,
        }
    }
    #[test]
    fn accepted_layout_preserves_uid_flags_padding_and_consumes_only_after_a_valid_write() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x700000, 0x1000, Permissions::RW)
            .unwrap();
        let mut state = Invites::default();
        let at = filetime(time::OffsetDateTime::now_utc().unix_timestamp());
        let ids = [2, 3, 0, 0];
        let flags = [0, 1, 0, 0];
        state.accepted.push_back(Accepted {
            invite: invite(),
            controller: 1,
            at,
            users: ids,
            flags,
        });
        assert!(state.accepted(&mut memory, 0xfffffff0, None).is_err());
        assert_eq!(state.accepted.len(), 1);
        assert_eq!(state.accepted(&mut memory, 0x700000, None).ok(), Some(0));
        assert_eq!(memory.u64(0x700000).unwrap(), 0x0009000000000001);
        assert_eq!(
            memory.u64(0x700000 + 40).unwrap(),
            u64::from_le_bytes([0x80, 1, 2, 3, 4, 5, 6, 7])
        );
        assert_eq!(memory.u16(0x700000 + 86).unwrap(), 0);
        assert_eq!(memory.u64(0x700000 + 88).unwrap(), 3);
        assert_eq!(memory.u32(0x700000 + 96).unwrap(), 1);
        assert_eq!(memory.u64(0x700000 + 100).unwrap(), at);
        for i in 0..4 {
            assert_eq!(
                memory.u64(0x700000 + 108 + i * 12).unwrap(),
                ids[i as usize]
            );
            assert_eq!(
                memory.u32(0x700000 + 116 + i * 12).unwrap(),
                flags[i as usize]
            );
        }
        assert_eq!(
            state.accepted(&mut memory, 0x700000, None).ok(),
            Some(NO_ACCEPTED_INVITE)
        );
        let mut bytes = [1; 156];
        memory.read_bytes(0x700000, &mut bytes, 1).unwrap();
        assert_eq!(bytes, [0; 156]);
    }
    /// The reader at `0x0022f3b5`: KeQuerySystemTime (`0x0022f3d7`), then
    /// the record's FILETIME at +100 must be before it and less than
    /// 0x2_18711a00 (15 minutes) before it, or the invitation is dropped.
    fn title_takes(record_at: u64, guest_now: u64) -> bool {
        guest_now > record_at && guest_now - record_at < 0x2_1871_1a00
    }
    #[test]
    fn the_title_takes_an_invitation_stamped_in_its_own_clock() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x700000, 0x1000, Permissions::RW)
            .unwrap();
        let wall = filetime(time::OffsetDateTime::now_utc().unix_timestamp());
        let accepted = || Accepted {
            invite: invite(),
            controller: 0,
            at: wall,
            users: [2, 0, 0, 0],
            flags: [0; 4],
        };
        // The guest's clock starts at the NT epoch (the kernel's epoch is
        // 0): ten minutes after boot it is still 1601 to the title, so a
        // wall-clock stamp is in its future and the title drops it.
        let guest_now = 10 * 60 * 10_000_000;
        assert!(!title_takes(wall, guest_now));
        let mut state = Invites::default();
        state.accepted.push_back(accepted());
        assert_eq!(
            state.accepted(&mut memory, 0x700000, Some(guest_now)).ok(),
            Some(0)
        );
        let stamped = memory.u64(0x700000 + 100).unwrap();
        assert!(title_takes(stamped, guest_now), "{stamped} vs {guest_now}");
        assert_eq!(state.take_read().map(|v| v.id), Some(invite().id));
        assert_eq!(state.take_read(), None, "said once");
        // Offered again: the same record, read once more.
        assert!(!state.offer_again(invite().id + 1));
        assert!(state.offer_again(invite().id));
        assert_eq!(state.unread().len(), 1);
        assert_eq!(
            state.accepted(&mut memory, 0x700000, Some(guest_now)).ok(),
            Some(0)
        );
        assert_eq!(
            memory.u64(0x700000 + 40).unwrap(),
            u64::from_le_bytes([0x80, 1, 2, 3, 4, 5, 6, 7])
        );
        // Without a guest clock the stamp stays the acceptance's.
        state.accepted.push_back(accepted());
        assert_eq!(state.accepted(&mut memory, 0x700000, None).ok(), Some(0));
        assert_eq!(memory.u64(0x700000 + 100).unwrap(), wall);
    }
    #[test]
    fn accepted_invites_expire_and_clear_invalidates_tasks_and_snapshots() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x700000, 0x1000, Permissions::RW)
            .unwrap();
        let mut state = Invites::default();
        state.accepted.push_back(Accepted {
            invite: invite(),
            controller: 0,
            at: 0,
            users: [2, 0, 0, 0],
            flags: [0; 4],
        });
        assert_eq!(
            state.accepted(&mut memory, 0x700000, None).ok(),
            Some(NO_ACCEPTED_INVITE)
        );
        state.details.insert(1, invite());
        assert_eq!(state.details_status(1), Some(TASK_SUCCESS));
        assert!(state.close(1));
        assert_eq!(state.details_status(1), None);
        state.details.insert(2, invite());
        state.clear();
        assert_eq!(state.details_status(2), None);
        assert!(state.friends(0).is_empty());
    }
    #[test]
    fn replacing_a_controller_cannot_deliver_the_previous_accounts_accepted_join() {
        let mut state = Invites::default();
        let at = filetime(time::OffsetDateTime::now_utc().unix_timestamp());
        state.accepted.push_back(Accepted {
            invite: invite(),
            controller: 0,
            at,
            users: [2, 0, 0, 0],
            flags: [0; 4],
        });
        state.details.insert(1, invite());
        state.users_changed(&[3, 0, 0, 0]);
        assert!(state.accepted.is_empty());
        assert!(state.details.is_empty());
        assert!(state.friends(0).is_empty());
    }
}
