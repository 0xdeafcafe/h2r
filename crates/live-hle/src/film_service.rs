//! Local and dedicated boots observe asynchronous sign-in, title downloads,
//! matchmaking, and advertisement tasks.
//! The guest observes their return statuses, task handles, and output buffers.
//! Capture these at the XOnline ABI rather than starting a worker in Theater.
use crate::{Entry, Operation, state::Storage};
use film::tape::{Call, Tap};
use runtime::Guest;

// The match-search ABI accepts result tables up to 16 MiB. The title
// storage service allows 2 MiB and may write ancillary outputs too.
const MAX_OUTPUT_BYTES: usize = 16 << 20;
const ARENAS: [u32; 2] = [0x1_0000, 0x10_0000];

pub(crate) fn claims(g: &Guest) -> Vec<(u32, u32)> {
    g.virtual_claims()
        .iter()
        .filter(|c| c.owner == "Live HLE")
        .map(|c| (c.virtual_start, c.len))
        .collect()
}

/// Only the existing allocator chooses where to map. For a fresh seek the
/// guest snapshot will independently check every claim, including its ID.
pub(crate) fn add_claims(g: &mut Guest, new: &[(u32, u32)]) -> Result<(), String> {
    if claims(g).len() + new.len() > 8 {
        return Err("too many Live service arenas".into());
    }
    for &(address, size) in new {
        if !ARENAS.contains(&size) {
            return Err("invalid Live service arena size".into());
        }
        if Storage::with_size(size)
            .ensure(g)
            .map_err(|e| e.to_string())?
            != address
        {
            return Err("Live service arena address differs".into());
        }
    }
    Ok(())
}

/// Two XOnline calls hand over guest output buffers that can be filled on
/// a later TaskContinue/results call. Keep the grants in the player only
/// until the guest closes/finishes the task. A seek waits for those tasks.
#[derive(Default)]
pub(crate) struct Side {
    pub view: View,
    pending: std::collections::BTreeMap<u32, Vec<(u32, u32)>>,
}
/// Observable account/session facts used by guest-facing feature hooks.
#[derive(Clone, Default, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct View {
    pub users: [u64; 4],
    pub hosted: Option<String>,
    pub matchmade: Option<(u32, String)>,
    pub frames_from_host: bool,
    pub status: Option<String>,
    pub hoppers: crate::clankermatch::ClankerHoppers,
    pub clanker_hosted: Option<(String, crate::clankermatch::ClankerHopper)>,
}
impl View {
    pub fn read(state: &crate::state::State, keys: &[[u8; 8]]) -> Self {
        let matchmade = state.matchmade(keys);
        Self {
            users: std::array::from_fn(|i| state.user(i)),
            hosted: state.hosted_session(),
            frames_from_host: matchmade
                .as_ref()
                .is_some_and(|(_, id)| state.frames_from_host(id)),
            matchmade,
            status: state.matchmaking_status().map(str::to_owned),
            hoppers: state.clanker_hoppers(),
            clanker_hosted: state.clanker_hosted(),
        }
    }
}
impl Side {
    pub(crate) fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn returned(&mut self, g: &Guest, op: Operation, a: &[u32], result: u32) {
        use crate::storage_api::Op as FileOp;
        use Operation::*;
        let new = match op {
            MuteGet if result == 0 && a[1] as usize <= crate::mutes::MAX => {
                Some((a[3], vec![(a[4], a[1] * 16), (a[5], 4)]))
            }
            Storage(FileOp::DownloadMemory) if result == 0 => Some((a[7], vec![(a[3], a[4])])),
            Close if result == 0 => {
                self.pending.remove(&a[0]);
                None
            }
            Continue | Storage(FileOp::Progress) if result != 0 => {
                self.pending.remove(&a[0]);
                None
            }
            Storage(FileOp::DownloadResults) if result == 0 => {
                self.pending.remove(&a[0]);
                None
            }
            _ => None,
        };
        if let Some((out, spans)) = new
            && let Ok(h) = g.u32(out)
            && h != 0
        {
            self.pending.insert(h, spans);
        }
    }

    fn deferred(&self, op: Operation, a: &[u32], address: u32, size: usize) -> bool {
        use crate::storage_api::Op as FileOp;
        matches!(
            op,
            Operation::Continue | Operation::Storage(FileOp::Progress | FileOp::DownloadResults)
        ) && self.pending.get(&a[0]).is_some_and(|spans| {
            spans.iter().any(|&(base, len)| {
                base != 0
                    && address >= base
                    && u64::from(address - base) + size as u64 <= u64::from(len)
            })
        })
    }
}

// ABI output windows for the XOnline shim. No guest memory
// outside a service-owned allocation or a call's declared output is writable.
// Calls that only return a status need no entry. Newly supported service
// operations that write elsewhere must describe that output first.
fn output(g: &Guest, op: Operation, a: &[u32], address: u32, bytes: usize) -> bool {
    use crate::{arbitration, invites, outbox, sessions, stats, storage_api as files, teams, ui};
    use Operation::*;
    let fits = |i: usize, len: usize| {
        a.get(i)
            .filter(|base| **base != 0)
            .and_then(|base| address.checked_sub(*base))
            .is_some_and(|offset| {
                (offset as usize)
                    .checked_add(bytes)
                    .is_some_and(|n| n <= len)
            })
    };
    match op {
        WsaStartup => fits(1, 400),
        TitleAddress => fits(0, 36),
        QosLookup => fits(2, 4),
        GetUsers => fits(0, crate::USER_SIZE * 16) || fits(1, 4),
        Logon => fits(4, 4),
        ServiceInfo => fits(1, 12),
        ChangeUsers => fits(2, 4),
        ChangeResults => fits(1, 16),
        SaveLogonState => fits(0, 516),
        AcceptedInvite => fits(0, crate::invites::ACCEPTED_SIZE),
        Offering => fits(2, 4),
        FriendsStartup | MuteStartup => fits(1, 4),
        PresenceInit => fits(2, 4),
        PresenceLatest => fits(3, a[2].min(1000) as usize * 61),
        Ui(ui::Op::Notification) => fits(1, 9) || fits(2, 4),
        Ui(ui::Op::TitleName) => fits(3, a[2].min(4096) as usize * 2),
        Ui(ui::Op::FriendsLatest) => fits(2, a[1].min(100) as usize * crate::invites::FRIEND_SIZE),
        Ui(ui::Op::FriendsEnumerate) => fits(2, 4),
        Ui(ui::Op::Query) => fits(9, 4),
        Sessions(sessions::Op::Create | sessions::Op::Update) => {
            let i = if op == Sessions(sessions::Op::Create) {
                0
            } else {
                2
            };
            fits(i + 7, 4)
                || (bytes == 4
                    && a[i + 4] <= 64
                    && fits(i + 5, a[i + 4] as usize * 16)
                    && (address - a[i + 5]) % 16 == 4)
        }
        Sessions(sessions::Op::Info) => fits(1, 8) || fits(2, 16),
        Sessions(sessions::Op::Delete | sessions::Op::Find) => fits(3, 4),
        Sessions(sessions::Op::Search) => fits(6, 4),
        Sessions(sessions::Op::Results) => fits(1, 4) || fits(2, 4),
        Sessions(sessions::Op::Parse) => fits(3, a[1].min(64) as usize * 1602),
        Feedback => fits(7, 4),
        MuteGet => fits(3, 4) || fits(5, 4) || fits(4, (a[1] as usize).min(crate::mutes::MAX) * 16),
        Invites(invites::Op::Answer) => fits(4, 4),
        MessageEnumerate => fits(1, 64 * 1024) || fits(2, 4),
        MessageDetails => fits(5, 4),
        MessageProperty => fits(3, a[2].min(1 << 20) as usize) || fits(4, 4) || fits(5, 4),
        Outbox(
            outbox::Op::Create
            | outbox::Op::Send
            | outbox::Op::FriendRequest
            | outbox::Op::FriendRequestByName
            | outbox::Op::Invite
            | outbox::Op::Recruit
            | outbox::Op::RecruitByName,
        ) => fits(a.len() - 1, 4),
        Stats(stats::Op::Read | stats::Op::Write) => fits(3, 4),
        Stats(stats::Op::WriteResult) => (1..4).any(|i| fits(i, 4)),
        Stats(stats::Op::ReadResult) => crate::stats::film_output(g, a, address, bytes),
        Storage(files::Op::CreatePath) => {
            let units = g.u32(a[7]).unwrap_or(0).min(65536 + 80) as usize;
            fits(6, units * 2) || fits(7, 4)
        }
        Storage(files::Op::Progress) => fits(1, 4) || fits(2, 8) || fits(3, 8),
        Storage(files::Op::DownloadResults) => {
            (1..4).any(|i| fits(i, 4)) || fits(4, 8) || fits(5, 8)
        }
        Storage(op) if op.starts_task() => fits(a.len() - 1, 4),
        Teams(teams::Op::Details) => fits(4, crate::TEAM_SIZE),
        Teams(teams::Op::MemberDetails) => fits(4, crate::MEMBER_SIZE),
        Teams(teams::Op::Results | teams::Op::MemberResults) => fits(1, 4) || fits(2, 12 * 1024),
        Teams(op) if op.starts() => fits(a.len() - 1, 4),
        Arbitration(
            arbitration::Op::Register | arbitration::Op::Extend | arbitration::Op::Report,
        ) => fits(a.len() - 1, 4),
        Arbitration(arbitration::Op::CreateRoundId) => fits(0, 8),
        _ => false,
    }
}
fn allowed(g: &Guest, side: &Side, e: Entry, a: &[u32], p: u32, n: usize) -> bool {
    output(g, e.operation, a, p, n)
        || side.deferred(e.operation, a, p, n)
        || g.virtual_claims().iter().any(|c| {
            c.owner == "Live HLE"
                && p >= c.virtual_start
                && u64::from(p) + n as u64 <= u64::from(c.virtual_start) + u64::from(c.len)
        })
}

pub(crate) fn invoke(
    tap: &dyn Tap,
    e: Entry,
    a: &[u32],
    thread: u32,
    g: &mut Guest,
    side: &mut Side,
    run: impl FnOnce(&mut Guest) -> Result<(u32, View), crate::state::Error>,
) -> Result<u32, crate::state::Error> {
    if tap.replaying() {
        let fail = |why: String| {
            tap.fault(format!("{}: {why}", e.name));
            crate::state::Error::Unverified("invalid film service call")
        };
        let call = tap
            .next(e.name, a, thread)
            .ok_or_else(|| fail("missing film call".into()))?;
        let (arenas, body) = call
            .outputs
            .split_first()
            .ok_or_else(|| fail("missing service frame".into()))?;
        let (view_bytes, writes) = body
            .split_first()
            .ok_or_else(|| fail("missing session identities".into()))?;
        if view_bytes.len() > 128 << 10 {
            return Err(fail("invalid user identities".into()));
        }
        if arenas.len() > 8 * 8 || !arenas.len().is_multiple_of(8) || writes.len() > 4096 {
            return Err(fail("invalid service frame length".into()));
        }
        let claims: Vec<_> = arenas
            .chunks_exact(8)
            .map(|p| {
                (
                    u32::from_le_bytes(p[..4].try_into().unwrap()),
                    u32::from_le_bytes(p[4..].try_into().unwrap()),
                )
            })
            .collect();
        add_claims(g, &claims).map_err(fail)?;
        let mut total = 0usize;
        for write in writes {
            let (word, bytes) = write
                .split_at_checked(4)
                .ok_or_else(|| fail("short service write".into()))?;
            let address = u32::from_le_bytes(word.try_into().unwrap());
            total += bytes.len();
            if bytes.is_empty() || total > MAX_OUTPUT_BYTES || !allowed(g, side, e, a, address, bytes.len())
            {
                return Err(fail("write outside service ABI".into()));
            }
            g.write_bytes(address, bytes, 1)?;
        }
        let result = call
            .result
            .map_err(|_| fail("invalid service return".into()))?;
        if !view_bytes.is_empty() {
            side.view = serde_json::from_slice(view_bytes)
                .map_err(|e| fail(format!("invalid service observations: {e}")))?;
        }
        side.returned(g, e.operation, a, result);
        return Ok(result);
    }
    let start = g.virtual_claims().len();
    let (result, writes) = g.capture_hle_writes(MAX_OUTPUT_BYTES, run);
    let (value, view) = result?;
    {
        let Some(captured) = writes else {
            tap.fault(format!(
                "{} exceeded service output limits or changed unsupported guest state",
                e.name
            ));
            return Ok(value);
        };
        let Some(new_claims) = g.virtual_claims().get(start..) else {
            tap.fault(format!("{} removed guest claims", e.name));
            return Ok(value);
        };
        if new_claims.len() != captured.zeroed.len()
            || (!new_claims.is_empty()
                && g.virtual_claims().iter().filter(|c| c.owner == "Live HLE").count() > 8)
        {
            tap.fault(format!("{} made an unknown guest allocation", e.name));
            return Ok(value);
        }
        let mut arenas = Vec::new();
        for (c, (address, size, permissions)) in new_claims.iter().zip(captured.zeroed) {
            if c.owner != "Live HLE"
                || !ARENAS.contains(&size)
                || c.virtual_start != address
                || c.len != size
                || permissions != runtime::guest::Permissions::RW
            {
                tap.fault(format!("{} made an unknown guest allocation", e.name));
                return Ok(value);
            }
            arenas.extend(address.to_le_bytes());
            arenas.extend(size.to_le_bytes());
        }
        let view_bytes = if side.view == view {
            Vec::new()
        } else {
            serde_json::to_vec(&view).expect("serialisable service observations")
        };
        if view_bytes.len() > 128 << 10 {
            tap.fault("oversized Live service observations".into());
            return Ok(value);
        }
        let mut outputs = vec![arenas, view_bytes];
        for (address, bytes) in captured.writes {
            if !allowed(g, side, e, a, address, bytes.len()) {
                tap.fault(format!("{} wrote an unrecognised ABI output", e.name));
                return Ok(value);
            }
            let mut item = address.to_le_bytes().to_vec();
            item.extend(bytes);
            outputs.push(item);
        }
        tap.record(
            film::NETR,
            Call {
                index: 0,
                op: e.name.into(),
                args: a.into(),
                thread,
                result: Ok(value),
                outputs,
            },
        );
    }
    side.view = view;
    side.returned(g, e.operation, a, value);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct Tape {
        replay: bool,
        legacy: bool,
        calls: Mutex<VecDeque<Call>>,
    }
    impl Tap for Tape {
        fn replaying(&self) -> bool {
            self.replay
        }
        fn service_calls(&self) -> bool {
            !self.legacy
        }
        fn record(&self, _: film::Tag, call: Call) {
            self.calls.lock().unwrap().push_back(call)
        }
        fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call> {
            let call = self.calls.lock().unwrap().pop_front()?;
            assert_eq!(
                (call.op.as_str(), call.args.as_slice(), call.thread),
                (op, args, thread)
            );
            Some(call)
        }
        fn fault(&self, why: String) {
            panic!("{why}")
        }
    }
    fn entry(name: &str) -> Entry {
        *crate::entries::ENTRIES
            .iter()
            .find(|e| e.name == name)
            .unwrap()
    }
    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x4000, 4096, runtime::guest::Permissions::RW)
            .unwrap();
        g
    }

    #[test]
    fn idle_bot_polls_can_be_skipped_without_consuming_a_lobby_result() {
        let cfg = crate::NetworkConfig::new("127.0.0.1:3074".parse().unwrap());
        let recording = crate::Hle::with_network(Default::default(), cfg.clone()).unwrap();
        let tape = Arc::new(Tape::default());
        recording.set_film_tap(tape.clone());
        let session = "000000000000000a";
        assert_eq!(recording.bot_roster_hosted_session(), None);
        assert_eq!(recording.lobby_hosted_session(), None);
        assert_eq!(recording.bot_roster_hosted_session(), None);
        assert_eq!(recording.hosted_session(), None);
        assert!(!recording.hosts_session(session));
        let calls = std::mem::take(&mut *tape.calls.lock().unwrap());
        assert_eq!(calls.len(), 2);

        let replay = crate::Hle::for_film(cfg, true).unwrap();
        let tape = Arc::new(Tape {
            replay: true,
            calls: Mutex::new(calls),
            ..Default::default()
        });
        replay.set_film_tap(tape.clone());
        replay.film_side.lock().unwrap().view.hosted = Some(session.into());
        assert_eq!(replay.hosted_session().as_deref(), Some(session));
        assert_eq!(replay.lobby_hosted_session(), None);
        assert!(!replay.hosts_session(session));
        assert!(tape.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn legacy_bot_poll_consumes_the_original_network_call() {
        let cfg = crate::NetworkConfig::new("127.0.0.1:3074".parse().unwrap());
        let replay = crate::Hle::for_film(cfg, true).unwrap();
        let session = "000000000000000a";
        let tape = Arc::new(Tape {
            replay: true,
            legacy: true,
            calls: Mutex::new(VecDeque::from([Call {
                op: "HostedSession".into(),
                index: 0,
                thread: 0,
                args: vec![],
                result: Ok(0),
                outputs: vec![session.as_bytes().to_vec()],
            }])),
        });
        replay.set_film_tap(tape.clone());
        assert_eq!(replay.hosted_session(), None);
        assert_eq!(replay.bot_roster_hosted_session().as_deref(), Some(session));
        assert!(tape.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn asynchronous_service_results_and_new_heap_round_trip_without_a_worker() {
        let tape = Tape::default();
        let mut recording = guest();
        let task = entry("XOnlineOfferingIsNewContentAvailable");
        let args = [0, 0, 0x4000];
        let mut heap = Storage::default();
        let handle = invoke(
            &tape,
            task,
            &args,
            7,
            &mut recording,
            &mut Side::default(),
            |g| {
                let handle = heap.alloc(g, 8).unwrap();
                g.write_bytes_from(0x1234, handle, &42_u32.to_le_bytes(), 4)?;
                g.set_u32(args[2], handle)?;
                Ok((
                    crate::status::RUNNING_IDLE,
                    View {
                        users: [5, 0, 0, 0],
                        ..View::default()
                    },
                ))
            },
        );
        assert_eq!(handle.ok(), Some(crate::status::RUNNING_IDLE));
        let tape = Tape {
            replay: true,
            ..tape
        };
        let mut replay = guest();
        let mut side = Side::default();
        let status = invoke(&tape, task, &args, 7, &mut replay, &mut side, |_| {
            panic!("ran service")
        });
        assert_eq!(status.ok(), Some(crate::status::RUNNING_IDLE));
        assert_eq!(replay.layout(), recording.layout());
        let handle = replay.u32(args[2]).unwrap();
        assert_eq!(handle, recording.u32(args[2]).unwrap());
        assert_eq!(replay.u32(handle).unwrap(), 42);
        assert_eq!(side.view.users, [5, 0, 0, 0]);
        assert!(!allowed(&replay, &side, task, &args, 0x4004, 4));
    }

    #[test]
    #[should_panic(expected = "made an unknown guest allocation")]
    fn unclaimed_service_mappings_fail_recording() {
        let task = entry("XOnlineOfferingIsNewContentAvailable");
        let _ = invoke(
            &Tape::default(),
            task,
            &[0, 0, 0x4000],
            7,
            &mut guest(),
            &mut Side::default(),
            |g| {
                g.map_zeroed(0xa000_0000, ARENAS[0], runtime::guest::Permissions::RW).unwrap();
                Ok((0, View::default()))
            },
        );
    }

    #[test]
    fn borrowed_download_buffer_is_scoped_to_its_task_and_lifetime() {
        let mut g = guest();
        let handle = 161;
        g.set_u32(0x4000, handle).unwrap();
        let mut side = Side::default();
        let file = entry("XOnlineStorageDownloadToMemory");
        let poll = entry("XOnlineTaskContinue");
        let args = [3, 0, 0, 0x4100, 32, 0, 0, 0x4000];
        side.returned(&g, file.operation, &args, 0);
        assert!(allowed(&g, &side, poll, &[handle], 0x4100, 32));
        assert!(!allowed(&g, &side, poll, &[handle + 1], 0x4100, 32));
        assert!(!allowed(&g, &side, poll, &[handle], 0x4100, 33));
        assert!(side.pending());
        side.returned(&g, poll.operation, &[handle], crate::status::TASK_SUCCESS);
        assert!(!side.pending());
        assert!(!allowed(&g, &side, poll, &[handle], 0x4100, 1));
    }

    #[test]
    fn only_isolated_theater_can_adopt_the_recorded_network_allocator() {
        let network = crate::NetworkConfig::new("127.0.0.1:3074".parse().unwrap());
        let record = crate::Hle::for_film(network.clone(), true).unwrap();
        record.set_film_tap(Arc::new(Tape::default()));
        let mut g = guest();
        Storage::default().alloc(&mut g, 8).unwrap();
        let bytes = record.save_film_network(&g).unwrap();
        let mut restored = guest();
        assert!(record.load_film_network(&bytes, &mut restored).is_err());
        let replay = crate::Hle::for_film(network, true).unwrap();
        replay.set_film_tap(Arc::new(Tape {
            replay: true,
            ..Default::default()
        }));
        replay.load_film_network(&bytes, &mut restored).unwrap();
        assert_eq!(g.virtual_claims(), restored.virtual_claims());
    }
}
