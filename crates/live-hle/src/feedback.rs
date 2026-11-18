//! `XOnlineFeedbackSend` (`0x003a0890`): a player's complaint about another
//! player, or a commendation, sent to the server as a player report
//! (docs/live/live-api.md "Player reports").
//!
//! The entry takes eight stack words: the controller, the target's XUID by
//! value (its 64-bit id and its flags, three words), the XDK feedback type,
//! a pointer to `XONLINE_FEEDBACK_PARAMS` (one field, a wide-string
//! pointer; the library sends it as UTF-8, at most 255 bytes), the work
//! event and the task output.
//!
//! The title calls it from one wrapper, `0x0008d3d0`, which maps the
//! player-details list's reason index 0..9 through a table on its stack to
//! the feedback types 5, 6, 0, 1, 2, 3, 4, 12, 11, 10
//! ([`live_proto::reports::Kind::GAME_ORDER`]), passes a params block whose
//! string is null, no event, and keeps the task in its task list. Its two
//! callers (`0x002b89be`, `0x002b8a65`) send reasons 0 and 1 (the
//! commendations) at once and the rest after a confirmation dialog.
//!
//! The retail library remembers the last 20 players each controller gave
//! feedback on, with one bit for each type below 8, and completes a repeat
//! of a type at once without sending it (`0x003b8cbb`). This keeps the same
//! memory, except that a send that fails is forgotten, so a retry is sent
//! rather than reported as done.
use crate::{
    client::{Pending, Service, Session},
    state::{Error, Storage},
    status::*,
};
use live_proto::reports::Kind;
use runtime::{Guest, guest::Access};
use std::collections::{BTreeMap, VecDeque};

type Result<T> = std::result::Result<T, Error>;

/// Players each controller's memory holds.
const RECENT: usize = 20;
/// The library's UTF-8 limit for the params string.
const MAX_TEXT_BYTES: usize = 255;
/// Feedback tasks open at once.
const MAX_TASKS: usize = 16;

/// The decoded arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Args {
    pub controller: u32,
    pub target: u64,
    pub target_flags: u32,
    /// The XDK `XONLINE_FEEDBACK_TYPE` value.
    pub kind: u32,
    /// The params block's string, when there is one.
    pub text: Option<String>,
    pub event: u32,
    pub out: u32,
}

/// Reads the arguments from the eight stack words `a`.
pub(crate) fn decode(a: &[u32], m: &Guest) -> Result<Args> {
    let text = match a[5] {
        0 => None,
        params => match m.u32(params)? {
            0 => None,
            p => Some(wide(m, p)?),
        },
    };
    Ok(Args {
        controller: a[0],
        target: u64::from(a[1]) | (u64::from(a[2]) << 32),
        target_flags: a[3],
        kind: a[4],
        text,
        event: a[6],
        out: a[7],
    })
}

/// A wide string, cut at the library's 255 UTF-8 bytes (on a character
/// boundary). A string with no terminator in reach faults on the guest.
fn wide(m: &Guest, p: u32) -> Result<String> {
    let mut units = Vec::new();
    for i in 0..(MAX_TEXT_BYTES as u32 + 1) {
        let v = m.u16(p.wrapping_add(i * 2))?;
        if v == 0 {
            break;
        }
        units.push(v);
    }
    let mut s = String::from_utf16_lossy(&units);
    let mut end = s.len().min(MAX_TEXT_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    Ok(s)
}

/// One controller's memory of the players it gave feedback on: XUID and a
/// bit per type below 8, oldest first.
#[derive(Default)]
struct Recent(VecDeque<(u64, u8)>);

impl Recent {
    fn bit(kind: u32) -> u8 {
        // As the library's byte shift: types 8 and up have no bit and are
        // always sent.
        if kind < 8 { 1 << kind } else { 0 }
    }
    /// Whether `kind` was already sent for `target`; marks it if not.
    fn seen_or_mark(&mut self, target: u64, kind: u32) -> bool {
        let bit = Self::bit(kind);
        if let Some((_, mask)) = self.0.iter_mut().find(|(x, _)| *x == target) {
            if *mask & bit != 0 {
                return true;
            }
            *mask |= bit;
            return false;
        }
        if self.0.len() == RECENT {
            self.0.pop_front();
        }
        self.0.push_back((target, bit));
        false
    }
    fn unmark(&mut self, target: u64, kind: u32) {
        let bit = Self::bit(kind);
        if let Some((_, mask)) = self.0.iter_mut().find(|(x, _)| *x == target) {
            *mask &= !bit;
        }
    }
}

struct Task {
    pending: Option<Pending>,
    status: u32,
    controller: usize,
    target: u64,
    kind: u32,
}

/// What a send needs from the session.
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    /// The signed-in players' XUIDs, by controller.
    pub users: &'a [u64; 4],
    /// The game this console is in, when the game said.
    pub game_id: Option<&'a str>,
}

#[derive(Default)]
pub(crate) struct Feedback {
    tasks: BTreeMap<u32, Task>,
    recent: [Recent; 4],
}

impl Feedback {
    pub fn clear(&mut self) {
        self.tasks.clear();
        self.recent = Default::default();
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }

    /// The task's status: 0 while the server has not answered.
    pub fn poll(&mut self, h: u32) -> Option<u32> {
        let t = self.tasks.get_mut(&h)?;
        if let Some(reply) = t.pending.as_ref().and_then(Pending::take) {
            t.pending = None;
            t.status = match reply {
                Ok(_) => TASK_SUCCESS,
                Err(e) => {
                    self.recent[t.controller].unmark(t.target, t.kind);
                    e
                }
            };
        }
        Some(t.status)
    }

    pub fn invoke(&mut self, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        if a[0] >= 4 {
            return Ok(INVALID_ARGUMENT);
        }
        let args = decode(a, m)?;
        if args.event != 0 {
            return Err(Error::Unverified("nonzero work event"));
        }
        m.check(args.out, 4, Access::Write, 1)?;
        let controller = args.controller as usize;
        let user = ctx.users[controller];
        if user == 0 {
            return Ok(NO_USER);
        }
        let Some(kind) = Kind::from_xdk(args.kind) else {
            // Types Halo 2 never sends, which the server has no name for.
            return Ok(INVALID_ARGUMENT);
        };
        let (Some(service), Some(session)) = (ctx.service, ctx.session) else {
            return Ok(NO_SESSION);
        };
        if self.tasks.len() >= MAX_TASKS {
            return Ok(TASK_BUSY);
        }
        let Some(h) = ctx.storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        let duplicate = self.recent[controller].seen_or_mark(args.target, args.kind);
        let pending = if duplicate {
            None
        } else {
            let reason = args.text.clone().unwrap_or_default();
            match service.report(
                session,
                user,
                live_proto::Xuid(args.target),
                kind,
                reason,
                ctx.game_id.map(str::to_owned),
            ) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.recent[controller].unmark(args.target, args.kind);
                    return Ok(e);
                }
            }
        };
        m.set_u32(args.out, h)?;
        self.tasks.insert(
            h,
            Task {
                status: if pending.is_some() { 0 } else { TASK_SUCCESS },
                pending,
                controller,
                target: args.target,
                kind: args.kind,
            },
        );
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const BASE: u32 = 0x10000;

    fn guest() -> Guest {
        let mut m = Guest::new();
        m.map_zeroed(BASE, 0x1000, Permissions::RW).unwrap();
        m
    }

    #[test]
    fn decodes_the_titles_call() {
        // As 0x0008d3d0 pushes it: controller 2, a XUID by value, type 3
        // (harassment), a params block whose string is null, no event.
        let mut m = guest();
        m.set_u32(BASE, 0).unwrap();
        let a = [2, 0x89ab_cdef, 0x0009_1234, 0x40, 3, BASE, 0, BASE + 0x10];
        assert_eq!(
            decode(&a, &m).ok(),
            Some(Args {
                controller: 2,
                target: 0x0009_1234_89ab_cdef,
                target_flags: 0x40,
                kind: 3,
                text: None,
                event: 0,
                out: BASE + 0x10,
            })
        );
        let mut none = a;
        none[5] = 0;
        assert_eq!(decode(&none, &m).ok().unwrap().text, None);
    }

    #[test]
    fn reads_the_params_string_up_to_the_librarys_limit() {
        let mut m = guest();
        let s: Vec<u8> = "Rude words"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        m.write_bytes(BASE + 0x100, &s, 1).unwrap();
        m.set_u32(BASE, BASE + 0x100).unwrap();
        let a = [0, 1, 0, 0, 0, BASE, 0, 0];
        assert_eq!(
            decode(&a, &m).ok().unwrap().text.as_deref(),
            Some("Rude words")
        );
        // 300 two-byte characters: cut at 255 bytes, on a boundary.
        let long: Vec<u8> = std::iter::repeat_n('é', 300)
            .map(|c| c as u16)
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        m.write_bytes(BASE + 0x200, &long, 1).unwrap();
        m.set_u32(BASE, BASE + 0x200).unwrap();
        let text = decode(&a, &m).ok().unwrap().text.unwrap();
        assert_eq!((text.len(), text.chars().count()), (254, 127));
        // A params pointer off the map faults.
        let bad = [0, 1, 0, 0, 0, 0x7000_0000, 0, 0];
        assert!(decode(&bad, &m).is_err());
    }

    #[test]
    fn remembers_twenty_players_and_types_below_eight() {
        let mut r = Recent::default();
        assert!(!r.seen_or_mark(1, 3));
        assert!(r.seen_or_mark(1, 3));
        assert!(!r.seen_or_mark(1, 2), "another type");
        for t in [10, 11, 12] {
            assert!(!r.seen_or_mark(1, t));
            assert!(!r.seen_or_mark(1, t), "no bit for type {t}");
        }
        r.unmark(1, 3);
        assert!(!r.seen_or_mark(1, 3), "a failed send is forgotten");
        for x in 2..=20 {
            assert!(!r.seen_or_mark(x, 0));
        }
        assert!(r.seen_or_mark(1, 2), "twenty players held");
        assert!(!r.seen_or_mark(21, 0));
        assert!(!r.seen_or_mark(1, 2), "the oldest fell out");
    }
}
