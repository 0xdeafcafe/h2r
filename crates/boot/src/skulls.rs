//! Campaign skulls kept per profile (docs/game/skulls.md).
//!
//! Halo 2 turns a skull on when a level script runs
//! `(ice_cream_flavor_stock <n>)`, after the player picks up the skull
//! object (the scripts wait on `unit_has_weapon` of a skull weapon). The
//! builtin's evaluator (`0x2ac230`) hands `n` to `0x1df3e0` in ESI, which
//! sets byte `n` of a 15-byte array at `0x4f55dc` to 1 (only when it was 0,
//! and only for `0 <= n < 15`), then shows the HUD message and plays the
//! sound. Nothing else in the executable writes that array, so a skull stays
//! on until the console is reset; gameplay code reads single bytes of it,
//! and `ice_cream_flavor_available` reads it only while the active game
//! options are a campaign (mode 1).
//!
//! What we add, as the `skulls` feature on the feature plan ([`Skulls`]):
//!
//! - **Pickup**: hooks on `0x1df3e0` around the game's own function: before,
//!   note skull ESI and its byte; after, record every skull that came on
//!   since the last look as collected (and on) for every signed-in local
//!   profile. A profile is its name: the 32 UTF-16 units at `slot + 0xc1c`
//!   of the local player slots at `0x54e8e0` (stride `0xc70`, signed in when
//!   `slot + 0` has bit 4), which is what `0x1902c1` returns and what
//!   campaign start copies into each player. With no named profile, the
//!   pickup is kept under [`DEFAULT_PROFILE`] and the log shows the slots as
//!   read. A watch after each tick ([`Tracker`]) records a skull that came
//!   on without passing through `0x1df3e0`.
//! - **Storage**: one [`Def`] per skull (`skulls.<name>`, bool) in the
//!   settings [`Store`], kept per profile ([`Store::profile_set`]): no value
//!   means not collected; `true` or `false` means collected and on or off.
//!   The profile values live in `settings.toml` under
//!   `[profiles."<name>".skulls]`, never in the signed Xbox profile.
//! - **Start**: a hook before the options queue `0x18e790` (EAX = the new
//!   game options). For a campaign (`+0` = 1) the array is set to exactly
//!   the skulls the signed-in profiles have on (their union), or
//!   [`DEFAULT_PROFILE`]'s with no named profile, before the level loads.
//! - **Menu**: [`ABOUT`] says what each skull does and where it is found;
//!   [`state`] is a profile's Locked, On or Off for it and [`toggle`] is a
//!   press. `crate::skulls_menu` lists all fifteen on its own screen, opened
//!   from the difficulty screen's "Skulls" row (docs/game/skulls.md "Menu").
//!
//! The logic is over [`Gm`] and the store, so it is tested without a guest.

use std::sync::{Arc, Mutex};

use settings::{Apply, Def, Kind, Menu, Scope, Store, Tier, Value};

use crate::layout_glue::Gm;

/// How many skulls the engine has (the array's length and the bound both
/// functions check).
pub const COUNT: usize = 15;
/// Every skull we list: the engine's fifteen, then ours from index
/// [`COUNT`] on (`crate::skulls_custom`, docs/game/skulls.md "Our skulls"). Ours
/// have no byte in the engine's array: their effects are our own hooks.
pub const TOTAL: usize = COUNT + crate::skulls_custom::OURS;
/// The skull-on array: one byte per skull.
pub const FLAGS: u32 = 0x004f_55dc;
/// The game options' mode word: 1 campaign, 2 multiplayer, 3 main menu.
pub const MODE_CAMPAIGN: u32 = 1;
/// Local player slots.
pub const SLOTS: u32 = 0x0054_e8e0;
pub const SLOT_STRIDE: u32 = 0xc70;
pub const LOCAL_PLAYERS: u32 = 4;
/// `slot + 0` bit: signed in with a profile.
pub const SLOT_SIGNED_IN: u32 = 0x10;
/// `slot +` the profile's name, UTF-16, 32 units.
pub const SLOT_NAME: u32 = 0xc1c;
pub const NAME_UNITS: usize = 32;
/// The menu screen the skull settings name.
pub const SCREEN: &str = "skulls";
/// The profile a pickup or a campaign start is kept under when no local
/// slot has a named profile signed in (the game's slots read as empty or
/// signed out), so a pickup is never dropped. The menu uses it the same way.
pub const DEFAULT_PROFILE: &str = "default";

const fn skull(key: &'static str, label: &'static str, order: u32) -> Def {
    Def {
        key,
        label,
        help: "A campaign skull this profile has picked up: on, it is turned on when a \
               campaign starts. Kept per profile under [profiles.\"<name>\".skulls].",
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
            screen: SCREEN,
            order,
        }),
    }
}

/// The skulls, in the engine's order: `SKULLS[n]` is `ice_cream_flavor_stock
/// n`. Names 0 to 13 are the game's own HUD names for each index (the string
/// ids `0x1df3e0` shows, table `0x445514`); 14 has none there and is the
/// community's Bandana (stocked by The Armory's script). From [`COUNT`] on
/// they are ours (`crate::skulls_custom`): hidden in the campaign by us, on
/// any difficulty, and stored the same way.
pub static SKULLS: [Def; TOTAL] = [
    skull("skulls.envy", "Envy", 0),
    skull("skulls.grunt_birthday_party", "Grunt Birthday Party", 1),
    skull("skulls.assassins", "Assassins", 2),
    skull("skulls.thunderstorm", "Thunderstorm", 3),
    skull("skulls.famine", "Famine", 4),
    skull("skulls.iwhbyd", "IWHBYD", 5),
    skull("skulls.blind", "Blind", 6),
    skull("skulls.ghost", "Ghost", 7),
    skull("skulls.black_eye", "Black Eye", 8),
    skull("skulls.catch", "Catch", 9),
    skull("skulls.sputnik", "Sputnik", 10),
    skull("skulls.iron", "Iron", 11),
    skull("skulls.mythic", "Mythic", 12),
    skull("skulls.angry", "Angry", 13),
    skull("skulls.bandana", "Bandana", 14),
    skull("skulls.acrophobia", "Acrophobia", 15),
    skull("skulls.low_gravity", "Low Gravity", 16),
    skull("skulls.fall_damage", "Fall Damage", 17),
    skull("skulls.unlimited_ammo", "Unlimited Ammo", 18),
    skull("skulls.unlimited_grenades", "Unlimited Grenades", 19),
    skull("skulls.invincible", "Invincible", 20),
];

/// What a skull is, for the menu: its name, what it does, where it is
/// found and whether only a Legendary game places it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct About {
    /// The game's own name (shared.map's English strings
    /// `unlocked_extra_1` to `unlocked_extra_14`, the names the pickup's HUD
    /// message shows); index 14 has none there, and Bandana is the
    /// community's name for it.
    pub name: &'static str,
    /// What it does, in our own words.
    pub effect: &'static str,
    /// The campaign level whose script stocks it.
    pub level: &'static str,
    /// Only a Legendary game places it: the level script that creates the
    /// skull (or the one that wakes that script) tests
    /// `(difficulty_legendary)` first. Ours are on any difficulty.
    pub legendary: bool,
    /// Anything else the script asks, for the description.
    pub note: Option<&'static str>,
    /// Gold (harder) or silver (for fun).
    pub tier: SkullTier,
}

/// A skull's kind, as MCC sorts its skulls (docs/game/skulls.md "Gold and
/// silver"): gold ones make the game harder (MCC scores them above 1.00x),
/// silver ones are for fun (1.00x or below).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkullTier {
    Gold,
    Silver,
}

impl SkullTier {
    pub fn name(self) -> &'static str {
        match self {
            SkullTier::Gold => "Gold",
            SkullTier::Silver => "Silver",
        }
    }
}

const fn about(
    name: &'static str,
    effect: &'static str,
    level: &'static str,
    legendary: bool,
    tier: SkullTier,
) -> About {
    About {
        name,
        effect,
        level,
        legendary,
        note: None,
        tier,
    }
}

/// Skull indices gold first, then silver, the engine's order within each,
/// retail's fifteen before ours: the order the Skulls screen lists them in.
pub fn by_tier() -> Vec<usize> {
    let mut v: Vec<usize> = (0..ABOUT.len()).collect();
    v.sort_by_key(|i| (is_ours(*i), ABOUT[*i].tier, *i));
    v
}

/// [`SKULLS`]' skulls, in the same order. Levels and the Legendary test are
/// read from each level's scripts (`h2 hs <map>`, docs/game/skulls.md "Where each
/// skull is"): every retail skull but Blind is placed on Legendary only.
/// Ours (from [`COUNT`]) are placed by us on any difficulty; their levels
/// are `crate::skulls_custom::SPOTS`'.
pub static ABOUT: [About; TOTAL] = [
    about(
        "Envy",
        "Your flashlight is swapped for active camouflage.",
        "Delta Halo",
        true,
        SkullTier::Silver,
    ),
    about(
        "Grunt Birthday Party",
        "A headshot on a Grunt bursts into confetti and cheering.",
        "The Arbiter",
        true,
        SkullTier::Silver,
    ),
    about(
        "Assassins",
        "Every enemy wears active camouflage.",
        "Regret",
        true,
        SkullTier::Gold,
    ),
    about(
        "Thunderstorm",
        "Enemies are promoted to their higher ranks.",
        "Cairo Station",
        true,
        SkullTier::Gold,
    ),
    about(
        "Famine",
        "Weapons dropped by enemies hold far less ammo.",
        "The Oracle",
        true,
        SkullTier::Gold,
    ),
    About {
        note: Some("solo; one try in seven"),
        ..about(
            "I Would Have Been Your Daddy",
            "Rare lines play far more often.",
            "Outskirts",
            true,
            SkullTier::Silver,
        )
    },
    about(
        "Blind",
        "Your HUD and first-person weapon are hidden.",
        "Outskirts",
        false,
        SkullTier::Gold,
    ),
    about(
        "Ghost",
        "Enemies no longer flinch when you hit them.",
        "Uprising",
        true,
        SkullTier::Silver,
    ),
    about(
        "Black Eye",
        "Your shields recharge only when you land a melee hit.",
        "The Great Journey",
        true,
        SkullTier::Gold,
    ),
    about(
        "Catch",
        "Enemies throw grenades much more often.",
        "Metropolis",
        true,
        SkullTier::Gold,
    ),
    about(
        "Sputnik",
        "Explosions and melee hits send bodies and objects flying.",
        "Quarantine Zone",
        true,
        SkullTier::Silver,
    ),
    about(
        "Iron",
        "A death sends you back to the last checkpoint.",
        "High Charity",
        true,
        SkullTier::Gold,
    ),
    about(
        "Mythic",
        "Enemies have twice the health.",
        "Sacred Icon",
        true,
        SkullTier::Gold,
    ),
    about(
        "Angry",
        "Enemies fire their weapons faster.",
        "Gravemind",
        true,
        SkullTier::Gold,
    ),
    About {
        note: Some("it appears once you keep Johnson waiting at the lift"),
        ..about(
            "Bandana",
            "Unlimited ammo and grenades.",
            "The Armory",
            true,
            SkullTier::Silver,
        )
    },
    // Ours (crate::skulls_custom).
    about(
        "Acrophobia",
        "You can fly: in the air, look up to climb and level out to hover; crouch to drop.",
        "Uprising",
        false,
        SkullTier::Silver,
    ),
    about(
        "Low Gravity",
        "Gravity is halved for everything.",
        "Delta Halo",
        false,
        SkullTier::Silver,
    ),
    about(
        "Fall Damage",
        "A hard landing hurts: past a jump's speed, a fall drains your shields, then your health.",
        "Metropolis",
        false,
        SkullTier::Gold,
    ),
    about(
        "Unlimited Ammo",
        "Your spare ammo never runs out, but you still reload; energy weapons never drain.",
        "Outskirts",
        false,
        SkullTier::Silver,
    ),
    about(
        "Unlimited Grenades",
        "Your grenades are topped up as you throw them.",
        "Sacred Icon",
        false,
        SkullTier::Silver,
    ),
    about(
        "Invincible",
        "You take no damage.",
        "The Great Journey",
        false,
        SkullTier::Silver,
    ),
];

/// Whether skull `index` is one of ours (no byte in the engine's array).
pub fn is_ours(index: usize) -> bool {
    (COUNT..TOTAL).contains(&index)
}

/// A skull on a profile's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not collected on this profile: shown, never toggled.
    Locked,
    /// Collected and turned on at the next campaign start.
    On,
    /// Collected and left off.
    Off,
}

/// Skull `index`'s state on `profile`: Locked until collected.
pub fn state(store: &Store, profile: &str, index: usize) -> Option<State> {
    let def = SKULLS.get(index)?;
    Some(
        match store.profile_get(profile, def.key).and_then(Value::as_bool) {
            None => State::Locked,
            Some(true) => State::On,
            Some(false) => State::Off,
        },
    )
}

/// The skull of a setting key.
pub fn index_of(key: &str) -> Option<usize> {
    SKULLS.iter().position(|d| d.key == key)
}

/// One state per skull, in engine order.
pub type Set = [bool; COUNT];

// ---- guest memory ----

/// The name of the profile signed in on local slot `slot`: `None` when the
/// slot is not signed in, unreadable, or the name is empty. Read as the
/// game copies it: at most 31 units, up to the first NUL.
pub fn slot_name<M: Gm>(m: &M, slot: u32) -> Option<String> {
    if slot >= LOCAL_PLAYERS {
        return None;
    }
    let base = SLOTS + slot * SLOT_STRIDE;
    if m.u32(base)? & SLOT_SIGNED_IN == 0 {
        return None;
    }
    let mut raw = [0u8; NAME_UNITS * 2];
    if !m.rd(base + SLOT_NAME, &mut raw) {
        return None;
    }
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take(NAME_UNITS - 1)
        .take_while(|u| *u != 0)
        .collect();
    let name = String::from_utf16_lossy(&units);
    (!name.trim().is_empty()).then_some(name)
}

/// Every signed-in profile's name, once each, in slot order.
pub fn signed_in<M: Gm>(m: &M) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for slot in 0..LOCAL_PLAYERS {
        if let Some(n) = slot_name(m, slot)
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

/// Skull `index`'s byte, when `index` is one the game accepts.
pub fn flag<M: Gm>(m: &M, index: u32) -> Option<bool> {
    (index < COUNT as u32)
        .then(|| m.u8(FLAGS + index))
        .flatten()
        .map(|b| b != 0)
}

/// The whole array.
pub fn flags<M: Gm>(m: &M) -> Option<Set> {
    let mut b = [0u8; COUNT];
    m.rd(FLAGS, &mut b).then(|| b.map(|v| v != 0))
}

/// Whether a stock call for `index` picked a skull up: the byte was 0 before
/// the game's function ran and is set after.
pub fn picked_up(index: u32, before: Option<bool>, after: Option<bool>) -> bool {
    index < COUNT as u32 && before == Some(false) && after == Some(true)
}

/// Sets the array to exactly `on`.
pub fn apply<M: Gm>(m: &mut M, on: &Set) -> bool {
    m.wr(FLAGS, &on.map(u8::from))
}

/// The mode of the game options at `options`.
pub fn options_mode<M: Gm>(m: &M, options: u32) -> Option<u32> {
    m.u32(options)
}

// ---- the store ----

/// Records skull `index` as collected and on for each of `profiles`.
/// Returns what to log: one line per profile, with the file written or why
/// nothing was.
pub fn record_pickup(store: &Store, profiles: &[String], index: usize) -> Vec<String> {
    let Some(def) = SKULLS.get(index) else {
        return Vec::new();
    };
    if profiles.is_empty() {
        return vec![format!(
            "skulls: {} picked up with no profile to keep it for; not kept",
            def.label
        )];
    }
    profiles
        .iter()
        .map(|p| {
            match (
                store.profile_set(p, def.key, Value::Bool(true)),
                store.file(),
            ) {
                (Ok(None), Some(path)) => format!(
                    "skulls: {} kept for {p}, saved to {}",
                    def.label,
                    path.display()
                ),
                (Ok(None), None) => format!(
                    "skulls: {} for {p} this run only; not saved: the settings store has no file",
                    def.label
                ),
                (Ok(Some(why)), _) => format!(
                    "skulls: {} for {p} this run only; not saved: {why}",
                    def.label
                ),
                (Err(e), _) => format!("skulls: {} for {p} not kept: {e}", def.label),
            }
        })
        .collect()
}

/// The skulls `profile` has collected, with whether each is on.
pub fn collected(store: &Store, profile: &str) -> Vec<(usize, bool)> {
    SKULLS
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            store
                .profile_get(profile, d.key)?
                .as_bool()
                .map(|on| (i, on))
        })
        .collect()
}

/// What a campaign start turns on for `profiles`: every skull any of them
/// has on. `None` with no profile, so the game's own state is kept.
pub fn selection(store: &Store, profiles: &[String]) -> Option<Set> {
    if profiles.is_empty() {
        return None;
    }
    let mut on = [false; COUNT];
    for p in profiles {
        for (i, v) in collected(store, p) {
            if i < COUNT {
                on[i] |= v;
            }
        }
    }
    Some(on)
}

/// Our skulls (indices from [`COUNT`]) that a campaign start turns on for
/// `profiles`: every one any of them has on, ascending.
pub fn custom_selection(store: &Store, profiles: &[String]) -> Vec<usize> {
    let mut on: Vec<usize> = profiles
        .iter()
        .flat_map(|p| collected(store, p))
        .filter(|(i, v)| *v && is_ours(*i))
        .map(|(i, _)| i)
        .collect();
    on.sort_unstable();
    on.dedup();
    on
}

/// A press on `profile`'s row of `key`: flip it. Only a collected skull can
/// be flipped. Returns what to log.
pub fn toggle(store: &Store, profile: &str, key: &str) -> Result<String, String> {
    let def = SKULLS
        .get(index_of(key).ok_or_else(|| format!("{key}: not a skull"))?)
        .expect("an index");
    let on = store
        .profile_get(profile, key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("skulls: {profile} has not collected {}", def.label))?;
    let now = !on;
    let saved = store
        .profile_set(profile, key, Value::Bool(now))
        .map_err(|e| e.to_string())?;
    let state = if now { "on" } else { "off" };
    Ok(match saved {
        None => format!("skulls: {} {state} for {profile}", def.label),
        Some(why) => format!(
            "skulls: {} {state} for {profile}; not saved: {why}",
            def.label
        ),
    })
}

// ---- hooks ----

/// What has been seen of the skull array: a pickup is a skull that is on
/// now and was not at the last look. Only the game's stock function and our
/// campaign start write the array, and the start tells the tracker what it
/// wrote, so any other change is a pickup, however it got there.
pub struct Tracker(Mutex<Set>);

impl Default for Tracker {
    /// The array as the game boots: every skull off.
    fn default() -> Self {
        Tracker(Mutex::new([false; COUNT]))
    }
}

impl Tracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Set> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Whether `now` differs from the last look.
    pub fn changed(&self, now: &Set) -> bool {
        *self.lock() != *now
    }
    /// The skulls on in `now` that were off at the last look; `now` becomes
    /// the last look.
    pub fn sweep(&self, now: Set) -> Vec<usize> {
        let mut last = self.lock();
        let new = (0..COUNT).filter(|i| now[*i] && !last[*i]).collect();
        *last = now;
        new
    }
    /// The array was set to `now` by us: not a pickup.
    pub fn reset(&self, now: Set) {
        *self.lock() = now;
    }
    /// The last look as a machine snapshot keeps it
    /// (docs/runtime/snapshots.md): a byte per skull. Without it a load
    /// would compare the loaded array with this run's last look and record
    /// the difference as pickups.
    pub fn save_state(&self) -> Vec<u8> {
        let mut w = snapshot::codec::Writer::new();
        for on in *self.lock() {
            w.bool(on);
        }
        w.into_bytes()
    }
    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let e = |e: snapshot::Error| e.to_string();
        let r = &mut snapshot::codec::Reader::new(bytes, "skulls");
        let mut last = [false; COUNT];
        for on in &mut last {
            *on = r.bool().map_err(e)?;
        }
        r.finish().map_err(e)?;
        self.reset(last);
        Ok(())
    }
}

/// Why `skulls` can't be snapshotted now: a pickup call is between its
/// before and after hooks, so the skull it is for and its byte before are
/// host state.
pub fn pickup_live(pending: &Mutex<Vec<Pending>>) -> Option<String> {
    let n = pending.lock().unwrap_or_else(|e| e.into_inner()).len();
    (n != 0).then(|| format!("{n} skull pickup call(s) between their hooks"))
}

/// What a pickup or a campaign start is for: every signed-in named profile,
/// else [`DEFAULT_PROFILE`] with why (the slots as read, for the log).
pub fn profiles_or_default<M: Gm>(m: &M) -> (Vec<String>, Option<String>) {
    let named = signed_in(m);
    if !named.is_empty() {
        return (named, None);
    }
    (
        vec![DEFAULT_PROFILE.to_owned()],
        Some(format!(
            "no named profile signed in ({}); using profile \"{DEFAULT_PROFILE}\"",
            slot_report(m)
        )),
    )
}

/// The profile of local slot `slot`: its name, else [`DEFAULT_PROFILE`].
pub fn profile_or_default<M: Gm>(m: &M, slot: u32) -> String {
    slot_name(m, slot).unwrap_or_else(|| DEFAULT_PROFILE.to_owned())
}

/// The local slots as read, for a log line: each slot's flags word and
/// name, whether or not it counts as signed in.
pub fn slot_report<M: Gm>(m: &M) -> String {
    (0..LOCAL_PLAYERS)
        .map(|slot| {
            let base = SLOTS + slot * SLOT_STRIDE;
            let Some(flags) = m.u32(base) else {
                return format!("slot {slot} unreadable");
            };
            let mut raw = [0u8; NAME_UNITS * 2];
            let name = if m.rd(base + SLOT_NAME, &mut raw) {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .take_while(|u| *u != 0)
                    .collect();
                format!("{:?}", String::from_utf16_lossy(&units))
            } else {
                "unreadable".into()
            };
            format!("slot {slot} flags {flags:#x} name {name}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The pickup hook's verdict after the game's stock function ran for skull
/// `index` (ESI at entry), which was `before`: records every skull that
/// came on since the last look for `profiles` and says why when `index`
/// itself was not kept. Takes the store's lock: never call it with guest
/// memory held.
pub fn after_stock(
    store: &Store,
    tracker: &Tracker,
    index: u32,
    before: Option<bool>,
    now: Option<Set>,
    profiles: &(Vec<String>, Option<String>),
) -> Vec<String> {
    let label = SKULLS
        .get(index as usize)
        .map_or("(no such skull)", |d| d.label);
    let Some(now) = now else {
        return vec![format!(
            "skulls: pickup of skull {index} ({label}) not kept: the skull array at {FLAGS:#x} is unreadable"
        )];
    };
    let new = tracker.sweep(now);
    let mut out = Vec::new();
    if !new.contains(&(index as usize)) {
        let why = if index as usize >= COUNT {
            "the game ignores that index".to_string()
        } else if before == Some(true) {
            "it was already on (stocked again)".to_string()
        } else if !now[index as usize] {
            "the game's function left it off".to_string()
        } else {
            "it was already on at the last look".to_string()
        };
        out.push(format!(
            "skulls: pickup of skull {index} ({label}) not kept: {why}"
        ));
    }
    out.extend(record(store, &new, profiles));
    out
}

/// A look at the array outside the pickup hook (each tick): records any
/// skull that came on without passing through it.
pub fn after_tick(
    store: &Store,
    tracker: &Tracker,
    now: Set,
    profiles: &(Vec<String>, Option<String>),
) -> Vec<String> {
    let new = tracker.sweep(now);
    if new.is_empty() {
        return Vec::new();
    }
    let names: Vec<&str> = new.iter().map(|i| SKULLS[*i].label).collect();
    let mut out = vec![format!(
        "skulls: {} turned on without the pickup hook (seen after a tick)",
        names.join(", ")
    )];
    out.extend(record(store, &new, profiles));
    out
}

fn record(
    store: &Store,
    new: &[usize],
    (names, fallback): &(Vec<String>, Option<String>),
) -> Vec<String> {
    let mut out = Vec::new();
    if !new.is_empty()
        && let Some(why) = fallback
    {
        out.push(format!("skulls: {why}"));
    }
    for i in new {
        out.extend(record_pickup(store, names, *i));
    }
    out
}

/// The campaign-start hook's work for options of mode `mode`: for a
/// campaign, the array to write (the profiles' selection) and what to log.
pub fn at_start(
    store: &Store,
    mode: Option<u32>,
    profiles: &(Vec<String>, Option<String>),
) -> Option<(Set, String)> {
    if mode != Some(MODE_CAMPAIGN) {
        return None;
    }
    let (names, fallback) = profiles;
    let on = selection(store, names)?;
    let ours = custom_selection(store, names);
    let labels: Vec<&str> = SKULLS
        .iter()
        .zip(on)
        .filter(|(_, o)| *o)
        .map(|(d, _)| d.label)
        .chain(ours.iter().map(|i| SKULLS[*i].label))
        .collect();
    Some((
        on,
        format!(
            "skulls: campaign start for {}: {}{}",
            names.join(", "),
            if labels.is_empty() {
                "none on".to_string()
            } else {
                labels.join(", ")
            },
            fallback
                .as_ref()
                .map_or(String::new(), |w| format!(" ({w})"))
        ),
    ))
}

/// A co-op guest's campaign start: exactly the host's engine skulls,
/// whatever this console's profiles have. Ours (from [`COUNT`]) have no
/// byte in the array: the start hook hands them to `crate::skulls_custom`
/// (`crate::coop_live::ours_of`).
pub fn coop_start(mode: Option<u32>, host: &[u8]) -> Option<(Set, String)> {
    if mode != Some(MODE_CAMPAIGN) {
        return None;
    }
    let mut on = [false; COUNT];
    for i in host {
        if let Some(b) = on.get_mut(usize::from(*i)) {
            *b = true;
        }
    }
    let labels: Vec<&str> = SKULLS
        .iter()
        .zip(on)
        .filter(|(_, o)| *o)
        .map(|(d, _)| d.label)
        .collect();
    Some((
        on,
        format!(
            "skulls: co-op campaign start with the host's skulls: {}",
            if labels.is_empty() {
                "none on".to_string()
            } else {
                labels.join(", ")
            }
        ),
    ))
}

/// A pickup call in progress: skull ESI and its byte before.
pub type Pending = (u32, Option<bool>);

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "skulls";

/// The skulls feature: hooks on the feature plan, so they chain with every
/// other feature's at the same points and a clash refuses the plan rather
/// than dropping one silently.
///
/// - [`SKULL_STOCK`](feature::points::SKULL_STOCK): `before` notes skull ESI
///   and its byte; `after` (the game's function has run) records it.
/// - [`OPTIONS_QUEUE`](feature::points::OPTIONS_QUEUE): `before` writes the
///   selection for a campaign start.
/// - the tick (`frame` bridge): records any skull that came on without the
///   pickup hook.
pub struct Skulls {
    pub store: Arc<Store>,
    pub tracker: Arc<Tracker>,
}

impl feature::Feature for Skulls {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Campaign skulls kept per profile")
            .doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::points::{OPTIONS_QUEUE as QUEUE, SKULL_STOCK};
        // The pickup in progress: (index, byte before). A stack, in case
        // the function is ever re-entered.
        let pending: Arc<Mutex<Vec<Pending>>> = Arc::default();
        // The store is the player's profiles, which a load does not rewind;
        // the tracker's last look is the run's, and a load puts it back.
        let (live, tracker) = (Arc::clone(&pending), Arc::clone(&self.tracker));
        let load = Arc::clone(&self.tracker);
        b.snapshot(feature::SnapshotPolicy::section_forbid_while(
            move || pickup_live(&live),
            move || tracker.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        {
            let pending = Arc::clone(&pending);
            b.raw().before(&SKULL_STOCK, "note the skull", move |c| {
                let index = c.cpu.esi;
                let before = flag(&*c.ctx.runtime().memory(), index);
                let label = SKULLS
                    .get(index as usize)
                    .map_or("(no such skull)", |d| d.label);
                let was = match before {
                    Some(true) => "on",
                    Some(false) => "off",
                    None => "unreadable",
                };
                eprintln!("skulls: pickup call for skull {index} ({label}), was {was}");
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((index, before));
                Ok(feature::Flow::Continue)
            });
        }
        {
            let (store, tracker) = (Arc::clone(&self.store), Arc::clone(&self.tracker));
            b.raw().after(&SKULL_STOCK, "record the pickup", move |c| {
                let Some((index, before)) = pending.lock().unwrap_or_else(|e| e.into_inner()).pop()
                else {
                    return Ok(());
                };
                let (now, profiles) = {
                    let mem = c.ctx.runtime().memory();
                    (flags(&*mem), profiles_or_default(&*mem))
                };
                for line in after_stock(&store, &tracker, index, before, now, &profiles) {
                    eprintln!("{line}");
                }
                Ok(())
            });
        }
        {
            let (store, tracker) = (Arc::clone(&self.store), Arc::clone(&self.tracker));
            b.raw()
                .before(&QUEUE, "apply the selection", move |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let (mode, profiles) = {
                        let mem = rt.memory();
                        (options_mode(&*mem, c.cpu.eax), profiles_or_default(&*mem))
                    };
                    // A co-op guest plays the host's skulls (docs/live/live-coop.md,
                    // "Co-op over Live").
                    let host = crate::coop_live::guest_skulls();
                    let start = match &host {
                        Some(host) => coop_start(mode, host),
                        None => at_start(&store, mode, &profiles),
                    };
                    if let Some((on, line)) = start {
                        if apply(&mut *rt.memory(), &on) {
                            tracker.reset(on);
                            eprintln!("{line}");
                        } else {
                            eprintln!("{line} (could not write the skulls)");
                        }
                        // Ours have no byte in the array: their hooks read
                        // this set (crate::skulls_custom).
                        // A co-op guest plays the host's.
                        crate::skulls_custom::set_campaign(&match &host {
                            Some(host) => crate::coop_live::ours_of(host),
                            None => custom_selection(&store, &profiles.0),
                        });
                    } else {
                        // Not a campaign: none of ours act in it.
                        crate::skulls_custom::set_campaign(&[]);
                    }
                    Ok(feature::Flow::Continue)
                })
                // Next to the queue: whatever else runs before it has run, so
                // the array is what the level loads with.
                .last();
        }
        let (store, tracker) = (Arc::clone(&self.store), Arc::clone(&self.tracker));
        b.frame().on_tick("watch the skulls", move |o| {
            let (now, profiles) = {
                let mem = o.guest();
                let Some(now) = flags(&*mem) else { return };
                if !tracker.changed(&now) {
                    return;
                }
                (now, profiles_or_default(&*mem))
            };
            for line in after_tick(&store, &tracker, now, &profiles) {
                eprintln!("{line}");
            }
        });
        Ok(())
    }
}

/// Adds the skulls feature to `registry` over `store`. Says why when it is
/// left out: no store (a headless run), `H2_NO_SKULLS`, or switched off.
/// Returns whether it was added; then call [`report`] after the plan is
/// installed.
pub fn install(
    registry: &mut feature::Registry,
    store: Option<Arc<Store>>,
) -> anyhow::Result<bool> {
    let Some(store) = store else {
        eprintln!("skulls: off: no settings store (a headless run); pickups are not kept");
        return Ok(false);
    };
    if std::env::var_os("H2_NO_SKULLS").is_some() {
        eprintln!("skulls: off: H2_NO_SKULLS is set");
        return Ok(false);
    }
    let added = registry
        .add(&Skulls {
            store,
            tracker: Arc::default(),
        })
        .map_err(|e| anyhow::anyhow!("skulls: {e}"))?;
    if !added {
        eprintln!("skulls: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

/// The install line, once `plan` is in: where the hooks went (the image's
/// addresses) and where pickups are saved, or why they are not.
pub fn report(plan: &feature::Plan, installed: &feature::Installed, store: &Store) -> String {
    use feature::points::{GAME_TICK, OPTIONS_QUEUE as QUEUE, SKULL_STOCK};
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!("skulls: NOT installed: {why}; pickups are not kept");
    }
    if !plan.is_on(FEATURE) {
        return "skulls: NOT installed: not in the feature plan; pickups are not kept".into();
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
        "skulls: hooks installed via feature plan at {}, {}, {}; saving to {}",
        at(&SKULL_STOCK, "pickup"),
        at(&QUEUE, "campaign start"),
        at(&GAME_TICK, "tick watch"),
        store
            .file()
            .map_or("nowhere (the store has no file)".into(), |p| p
                .display()
                .to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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

    fn defs() -> Vec<&'static Def> {
        SKULLS.iter().collect()
    }

    fn store() -> Store {
        Store::defaults(&defs())
    }

    /// A guest with the skull array zeroed and `profiles` signed in on
    /// slots 0, 1, ...
    fn guest(profiles: &[&str]) -> Fake {
        let mut f = Fake::default();
        f.wr(FLAGS, &[0; COUNT]);
        for slot in 0..LOCAL_PLAYERS {
            let base = SLOTS + slot * SLOT_STRIDE;
            let mut name = [0u8; NAME_UNITS * 2];
            let signed = match profiles.get(slot as usize) {
                Some(p) => {
                    for (i, u) in p.encode_utf16().enumerate() {
                        name[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
                    }
                    SLOT_SIGNED_IN | 0x3
                }
                None => 0x3,
            };
            f.set_u32(base, signed);
            f.wr(base + SLOT_NAME, &name);
        }
        f
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_catalog_is_the_engines_fifteen_then_ours() {
        assert_eq!(COUNT, 15);
        assert_eq!(SKULLS.len(), TOTAL);
        let mut keys: Vec<_> = SKULLS.iter().map(|d| d.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), TOTAL);
        assert!(
            SKULLS
                .iter()
                .all(|d| d.key.starts_with("skulls.") && d.check(d.default).is_ok())
        );
        assert_eq!(SKULLS[12].label, "Mythic", "Sacred Icon's script stocks 12");
        assert_eq!(SKULLS[13].label, "Angry", "Gravemind's script stocks 13");
        assert_eq!(index_of("skulls.iron"), Some(11));
        assert_eq!(index_of("graphics.fov"), None);
        // Every skull is in the store the game runs with.
        for d in &SKULLS {
            assert!(
                crate::setting_defs::ALL.iter().any(|a| a.key == d.key),
                "{}",
                d.key
            );
        }
    }

    #[test]
    fn profile_names_are_read_from_signed_in_slots_only() {
        let g = guest(&["Chief", "Arbiter"]);
        assert_eq!(slot_name(&g, 0).as_deref(), Some("Chief"));
        assert_eq!(slot_name(&g, 2), None, "not signed in");
        assert_eq!(slot_name(&g, 4), None, "no such slot");
        assert_eq!(signed_in(&g), names(&["Chief", "Arbiter"]));
        // A name filling all 32 units is cut at 31, as the game copies it.
        let long: String = "x".repeat(40);
        let g = guest(&[&long[..32]]);
        assert_eq!(slot_name(&g, 0).unwrap().len(), 31);
        // The same profile on two slots counts once; an empty name is none.
        assert_eq!(signed_in(&guest(&["A", "A", ""])), names(&["A"]));
        assert_eq!(signed_in(&Fake::default()), Vec::<String>::new());
    }

    #[test]
    fn a_pickup_is_a_byte_going_from_zero_to_one() {
        let mut g = guest(&["Chief"]);
        let before = flag(&g, 12);
        g.wr(FLAGS + 12, &[1]);
        assert!(picked_up(12, before, flag(&g, 12)));
        // Stocked again (already on): not a new pickup.
        assert!(!picked_up(12, flag(&g, 12), flag(&g, 12)));
        // Out of range: the game ignores it, so do we.
        assert_eq!(flag(&g, 15), None);
        assert!(!picked_up(15, Some(false), Some(true)));
        assert!(!picked_up(3, None, Some(true)), "unreadable before");
    }

    #[test]
    fn a_pickup_is_kept_for_every_signed_in_profile() {
        let s = store();
        let log = record_pickup(&s, &names(&["Chief", "Arbiter"]), 11);
        assert_eq!(
            log,
            [
                "skulls: Iron for Chief this run only; not saved: the settings store has no file",
                "skulls: Iron for Arbiter this run only; not saved: the settings store has no file"
            ]
        );
        assert_eq!(collected(&s, "Chief"), [(11, true)]);
        assert_eq!(collected(&s, "Arbiter"), [(11, true)]);
        assert!(collected(&s, "Nobody").is_empty());
        assert!(record_pickup(&s, &[], 11)[0].contains("not kept"));
        assert!(record_pickup(&s, &names(&["Chief"]), TOTAL).is_empty());
        // Ours are kept the same way (skulls_custom places them).
        assert_eq!(record_pickup(&s, &names(&["Chief"]), 15).len(), 1);
        assert_eq!(collected(&s, "Chief"), [(11, true), (15, true)]);
    }

    #[test]
    fn a_skull_is_locked_until_collected_and_only_then_toggles() {
        let s = store();
        assert!((0..COUNT).all(|i| state(&s, "Chief", i) == Some(State::Locked)));
        assert!((COUNT..TOTAL).all(|i| state(&s, "Chief", i) == Some(State::Locked)));
        assert_eq!(state(&s, "Chief", TOTAL), None);
        record_pickup(&s, &names(&["Chief"]), 12);
        record_pickup(&s, &names(&["Chief"]), 1);
        assert_eq!(state(&s, "Chief", 12), Some(State::On));
        assert_eq!(state(&s, "Chief", 1), Some(State::On));
        assert_eq!(state(&s, "Chief", 11), Some(State::Locked));
        assert_eq!(
            state(&s, "Arbiter", 12),
            Some(State::Locked),
            "another profile has none"
        );
        // A press flips a collected skull; an uncollected one is refused.
        assert_eq!(
            toggle(&s, "Chief", "skulls.mythic").unwrap(),
            "skulls: Mythic off for Chief"
        );
        assert_eq!(state(&s, "Chief", 12), Some(State::Off));
        assert_eq!(collected(&s, "Chief"), [(1, true), (12, false)]);
        assert!(toggle(&s, "Chief", "skulls.iron").is_err());
        assert_eq!(state(&s, "Chief", 11), Some(State::Locked));
        assert!(toggle(&s, "Chief", "graphics.fov").is_err());
    }

    /// Where each skull is and whether it needs Legendary, as the level
    /// scripts say (docs/game/skulls.md "Where each skull is").
    #[test]
    fn the_catalogue_follows_the_level_scripts() {
        let names: Vec<_> = ABOUT.iter().map(|a| a.name).collect();
        assert_eq!(
            names,
            [
                "Envy",
                "Grunt Birthday Party",
                "Assassins",
                "Thunderstorm",
                "Famine",
                "I Would Have Been Your Daddy",
                "Blind",
                "Ghost",
                "Black Eye",
                "Catch",
                "Sputnik",
                "Iron",
                "Mythic",
                "Angry",
                "Bandana",
                // Ours (skulls_custom).
                "Acrophobia",
                "Low Gravity",
                "Fall Damage",
                "Unlimited Ammo",
                "Unlimited Grenades",
                "Invincible",
            ]
        );
        // The settings' labels are the same names (IWHBYD shortened).
        for (d, a) in SKULLS.iter().zip(&ABOUT) {
            if d.label != "IWHBYD" {
                assert_eq!(d.label, a.name);
            }
        }
        let anywhere: Vec<_> = ABOUT
            .iter()
            .filter(|a| !a.legendary)
            .map(|a| a.name)
            .collect();
        assert_eq!(
            anywhere[0], "Blind",
            "only Outskirts' Blind skull has no Legendary test"
        );
        assert_eq!(
            anywhere.len(),
            1 + crate::skulls_custom::OURS,
            "ours are found on any difficulty"
        );
        assert_eq!(ABOUT[7].level, "Uprising", "08a_deltacliffs stocks 7");
        assert_eq!(
            ABOUT[11].level, "High Charity",
            "07b_forerunnership stocks 11"
        );
        assert_eq!(ABOUT[3].level, "Cairo Station");
        assert_eq!(ABOUT[14].level, "The Armory");
        assert!(
            ABOUT
                .iter()
                .all(|a| !a.effect.is_empty() && a.effect.ends_with('.'))
        );
    }

    #[test]
    fn a_campaign_start_applies_exactly_the_selected_skulls() {
        let s = store();
        record_pickup(&s, &names(&["Chief"]), 0);
        record_pickup(&s, &names(&["Chief"]), 12);
        record_pickup(&s, &names(&["Arbiter"]), 3);
        toggle(&s, "Chief", "skulls.envy").unwrap();
        let mut g = guest(&["Chief"]);
        // Skulls the game turned on this session but not selected go off.
        g.wr(FLAGS + 7, &[1]);
        let on = selection(&s, &signed_in(&g)).unwrap();
        assert!(apply(&mut g, &on));
        let mut want = [false; COUNT];
        want[12] = true;
        assert_eq!(flags(&g), Some(want));
        // Co-op: the union of both profiles' selections.
        let mut g = guest(&["Chief", "Arbiter"]);
        let both = selection(&s, &signed_in(&g)).unwrap();
        apply(&mut g, &both);
        want[3] = true;
        assert_eq!(flags(&g), Some(want));
        // No named profile: the game's own state is left alone.
        assert_eq!(selection(&s, &signed_in(&guest(&[]))), None);
        // A known profile with nothing collected clears what is not selected.
        assert_eq!(selection(&s, &names(&["Nobody"])), Some([false; COUNT]));
    }

    #[test]
    fn only_campaign_options_count_as_a_campaign_start() {
        let mut g = Fake::default();
        g.set_u32(0x1000, 1);
        g.set_u32(0x2000, 3);
        assert_eq!(options_mode(&g, 0x1000), Some(MODE_CAMPAIGN));
        assert_ne!(options_mode(&g, 0x2000), Some(MODE_CAMPAIGN));
        assert_eq!(options_mode(&g, 0x3000), None);
    }

    #[test]
    fn collected_skulls_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("h2-skulls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = settings::path_in(&dir);
        let (s, p) = Store::load(&defs(), Some(&path), &|_| None, &[]);
        assert!(p.is_empty(), "{p:?}");
        record_pickup(&s, &names(&["Chief"]), 5);
        toggle(&s, "Chief", "skulls.iwhbyd").unwrap();
        let (again, p) = Store::load(&defs(), Some(&path), &|_| None, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(collected(&again, "Chief"), [(5, false)]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[profiles.Chief.skulls]"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp_store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("h2-skulls-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = settings::path_in(&dir);
        let (s, p) = Store::load(&defs(), Some(&path), &|_| None, &[]);
        assert!(p.is_empty(), "{p:?}");
        (s, dir)
    }

    fn on(indexes: &[usize]) -> Set {
        let mut s = [false; COUNT];
        for i in indexes {
            s[*i] = true;
        }
        s
    }

    #[test]
    fn the_tracker_sees_each_skull_come_on_once_and_not_what_we_wrote() {
        let t = Tracker::default();
        assert!(!t.changed(&[false; COUNT]), "the game boots with none on");
        assert_eq!(t.sweep(on(&[12])), [12]);
        assert!(t.sweep(on(&[12])).is_empty(), "seen already");
        // A campaign start wrote 3 and 12: not pickups.
        t.reset(on(&[3, 12]));
        assert!(!t.changed(&on(&[3, 12])));
        assert_eq!(t.sweep(on(&[3, 7, 12])), [7]);
        // A skull going off is never a pickup; on again, it is.
        assert!(t.sweep(on(&[3])).is_empty());
        assert_eq!(t.sweep(on(&[3, 12])), [12]);
    }

    /// Case: no named profile signed in (the slot test fails, or the game
    /// signs in no profile). Kept under the default profile, with the slots
    /// as read in the log.
    #[test]
    fn a_pickup_with_no_named_profile_is_kept_under_the_default_profile() {
        let (s, dir) = tmp_store("nobody");
        let g = guest(&[]);
        let profiles = profiles_or_default(&g);
        assert_eq!(profiles.0, names(&[DEFAULT_PROFILE]));
        let t = Tracker::default();
        let log = after_stock(&s, &t, 12, Some(false), Some(on(&[12])), &profiles);
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(
            log[0],
            "skulls: no named profile signed in (slot 0 flags 0x3 name \"\", slot 1 flags 0x3 name \"\", \
             slot 2 flags 0x3 name \"\", slot 3 flags 0x3 name \"\"); using profile \"default\""
        );
        let path = settings::path_in(&dir);
        assert_eq!(
            log[1],
            format!(
                "skulls: Mythic kept for default, saved to {}",
                path.display()
            )
        );
        assert_eq!(collected(&s, DEFAULT_PROFILE), [(12, true)]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[profiles.default.skulls]") && text.contains("mythic = true"),
            "{text}"
        );
        // Signed in but the name empty, or unreadable slots: the same.
        assert_eq!(
            profiles_or_default(&guest(&[""])).0,
            names(&[DEFAULT_PROFILE])
        );
        let (_, why) = profiles_or_default(&Fake::default());
        assert!(why.unwrap().contains("slot 0 unreadable"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Case: a named profile (what the slot holds, a Live gamertag or not)
    /// is kept by that name, and the line names the file.
    #[test]
    fn a_pickup_is_kept_by_the_slots_name_with_the_file_in_the_log() {
        let (s, dir) = tmp_store("named");
        let profiles = profiles_or_default(&guest(&["Chief", "Arbiter"]));
        assert_eq!(profiles, (names(&["Chief", "Arbiter"]), None));
        let log = after_stock(
            &s,
            &Tracker::default(),
            11,
            Some(false),
            Some(on(&[11])),
            &profiles,
        );
        let path = settings::path_in(&dir);
        assert_eq!(
            log,
            [
                format!("skulls: Iron kept for Chief, saved to {}", path.display()),
                format!("skulls: Iron kept for Arbiter, saved to {}", path.display()),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Case: no settings file (a store without a path): kept for the run,
    /// and the line says it was not saved.
    #[test]
    fn a_store_without_a_file_says_the_pickup_is_not_saved() {
        let s = store();
        let log = record_pickup(&s, &names(&["Chief"]), 11);
        assert_eq!(
            log,
            ["skulls: Iron for Chief this run only; not saved: the settings store has no file"]
        );
        assert_eq!(collected(&s, "Chief"), [(11, true)]);
    }

    /// Case: a stock call that keeps nothing says why.
    #[test]
    fn a_pickup_that_keeps_nothing_says_why() {
        let s = store();
        let p = (names(&["Chief"]), None);
        let t = Tracker::default();
        t.reset(on(&[12]));
        let why = |log: Vec<String>| log.join("\n");
        assert_eq!(
            why(after_stock(&s, &t, 12, Some(true), Some(on(&[12])), &p)),
            "skulls: pickup of skull 12 (Mythic) not kept: it was already on (stocked again)"
        );
        assert_eq!(
            why(after_stock(&s, &t, 3, Some(false), Some(on(&[12])), &p)),
            "skulls: pickup of skull 3 (Thunderstorm) not kept: the game's function left it off"
        );
        assert_eq!(
            why(after_stock(&s, &t, 15, None, Some(on(&[12])), &p)),
            "skulls: pickup of skull 15 (Acrophobia) not kept: the game ignores that index"
        );
        assert_eq!(
            why(after_stock(&s, &t, 3, None, None, &p)),
            "skulls: pickup of skull 3 (Thunderstorm) not kept: the skull array at 0x4f55dc is unreadable"
        );
        assert!(collected(&s, "Chief").is_empty());
    }

    /// Case: a skull that comes on without passing through `0x1df3e0` is
    /// still kept, from the watch after each tick.
    #[test]
    fn a_skull_that_skips_the_pickup_hook_is_kept_from_the_tick() {
        let s = store();
        let p = (names(&["Chief"]), None);
        let t = Tracker::default();
        assert!(after_tick(&s, &t, [false; COUNT], &p).is_empty());
        let log = after_tick(&s, &t, on(&[0, 14]), &p);
        assert_eq!(
            log[0],
            "skulls: Envy, Bandana turned on without the pickup hook (seen after a tick)"
        );
        assert_eq!(collected(&s, "Chief"), [(0, true), (14, true)]);
        assert!(after_tick(&s, &t, on(&[0, 14]), &p).is_empty(), "once");
        // After the pickup hook saw it, the tick does not record it again.
        after_stock(&s, &t, 5, Some(false), Some(on(&[0, 5, 14])), &p);
        assert!(after_tick(&s, &t, on(&[0, 5, 14]), &p).is_empty());
    }

    #[test]
    fn a_campaign_start_with_no_named_profile_uses_the_default_profiles_skulls() {
        let s = store();
        record_pickup(&s, &names(&[DEFAULT_PROFILE]), 12);
        let p = profiles_or_default(&guest(&[]));
        let (set, line) = at_start(&s, Some(MODE_CAMPAIGN), &p).unwrap();
        assert_eq!(set, on(&[12]));
        assert!(
            line.starts_with("skulls: campaign start for default: Mythic (no named profile"),
            "{line}"
        );
        assert!(at_start(&s, Some(2), &p).is_none(), "multiplayer");
        assert!(at_start(&s, None, &p).is_none(), "unreadable options");
        let (none, line) = at_start(&s, Some(MODE_CAMPAIGN), &(names(&["Chief"]), None)).unwrap();
        assert_eq!(none, [false; COUNT]);
        assert_eq!(line, "skulls: campaign start for Chief: none on");
    }

    /// The skulls feature on the plan, with another feature's hook on the
    /// options queue (as anything else at `0x18e790` would be) and a
    /// stand-in for the game's stock function: all three hooks run through
    /// the real runner, chained, and the install line names them.
    /// Synthetic: the fixture module's leaf (`eax = 41; ret`) is the stock
    /// function, its `ret` (`0x404000`) the options queue, its `eax = 1;
    /// ret` (`0x403005`) the tick.
    #[test]
    fn the_hooks_run_on_the_feature_plan_alongside_others() {
        use feature::points::{GAME_TICK, OPTIONS_QUEUE as QUEUE, SKULL_STOCK};
        use runtime::guest::Permissions;
        use runtime::{Dispatch, Event, Guest, Runner, Runtime, exec::HARNESS_EXIT};
        use std::sync::atomic::{AtomicU32, Ordering};
        const LEAF: u32 = 0x40_2000;
        const RET: u32 = 0x40_4000;
        const TICK: u32 = 0x40_3005;
        const FRAME: u32 = 0x60_0e00;
        const OPTIONS: u32 = 0x60_0100;

        let mut d = Dispatch::default();
        // SAFETY: the fixture's tables, handlers and probes follow its ABI.
        unsafe {
            d.register_generated_module(
                &runtime_test_probe::h2gen_fixture_module,
                runtime_test_probe::h2gen_fixture_module_abi_version,
            )
            .unwrap();
        }
        let mut g = Guest::new();
        g.map_zeroed(0x60_0000, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(0x40_0000, 0x5000, Permissions::RX).unwrap();
        g.map_zeroed(FLAGS & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        g.map_zeroed(SLOTS & !0xfff, 0x4000, Permissions::RW)
            .unwrap();
        let rt = Runtime::new(g, d);
        {
            let mut mem = rt.memory();
            let mut name = Vec::new();
            for u in "Chief".encode_utf16() {
                name.extend_from_slice(&u.to_le_bytes());
            }
            mem.set_u32(SLOTS, SLOT_SIGNED_IN).unwrap();
            mem.write_bytes(SLOTS + SLOT_NAME, &name, 1).unwrap();
            mem.set_u32(OPTIONS, MODE_CAMPAIGN).unwrap();
        }
        let (s, dir) = tmp_store("plan");
        let store = Arc::new(s);
        let mut r = feature::Registry::new(feature::Switches::defaults());
        // The game's stock: sets byte ESI, then `eax = 41; ret`.
        r.add(&feature::from_fn("stand-in", |b| {
            b.raw().implement(&SKULL_STOCK, "stock", |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                if cpu.esi < COUNT as u32 {
                    mem.set_u8(FLAGS + cpu.esi, 1)?;
                }
                cpu.eax = 41;
                mem.ret(cpu, 0)
            });
            Ok(())
        }))
        .unwrap();
        let other = Arc::new(AtomicU32::new(0));
        let o = Arc::clone(&other);
        r.add(&feature::from_fn("other", move |b| {
            let o = Arc::clone(&o);
            b.raw().before(&QUEUE, "count", move |_| {
                o.fetch_add(1, Ordering::Relaxed);
                Ok(feature::Flow::Continue)
            });
            Ok(())
        }))
        .unwrap();
        assert!(install(&mut r, Some(Arc::clone(&store))).unwrap());
        let plan = r
            .build(&feature::Remap::new(&[
                (&SKULL_STOCK, LEAF),
                (&QUEUE, RET),
                (&GAME_TICK, TICK),
            ]))
            .unwrap();
        let text = plan.text();
        assert!(text.contains("feature skulls: on"), "{text}");
        assert!(
            text.contains(
                "SKULL_STOCK: before [skulls:note the skull] -> implement stand-in:stock -> after [skulls:record the pickup]"
            ),
            "{text}"
        );
        assert!(
            text.contains("OPTIONS_QUEUE: before [other:count, skulls:apply the selection]"),
            "{text}"
        );
        let installed = plan.install(&rt).unwrap();
        let line = report(&plan, &installed, &store);
        assert!(
            line.starts_with("skulls: hooks installed via feature plan at 0x402000 (pickup), 0x404000 (campaign start), 0x403005 (tick watch); saving to "),
            "{line}"
        );
        let run = |at: u32, esi: u32, eax: u32| {
            rt.memory().set_u32(FRAME, HARNESS_EXIT).unwrap();
            let mut cpu = runtime::CpuAbi {
                esp: FRAME,
                esi,
                eax,
                ..Default::default()
            };
            let mut ctx = runtime::ExecCtx::new(rt.clone(), 1);
            let t = Runner::default().run(&mut cpu, &mut ctx, at);
            assert!(
                matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
                "{:?}",
                ctx.event(t.event_id)
            );
            assert_eq!(cpu.esp, FRAME + 4);
            cpu
        };
        // The pickup: the stand-in ran (its result came back) and Mythic is kept.
        assert_eq!(run(LEAF, 12, 0).eax, 41);
        assert_eq!(collected(&store, "Chief"), [(12, true)]);
        // A skull set some other way is kept from the tick.
        rt.memory().set_u8(FLAGS + 9, 1).unwrap();
        run(TICK, 0, 0);
        assert_eq!(collected(&store, "Chief"), [(9, true), (12, true)]);
        // Turned off in the menu; the campaign start writes the selection,
        // and the other feature's hook ran too.
        toggle(&store, "Chief", "skulls.catch").unwrap();
        run(RET, 0, OPTIONS);
        assert_eq!(other.load(Ordering::Relaxed), 1);
        assert_eq!(flags(&*rt.memory()), Some(on(&[12])));
        // What the start wrote is not a pickup.
        run(TICK, 0, 0);
        assert_eq!(collected(&store, "Chief"), [(9, false), (12, true)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_line_says_when_the_feature_is_off() {
        let empty = feature::Registry::new(feature::Switches::defaults())
            .build(&feature::Base)
            .unwrap();
        assert_eq!(
            report(&empty, &feature::Installed::default(), &store()),
            "skulls: NOT installed: not in the feature plan; pickups are not kept"
        );
        let mut installed = feature::Installed::default();
        installed
            .off
            .push((FEATURE, "SKULL_STOCK has no generated original".into()));
        assert_eq!(
            report(&empty, &installed, &store()),
            "skulls: NOT installed: SKULL_STOCK has no generated original; pickups are not kept"
        );
        let mut r = feature::Registry::new(feature::Switches::defaults());
        assert!(!install(&mut r, None).unwrap(), "no store");
    }

    #[test]
    fn a_snapshot_carries_the_trackers_last_look() {
        let a = Tracker::default();
        let mut on = [false; COUNT];
        on[3] = true;
        on[COUNT - 1] = true;
        a.reset(on);
        let bytes = a.save_state();
        assert_eq!(bytes.len(), COUNT);
        let b = Tracker::default();
        b.load_state(&bytes).unwrap();
        assert!(!b.changed(&on), "the loaded run's last look");
        assert_eq!(b.save_state(), bytes);
        // Not a pickup after the load: the skulls were on at the save.
        assert_eq!(b.sweep(on), Vec::<usize>::new());
        assert!(b.load_state(&bytes[1..]).is_err(), "short");
        let mut long = bytes.clone();
        long.push(0);
        assert!(b.load_state(&long).is_err(), "trailing");
        assert!(b.load_state(&[2; COUNT]).is_err(), "not a bool");
        assert!(!b.changed(&on), "a bad load changes nothing");
    }

    #[test]
    fn snapshots_wait_while_a_pickup_is_between_its_hooks() {
        let pending = Mutex::new(Vec::new());
        assert_eq!(pickup_live(&pending), None);
        pending.lock().unwrap().push((3, Some(false)));
        assert_eq!(
            pickup_live(&pending).as_deref(),
            Some("1 skull pickup call(s) between their hooks")
        );
        pending.lock().unwrap().clear();
        assert_eq!(pickup_live(&pending), None);
    }

    /// Gold and silver as MCC scores them (docs/game/skulls.md "Gold and
    /// silver"): gold above 1.00x.
    #[test]
    fn gold_skulls_make_it_harder_and_come_first() {
        let gold: Vec<&str> = ABOUT
            .iter()
            .filter(|a| a.tier == SkullTier::Gold)
            .map(|a| a.name)
            .collect();
        assert_eq!(
            gold,
            [
                "Assassins",
                "Thunderstorm",
                "Famine",
                "Blind",
                "Black Eye",
                "Catch",
                "Iron",
                "Mythic",
                "Angry",
                "Fall Damage"
            ]
        );
        let order = by_tier();
        assert_eq!(order.len(), ABOUT.len());
        assert_eq!(&order[..2], &[2, 3]);
        assert_eq!(&order[9..15], &[0, 1, 5, 7, 10, 14]);
        // Ours after retail's, gold first too.
        assert_eq!(&order[15..], &[17, 15, 16, 18, 19, 20]);
    }
}
