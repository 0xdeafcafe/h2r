//! Skulls in multiplayer games (docs/game/skulls.md "In multiplayer").
//!
//! **Why only two.** Every reader of the skull array (`0x4f55dc`) tests the
//! active game options for a campaign (`[[0x4e6948]+8] == 1`) before it
//! loads its byte, so in a multiplayer game no skull does anything, whatever
//! the array holds. Most skulls change AI, character or campaign data that a
//! multiplayer game does not have. Two have a reader we can reproduce
//! exactly at a function boundary, without the campaign test:
//!
//! - **Blind** (6): `0x13939b`, whether the HUD draws, returns 0 with the
//!   skull on; `0x165f16`, one local user's first-person weapons, returns 0
//!   having drawn nothing. Our `before` hooks answer the same way. Local:
//!   each machine hides its own HUD.
//! - **Famine** (4): `0x0fda00` makes a weapon from its scenario placement
//!   and, with the skull on, halves the placement's rounds (`+0x4c`, and
//!   `+0x4e`, which it reads but does not use) before passing them on. Our
//!   `before` halves both in the placement for the length of the call and
//!   the `after` puts them back. Simulation: weapons are objects the host
//!   decides, so the set must be the host's on every machine.
//!
//! **Whose set.** The host's: the union of the "on" multiplayer skulls of
//! the profiles signed in on the host ([`chosen`]), each only if that
//! profile has collected it in the campaign (the rule, not a setting). A
//! guest applies
//! what its host advertised on our Live service ([`at_start`]); with
//! nothing heard (System Link, no server) it applies none.
//!
//! **When.** A `before` on game activation (`0x18f000`, options in EAX)
//! makes the set active for a multiplayer game and clears it for anything
//! else, so a campaign never sees these hooks act (its own skull code runs
//! there).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use settings::{Apply, Def, Kind, Menu, Scope, Store, Tier, Value};

use crate::coop_probe::Session;
use crate::layout_glue::Gm;
use crate::skulls::{self, ABOUT, SKULLS, State};

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "skulls-mp";
/// The options mode of a multiplayer game.
pub const MODE_MULTIPLAYER: u32 = 2;
/// How long a guest waits at game start for the server's answer.
pub const HOST_WAIT: Duration = Duration::from_millis(1500);

/// What a multiplayer skull changes, for whom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Only what this machine draws: every machine applies its own.
    Local,
    /// What the simulation computes: the host decides; every machine
    /// applies the host's set so prediction agrees.
    Simulation,
}

/// A skull offered in multiplayer.
#[derive(Debug)]
pub struct MpSkull {
    /// The engine's index (`SKULLS[index]`).
    pub index: usize,
    /// The profile's multiplayer on/off (`skulls.mp_<name>`).
    pub def: &'static Def,
    pub reach: Reach,
    /// What it does in a multiplayer game, in our words.
    pub effect: &'static str,
}

const fn mp_def(key: &'static str, label: &'static str, order: u32) -> Def {
    Def {
        key,
        label,
        help: "On, this skull is on in the multiplayer games this profile hosts. Kept per \
               profile under [profiles.\"<name>\".skulls].",
        kind: Kind::Bool,
        default: Value::Bool(false),
        env: None,
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::NextMap,
        scope: Scope::Sim,
        tier: Tier::Advanced,
        menu: Some(Menu {
            screen: skulls::SCREEN,
            order,
        }),
    }
}

pub static MP_FAMINE: Def = mp_def("skulls.mp_famine", "Famine (multiplayer)", 104);
pub static MP_BLIND: Def = mp_def("skulls.mp_blind", "Blind (multiplayer)", 106);
pub static MP_LOW_GRAVITY: Def = mp_def("skulls.mp_low_gravity", "Low Gravity (multiplayer)", 116);

/// The skulls offered in multiplayer, in the engine's order:
/// `live_proto::skulls::MULTIPLAYER`.
pub static MP: [MpSkull; 3] = [
    MpSkull {
        index: 4,
        def: &MP_FAMINE,
        reach: Reach::Simulation,
        effect: "Weapons placed on the map start with half their ammo.",
    },
    MpSkull {
        index: 6,
        def: &MP_BLIND,
        reach: Reach::Local,
        effect: "Every player's HUD and first-person weapon are hidden.",
    },
    // Ours (crate::skulls_custom): the physics constants' gravity, which
    // every machine simulates with, so the host's choice is everyone's.
    MpSkull {
        index: crate::skulls_custom::LOW_GRAVITY,
        def: &MP_LOW_GRAVITY,
        reach: Reach::Simulation,
        effect: "Gravity is halved for everyone.",
    },
];

pub const FAMINE: usize = 4;
pub const BLIND: usize = 6;
/// Black Eye's engine index: a Firefight skull, shared by every machine.
pub const BLACK_EYE: usize = 8;

/// The row of skull `index` on the multiplayer screen.
pub fn row_of(index: usize) -> Option<usize> {
    MP.iter().position(|m| m.index == index)
}

/// Row `row`'s state for `profile`: Locked until the profile has collected
/// the skull in the campaign (a host can only turn on what it has found),
/// else the profile's multiplayer On or Off (Off until set). A value left
/// in the store for a skull not collected stays Locked and is not applied.
pub fn state(store: &Store, profile: &str, row: usize) -> Option<State> {
    let m = MP.get(row)?;
    if skulls::state(store, profile, m.index)? == State::Locked {
        return Some(State::Locked);
    }
    Some(
        match store
            .profile_get(profile, m.def.key)
            .and_then(Value::as_bool)
        {
            Some(true) => State::On,
            _ => State::Off,
        },
    )
}

/// A press on row `row` of `profile`'s multiplayer screen: flips it, or
/// says why not. Returns what to log.
pub fn press(store: &Store, profile: &str, row: usize) -> Option<String> {
    let m = MP.get(row)?;
    let (def, about) = (&SKULLS[m.index], &ABOUT[m.index]);
    Some(match state(store, profile, row)? {
        State::Locked => format!(
            "skulls menu: {} is locked for {profile} in multiplayer: not collected yet (found on {}{})",
            def.label,
            about.level,
            if about.legendary {
                ", Legendary only"
            } else {
                ""
            }
        ),
        s => {
            let now = s != State::On;
            let on = if now { "on" } else { "off" };
            match store.profile_set(profile, m.def.key, Value::Bool(now)) {
                Ok(None) => format!("skulls: {} {on} in multiplayer for {profile}", def.label),
                Ok(Some(why)) => format!(
                    "skulls: {} {on} in multiplayer for {profile}; not saved: {why}",
                    def.label
                ),
                Err(e) => format!("skulls: {}: {e}", def.label),
            }
        }
    })
}

/// The skulls `profile` has on for multiplayer (engine indices, ascending).
pub fn chosen(store: &Store, profile: &str) -> Vec<u8> {
    (0..MP.len())
        .filter(|r| state(store, profile, *r) == Some(State::On))
        .map(|r| MP[r].index as u8)
        .collect()
}

/// The union of `sets`, ascending.
pub fn union<'a>(sets: impl IntoIterator<Item = &'a Vec<u8>>) -> Vec<u8> {
    live_proto::skulls::normalize(sets.into_iter().flatten().copied()).unwrap_or_default()
}

/// Names of a set, for a log line.
pub fn names(set: &[u8]) -> String {
    if set.is_empty() {
        return "none".into();
    }
    set.iter()
        .filter_map(|i| SKULLS.get(*i as usize).map(|d| d.label))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---- who decides ----

/// This machine's part in the game about to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// No networked session, or the only machine in it (split-screen).
    Alone,
    /// Hosting `peers` machines.
    Host { peers: u32 },
    /// Joined someone else's; `class` the squad session's network class
    /// (`crate::variant_ext::SESSION_NETWORK_CLASS`): 1 System Link, 2 Live.
    Guest { class: u32 },
}

/// The role the squad session says, when there is one in a lobby or game
/// state (`coop_probe::read_session`), with `class` its network class at
/// `+0x18` (not `Session::class`, the `+0x4980` stage, which is 1 in
/// every lobby).
pub fn role(session: Option<&Session>, class: Option<u32>) -> Role {
    match session {
        Some(s) if s.live() && s.peers > 1 => {
            if s.is_host() {
                Role::Host { peers: s.peers }
            } else {
                Role::Guest {
                    class: class.unwrap_or(0),
                }
            }
        }
        _ => Role::Alone,
    }
}

/// [`role`] of the squad session in `m` now.
pub fn role_in<M: Gm + ?Sized>(m: &M) -> Role {
    let session = crate::coop_probe::read_session(m);
    let class = session
        .as_ref()
        .and_then(|s| m.u32(s.addr.wrapping_add(crate::variant_ext::SESSION_NETWORK_CLASS)));
    role(session.as_ref(), class)
}

/// What a guest heard of its host's skulls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// The host's advertised set; `fresh` when read from the server just
    /// now rather than at the join.
    Set { skulls: Vec<u8>, fresh: bool },
    /// Nothing: why.
    Nothing(String),
}

impl Heard {
    /// From the HLE's answer.
    pub fn from_hle(h: live_hle::HostSkulls) -> Self {
        match h {
            live_hle::HostSkulls::Known {
                skulls, refreshed, ..
            } => Heard::Set {
                skulls,
                fresh: refreshed,
            },
            live_hle::HostSkulls::Unknown { session } => Heard::Nothing(format!(
                "session {session} was never read from the server (a System Link game, or the \
                 server did not answer)"
            )),
            live_hle::HostSkulls::NotJoined => {
                Heard::Nothing("no joined Live session is known to the HLE".into())
            }
        }
    }
}

/// The set a multiplayer start applies on this machine and the line that
/// says so. `own` is this machine's profiles' choice; `hear` asks for the
/// host's, only on a guest.
pub fn at_start(role: &Role, own: &[u8], hear: impl FnOnce() -> Heard) -> (Vec<u8>, String) {
    match role {
        Role::Alone => (
            own.to_vec(),
            format!("skulls-mp: multiplayer start (local): {}", names(own)),
        ),
        Role::Host { peers } => (
            own.to_vec(),
            format!(
                "skulls-mp: multiplayer start as host of {peers} machines: {} (advertised to guests)",
                names(own)
            ),
        ),
        Role::Guest { class } => match hear() {
            Heard::Set { skulls, fresh } => {
                let set: Vec<u8> = skulls
                    .into_iter()
                    .filter(|s| live_proto::skulls::MULTIPLAYER.contains(s))
                    .collect();
                let set = live_proto::skulls::normalize(set).unwrap_or_default();
                (
                    set.clone(),
                    format!(
                        "skulls-mp: multiplayer start as a guest: the host's skulls, {}: {}",
                        if fresh {
                            "read from the server now"
                        } else {
                            "as advertised when we joined (the server did not answer now)"
                        },
                        names(&set)
                    ),
                )
            }
            Heard::Nothing(why) => (
                Vec::new(),
                format!(
                    "skulls-mp: multiplayer start as a guest (session class {class}): the host's \
                     skulls are unknown ({why}); none here. The host still applies its own: \
                     Famine to its weapons, which it decides for everyone"
                ),
            ),
        },
    }
}

// ---- Famine ----

/// The placement's rounds `0x0fda00` reads (`+0x4c` is the one it uses).
pub const PLACEMENT_ROUNDS: [u32; 2] = [0x4c, 0x4e];

/// Famine's halving: `cdq; sub eax, edx; sar eax, 1` on the sign-extended
/// short, a division by two towards zero.
pub fn famine(rounds: i16) -> i16 {
    rounds / 2
}

/// Halves the placement's rounds for one call; the originals to put back.
pub fn halve_placement<M: Gm>(m: &mut M, placement: u32) -> Option<[u16; 2]> {
    let old = PLACEMENT_ROUNDS.map(|o| m.u16(placement.wrapping_add(o)));
    let old = [old[0]?, old[1]?];
    for (o, v) in PLACEMENT_ROUNDS.iter().zip(old) {
        if !m.set_u16(placement.wrapping_add(*o), famine(v as i16) as u16) {
            return None;
        }
    }
    Some(old)
}

/// Puts back what [`halve_placement`] changed.
pub fn restore_placement<M: Gm>(m: &mut M, placement: u32, old: [u16; 2]) -> bool {
    PLACEMENT_ROUNDS
        .iter()
        .zip(old)
        .all(|(o, v)| m.set_u16(placement.wrapping_add(*o), v))
}

// ---- Firefight ----

/// The engine's skull array: one byte per skull, 1 = on
/// (docs/game/skulls.md "What the game does").
pub const SKULL_ARRAY: u32 = 0x4f55dc;

/// Skulls whose readers only the actors reach: Assassins, Thunderstorm,
/// Famine (AI weapon drops), Ghost, Catch, Mythic and Angry. Every reader
/// tests the campaign gate first; on a Firefight host the feature's hook at
/// each reader's gate (`points::skull_gates`, [`gate_resume`]) answers it
/// "on" for the skulls of [`firefight_ai`], and the native active
/// camouflage asks `engine::skull_gate`. The host alone runs actors.
pub const FIREFIGHT_AI_SKULLS: [usize; 7] = [2, 3, 4, 7, 9, 12, 13];

/// Skulls that change what a machine draws or simulates without actors,
/// through the multiplayer set above: Famine (placed weapons), Black Eye and
/// Low Gravity.
pub const FIREFIGHT_SHARED_SKULLS: [usize; 3] = [FAMINE, BLACK_EYE, crate::skulls_custom::LOW_GRAVITY];

/// Shared skulls whose reader every machine runs, actors or not, so every
/// machine answers the reader's campaign gate and holds the skull array's
/// byte for them, whatever its role: Black Eye (shield recharge). The
/// director's `Skull` entry reaches every machine that applies entries
/// (the host, a synchronous peer, a film), and a distributed client takes
/// the host's mode state, so host and guest agree on the byte.
pub const FIREFIGHT_SIM_SKULLS: [usize; 1] = [BLACK_EYE];

/// What a Firefight skull does on a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirefightScope {
    /// The host's actors only: set in the skull array of the machine that
    /// runs AI, and nowhere else.
    HostAi,
    /// Both: the actors' reader on the host (a drop's rounds) and the
    /// multiplayer reader on every machine (a placed weapon's rounds).
    HostAiAndShared(Reach),
    /// The multiplayer path: Black Eye and Low Gravity for everyone.
    Shared(Reach),
    /// A director rule: no engine byte on any machine.
    Rule,
}

/// The engine's index of a Firefight skull (its discriminant).
pub fn firefight_index(skull: ::firefight::Skull) -> usize {
    skull as usize
}

/// Where `skull` takes effect in a Firefight match.
pub fn firefight_scope(skull: ::firefight::Skull) -> FirefightScope {
    use ::firefight::Skull as S;
    match skull {
        S::Assassins | S::Thunderstorm | S::Ghost | S::Catch | S::Mythic | S::Angry => {
            FirefightScope::HostAi
        }
        S::Famine => FirefightScope::HostAiAndShared(Reach::Simulation),
        S::BlackEye => FirefightScope::Shared(Reach::Simulation),
        // A director rule (no respawn in the bonus round), no engine byte.
        S::Iron => FirefightScope::Rule,
        S::LowGravity => FirefightScope::Shared(Reach::Simulation),
    }
}

/// The AI skulls (engine indices, ascending) a machine in `role` runs for
/// the director's active set `on`: all of them on the machine that runs the
/// actors, none on a guest, whose AI is the host's.
pub fn firefight_ai(role: &Role, on: &BTreeSet<::firefight::Skull>) -> Vec<u8> {
    if matches!(role, Role::Guest { .. }) {
        return Vec::new();
    }
    let mut set: Vec<u8> = on
        .iter()
        .filter(|s| {
            matches!(
                firefight_scope(**s),
                FirefightScope::HostAi | FirefightScope::HostAiAndShared(_)
            )
        })
        .map(|s| firefight_index(*s) as u8)
        .collect();
    set.sort_unstable();
    set
}

/// The player-scoped skulls (engine indices, ascending) of `on`: the ones
/// the multiplayer path carries to every machine.
pub fn firefight_shared(on: &BTreeSet<::firefight::Skull>) -> Vec<u8> {
    let mut set: Vec<u8> = on
        .iter()
        .filter(|s| {
            matches!(
                firefight_scope(**s),
                FirefightScope::Shared(_) | FirefightScope::HostAiAndShared(_)
            )
        })
        .map(|s| firefight_index(*s) as u8)
        .collect();
    set.sort_unstable();
    set
}

/// The simulation skulls of `on` ([`FIREFIGHT_SIM_SKULLS`]) as bits: every
/// machine's, in any role.
pub fn sim_bits(on: &BTreeSet<::firefight::Skull>) -> u32 {
    firefight_shared(on)
        .iter()
        .filter(|&&s| FIREFIGHT_SIM_SKULLS.contains(&usize::from(s)))
        .fold(0u32, |b, s| b | 1 << s)
}

/// The skulls a machine in `role` answers the campaign gate for, as bits
/// (bit `n` skull `n`): [`firefight_ai`]'s set on the machine that runs the
/// actors, and [`sim_bits`] on every machine.
pub fn gate_bits(role: &Role, on: &BTreeSet<::firefight::Skull>) -> u32 {
    firefight_ai(role, on)
        .iter()
        .fold(0u32, |b, s| b | 1 << s)
        | sim_bits(on)
}

/// Where a reader stopped at `gate`'s `jne` goes when this machine answers
/// the gate for the skulls in `answered` (bit `n` skull `n`): the
/// fall-through, the load of the skull's byte, for an answered skull;
/// nowhere (the `jne` decides, as retail) for any other.
pub fn gate_resume(gate: &guest_layout::points::skull_gates::Gate, answered: u32) -> Option<u32> {
    (gate.skull < 32 && answered & (1 << gate.skull) != 0).then_some(gate.resume)
}

/// The transfer to a gate's fall-through `to`, if the image has it. Each
/// is a registered instruction entry (`boot::image_entries`), which
/// discovery makes a function entry of the image, not a resume: it is
/// entered by entry dispatch.
pub fn gate_transfer(d: &runtime::Dispatch, to: u32) -> Option<runtime::TransferAbi> {
    d.has_generated(to).then(|| runtime::TransferAbi::jump(to))
}

// ---- the shared state ----

/// The multiplayer skulls of a run: the choice per profile (cached, so the
/// HLE's probe never reaches the store), the host's last-seen profiles, and
/// the set active in the game being played.
pub struct MpSkulls {
    store: Arc<Store>,
    hle: Option<Arc<live_hle::Hle>>,
    telemetry: Option<Arc<live_telemetry::Telemetry>>,
    choice: Mutex<BTreeMap<String, Vec<u8>>>,
    profiles: Mutex<Vec<String>>,
    /// Bit `n`: skull `n` is on in this game.
    active: AtomicU32,
    /// Bit `n`: this machine answers skull `n`'s reader gate for the
    /// Firefight director ([`gate_bits`]: a host's AI skulls, every
    /// machine's simulation skulls). Not part of a snapshot: the mode's own
    /// blob rebuilds it from the director.
    ff_ai: AtomicU32,
    /// Bit `n`: the Firefight director added player-scoped skull `n` to
    /// `active` (it was not in the game's own set), so it can take it away.
    ff_added: AtomicU32,
    /// Each effect says the first time it acts in a game.
    said_blind: AtomicBool,
    said_famine: AtomicBool,
    /// Famine calls in progress: (placement, rounds before).
    famine: Mutex<Vec<(u32, [u16; 2])>>,
}

impl MpSkulls {
    pub fn new(
        store: Arc<Store>,
        hle: Option<Arc<live_hle::Hle>>,
        telemetry: Option<Arc<live_telemetry::Telemetry>>,
    ) -> Self {
        let choice = store
            .profiles()
            .into_iter()
            .map(|p| {
                let c = chosen(&store, &p);
                (p, c)
            })
            .collect();
        MpSkulls {
            store,
            hle,
            telemetry,
            choice: Mutex::new(choice),
            profiles: Mutex::new(Vec::new()),
            active: AtomicU32::new(0),
            ff_ai: AtomicU32::new(0),
            ff_added: AtomicU32::new(0),
            said_blind: AtomicBool::new(false),
            said_famine: AtomicBool::new(false),
            famine: Mutex::new(Vec::new()),
        }
    }
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
    /// `profile`'s choice changed (a press): read it again and tell the
    /// HLE, so an open lobby's advertisement follows.
    pub fn changed(&self, profile: &str) {
        let c = chosen(&self.store, profile);
        self.choice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(profile.to_owned(), c);
        self.advertise();
    }
    /// The host's profiles, as last read.
    pub fn set_profiles(&self, profiles: Vec<String>) {
        *self.profiles.lock().unwrap_or_else(|e| e.into_inner()) = profiles;
    }
    /// The union of `profiles`' choices, from the cache (no store).
    pub fn chosen_for(&self, profiles: &[String]) -> Vec<u8> {
        let choice = self.choice.lock().unwrap_or_else(|e| e.into_inner());
        union(profiles.iter().filter_map(|p| choice.get(p)))
    }
    /// The host's set for the last-seen profiles.
    pub fn current(&self) -> Vec<u8> {
        let profiles = self
            .profiles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.chosen_for(&profiles)
    }
    /// Tells the HLE the host's set (advertisements carry it).
    pub fn advertise(&self) {
        if let Some(h) = &self.hle {
            h.set_session_skulls(live_proto::skulls::normalize(self.current()));
        }
    }
    /// The HLE's probe, at each advertisement the game makes: the
    /// signed-in profiles' set. Guest memory is held: only memory reads and
    /// our own leaf locks.
    pub fn probe<M: Gm>(&self, m: &M) -> Option<Vec<u8>> {
        let (profiles, _) = skulls::profiles_or_default(m);
        let set = self.chosen_for(&profiles);
        self.set_profiles(profiles);
        live_proto::skulls::normalize(set)
    }
    /// Makes `set` the active one (empty: none) for the game starting.
    pub fn activate(&self, set: &[u8]) {
        let bits = set.iter().fold(0u32, |b, s| b | 1 << s);
        self.active.store(bits, Ordering::SeqCst);
        self.ff_ai.store(0, Ordering::SeqCst);
        self.ff_added.store(0, Ordering::SeqCst);
        engine::skull_gate::answer(0);
        self.said_blind.store(false, Ordering::Relaxed);
        self.said_famine.store(false, Ordering::Relaxed);
        if let Some(t) = &self.telemetry {
            t.set_multiplayer_skulls(live_proto::skulls::normalize(set.iter().copied()));
        }
    }
    pub fn active(&self, index: usize) -> bool {
        index < 32 && self.active.load(Ordering::SeqCst) & (1 << index) != 0
    }
    /// The Firefight director's skull `index` is answered on this machine:
    /// a host's AI set, and the simulation skulls on every machine.
    pub fn ff_ai_active(&self, index: usize) -> bool {
        index < 32 && self.ff_ai.load(Ordering::SeqCst) & (1 << index) != 0
    }
    /// The Firefight director's skulls `on` for this machine's `role`: the
    /// AI ones only on the machine that runs the actors, the player-scoped
    /// ones joining the multiplayer set (a host's reach everyone through
    /// the session data, as any multiplayer skull does). The game's start
    /// ([`Self::activate`]) clears the AI set; call this after each `Skull`
    /// command with the director's whole active set.
    pub fn set_firefight(&self, role: &Role, on: &BTreeSet<::firefight::Skull>) {
        let bits = |v: Vec<u8>| v.iter().fold(0u32, |b, s| b | 1 << s);
        let ai = gate_bits(role, on);
        self.ff_ai.store(ai, Ordering::SeqCst);
        // The readers' campaign gate passes for exactly these
        // ([`gate_resume`], and the native bodies through `skull_gate`).
        engine::skull_gate::answer(ai);
        if !matches!(role, Role::Guest { .. }) {
            // What the director put there last time and the game's own set
            // did not hold goes first: a skull it switched off leaves.
            let before = self
                .active
                .fetch_and(!self.ff_added.load(Ordering::SeqCst), Ordering::SeqCst);
            let own = before & !self.ff_added.load(Ordering::SeqCst);
            let now = bits(firefight_shared(on));
            self.active.fetch_or(now, Ordering::SeqCst);
            self.ff_added.store(now & !own, Ordering::SeqCst);
        }
    }
    /// Writes the engine's skull array for the Firefight set: 1 for each
    /// AI skull on and each simulation skull on ([`FIREFIGHT_SIM_SKULLS`]),
    /// 0 for each one off, other bytes untouched (they are the campaign's).
    /// `false` if the array could not be written.
    pub fn write_firefight_array<M: Gm>(&self, m: &mut M) -> bool {
        FIREFIGHT_AI_SKULLS
            .iter()
            .chain(&FIREFIGHT_SIM_SKULLS)
            .all(|&i| m.wr(SKULL_ARRAY + i as u32, &[u8::from(self.ff_ai_active(i))]))
    }
    /// The multiplayer start: work out the set for `role` and make it
    /// active. Call with no guest memory held (a guest may wait for the
    /// server). Returns the line to log.
    pub fn start(&self, role: &Role, profiles: Vec<String>) -> String {
        let own = self.chosen_for(&profiles);
        self.set_profiles(profiles);
        let hle = self.hle.clone();
        let (set, line) = at_start(role, &own, || match &hle {
            Some(h) => Heard::from_hle(h.host_skulls(HOST_WAIT)),
            None => Heard::Nothing("Live is off here; System Link shares no skulls".into()),
        });
        if !matches!(role, Role::Guest { .. }) {
            self.advertise();
        }
        self.activate(&set);
        line
    }
    /// Why a machine snapshot must wait: a Famine call is between its
    /// hooks, with the placement's rounds halved in guest memory.
    fn snapshot_live(&self) -> Option<String> {
        let open = self.famine.lock().unwrap_or_else(|e| e.into_inner()).len();
        (open != 0).then(|| format!("{open} Famine calls are between their hooks"))
    }

    /// The game's state as a machine snapshot keeps it
    /// (docs/runtime/snapshots.md): the active set, which effects have
    /// said they act, and the host's profiles as last read. Choices are
    /// the settings store's.
    fn save_state(&self) -> Vec<u8> {
        let w = &mut snapshot::codec::Writer::new();
        w.u32(self.active.load(Ordering::SeqCst))
            .bool(self.said_blind.load(Ordering::Relaxed))
            .bool(self.said_famine.load(Ordering::Relaxed));
        let profiles = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        w.list(&profiles[..], |w, p| {
            w.str(p);
        });
        std::mem::take(w).into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let r = &mut snapshot::codec::Reader::new(bytes, "skulls-mp");
        let decoded = (|| {
            let active = r.u32()?;
            let blind = r.bool()?;
            let famine = r.bool()?;
            let profiles = r.list(4, |r| Ok(r.str()?.to_owned()))?;
            r.finish()?;
            Ok::<_, snapshot::Error>((active, blind, famine, profiles))
        })();
        let (active, blind, famine, profiles) = decoded.map_err(|e| e.to_string())?;
        self.active.store(active, Ordering::SeqCst);
        self.said_blind.store(blind, Ordering::Relaxed);
        self.said_famine.store(famine, Ordering::Relaxed);
        self.set_profiles(profiles);
        if let Some(t) = &self.telemetry {
            let set = (0..32u8).filter(|&i| active & (1u32 << i) != 0);
            t.set_multiplayer_skulls(live_proto::skulls::normalize(set));
        }
        Ok(())
    }

    /// The first time `skull`'s effect acts this game: the line to log.
    fn first(&self, skull: usize) -> Option<&'static str> {
        let (flag, line) = match skull {
            BLIND => (
                &self.said_blind,
                "skulls-mp: Blind is hiding the HUD and first-person weapon this game",
            ),
            FAMINE => (
                &self.said_famine,
                "skulls-mp: Famine halved a placed weapon's ammo (the first this game)",
            ),
            _ => return None,
        };
        (!flag.swap(true, Ordering::Relaxed)).then_some(line)
    }
}

static SHARED: OnceLock<Arc<MpSkulls>> = OnceLock::new();

/// The run's multiplayer skulls, once installed.
pub fn shared() -> Option<&'static Arc<MpSkulls>> {
    SHARED.get()
}

/// A press changed `profile`'s multiplayer choice.
pub fn changed(profile: &str) {
    if let Some(s) = shared() {
        s.changed(profile);
    }
}

// ---- the feature ----

/// The multiplayer skulls' hooks on the feature plan: the start, Blind's
/// two and Famine's.
pub struct SkullsMp(pub Arc<MpSkulls>);

impl hooks::Feature for SkullsMp {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Skulls in multiplayer games").doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use hooks::points::{FIRST_PERSON_RENDER, GAME_ACTIVATE, HUD_SHOWN, WEAPON_FROM_PLACEMENT};
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&GAME_ACTIVATE, "the host's skulls", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let (mode, role, profiles) = {
                    let mem = rt.memory();
                    (
                        skulls::options_mode(&*mem, c.cpu.eax),
                        role_in(&*mem),
                        skulls::profiles_or_default(&*mem).0,
                    )
                };
                if mode == Some(MODE_MULTIPLAYER) {
                    eprintln!("{}", s.start(&role, profiles));
                } else {
                    s.activate(&[]);
                }
                Ok(Flow::Continue)
            })
            // After any other start hook: the set is what the map loads with.
            .last();
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&HUD_SHOWN, "Blind hides the HUD", move |c| {
                if !s.active(BLIND) {
                    return Ok(Flow::Continue);
                }
                if let Some(line) = s.first(BLIND) {
                    eprintln!("{line}");
                }
                c.cpu.eax = 0;
                c.ret(0).map(Flow::Return)
            })
            // Both Blind hooks read the set fixed at the game's start and
            // answer through the registers and stack of the context they run
            // on.
            .context_only();
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&FIRST_PERSON_RENDER, "Blind hides the weapon", move |c| {
                if !s.active(BLIND) {
                    return Ok(Flow::Continue);
                }
                c.cpu.eax = 0;
                c.ret(16).map(Flow::Return)
            })
            .context_only();
        let s = Arc::clone(&self.0);
        b.raw().before(
            &WEAPON_FROM_PLACEMENT,
            "Famine halves the rounds",
            move |c| {
                if !s.active(FAMINE) {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                // stdcall (object, placement): the placement at [esp+8].
                let Some(placement) = Gm::u32(&*mem, c.cpu.esp.wrapping_add(8)) else {
                    return Ok(Flow::Continue);
                };
                if let Some(old) = halve_placement(&mut *mem, placement) {
                    drop(mem);
                    if let Some(line) = s.first(FAMINE) {
                        eprintln!("{line}");
                    }
                    s.famine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((placement, old));
                }
                Ok(Flow::Continue)
            },
        );
        let s = Arc::clone(&self.0);
        b.raw()
            .after(&WEAPON_FROM_PLACEMENT, "Famine puts the rounds back", move |c| {
                let Some((placement, old)) =
                    s.famine.lock().unwrap_or_else(|e| e.into_inner()).pop()
                else {
                    return Ok(());
                };
                let rt = Arc::clone(c.ctx.runtime());
                if !restore_placement(&mut *rt.memory(), placement, old) {
                    eprintln!(
                        "skulls-mp: could not put placement {placement:#x}'s rounds back after Famine"
                    );
                }
                Ok(())
            });
        // A Firefight host's AI skulls: each reader's campaign gate
        // (docs/game/skulls.md, "In Firefight"). Optional: an image built
        // without the gates' entries drops them with a note, and the rest
        // of the multiplayer skulls stay.
        for gate in &guest_layout::points::skull_gates::GATES {
            b.raw()
                .before(
                    &gate.site,
                    "a Firefight host's AI skull passes the campaign gate",
                    move |c| {
                        let Some(to) = gate_resume(gate, engine::skull_gate::answered_bits())
                        else {
                            return Ok(Flow::Continue);
                        };
                        let Some(transfer) = c.ctx.runtime().read_dispatch(|d| gate_transfer(d, to))
                        else {
                            eprintln!(
                                "skulls-mp: {}'s fall-through {to:#010x} is not an entry of this image; the campaign gate stands",
                                gate.site.name
                            );
                            return Ok(Flow::Continue);
                        };
                        Ok(Flow::Return(transfer))
                    },
                )
                .optional();
        }
        let (live, save, load) = (
            Arc::clone(&self.0),
            Arc::clone(&self.0),
            Arc::clone(&self.0),
        );
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || live.snapshot_live(),
            move || save.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        Ok(())
    }
}

/// Adds the multiplayer skulls to `registry`. Says why when they are left
/// out: no store (a headless run), or switched off
/// (`H2_FEATURES=skulls-mp=off`). With Live, advertisements carry the
/// host's set from now on.
pub fn install(
    registry: &mut hooks::Registry,
    store: Option<Arc<Store>>,
    hle: Option<Arc<live_hle::Hle>>,
    telemetry: Option<Arc<live_telemetry::Telemetry>>,
) -> anyhow::Result<bool> {
    let Some(store) = store else {
        eprintln!("skulls-mp: off: no settings store (a headless run)");
        return Ok(false);
    };
    let shared = Arc::new(MpSkulls::new(store, hle.clone(), telemetry));
    let added = registry
        .add(&SkullsMp(Arc::clone(&shared)))
        .map_err(|e| anyhow::anyhow!("skulls-mp: {e}"))?;
    if !added {
        eprintln!("skulls-mp: off: switched off (H2_FEATURES)");
        return Ok(false);
    }
    if let Some(h) = &hle {
        let probe = Arc::clone(&shared);
        h.set_skulls_probe(Some(Arc::new(move |g: &runtime::Guest| probe.probe(g))));
    }
    let _ = SHARED.set(shared);
    Ok(true)
}

/// The install line, once `plan` is in.
pub fn report(plan: &hooks::Plan, installed: &hooks::Installed) -> String {
    use hooks::points::{FIRST_PERSON_RENDER, GAME_ACTIVATE, HUD_SHOWN, WEAPON_FROM_PLACEMENT};
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!("skulls-mp: NOT installed: {why}; multiplayer games run without skulls");
    }
    if !plan.is_on(FEATURE) {
        return "skulls-mp: NOT installed: not in the feature plan".into();
    }
    let at = |p: &hooks::points::Point, what: &str| {
        let chain = plan
            .chains
            .values()
            .find(|c| std::ptr::eq(c.point, p) && c.hooks().any(|h| h.feature == FEATURE));
        match chain {
            Some(c) if installed.placed.contains(&c.addr) => format!("{:#x} ({what})", c.addr),
            Some(c) => format!("{:#x} ({what}, NOT placed)", c.addr),
            None => format!("{} ({what}, NOT in the plan)", p.name),
        }
    };
    let gates = &guest_layout::points::skull_gates::GATES;
    let placed = gates
        .iter()
        .filter(|g| {
            plan.chains.values().any(|c| {
                std::ptr::eq(c.point, &g.site)
                    && installed.placed.contains(&c.addr)
                    && c.hooks().any(|h| h.feature == FEATURE)
            })
        })
        .count();
    format!(
        "skulls-mp: hooks installed via feature plan at {}, {}, {}, {}; offered: {} (each once \
         the host's profile has collected it); Firefight AI skull gates: {placed} of {} placed",
        at(&GAME_ACTIVATE, "multiplayer start"),
        at(&HUD_SHOWN, "Blind: HUD"),
        at(&FIRST_PERSON_RENDER, "Blind: first-person weapon"),
        at(&WEAPON_FROM_PLACEMENT, "Famine: placed weapons"),
        names(&MP.iter().map(|m| m.index as u8).collect::<Vec<_>>()),
        gates.len(),
    )
}

// ---- the variant editor's Skulls row ----

/// The variant editor's category list (`variant_editing_options`, screen
/// 178, list handler `0x2b74c3` at screen `+0x614`): a datum array named
/// "variant editing options list" of six rows, each row's word (`+2`) its
/// index, set by the constructor's loop over the count. Grown by one, the
/// loop gives our row word 6, which retail's row text leaves blank and its
/// row chosen ignores (`ret 8`).
pub const VARIANT_LIST_NAME: u32 = 0x0045_be10;
pub const VARIANT_ROWS: u32 = 6;
pub const VARIANT_ROW: i16 = 6;
pub const VARIANT_SCREEN_ID: i16 = 178;

/// The variant editor list's count when the game builds the array `name`
/// with `retail` rows: one more (the Skulls row) for that list at its
/// retail size.
pub fn variant_count(name: u32, retail: u32) -> u32 {
    if name == VARIANT_LIST_NAME && retail == VARIANT_ROWS {
        retail + 1
    } else {
        retail
    }
}

/// A pane change to skip: our row on the variant editor, whose tag has a
/// pane for each of the six categories and none for ours (`0x230427` does
/// not check), so the last category's pane stays up.
pub fn skip_pane(screen_id: i16, row: i16) -> bool {
    screen_id == VARIANT_SCREEN_ID && row == VARIANT_ROW
}

/// A screen's window (`+0x74`) and slot (`+0x78`): the request fields its
/// loader was handed (`+4`, `+8`), kept by the base screen constructor
/// (`0x22f5ca`). The push (`0x147f6d`) and the back handler (`0x2300ea`
/// through `0x14800c`) pick the screen stack by them: window 5 is the main
/// menu's, 3 the lobby dialogs'.
pub const SCREEN_WINDOW: u32 = 0x74;
pub const SCREEN_SLOT: u32 = 0x78;

/// The request that opens a screen with `loader` for `mask` on the stack of
/// window `window`, slot `slot`: what `0x149f49` builds (`+0` 0, `+2` the
/// mask, `+4` the window, `+8` the slot, `+0xc` 0, `+0x10..0x1c` -1, `+0x1c`
/// the loader). `0x149f1e` always says window 5; a screen opened from a
/// screen on another stack must say that one's.
pub fn open_request(mask: u16, window: u32, slot: u32, loader: u32) -> [u8; 0x20] {
    let mut r = [0u8; 0x20];
    r[2..4].copy_from_slice(&mask.to_le_bytes());
    r[4..8].copy_from_slice(&window.to_le_bytes());
    r[8..0xc].copy_from_slice(&slot.to_le_bytes());
    r[0x10..0x1c].fill(0xff);
    r[0x1c..0x20].copy_from_slice(&loader.to_le_bytes());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coop_probe::Session;

    fn store() -> Arc<Store> {
        Arc::new(Store::defaults(crate::setting_defs::ALL))
    }

    fn names_of(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `profiles` pick up both multiplayer skulls in the campaign.
    fn collect(st: &Store, profiles: &[&str]) {
        for i in [FAMINE, BLIND] {
            skulls::record_pickup(st, &names_of(profiles), i);
        }
    }

    #[test]
    fn only_the_skulls_that_work_in_multiplayer_are_offered() {
        let offered: Vec<u8> = MP.iter().map(|m| m.index as u8).collect();
        assert_eq!(offered, live_proto::skulls::MULTIPLAYER);
        assert_eq!(SKULLS[FAMINE].label, "Famine");
        assert_eq!(SKULLS[BLIND].label, "Blind");
        assert_eq!(MP[0].reach, Reach::Simulation);
        assert_eq!(MP[1].reach, Reach::Local);
        assert_eq!(row_of(BLIND), Some(1));
        assert_eq!(row_of(12), None, "Mythic needs AI");
        for d in [&MP_FAMINE, &MP_BLIND] {
            assert!(
                crate::setting_defs::ALL.iter().any(|a| a.key == d.key),
                "{}",
                d.key
            );
        }
    }

    /// Collecting is the rule: no setting waives it.
    #[test]
    fn a_skull_must_be_collected_before_multiplayer_can_use_it() {
        let st = store();
        assert!(
            crate::setting_defs::ALL
                .iter()
                .all(|d| d.key != "skulls.mp_require_collected"),
            "no setting turns the rule off"
        );
        assert_eq!(state(&st, "Chief", 1), Some(State::Locked));
        let line = press(&st, "Chief", 1).unwrap();
        assert!(
            line.contains("Blind is locked for Chief in multiplayer"),
            "{line}"
        );
        assert!(line.contains("found on Outskirts)"), "{line}");
        assert_eq!(chosen(&st, "Chief"), Vec::<u8>::new());
        // Collected in the campaign: off for multiplayer until turned on,
        // whatever its campaign state.
        skulls::record_pickup(&st, &names_of(&["Chief"]), BLIND);
        assert_eq!(state(&st, "Chief", 1), Some(State::Off));
        assert_eq!(
            press(&st, "Chief", 1).as_deref(),
            Some("skulls: Blind on in multiplayer for Chief")
        );
        assert_eq!(chosen(&st, "Chief"), vec![6]);
        // The campaign toggle is its own.
        skulls::toggle(&st, "Chief", "skulls.blind").unwrap();
        assert_eq!(chosen(&st, "Chief"), vec![6]);
        // A value in the store for a skull not collected (an edited file,
        // or an older build's waiver): still locked, and not applied.
        st.profile_set("Chief", MP_FAMINE.key, Value::Bool(true))
            .unwrap();
        assert_eq!(state(&st, "Chief", 0), Some(State::Locked));
        assert_eq!(chosen(&st, "Chief"), vec![6]);
        assert!(press(&st, "Chief", 0).unwrap().contains("is locked"));
        // Low Gravity, ours, is offered the same way: locked until found.
        assert!(
            press(&st, "Chief", 2)
                .unwrap()
                .contains("Low Gravity is locked")
        );
        skulls::record_pickup(
            &st,
            &names_of(&["Chief"]),
            crate::skulls_custom::LOW_GRAVITY,
        );
        press(&st, "Chief", 2);
        assert_eq!(chosen(&st, "Chief"), vec![6, 16]);
        assert_eq!(press(&st, "Chief", 3), None);
    }

    fn session(peers: u32, host: bool) -> Session {
        Session {
            state: 5,
            peers,
            local_peer: 0,
            host_peer: if host { 0 } else { 1 },
            // The stage `0x5a220` leaves in every lobby, Live or System Link.
            class: 1,
            ..Session::default()
        }
    }

    #[test]
    fn the_role_comes_from_the_squad_session() {
        assert_eq!(role(None, None), Role::Alone);
        assert_eq!(role(Some(&session(1, true)), Some(2)), Role::Alone, "one machine");
        assert_eq!(
            role(
                Some(&Session {
                    state: 0,
                    ..session(2, false)
                }),
                Some(2)
            ),
            Role::Alone,
            "no lobby or game state"
        );
        assert_eq!(role(Some(&session(3, true)), Some(1)), Role::Host { peers: 3 });
        // The guest's class is the network class, not the stage byte.
        assert_eq!(role(Some(&session(2, false)), Some(2)), Role::Guest { class: 2 });
        assert_eq!(role(Some(&session(2, false)), Some(1)), Role::Guest { class: 1 });
    }

    #[test]
    fn a_live_guest_reads_the_network_class_not_the_stage() {
        use crate::coop_probe::layout as l;
        use guest_layout::points::{NETWORK_READY, SQUAD_SESSION};
        const S: u32 = 0x10_0000;
        let mut g = guest_layout::testing::FakeGuest::new();
        g.map(NETWORK_READY.base, 1)
            .map(SQUAD_SESSION.base, 4)
            .map(S, 0x8000)
            .put_u8(NETWORK_READY.base, 1)
            .put_u32(SQUAD_SESSION.base, S)
            .put_u32(S + l::STATE, 5)
            .put_u32(S + l::PEERS, 2)
            .put_u32(S + l::LOCAL_PEER, 1)
            .put_u32(S + l::HOST_PEER, 0)
            .put_u32(S + l::CLASS, 1)
            .put_u32(S + crate::variant_ext::SESSION_NETWORK_CLASS, 2);
        g.put_u16(S + l::DIFFICULTY, 0xffff);
        assert_eq!(
            role_in(&crate::layout_glue::ReadGm(&g)),
            Role::Guest { class: 2 }
        );
    }

    #[test]
    fn the_hosts_set_is_used_everywhere() {
        let never = || -> Heard { panic!("only a guest asks") };
        let (set, line) = at_start(&Role::Alone, &[6], never);
        assert_eq!(set, [6]);
        assert_eq!(line, "skulls-mp: multiplayer start (local): Blind");
        let (set, line) = at_start(&Role::Host { peers: 2 }, &[4, 6], never);
        assert_eq!(set, [4, 6]);
        assert!(line.contains("host of 2 machines: Famine, Blind"), "{line}");
        // A guest's own choice is never used: the host's is.
        let (set, line) = at_start(&Role::Guest { class: 2 }, &[6], || Heard::Set {
            skulls: vec![4],
            fresh: true,
        });
        assert_eq!(set, [4]);
        assert!(line.contains("read from the server now: Famine"), "{line}");
        // Only multiplayer skulls, whatever the host's build sends.
        let (set, _) = at_start(&Role::Guest { class: 2 }, &[], || Heard::Set {
            skulls: vec![4, 10, 12],
            fresh: false,
        });
        assert_eq!(set, [4]);
        let (set, line) = at_start(&Role::Guest { class: 1 }, &[4, 6], || {
            Heard::Nothing("System Link".into())
        });
        assert!(set.is_empty());
        assert!(line.contains("unknown (System Link); none here"), "{line}");
    }

    #[test]
    fn the_guest_answer_comes_from_the_hle() {
        assert_eq!(
            Heard::from_hle(live_hle::HostSkulls::Known {
                session: "s".into(),
                skulls: vec![6],
                refreshed: false
            }),
            Heard::Set {
                skulls: vec![6],
                fresh: false
            }
        );
        assert!(matches!(
            Heard::from_hle(live_hle::HostSkulls::Unknown { session: "ab".into() }),
            Heard::Nothing(w) if w.contains("ab")
        ));
        assert!(matches!(
            Heard::from_hle(live_hle::HostSkulls::NotJoined),
            Heard::Nothing(_)
        ));
    }

    /// The selection a multiplayer start uses: the union of the host's
    /// signed-in profiles' choices, as the menu left them, from the cache
    /// the HLE probe also reads.
    #[test]
    fn the_start_uses_the_profiles_choice_as_pressed() {
        let st = store();
        collect(&st, &["Chief", "Arbiter"]);
        press(&st, "Chief", 1);
        let mp = MpSkulls::new(Arc::clone(&st), None, None);
        assert_eq!(
            mp.chosen_for(&names_of(&["Chief"])),
            vec![6],
            "read at install"
        );
        press(&st, "Arbiter", 0);
        assert_eq!(
            mp.chosen_for(&names_of(&["Arbiter"])),
            Vec::<u8>::new(),
            "not yet told"
        );
        mp.changed("Arbiter");
        assert_eq!(mp.chosen_for(&names_of(&["Chief", "Arbiter"])), vec![4, 6]);
        let line = mp.start(&Role::Alone, names_of(&["Arbiter"]));
        assert_eq!(line, "skulls-mp: multiplayer start (local): Famine");
        assert!(mp.active(FAMINE) && !mp.active(BLIND));
        assert_eq!(mp.current(), vec![4]);
        // A guest with no Live hears nothing and applies none.
        let line = mp.start(&Role::Guest { class: 1 }, names_of(&["Chief"]));
        assert!(line.contains("none here"), "{line}");
        assert!(!mp.active(FAMINE) && !mp.active(BLIND));
        // Any other start clears the set.
        mp.start(&Role::Host { peers: 2 }, names_of(&["Chief"]));
        assert!(mp.active(BLIND));
        mp.activate(&[]);
        assert!(!mp.active(BLIND));
    }

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    #[test]
    fn famine_halves_the_placement_for_one_call_as_the_game_does() {
        // cdq; sub eax, edx; sar eax, 1: towards zero.
        for (v, want) in [
            (12, 6),
            (7, 3),
            (1, 0),
            (0, 0),
            (-7, -3),
            (i16::MIN, -16384),
        ] {
            assert_eq!(famine(v), want, "{v}");
        }
        let mut f = Fake::default();
        let p = 0x9000;
        f.set_u16(p + 0x4c, 60);
        f.set_u16(p + 0x4e, 7);
        let old = halve_placement(&mut f, p).unwrap();
        assert_eq!((f.u16(p + 0x4c), f.u16(p + 0x4e)), (Some(30), Some(3)));
        assert!(restore_placement(&mut f, p, old));
        assert_eq!((f.u16(p + 0x4c), f.u16(p + 0x4e)), (Some(60), Some(7)));
        assert_eq!(halve_placement(&mut f, 0x1_0000), None, "unreadable");
    }

    #[test]
    fn the_probe_reads_the_signed_in_profiles() {
        let st = store();
        collect(&st, &[skulls::DEFAULT_PROFILE]);
        press(&st, skulls::DEFAULT_PROFILE, 1);
        let mp = MpSkulls::new(st, None, None);
        // Nobody signed in: the default profile's choice.
        let mut f = Fake::default();
        for slot in 0..skulls::LOCAL_PLAYERS {
            f.set_u32(skulls::SLOTS + slot * skulls::SLOT_STRIDE, 0);
            f.wr(
                skulls::SLOTS + slot * skulls::SLOT_STRIDE + skulls::SLOT_NAME,
                &[0; skulls::NAME_UNITS * 2],
            );
        }
        assert_eq!(mp.probe(&f), Some(vec![6]));
        assert_eq!(mp.current(), vec![6]);
    }

    /// The variant editor's list grows by our row, only at its size.
    #[test]
    fn the_variant_editor_list_gets_one_more_row() {
        assert_eq!(variant_count(VARIANT_LIST_NAME, 6), 7);
        assert_eq!(
            variant_count(VARIANT_LIST_NAME, 7),
            7,
            "not its retail size"
        );
        assert_eq!(
            variant_count(crate::skulls_menu::DIFFICULTY_LIST_NAME, 6),
            6
        );
        assert!(skip_pane(178, 6));
        assert!(!skip_pane(178, 5), "a category keeps its pane");
        assert!(!skip_pane(12, 6));
    }

    /// The request names the pressing screen's stack, not the main menu's
    /// alone, as `0x149f49` lays a request out.
    #[test]
    fn the_request_opens_on_the_pressing_screens_stack() {
        let r = open_request(1 << 2, 3, 4, 0xa0f0_5000);
        let word = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        assert_eq!(
            u16::from_le_bytes([r[0], r[1]]),
            0,
            "an opening, not a back"
        );
        assert_eq!(u16::from_le_bytes([r[2], r[3]]), 4);
        assert_eq!((word(4), word(8), word(0xc)), (3, 4, 0));
        assert_eq!(&r[0x10..0x1c], &[0xff; 12]);
        assert_eq!(word(0x1c), 0xa0f0_5000);
    }

    #[test]
    fn a_snapshot_carries_the_games_multiplayer_skulls() {
        let a = MpSkulls::new(store(), None, None);
        a.activate(&[FAMINE as u8, BLIND as u8]);
        assert!(a.first(BLIND).is_some());
        a.set_profiles(names_of(&["Chief", "Arbiter"]));
        assert_eq!(a.snapshot_live(), None);
        let bytes = a.save_state();

        let b = MpSkulls::new(store(), None, None);
        b.load_state(&bytes).unwrap();
        assert!(b.active(FAMINE) && b.active(BLIND));
        assert!(b.first(BLIND).is_none(), "Blind already said so this game");
        assert!(b.first(FAMINE).is_some());
        assert_eq!(*b.profiles.lock().unwrap(), names_of(&["Chief", "Arbiter"]));
        assert!(b.load_state(&bytes[..bytes.len() - 1]).is_err());
        assert!(b.active(FAMINE), "a damaged section changes nothing");

        // A Famine call between its hooks holds a snapshot off.
        b.famine.lock().unwrap().push((0x1000, [1, 2]));
        assert!(b.snapshot_live().is_some());
    }

    fn ff(skulls: &[::firefight::Skull]) -> BTreeSet<::firefight::Skull> {
        skulls.iter().copied().collect()
    }

    #[test]
    fn ai_skulls_apply_on_the_host_only() {
        use ::firefight::Skull as S;
        let on = ff(&[S::Mythic, S::Angry, S::Famine, S::BlackEye, S::LowGravity]);
        for role in [Role::Alone, Role::Host { peers: 3 }] {
            assert_eq!(firefight_ai(&role, &on), [4, 12, 13], "{role:?}");
        }
        assert!(firefight_ai(&Role::Guest { class: 2 }, &on).is_empty());
        // Player-scoped skulls do not depend on the role.
        assert_eq!(firefight_shared(&on), [4, 8, 16]);

        let mp = MpSkulls::new(store(), None, None);
        mp.activate(&[]);
        mp.set_firefight(&Role::Host { peers: 2 }, &on);
        assert!(mp.ff_ai_active(12) && mp.ff_ai_active(13) && mp.ff_ai_active(4));
        assert!(!mp.ff_ai_active(2), "Assassins is not on");
        assert!(mp.active(BLACK_EYE) && mp.active(FAMINE));
        assert!(mp.active(crate::skulls_custom::LOW_GRAVITY));
        // A guest applies no AI skull and adds nothing of its own: the
        // host's player-scoped set reaches it through the session. Black
        // Eye's byte is the one thing every machine holds
        // (`black_eye_reaches_every_machine`).
        let guest = MpSkulls::new(store(), None, None);
        guest.activate(&[]);
        guest.set_firefight(&Role::Guest { class: 2 }, &on);
        assert!(
            !(0..32).any(|i| i != BLACK_EYE && (guest.ff_ai_active(i) || guest.active(i)))
        );
        assert!(guest.ff_ai_active(BLACK_EYE) && !guest.active(BLACK_EYE));
        // The next game's start clears the AI set.
        mp.activate(&[]);
        assert!(!mp.ff_ai_active(12));
    }

    /// Each AI skull's readers pass their campaign gate on the machine
    /// that runs actors and nowhere else: a gate resumes at the load of its
    /// skull's byte only for a skull of the host's AI set.
    #[test]
    fn a_hosts_ai_skulls_pass_each_readers_campaign_gate() {
        use ::firefight::Skull as S;
        use guest_layout::points::skull_gates::{GATES, of};
        let every = ff(&[
            S::Assassins,
            S::Thunderstorm,
            S::Famine,
            S::Ghost,
            S::Catch,
            S::Mythic,
            S::Angry,
        ]);
        let host = gate_bits(&Role::Host { peers: 2 }, &every);
        assert_eq!(gate_bits(&Role::Alone, &every), host);
        // Every reader of every AI skull: the scan's thirteen gates.
        assert_eq!(GATES.len(), 13);
        for &skull in &FIREFIGHT_AI_SKULLS {
            let gates: Vec<_> = of(skull as u8).collect();
            assert!(!gates.is_empty(), "skull {skull}");
            for g in gates {
                assert_eq!(gate_resume(g, host), Some(g.site.base + 2), "{}", g.site.name);
            }
        }
        // A guest's actors are the host's: no gate passes there.
        let guest = gate_bits(&Role::Guest { class: 2 }, &every);
        assert_eq!(guest, 0);
        assert!(GATES.iter().all(|g| gate_resume(g, guest).is_none()));
        // One skull on answers its own readers only.
        let mythic = gate_bits(&Role::Host { peers: 2 }, &ff(&[S::Mythic, S::BlackEye]));
        assert_eq!(mythic, 1 << 12 | 1 << BLACK_EYE);
        for g in &GATES {
            assert_eq!(gate_resume(g, mythic).is_some(), g.skull == 12, "{}", g.site.name);
        }
        // The player-scoped skulls have no gate of their own.
        for skull in [8u8, 16] {
            assert!(of(skull).next().is_none());
        }
    }

    /// A gate's fall-through is a function entry of the image, never a
    /// resume (`recomp::register_retail_entries`): the hook enters it by
    /// entry dispatch, so a resume check refused it on every image.
    #[test]
    fn a_gate_enters_its_fall_through_as_the_image_builds_it() {
        let to = guest_layout::points::skull_gates::GATES[0].resume;
        assert_eq!(gate_transfer(&runtime::Dispatch::default(), to), None);
        let mut d = runtime::Dispatch::default();
        d.register_generated_rust(to, |_, _, _| unreachable!())
            .unwrap();
        assert!(!d.has_resume(to));
        assert_eq!(gate_transfer(&d, to), Some(runtime::TransferAbi::jump(to)));
    }

    #[test]
    fn the_skull_array_gets_exactly_the_hosts_ai_set() {
        use ::firefight::Skull as S;
        let mp = MpSkulls::new(store(), None, None);
        mp.set_firefight(&Role::Host { peers: 2 }, &ff(&[S::Thunderstorm, S::Catch]));
        let mut f = Fake::default();
        // A campaign byte (Blind, 6) is not ours to touch.
        f.wr(SKULL_ARRAY + 6, &[1]);
        f.wr(SKULL_ARRAY + 12, &[1]);
        assert!(mp.write_firefight_array(&mut f));
        let bytes: Vec<_> = (0..15)
            .map(|i| f.u8(SKULL_ARRAY + i).unwrap_or(0))
            .collect();
        assert_eq!(bytes, [0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn every_firefight_skull_has_a_scope_and_an_engine_index() {
        use ::firefight::Skull as S;
        for (skull, index, scope) in [
            (S::Assassins, 2, FirefightScope::HostAi),
            (S::Thunderstorm, 3, FirefightScope::HostAi),
            (
                S::Famine,
                4,
                FirefightScope::HostAiAndShared(Reach::Simulation),
            ),
            (S::Ghost, 7, FirefightScope::HostAi),
            (S::BlackEye, 8, FirefightScope::Shared(Reach::Simulation)),
            (S::Catch, 9, FirefightScope::HostAi),
            (S::Mythic, 12, FirefightScope::HostAi),
            (S::Iron, 11, FirefightScope::Rule),
            (S::Angry, 13, FirefightScope::HostAi),
            (S::LowGravity, 16, FirefightScope::Shared(Reach::Simulation)),
        ] {
            assert_eq!(firefight_index(skull), index);
            assert_eq!(firefight_scope(skull), scope);
        }
        // The AI list is exactly the skulls whose scope reaches actors.
        let ai: Vec<usize> = [
            S::Assassins,
            S::Thunderstorm,
            S::Famine,
            S::BlackEye,
            S::Ghost,
            S::Catch,
            S::Iron,
            S::Mythic,
            S::Angry,
            S::LowGravity,
        ]
        .into_iter()
        .filter(|s| {
            !matches!(
                firefight_scope(*s),
                FirefightScope::Shared(_) | FirefightScope::Rule
            )
        })
        .map(firefight_index)
        .collect();
        assert_eq!(ai, FIREFIGHT_AI_SKULLS);
    }

    /// Black Eye's reader runs in every machine's simulation: the host, a
    /// guest and a machine alone all answer its gate and hold its byte of
    /// the skull array, and a guest adds nothing else of its own.
    #[test]
    fn black_eye_reaches_every_machine() {
        use ::firefight::Skull as S;
        let on = ff(&[S::BlackEye, S::Mythic]);
        assert_eq!(firefight_shared(&on), [8]);
        for role in [Role::Alone, Role::Host { peers: 2 }, Role::Guest { class: 2 }] {
            let mp = MpSkulls::new(store(), None, None);
            mp.activate(&[]);
            mp.set_firefight(&role, &on);
            assert!(mp.ff_ai_active(BLACK_EYE), "{role:?}");
            assert_ne!(gate_bits(&role, &on) & 1 << BLACK_EYE, 0, "{role:?}");
            let mut f = Fake::default();
            assert!(mp.write_firefight_array(&mut f));
            assert_eq!(f.u8(SKULL_ARRAY + BLACK_EYE as u32), Some(1), "{role:?}");
            // Mythic is the host's actors' alone.
            let mythic = f.u8(SKULL_ARRAY + 12) == Some(1);
            assert_eq!(mythic, !matches!(role, Role::Guest { .. }), "{role:?}");
        }
        assert_eq!(
            firefight_scope(S::BlackEye),
            FirefightScope::Shared(Reach::Simulation)
        );
    }

    /// A skull the director switches off leaves the array, the gate set and
    /// the game's active set; the game's own skulls and the campaign's bytes
    /// stay.
    #[test]
    fn a_skull_switched_off_leaves_the_array() {
        use ::firefight::Skull as S;
        let mp = MpSkulls::new(store(), None, None);
        // Famine is in the game's own set: the director's off never takes it.
        mp.activate(&[FAMINE as u8]);
        let host = Role::Host { peers: 2 };
        mp.set_firefight(&host, &ff(&[S::Thunderstorm, S::Catch, S::BlackEye]));
        let mut f = Fake::default();
        f.wr(SKULL_ARRAY + 6, &[1]);
        assert!(mp.write_firefight_array(&mut f));
        assert_eq!(f.u8(SKULL_ARRAY + 9), Some(1));
        assert_eq!(f.u8(SKULL_ARRAY + 8), Some(1));
        assert!(mp.active(BLACK_EYE));

        mp.set_firefight(&host, &ff(&[S::Thunderstorm, S::Famine]));
        assert!(mp.write_firefight_array(&mut f));
        let bytes: Vec<_> = (0..15)
            .map(|i| f.u8(SKULL_ARRAY + i).unwrap_or(0))
            .collect();
        assert_eq!(bytes, [0, 0, 0, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!mp.ff_ai_active(9) && !mp.ff_ai_active(BLACK_EYE));
        assert!(!mp.active(BLACK_EYE), "Black Eye left the active set");
        assert!(mp.active(FAMINE));
        // Off for good: nothing of the director's is left, the game's own
        // set is.
        mp.set_firefight(&host, &ff(&[]));
        assert!(mp.write_firefight_array(&mut f));
        assert!((0..32)
            .filter(|&i| i != 6)
            .all(|i| f.u8(SKULL_ARRAY + i).unwrap_or(0) == 0 || i == 6));
        assert!(mp.active(FAMINE), "the game's own Famine stays");
    }
}
