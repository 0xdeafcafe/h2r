//! Skulls in multiplayer games (docs/skulls.md "In multiplayer").
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
//! the profiles signed in on the host ([`chosen`]), each only if collected
//! while `skulls.mp_require_collected` holds (the default). A guest applies
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
/// Whether a profile must have collected a skull in the campaign before it
/// can turn it on for multiplayer.
pub static MP_REQUIRE_COLLECTED: Def = Def {
    key: "skulls.mp_require_collected",
    label: "Multiplayer Skulls Need Collecting",
    help: "On, a skull can be turned on for multiplayer only once the profile has picked it \
           up in the campaign, as for the campaign. Off, any skull that works in multiplayer \
           can be.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::Advanced,
    menu: None,
};

/// The skulls offered in multiplayer, in the engine's order:
/// `live_proto::skulls::MULTIPLAYER`.
pub static MP: [MpSkull; 2] = [
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
];

pub const FAMINE: usize = 4;
pub const BLIND: usize = 6;

/// The row of skull `index` on the multiplayer screen.
pub fn row_of(index: usize) -> Option<usize> {
    MP.iter().position(|m| m.index == index)
}

/// Whether a skull must be collected first (`skulls.mp_require_collected`).
pub fn require_collected(store: &Store) -> bool {
    store
        .get(MP_REQUIRE_COLLECTED.key)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Row `row`'s state for `profile`: Locked when it must be collected and is
/// not, else the profile's multiplayer On or Off (Off until set).
pub fn state(store: &Store, profile: &str, row: usize) -> Option<State> {
    let m = MP.get(row)?;
    let collected = skulls::state(store, profile, m.index)? != State::Locked;
    if require_collected(store) && !collected {
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
            "skulls menu: {} is locked for {profile} in multiplayer: not collected yet (found on {}{}); \
             skulls.mp_require_collected = false allows it",
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
            Some(h) => Heard::from_hle(h.host_skulls(HOST_WAIT)),
            None => Heard::Nothing("Live is off here; System Link shares no skulls".into()),
        });
        if !matches!(role, Role::Guest { .. }) {
            self.advertise();
        }
        self.activate(&set);
        line
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

impl feature::Feature for SkullsMp {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Skulls in multiplayer games").doc("docs/skulls.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::Flow;
        use feature::points::{
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
        b.raw().before(&HUD_SHOWN, "Blind hides the HUD", move |c| {
            if !s.active(BLIND) {
                return Ok(Flow::Continue);
            }
            if let Some(line) = s.first(BLIND) {
                eprintln!("{line}");
            }
            c.cpu.eax = 0;
            c.ret(0).map(Flow::Return)
        });
        let s = Arc::clone(&self.0);
        b.raw()
            .before(&FIRST_PERSON_RENDER, "Blind hides the weapon", move |c| {
                if !s.active(BLIND) {
                    return Ok(Flow::Continue);
                }
                c.cpu.eax = 0;
                c.ret(16).map(Flow::Return)
            });
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
        Ok(())
    }
}

/// Adds the multiplayer skulls to `registry`. Says why when they are left
/// out: no store (a headless run), `H2_NO_SKULLS`, or switched off
/// (`H2_FEATURES=skulls-mp=off`). With Live, advertisements carry the
/// host's set from now on.
pub fn install(
    registry: &mut feature::Registry,
    store: Option<Arc<Store>>,
    hle: Option<Arc<live_hle::Hle>>,
    telemetry: Option<Arc<live_telemetry::Telemetry>>,
) -> anyhow::Result<bool> {
    let Some(store) = store else {
        eprintln!("skulls-mp: off: no settings store (a headless run)");
        return Ok(false);
    };
    if std::env::var_os("H2_NO_SKULLS").is_some() {
        eprintln!("skulls-mp: off: H2_NO_SKULLS is set");
        return Ok(false);
    }
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
pub fn report(plan: &feature::Plan, installed: &feature::Installed, store: &Store) -> String {
    use feature::points::{FIRST_PERSON_RENDER, GAME_ACTIVATE, HUD_SHOWN, WEAPON_FROM_PLACEMENT};
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!("skulls-mp: NOT installed: {why}; multiplayer games run without skulls");
    }
    if !plan.is_on(FEATURE) {
        return "skulls-mp: NOT installed: not in the feature plan".into();
    }
    let at = |p: &feature::points::Point, what: &str| {
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
        "skulls-mp: hooks installed via feature plan at {}, {}, {}, {}; offered: {}; collecting \
         required: {}",
        at(&GAME_ACTIVATE, "multiplayer start"),
        at(&HUD_SHOWN, "Blind: HUD"),
        at(&FIRST_PERSON_RENDER, "Blind: first-person weapon"),
        at(&WEAPON_FROM_PLACEMENT, "Famine: placed weapons"),
        names(&MP.iter().map(|m| m.index as u8).collect::<Vec<_>>()),
        if require_collected(store) {
            "yes"
        } else {
            "no"
        }
    )
}

// ---- the Game Setup row ----

/// The Game Setup dialog's list (screen 23): its rows' datum array is at
/// handler `+0x70`, each row's action word at datum `+2`.
pub const SETUP_ROWS: u32 = 0x70;
pub const SETUP_ACTION: u32 = 2;
/// The action of retail's "Change Map": only a multiplayer custom game's
/// list has it (lobby kinds 1, 3 and 5).
pub const CHANGE_MAP: i16 = 0;
/// Our row's action: past retail's nine (0 to 9; the row-chosen jump table
/// has no bounds check, so the press must never reach the original).
pub const SKULLS_ACTION: i16 = 10;

/// Whether a Game Setup list with these actions gets our row.
pub fn wants_row(actions: &[i16]) -> bool {
    actions.contains(&CHANGE_MAP) && !actions.contains(&SKULLS_ACTION)
}

/// The actions of the list at `list`, in slot order.
pub fn setup_actions(
    m: &dyn engine::memory::GuestMemory,
    list: u32,
) -> Result<Vec<i16>, engine::memory::MemFault> {
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS))?);
    let hw = array.header_field(m, engine::datum::layout::HIGH_WATER)?;
    let mut out = Vec::new();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i)? {
            out.push(m.u16(elem.wrapping_add(SETUP_ACTION))? as i16);
        }
    }
    Ok(out)
}

/// Adds our row to a multiplayer custom game's Game Setup list. `Ok(None)`
/// when the list is not one (or has it); the new count otherwise.
pub fn add_setup_row(
    m: &mut dyn engine::memory::GuestMemory,
    list: u32,
) -> Result<Option<usize>, String> {
    let actions = setup_actions(m, list).map_err(|e| format!("{e:?}"))?;
    if !wants_row(&actions) {
        return Ok(None);
    }
    let array = engine::datum::DatumArray::at(
        m.u32(list.wrapping_add(SETUP_ROWS))
            .map_err(|e| format!("{e:?}"))?,
    );
    let h = array
        .new_datum(m)
        .map_err(|e| format!("{e:?}"))?
        .ok_or("the list is full")?;
    let elem = array
        .try_get(m, h)
        .map_err(|e| format!("{e:?}"))?
        .ok_or("the new row is not there")?;
    m.set_u16(elem.wrapping_add(SETUP_ACTION), SKULLS_ACTION as u16)
        .map_err(|e| format!("{e:?}"))?;
    Ok(Some(actions.len() + 1))
}

/// The action of the row whose handle is `row` on the list at `list`.
pub fn row_action(m: &dyn engine::memory::GuestMemory, list: u32, row: u32) -> Option<i16> {
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).ok()?);
    let elem = array
        .try_get(m, engine::DatumHandle::from_raw(row)?)
        .ok()??;
    m.u16(elem.wrapping_add(SETUP_ACTION))
        .ok()
        .map(|v| v as i16)
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
        for d in [&MP_FAMINE, &MP_BLIND, &MP_REQUIRE_COLLECTED] {
            assert!(
                crate::setting_defs::ALL.iter().any(|a| a.key == d.key),
                "{}",
                d.key
            );
        }
    }

    #[test]
    fn collecting_is_required_by_default_and_can_be_waived() {
        let st = store();
        assert!(require_collected(&st));
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
        // Waived: Famine can be turned on uncollected.
        st.set(MP_REQUIRE_COLLECTED.key, Value::Bool(false))
            .unwrap();
        assert_eq!(state(&st, "Chief", 0), Some(State::Off));
        press(&st, "Chief", 0);
        assert_eq!(chosen(&st, "Chief"), vec![4, 6]);
        // Required again: the uncollected one is locked, and not applied.
        st.set(MP_REQUIRE_COLLECTED.key, Value::Bool(true)).unwrap();
        assert_eq!(state(&st, "Chief", 0), Some(State::Locked));
        assert_eq!(chosen(&st, "Chief"), vec![6]);
        assert_eq!(press(&st, "Chief", 2), None);
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
        st.set(MP_REQUIRE_COLLECTED.key, Value::Bool(false))
            .unwrap();
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
        st.set(MP_REQUIRE_COLLECTED.key, Value::Bool(false))
            .unwrap();
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

    /// A Game Setup list as the constructor leaves it: a datum array of
    /// ten, rows with their action word at `+2`.
    fn setup_list(actions: &[i16]) -> (engine::testing::FakeGuest, u32) {
        use engine::datum::DatumArray;
        use engine::memory::GuestMemory;
        let mut g = engine::testing::FakeGuest::new();
        g.map(0x10000, 0x4000);
        let (list, header, data, bitmap, name) = (0x10000, 0x11000, 0x12000, 0x13000, 0x13800);
        g.set_u32(list + SETUP_ROWS, header).unwrap();
        g.write(name, b"squad setting list\0").unwrap();
        let a = DatumArray::at(header);
        a.initialize(&mut g, name, 10, 4, 0, 0, bitmap).unwrap();
        a.attach(&mut g, 10, data).unwrap();
        for act in actions {
            let h = a.new_datum(&mut g).unwrap().unwrap();
            let e = a.try_get(&g, h).unwrap().unwrap();
            g.set_u16(e + 2, *act as u16).unwrap();
        }
        (g, list)
    }

    #[test]
    fn the_skulls_row_joins_only_a_custom_games_setup() {
        // Local, System Link or Live custom game (kinds 1, 3, 5).
        for retail in [&[0, 1, 4, 6][..], &[0, 1, 4, 6, 8]] {
            let (mut g, list) = setup_list(retail);
            assert_eq!(add_setup_row(&mut g, list), Ok(Some(retail.len() + 1)));
            let mut want = retail.to_vec();
            want.push(SKULLS_ACTION);
            assert_eq!(setup_actions(&g, list).unwrap(), want);
            assert_eq!(add_setup_row(&mut g, list), Ok(None), "once");
            // The new row's handle reads back as ours.
            let a = engine::datum::DatumArray::at(0x11000);
            let h = a.handle_of(&g, retail.len() as i32).unwrap().unwrap();
            assert_eq!(row_action(&g, list, h.raw()), Some(SKULLS_ACTION));
            let h0 = a.handle_of(&g, 0).unwrap().unwrap();
            assert_eq!(row_action(&g, list, h0.raw()), Some(0));
            assert_eq!(row_action(&g, list, u32::MAX), None, "no row");
        }
        // Co-op lobbies and matchmaking: no Change Map, no row.
        for retail in [&[2, 3, 7][..], &[2, 3, 7, 8], &[9, 6, 7], &[]] {
            let (mut g, list) = setup_list(retail);
            assert_eq!(add_setup_row(&mut g, list), Ok(None), "{retail:?}");
            assert_eq!(setup_actions(&g, list).unwrap(), retail);
        }
    }
}
