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

use std::collections::BTreeMap;
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
    /// Joined someone else's; `class` 1 System Link, 2 Live.
    Guest { class: u32 },
}

/// The role the squad session says, when there is one in a lobby or game
/// state (`coop_probe::read_session`).
pub fn role(session: Option<&Session>) -> Role {
    match session {
        Some(s) if s.live() && s.peers > 1 => {
            if s.is_host() {
                Role::Host { peers: s.peers }
            } else {
                Role::Guest { class: s.class }
            }
        }
        _ => Role::Alone,
    }
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
        self.said_blind.store(false, Ordering::Relaxed);
        self.said_famine.store(false, Ordering::Relaxed);
        if let Some(t) = &self.telemetry {
            t.set_multiplayer_skulls(live_proto::skulls::normalize(set.iter().copied()));
        }
    }
    pub fn active(&self, index: usize) -> bool {
        index < 32 && self.active.load(Ordering::SeqCst) & (1 << index) != 0
    }
    /// The multiplayer start: work out the set for `role` and make it
    /// active. Call with no guest memory held (a guest may wait for the
    /// server). Returns the line to log.
    pub fn start(&self, role: &Role, profiles: Vec<String>) -> String {
        let own = self.chosen_for(&profiles);
        self.set_profiles(profiles);
        let hle = self.hle.clone();
        let (set, line) = at_start(role, &own, || match &hle {
            Some(h) if h.film_service_taped() => {
                let bits = h.game_setting("HostSkulls", &[], || {
                    match Heard::from_hle(h.host_skulls(HOST_WAIT)) {
                        Heard::Set { skulls, .. } => Some(
                            skulls
                                .into_iter()
                                .filter(|s| live_proto::skulls::MULTIPLAYER.contains(s))
                                .fold(0u32, |bits, s| bits | (1 << s))
                                .to_le_bytes(),
                        ),
                        Heard::Nothing(_) => None,
                    }
                });
                match bits {
                    Some(bits) => Heard::Set {
                        skulls: live_proto::skulls::MULTIPLAYER
                            .into_iter()
                            .filter(|s| u32::from_le_bytes(bits) & (1 << s) != 0)
                            .collect(),
                        fresh: false,
                    },
                    None => Heard::Nothing("no host skulls".into()),
                }
            }
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
        use hooks::points::{
            FIRST_PERSON_RENDER, GAME_ACTIVATE, HUD_SHOWN, WEAPON_FROM_PLACEMENT,
        };
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&GAME_ACTIVATE, "the host's skulls", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let (mode, role, profiles) = {
                    let mem = rt.memory();
                    (
                        skulls::options_mode(&*mem, c.cpu.eax),
                        role(crate::coop_probe::read_session(&*mem).as_ref()),
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
            // on: the pipelined render thread runs them too.
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
    format!(
        "skulls-mp: hooks installed via feature plan at {}, {}, {}, {}; offered: {} (each once \
         the host's profile has collected it)",
        at(&GAME_ACTIVATE, "multiplayer start"),
        at(&HUD_SHOWN, "Blind: HUD"),
        at(&FIRST_PERSON_RENDER, "Blind: first-person weapon"),
        at(&WEAPON_FROM_PLACEMENT, "Famine: placed weapons"),
        names(&MP.iter().map(|m| m.index as u8).collect::<Vec<_>>()),
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

    fn session(peers: u32, host: bool, class: u32) -> Session {
        Session {
            state: 5,
            peers,
            local_peer: 0,
            host_peer: if host { 0 } else { 1 },
            class,
            ..Session::default()
        }
    }

    #[test]
    fn the_role_comes_from_the_squad_session() {
        assert_eq!(role(None), Role::Alone);
        assert_eq!(role(Some(&session(1, true, 2))), Role::Alone, "one machine");
        assert_eq!(
            role(Some(&Session {
                state: 0,
                ..session(2, false, 2)
            })),
            Role::Alone,
            "no lobby or game state"
        );
        assert_eq!(role(Some(&session(3, true, 1))), Role::Host { peers: 3 });
        assert_eq!(role(Some(&session(2, false, 2))), Role::Guest { class: 2 });
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
}
