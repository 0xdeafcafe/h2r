//! Join in progress over the retail synchronous join (docs/live/live-coop.md
//! section 14): a friend joins a running co-op campaign and nobody reverts.
//!
//! The retail engine already carries the whole path (14.1): the host's
//! session admits a peer, its sync-server compresses the game-state block
//! and queues a synchronous-join and the state in `0x400` byte pieces ahead
//! of the client's updates (`SIM_JOIN_SEND_STATE`, `0x862e0`); the client
//! takes the join (`SIM_JOIN_RECEIVE`, `0x85dc0`), decompresses the pieces
//! straight into the game-state block (`SIM_STATE_RECEIVED`, `0x6aa20`) and
//! runs from the next update, in which the new player arrives as the
//! update's player-set change (`+0xDD0`, applied by `SIM_UPDATE_APPLY`).
//!
//! What stops it is the host's join check (`SESSION_JOIN_CHECK`, `0x62fe0`):
//! it refuses (code 3) unless the session is in state 5, the lobby. Here,
//! on the host of a co-op campaign with `coop.join_in_progress` on, a check
//! made while the game runs (states 6 to 8) sees state 5 for the length of
//! the call, so the rest of retail's check (privacy, the peer limit) still
//! decides. Everything else is observed only, one `coop-jip:` line per
//! stage, which `h2 coop test`'s checklist reads.
//!
//! Each part can be turned off without a rebuild: `coop.join_in_progress`
//! (`H2_COOP_JOIN_IN_PROGRESS=0`) or `H2_COOP_JIP_LIFT=0` keeps retail's
//! check; `H2_COOP_JIP_LOG=0` drops the stage lines; `H2_FEATURES=-coop-jip`
//! drops the feature.
//!
//! The protocol of our own (`coop::join`) is the fallback if the retail
//! client side does not hold up on a run (14.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::points;
use hooks::Flow;

use crate::coop_probe::{Arm, Session, layout, read_session, read_session_at};
use crate::layout_glue::Gm;

/// The feature id (`H2_FEATURES=-coop-jip` turns it off).
pub const FEATURE: &str = "coop-jip";

/// The session state the join check accepts (the lobby).
pub const LOBBY: u32 = 5;
/// The states of a running game the check refuses with 3. [guess] 6 is
/// the start, 7 and 8 in game; all three are past the lobby.
pub const RUNNING: std::ops::RangeInclusive<u32> = 6..=8;

/// Guest layouts read here (docs/live/live-coop.md 14.1).
pub mod at {
    /// The players' globals pointer (`0x4e8c20`): the player-set mask at
    /// `+0x2C`.
    pub const PLAYERS_GLOBALS: u32 = 0x004e_8c20;
    pub const PLAYER_MASK: u32 = 0x2c;
    /// In an update record: the update number, the player-set change flag
    /// (byte) and its mask.
    pub const RECORD_UPDATE: u32 = 0;
    pub const RECORD_SET_CHANGED: u32 = 0xdd0;
    pub const RECORD_SET_MASK: u32 = 0xdd4;
    /// In a server's client view: the join attempts so far.
    pub const VIEW_ATTEMPTS: u32 = 0x90;
}

/// The host's `coop.join_in_progress`, set once settings are read.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// The parts that can be turned off by environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stages {
    /// Let the join check through while the game runs.
    pub lift: bool,
    /// Print the stage lines.
    pub log: bool,
}

impl Stages {
    /// `H2_COOP_JIP_LIFT` and `H2_COOP_JIP_LOG`: `0`, `off` or `false`
    /// turn a part off; anything else (or unset) leaves it on.
    pub fn parse(env: impl Fn(&str) -> Option<String>) -> Stages {
        let on = |k: &str| {
            !env(k).is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "off" | "false" | "no"
                )
            })
        };
        Stages {
            lift: on("H2_COOP_JIP_LIFT"),
            log: on("H2_COOP_JIP_LOG"),
        }
    }
}

/// What the join check's `before` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lift {
    /// Retail's check, untouched; why.
    Retail(&'static str),
    /// Run retail's check with the session in the lobby state, then put
    /// `real` back.
    AsLobby { real: u32 },
}

/// The decision for a join request on session `s`, with join in progress
/// `on` and a co-op limit of `limit` players.
pub fn lift(s: &Session, on: bool, stages: Stages, limit: u32) -> Lift {
    if !on {
        return Lift::Retail("coop.join_in_progress is off");
    }
    if !stages.lift {
        return Lift::Retail("H2_COOP_JIP_LIFT is off");
    }
    if !s.is_campaign() {
        return Lift::Retail("not a campaign session");
    }
    if !s.is_host() {
        return Lift::Retail("not the host");
    }
    if !RUNNING.contains(&s.state) {
        return Lift::Retail("not a running game");
    }
    if s.players >= limit {
        return Lift::Retail("the game is full");
    }
    Lift::AsLobby { real: s.state }
}

/// Why retail's check said `code` (`0x62fe0`).
pub fn refusal(code: u32) -> &'static str {
    match code {
        0 => "accepted",
        2 => "the session is invite only",
        3 => "the session is not taking joins in this state",
        4 => "no room for the console or its players",
        _ => "refused",
    }
}

/// The join check with the session read as in the lobby: state 5 for the
/// length of retail's call, `real` back after it, whatever it answered.
pub async fn check_as_lobby(
    cx: Cx,
    session: u32,
    request: u32,
    real: u32,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    let state = session.wrapping_add(layout::STATE);
    cx.write(|m| m.set_u32(state, LOBBY))
        .map_err(EntryError::Fault)?;
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(
            GuestCall::new(points::SESSION_JOIN_CHECK.base)
                .eax(session)
                .edi(request),
        )
        .await;
    reentry.store(false, Ordering::SeqCst);
    cx.write(|m| m.set_u32(state, real))
        .map_err(EntryError::Fault)?;
    let code = r?.eax;
    say(&lift_line(real, code));
    Ok(Ret::new(code, 0))
}

pub fn lift_line(real: u32, code: u32) -> String {
    if code == 0 {
        format!(
            "coop-jip: host: a console joins the running game (session state {real} read as the lobby's for the join check)"
        )
    } else {
        format!(
            "coop-jip: host: a console asked to join the running game; still refused ({code}: {})",
            refusal(code)
        )
    }
}

/// What the stages have seen on this console, and the lines they make.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// When the host last queued the state for a joiner (ms).
    sent_at: Option<u64>,
    /// This console restored a state from the host.
    restored: bool,
    /// ... and has said it joined.
    joined: bool,
}

impl Track {
    /// The host starts sending the state; `attempts` before this one.
    pub fn sending(&mut self, now: u64, attempts: u32) -> String {
        self.sent_at = Some(now);
        format!(
            "coop-jip: host: sending the game state to a joining console (attempt {})",
            attempts.wrapping_add(1)
        )
    }

    pub fn sent(&mut self, queued: bool) -> String {
        if queued {
            "coop-jip: host: game state queued ahead of the joiner's updates".into()
        } else {
            self.sent_at = None;
            "coop-jip: host: could not queue the game state; the game goes on without the joiner"
                .into()
        }
    }

    pub fn join_message(&mut self, update: u32) -> String {
        self.restored = false;
        self.joined = false;
        format!("coop-jip: guest: join message; the host's game is at update {update}")
    }

    /// The client's last piece: the state restored (`ok`) or not, from
    /// `length` compressed bytes.
    pub fn restored(&mut self, ok: bool, length: u32) -> String {
        self.restored = ok;
        if ok {
            format!(
                "coop-jip: guest: game state restored ({} KiB compressed)",
                length.div_ceil(1024)
            )
        } else {
            "coop-jip: back to the lobby: the game state did not restore".into()
        }
    }

    /// An update applied at update number `update`: the players' mask
    /// before it (`before`) and its player-set change, if any.
    pub fn applied(
        &mut self,
        now: u64,
        host: bool,
        update: u32,
        before: u32,
        change: Option<u32>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let after = change.unwrap_or(before) & 0xffff;
        if self.restored && !self.joined {
            self.joined = true;
            out.push(joined_line(update, after));
        }
        if host && let (Some(at), Some(mask)) = (self.sent_at, change) {
            let new = mask & !before & 0xffff;
            if new != 0 {
                self.sent_at = None;
                for i in (0..16).filter(|i| new & (1 << i) != 0) {
                    out.push(format!(
                        "coop-jip: admitted player {} at tick {update} ({} ms after the state went out)",
                        i + 1,
                        now.saturating_sub(at)
                    ));
                }
            }
        }
        out
    }
}

/// The newcomer's line (`coop::join::joined_line`'s wording): the
/// players in the game after the update, the newcomer being the last.
/// [guess] The newcomer's slot is the highest; the count is what is known.
pub fn joined_line(update: u32, mask: u32) -> String {
    format!(
        "coop-jip: joined in progress at tick {update} as player {}",
        mask.count_ones().max(1)
    )
}

fn say(line: &str) {
    eprintln!("{line}");
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn lock(t: &Mutex<Track>) -> std::sync::MutexGuard<'_, Track> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every point the feature hooks.
pub static HOOKED: &[&points::Point] = &[
    &points::SESSION_JOIN_CHECK,
    &points::SIM_JOIN_SEND_STATE,
    &points::SIM_JOIN_RECEIVE,
    &points::SIM_STATE_RECEIVED,
    &points::SIM_UPDATE_APPLY,
];

pub struct CoopJip {
    arm: Arc<Arm>,
    stages: Stages,
    track: Arc<Mutex<Track>>,
}

impl CoopJip {
    pub fn new(arm: Arc<Arm>, stages: Stages) -> CoopJip {
        CoopJip {
            arm,
            stages,
            track: Arc::default(),
        }
    }
}

impl hooks::Feature for CoopJip {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Co-op over Live: a friend joins a running campaign through the retail synchronous join",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let stages = self.stages;
        // docs/runtime/snapshots.md: armed, a join is a conversation with
        // a peer no snapshot holds; disarmed, the track only shapes log
        // lines and `ENABLED` is the settings' value read at boot. The
        // lifted check's call is a continuation the machine check sees.
        let arm = Arc::clone(&self.arm);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || arm.snapshot_live()));

        if stages.lift {
            let arm = Arc::clone(&self.arm);
            let reentry = Arc::new(AtomicBool::new(false));
            b.raw()
                .before(
                    &points::SESSION_JOIN_CHECK,
                    "join in progress: the join check while the game runs",
                    move |c| {
                        if reentry.load(Ordering::SeqCst) || !arm.active() {
                            return Ok(Flow::Continue);
                        }
                        let rt = Arc::clone(c.ctx.runtime());
                        let (session, request) = (c.cpu.eax, c.cpu.edi);
                        let decision = {
                            let m = rt.memory();
                            match read_session_at(&*m, session) {
                                Some(s) => lift(&s, enabled(), stages, arm.limit_for(&s)),
                                None => Lift::Retail("no session"),
                            }
                        };
                        let Lift::AsLobby { real } = decision else {
                            return Ok(Flow::Continue);
                        };
                        let reentry = Arc::clone(&reentry);
                        c.answer_async(move |cx| -> EntryFuture {
                            Box::pin(check_as_lobby(cx, session, request, real, reentry))
                        })
                    },
                )
                .optional();
        }

        if !stages.log {
            return Ok(());
        }
        {
            let (arm, track) = (Arc::clone(&self.arm), Arc::clone(&self.track));
            b.raw()
                .observe_before(
                    &points::SIM_JOIN_SEND_STATE,
                    "join in progress: host sends the state",
                    move |o| {
                        if !arm.active() {
                            return;
                        }
                        let attempts = o
                            .guest()
                            .u32(o.cpu.eax.wrapping_add(at::VIEW_ATTEMPTS))
                            .unwrap_or(0);
                        say(&lock(&track).sending(now_ms(), attempts));
                    },
                )
                .optional();
        }
        {
            let (arm, track) = (Arc::clone(&self.arm), Arc::clone(&self.track));
            b.raw()
                .observe_after(
                    &points::SIM_JOIN_SEND_STATE,
                    "join in progress: state queued",
                    move |o| {
                        if arm.active() {
                            say(&lock(&track).sent(o.cpu.eax & 0xff != 0));
                        }
                    },
                )
                .optional();
        }
        {
            let (arm, track) = (Arc::clone(&self.arm), Arc::clone(&self.track));
            b.raw()
                .observe_before(
                    &points::SIM_JOIN_RECEIVE,
                    "join in progress: guest takes the join",
                    move |o| {
                        if arm.active() {
                            let update = o.arg_u32("update").unwrap_or(0);
                            say(&lock(&track).join_message(update));
                        }
                    },
                )
                .optional();
        }
        {
            // The length is a stack argument: read it before, report after.
            let length = Arc::new(Mutex::new(0u32));
            let (arm, l) = (Arc::clone(&self.arm), Arc::clone(&length));
            b.raw()
                .observe_before(
                    &points::SIM_STATE_RECEIVED,
                    "join in progress: last state piece",
                    move |o| {
                        if arm.active() {
                            *l.lock().unwrap_or_else(|e| e.into_inner()) =
                                o.arg_u32("length").unwrap_or(0);
                        }
                    },
                )
                .optional();
            let (arm, track) = (Arc::clone(&self.arm), Arc::clone(&self.track));
            b.raw()
                .observe_after(
                    &points::SIM_STATE_RECEIVED,
                    "join in progress: state restored",
                    move |o| {
                        if arm.active() {
                            let n = *length.lock().unwrap_or_else(|e| e.into_inner());
                            say(&lock(&track).restored(o.cpu.eax & 0xff != 0, n));
                        }
                    },
                )
                .optional();
        }
        {
            let (arm, track) = (Arc::clone(&self.arm), Arc::clone(&self.track));
            b.raw()
                .observe_before(
                    &points::SIM_UPDATE_APPLY,
                    "join in progress: player-set changes",
                    move |o| {
                        if !arm.active() {
                            return;
                        }
                        let lines = {
                            let m = o.guest();
                            let g: &runtime::Guest = &m;
                            let Some(record) = Gm::u32(g, o.cpu.esp.wrapping_add(4)) else {
                                return;
                            };
                            let update =
                                Gm::u32(g, record.wrapping_add(at::RECORD_UPDATE)).unwrap_or(0);
                            let change = (Gm::u8(g, record.wrapping_add(at::RECORD_SET_CHANGED))
                                == Some(1))
                            .then(|| Gm::u32(g, record.wrapping_add(at::RECORD_SET_MASK)))
                            .flatten();
                            let before = Gm::u32(g, at::PLAYERS_GLOBALS)
                                .and_then(|p| Gm::u32(g, p.wrapping_add(at::PLAYER_MASK)))
                                .unwrap_or(0);
                            let host = read_session(g).is_some_and(|s| s.is_host());
                            lock(&track).applied(now_ms(), host, update, before, change)
                        };
                        for l in lines {
                            say(&l);
                        }
                    },
                )
                .optional();
        }
        Ok(())
    }
}

/// Adds the feature on the co-op over Live arm; the stages from the
/// environment.
pub fn install(registry: &mut hooks::Registry, arm: Arc<Arm>) -> anyhow::Result<bool> {
    let stages = Stages::parse(|k| std::env::var(k).ok());
    registry
        .add(&CoopJip::new(arm, stages))
        .map_err(|e| anyhow::anyhow!("coop-jip: {e}"))
}

#[cfg(test)]
mod tests;
