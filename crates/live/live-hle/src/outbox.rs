//! Owned outgoing messages. Generic sends are accepted locally. Server-backed
//! invitations attach a real asynchronous completion; captures survive destruction.
use crate::{
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_DETAILS: usize = 4096;
const MAX_ATTACHMENT: usize = 1024 * 1024;
const MAX_BUILDERS: usize = 64;
const MAX_CAPTURES: usize = 256;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageValue {
    Null,
    I1(u8),
    I2(u16),
    I4(u32),
    I8(u64),
    String(String),
    FileTime(u64),
    Binary(Vec<u8>),
    Attachment(Vec<u8>),
    Bool(bool),
    StringId(u32),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageProperty {
    pub tag: u16,
    pub attachment_flags: u32,
    pub value: MessageValue,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltMessage {
    pub message_type: u8,
    pub context: u64,
    pub flags: u32,
    pub expire_minutes: u16,
    pub properties: BTreeMap<u16, MessageProperty>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageUser {
    pub xuid: u64,
    pub flags: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRecipient {
    User(MessageUser),
    Gamertag(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecruitProperties {
    pub privileges: u32,
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSendKind {
    Message,
    FriendRequest,
    GameInvite {
        session_id: u64,
        flags: u32,
    },
    TeamRecruit {
        team: MessageUser,
        properties: RecruitProperties,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSend {
    pub sequence: u64,
    pub captured_at_unix_ms: i64,
    pub sender_index: u32,
    pub sender_xuid: u64,
    pub kind: MessageSendKind,
    pub message: BuiltMessage,
    pub recipients: Vec<MessageRecipient>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Create,
    Destroy,
    SetProperty,
    Send,
    FriendRequest,
    FriendRequestByName,
    Invite,
    Recruit,
    RecruitByName,
}
impl Op {
    pub fn sends(self) -> bool {
        !matches!(self, Self::Create | Self::Destroy | Self::SetProperty)
    }
}
#[derive(Default)]
pub(crate) struct Outbox {
    builders: BTreeMap<u32, BuiltMessage>,
    tasks: BTreeSet<u32>,
    remote: BTreeMap<u32, (Option<crate::client::Pending>, u32)>,
    captures: VecDeque<MessageSend>,
    bytes: usize,
    sequence: u64,
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
fn hr<T>(v: u32) -> Result<T> {
    Err(Failure::Hr(v))
}
impl Outbox {
    pub fn accepts(&self, h: u32, ty: u8) -> bool {
        h == 0 || self.builders.get(&h).is_some_and(|b| b.message_type == ty)
    }
    pub fn last(&self) -> Option<&MessageSend> {
        self.captures.back()
    }
    pub fn snapshot(&self) -> Vec<MessageSend> {
        self.captures.iter().cloned().collect()
    }
    pub fn drain(&mut self) -> Vec<MessageSend> {
        self.bytes = 0;
        self.captures.drain(..).collect()
    }
    pub fn clear_session(&mut self) {
        self.builders.clear();
        self.tasks.clear();
        self.remote.clear();
    }
    pub fn attach(&mut self, h: u32, pending: std::result::Result<crate::client::Pending, u32>) {
        self.remote.insert(
            h,
            match pending {
                Ok(p) => (Some(p), 0),
                Err(e) => (None, e),
            },
        );
    }
    pub fn poll(&mut self, h: u32) -> Option<u32> {
        if let Some((pending, status)) = self.remote.get_mut(&h) {
            if let Some(reply) = pending.as_ref().and_then(crate::client::Pending::take) {
                *status = match reply {
                    Ok(crate::client::Reply::Done) => TASK_SUCCESS,
                    Err(e) => e,
                    _ => INVALID_ARGUMENT,
                };
                *pending = None;
            }
            return Some(*status);
        }
        self.tasks.contains(&h).then_some(TASK_SUCCESS)
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.remote.remove(&h);
        self.tasks.remove(&h)
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        sender: u64,
    ) -> std::result::Result<u32, Error> {
        match self.checked(op, a, m, storage, sender) {
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
        sender: u64,
    ) -> Result<u32> {
        match op {
            Op::Create => {
                let ty = a[0] as u8;
                if !(1..=6).contains(&ty)
                    || (ty == 4 && a[5] & 0x40 == 0)
                    || ((ty == 5 || ty == 6) && a[5] & 0x80 == 0)
                {
                    return hr(INVALID_ARGUMENT);
                }
                if self.builders.len() >= MAX_BUILDERS
                    || self.builders.values().map(charge).sum::<usize>() > MAX_BYTES - 64
                {
                    return hr(OUT_OF_MEMORY);
                }
                // These are allocation hints in retail, not hard limits on later setters.
                if 8 + usize::from(a[1] as u16) * 4 + usize::from(a[2] as u16) > MAX_DETAILS {
                    return hr(MESSAGE_MAX_DETAILS);
                }
                m.check(a[7], 4, Access::Write, 1)?;
                let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                let message = BuiltMessage {
                    message_type: ty,
                    context: pair(a[3], a[4]),
                    flags: a[5],
                    expire_minutes: a[6] as u16,
                    properties: BTreeMap::new(),
                };
                m.set_u32(a[7], h)?;
                self.builders.insert(h, message);
                Ok(0)
            }
            Op::Destroy => {
                if self.builders.remove(&a[0]).is_some() {
                    Ok(0)
                } else {
                    hr(INVALID_ARGUMENT)
                }
            }
            Op::SetProperty => {
                let Some(message) = self.builders.get(&a[0]) else {
                    return hr(INVALID_ARGUMENT);
                };
                let tag = a[1] as u16;
                let property = property(m, tag, a[2], a[3], a[4])?;
                let mut next = message.clone();
                // An identifier denotes one property, including when its type changes.
                next.properties.retain(|old, _| old & 0xff != tag & 0xff);
                next.properties.insert(tag, property);
                if details_size(&next) > MAX_DETAILS {
                    return hr(MESSAGE_MAX_DETAILS);
                }
                if next
                    .properties
                    .values()
                    .filter(|p| matches!(p.value, MessageValue::Attachment(_)))
                    .count()
                    > 16
                {
                    return hr(BUFFER_TOO_SMALL);
                }
                let old_charge = charge(message);
                let used: usize = self.builders.values().map(charge).sum();
                if charge(&next) > MAX_BYTES - (used - old_charge) {
                    return hr(OUT_OF_MEMORY);
                }
                self.builders.insert(a[0], next);
                Ok(0)
            }
            _ => self.send(op, a, m, storage, sender),
        }
    }
    fn send(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        sender: u64,
    ) -> Result<u32> {
        let (h, event, out, ty) = match op {
            Op::Send => (a[1], a[4], a[5], 1),
            Op::FriendRequest => (a[4], a[5], a[6], 2),
            Op::FriendRequestByName => (a[2], a[3], a[4], 2),
            Op::Invite => (a[6], a[7], a[8], 3),
            Op::Recruit => (a[8], a[9], a[10], 4),
            Op::RecruitByName => (a[6], a[7], a[8], 4),
            _ => unreachable!(),
        };
        if event != 0 {
            return hr(INVALID_ARGUMENT);
        }
        m.check(out, 4, Access::Write, 1)?;
        let message = if h == 0 && op != Op::Send {
            BuiltMessage {
                message_type: ty,
                context: 0,
                flags: if ty == 4 { 0x40 } else { 0 },
                expire_minutes: 0,
                properties: BTreeMap::new(),
            }
        } else {
            self.builders
                .get(&h)
                .cloned()
                .ok_or(Failure::Hr(INVALID_ARGUMENT))?
        };
        if op != Op::Send && message.message_type != ty {
            return hr(INVALID_ARGUMENT);
        }
        let (recipients, kind) = match op {
            Op::Send => (users(m, a[3], a[2])?, MessageSendKind::Message),
            Op::FriendRequest => (
                vec![MessageRecipient::User(user(&a[1..4])?)],
                MessageSendKind::FriendRequest,
            ),
            Op::FriendRequestByName => (
                vec![MessageRecipient::Gamertag(name(m, a[1])?)],
                MessageSendKind::FriendRequest,
            ),
            Op::Invite => (
                users(m, a[2], a[1])?,
                MessageSendKind::GameInvite {
                    session_id: pair(a[3], a[4]),
                    flags: a[5],
                },
            ),
            Op::Recruit | Op::RecruitByName => {
                let team = user(&a[1..4])?;
                let (recipient, p) = if op == Op::Recruit {
                    (MessageRecipient::User(user(&a[4..7])?), a[7])
                } else {
                    (MessageRecipient::Gamertag(name(m, a[4])?), a[5])
                };
                m.check(p, 106, Access::Read, 1)?;
                let count = m.u16(p + 4)?;
                if count > 100 {
                    return hr(INVALID_ARGUMENT);
                }
                let properties = RecruitProperties {
                    privileges: m.u32(p)?,
                    data: bytes(m, p + 6, u32::from(count))?,
                };
                (
                    vec![recipient],
                    MessageSendKind::TeamRecruit { team, properties },
                )
            }
            _ => unreachable!(),
        };
        if recipients.iter().any(|r| {
            matches!(r, MessageRecipient::User(u)
            if live_proto::bots::is_bot_xuid(live_proto::Xuid(u.xuid)))
        }) {
            return hr(INVALID_ARGUMENT);
        }
        let cost = charge(&message)
            + recipients
                .iter()
                .map(|r| match r {
                    MessageRecipient::User(_) => 12,
                    MessageRecipient::Gamertag(s) => s.len(),
                })
                .sum::<usize>()
            + 256;
        if self.captures.len() >= MAX_CAPTURES || cost > MAX_BYTES - self.bytes {
            return hr(OUT_OF_MEMORY);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        let task = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        let capture = MessageSend {
            sequence,
            captured_at_unix_ms: (time::OffsetDateTime::now_utc().unix_timestamp_nanos()
                / 1_000_000) as i64,
            sender_index: a[0],
            sender_xuid: sender,
            kind,
            message,
            recipients,
        };
        m.set_u32(out, task)?;
        if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
            eprintln!("live message debug: {capture:?}");
        }
        self.sequence = sequence;
        self.bytes += cost;
        self.captures.push_back(capture);
        self.tasks.insert(task);
        Ok(0)
    }
}
fn pair(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}
fn user(a: &[u32]) -> Result<MessageUser> {
    let xuid = pair(a[0], a[1]);
    if xuid == 0 {
        return hr(INVALID_ARGUMENT);
    }
    Ok(MessageUser { xuid, flags: a[2] })
}
fn users(m: &Guest, p: u32, count: u32) -> Result<Vec<MessageRecipient>> {
    if count == 0 || count > 100 {
        return hr(INVALID_ARGUMENT);
    }
    m.check(p, count as usize * 12, Access::Read, 1)?;
    (0..count)
        .map(|i| {
            let p = p + i * 12;
            Ok(MessageRecipient::User(user(&[
                m.u32(p)?,
                m.u32(p + 4)?,
                m.u32(p + 8)?,
            ])?))
        })
        .collect()
}
fn name(m: &Guest, p: u32) -> Result<String> {
    let mut buf = Vec::new();
    for i in 0..16 {
        let b = m.u8(p.wrapping_add(i))?;
        if b == 0 {
            return if buf.is_empty() {
                hr(INVALID_ARGUMENT)
            } else {
                String::from_utf8(buf).map_err(|_| Failure::Hr(INVALID_ARGUMENT))
            };
        }
        buf.push(b);
    }
    hr(INVALID_ARGUMENT)
}
fn bytes(m: &Guest, p: u32, n: u32) -> Result<Vec<u8>> {
    let mut b = vec![0; n as usize];
    m.read_bytes(p, &mut b, 1)?;
    Ok(b)
}
fn property(m: &Guest, tag: u16, n: u32, p: u32, flags: u32) -> Result<MessageProperty> {
    let ty = tag >> 8;
    if (ty != 9 && flags != 0) || (ty == 9 && flags & !1 != 0) {
        return hr(INVALID_ARGUMENT);
    }
    let expected = match ty {
        1 => 0,
        2 => 1,
        3 => 2,
        4 | 10 | 11 => 4,
        5 | 7 => 8,
        6 | 8 | 9 => n,
        _ => return hr(INVALID_ARGUMENT),
    };
    if n != expected {
        return hr(INVALID_ARGUMENT);
    }
    if n as usize > if ty == 9 { MAX_ATTACHMENT } else { MAX_DETAILS } {
        return hr(MESSAGE_MAX_DETAILS);
    }
    let b = bytes(m, p, n)?;
    let value = match ty {
        1 => MessageValue::Null,
        2 => MessageValue::I1(b[0]),
        3 => MessageValue::I2(u16::from_le_bytes(b.try_into().unwrap())),
        4 => MessageValue::I4(u32::from_le_bytes(b.try_into().unwrap())),
        5 => MessageValue::I8(u64::from_le_bytes(b.try_into().unwrap())),
        6 => {
            if b.len() < 2 || b.len() % 2 != 0 {
                return hr(INVALID_ARGUMENT);
            }
            let units: Vec<_> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|u| u16::from_le_bytes(*u))
                .collect();
            if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
                return hr(INVALID_ARGUMENT);
            }
            MessageValue::String(
                String::from_utf16(&units[..units.len() - 1])
                    .map_err(|_| Failure::Hr(INVALID_ARGUMENT))?,
            )
        }
        7 => MessageValue::FileTime(u64::from_le_bytes(b.try_into().unwrap())),
        8 => MessageValue::Binary(b),
        9 => MessageValue::Attachment(b),
        10 => MessageValue::Bool(u32::from_le_bytes(b.try_into().unwrap()) != 0),
        11 => MessageValue::StringId(u32::from_le_bytes(b.try_into().unwrap())),
        _ => unreachable!(),
    };
    Ok(MessageProperty {
        tag,
        attachment_flags: flags,
        value,
    })
}
fn details_size(m: &BuiltMessage) -> usize {
    8 + m
        .properties
        .values()
        .map(|p| {
            4 + match &p.value {
                MessageValue::Null
                | MessageValue::I1(_)
                | MessageValue::I2(_)
                | MessageValue::Bool(_) => 0,
                MessageValue::I4(_) | MessageValue::StringId(_) => 4,
                MessageValue::I8(_) | MessageValue::FileTime(_) => 8,
                MessageValue::String(s) => s.len() + 3,
                MessageValue::Binary(b) => b.len() + 2,
                MessageValue::Attachment(_) => 80,
            }
        })
        .sum::<usize>()
}
fn charge(m: &BuiltMessage) -> usize {
    64 + m
        .properties
        .values()
        .map(|p| {
            64 + match &p.value {
                MessageValue::String(s) => s.len(),
                MessageValue::Binary(b) | MessageValue::Attachment(b) => b.len(),
                _ => 0,
            }
        })
        .sum::<usize>()
}
