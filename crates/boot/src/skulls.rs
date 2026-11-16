//! Campaign skulls kept per profile (docs/skulls.md).
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
//! What we add:
//!
//! - **Pickup**: a hook on `0x1df3e0` that runs the game's own function and,
//!   when byte `n` went from 0 to 1, records skull `n` as collected (and on)
//!   for every signed-in local profile. A profile is its name: the 32
//!   UTF-16 units at `slot + 0xc1c` of the local player slots at `0x54e8e0`
//!   (stride `0xc70`, signed in when `slot + 0` has bit 4), which is what
//!   `0x1902c1` returns and what campaign start copies into each player.
//! - **Storage**: one [`Def`] per skull (`skulls.<name>`, bool) in the
//!   settings [`Store`], kept per profile ([`Store::profile_set`]): no value
//!   means not collected; `true` or `false` means collected and on or off.
//!   The profile values live in `settings.toml` under
//!   `[profiles."<name>".skulls]`, never in the signed Xbox profile.
//! - **Start**: a hook on the options queue `0x18e790` (EAX = the new game
//!   options). For a campaign (`+0` = 1) with at least one named profile
//!   signed in, the array is set to exactly the skulls those profiles have
//!   on (their union), before the level loads. With no named profile it is
//!   left as the game has it.
//! - **Menu**: [`screen_for`] gives the "Skulls" screen of a profile as
//!   rows of the `ui` model, one per collected skull; [`toggle`] is a press.
//!   The guest list wiring that shows it from the difficulty screen is not
//!   built yet (docs/skulls.md "Menu").
//!
//! The logic is over [`Gm`] and the store, so it is tested without a guest.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use runtime::TransferAbi;
use runtime::dispatch::Dispatch;
use runtime::exec::OriginalHandler;
use runtime::guest::Permissions;
use settings::{Apply, Def, Kind, Menu, Scope, Store, Tier, Value};

use crate::layout_glue::Gm;
use crate::ui::{Row, Screen};

/// How many skulls the engine has (the array's length and the bound both
/// functions check).
pub const COUNT: usize = 15;
/// The skull-on array: one byte per skull.
pub const FLAGS: u32 = 0x004f_55dc;
/// Turn skull ESI on (plain `ret`): `ice_cream_flavor_stock`'s worker.
pub const STOCK: u32 = 0x001d_f3e0;
/// Queue new game options (EAX = the options, plain `ret`).
pub const OPTIONS_QUEUE: u32 = 0x0018_e790;
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
/// Our screen in the `ui` model.
pub const SCREEN: &str = "skulls";
/// Our guest memory: the hooks' aliases (RX).
const SHADOW_BASE: u32 = 0xa2f6_0000;

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
/// community's Bandana (stocked by The Armory's script).
pub static SKULLS: [Def; COUNT] = [
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
];

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
/// Returns what to log.
pub fn record_pickup(store: &Store, profiles: &[String], index: usize) -> Vec<String> {
    let Some(def) = SKULLS.get(index) else {
        return Vec::new();
    };
    if profiles.is_empty() {
        return vec![format!(
            "skulls: {} picked up with no profile signed in; not kept",
            def.label
        )];
    }
    profiles
        .iter()
        .map(|p| match store.profile_set(p, def.key, Value::Bool(true)) {
            Ok(None) => format!("skulls: {} kept for {p}", def.label),
            Ok(Some(why)) => format!(
                "skulls: {} for {p} this run only; not saved: {why}",
                def.label
            ),
            Err(e) => format!("skulls: {} for {p}: {e}", def.label),
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
            on[i] |= v;
        }
    }
    Some(on)
}

/// `profile`'s Skulls screen in the `ui` model: one row per collected skull,
/// in engine order. No rows when it has none (the model then leaves out any
/// row that opens it).
pub fn screen_for(store: &Store, profile: &str) -> Screen {
    collected(store, profile)
        .into_iter()
        .fold(Screen::new(SCREEN).title("Skulls"), |s, (i, _)| {
            s.row(Row::setting(SKULLS[i].key))
        })
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

/// The override already at `at` (another feature's), else the generated
/// original: what a hook runs underneath itself, so it chains rather than
/// displaces (as `level_boot::install_activate` does).
fn below(d: &Dispatch, at: u32, original: Arc<OriginalHandler>) -> Arc<OriginalHandler> {
    if let Some(n) = d.override_name(at) {
        eprintln!("skulls: chained after {n:?} on {at:#x}");
    }
    d.override_handler(at).map_or(original, Arc::new)
}

fn announce(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("skulls: {what} active");
    }
}

/// The pickup hook's body after the game's function ran: skull `index`
/// was `before`; record it when it went on. Takes the store's lock: never
/// call it with guest memory held.
fn after_stock(
    store: &Store,
    index: u32,
    before: Option<bool>,
    after: Option<bool>,
    profiles: Vec<String>,
) {
    if picked_up(index, before, after) {
        for line in record_pickup(store, &profiles, index as usize) {
            eprintln!("{line}");
        }
    }
}

/// Install the pickup and campaign-start hooks over `store`.
/// `H2_NO_SKULLS=1` skips both. A missing original skips its hook and says
/// so.
pub fn install(rt: &runtime::Runtime, store: Arc<Store>) -> anyhow::Result<()> {
    if std::env::var_os("H2_NO_SKULLS").is_some() {
        return Ok(());
    }
    let stock = rt.resolve_original(STOCK).map(Arc::new);
    let queue = rt.resolve_original(OPTIONS_QUEUE).map(Arc::new);
    if stock.is_none() || queue.is_none() {
        eprintln!(
            "skulls: no generated entry for the pickup or the campaign start; skulls not kept"
        );
        return Ok(());
    }
    rt.memory()
        .map_zeroed(SHADOW_BASE, 0x1000, Permissions::RX)?;
    rt.edit_dispatch(|d| {
        if let Some(o) = stock {
            install_stock(d, STOCK, SHADOW_BASE, o, Arc::clone(&store));
        }
        if let Some(o) = queue {
            install_queue(d, OPTIONS_QUEUE, o, Arc::clone(&store));
        }
    });
    Ok(())
}

/// The pickup hook on `at`: note skull ESI's byte, run what was there
/// (through `shadow`), then record a pickup.
pub(crate) fn install_stock(
    d: &mut Dispatch,
    at: u32,
    shadow: u32,
    original: Arc<OriginalHandler>,
    store: Arc<Store>,
) {
    let below = below(d, at, original);
    d.register_override(shadow, "skulls:stock-below", move |cpu, ctx, _| {
        Ok(below.invoke(cpu, ctx, at))
    });
    let seen = AtomicBool::new(false);
    d.register_override(at, "skulls:stock", move |cpu, ctx, _| {
        announce(&seen, "pickup hook");
        let index = cpu.esi;
        let store = Arc::clone(&store);
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        let before = flag(&*mem, index);
        let ret_pc = mem.u32(cpu.esp)?;
        cpu.esp = cpu.esp.wrapping_add(4);
        ctx.call_guest(cpu, &mut mem, shadow, move |_, ctx| {
            let (after, profiles) = {
                let mem = ctx.runtime().memory();
                (flag(&*mem, index), signed_in(&*mem))
            };
            after_stock(&store, index, before, after, profiles);
            Ok(TransferAbi::ret(ret_pc))
        })
    });
}

/// The campaign-start hook on `at`: for campaign options (EAX), set the
/// array to the signed-in profiles' skulls, then run what was there.
pub(crate) fn install_queue(
    d: &mut Dispatch,
    at: u32,
    original: Arc<OriginalHandler>,
    store: Arc<Store>,
) {
    let below = below(d, at, original);
    let seen = AtomicBool::new(false);
    d.register_override(at, "skulls:campaign-start", move |cpu, ctx, _| {
        announce(&seen, "campaign start hook");
        let (mode, profiles) = {
            let mem = ctx.runtime().memory();
            (options_mode(&*mem, cpu.eax), signed_in(&*mem))
        };
        if mode == Some(MODE_CAMPAIGN)
            && let Some(on) = selection(&store, &profiles)
        {
            let ok = apply(&mut *ctx.runtime().memory(), &on);
            let names: Vec<&str> = SKULLS
                .iter()
                .zip(on)
                .filter(|(_, o)| *o)
                .map(|(d, _)| d.label)
                .collect();
            eprintln!(
                "skulls: campaign start for {}: {}{}",
                profiles.join(", "),
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                },
                if ok {
                    ""
                } else {
                    " (could not write the skulls)"
                }
            );
        }
        Ok(below.invoke(cpu, ctx, at))
    });
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
    fn the_catalog_is_the_engines_fifteen() {
        assert_eq!(SKULLS.len(), COUNT);
        let mut keys: Vec<_> = SKULLS.iter().map(|d| d.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), COUNT);
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
                "skulls: Iron kept for Chief",
                "skulls: Iron kept for Arbiter"
            ]
        );
        assert_eq!(collected(&s, "Chief"), [(11, true)]);
        assert_eq!(collected(&s, "Arbiter"), [(11, true)]);
        assert!(collected(&s, "Nobody").is_empty());
        assert!(record_pickup(&s, &[], 11)[0].contains("not kept"));
        assert!(record_pickup(&s, &names(&["Chief"]), 15).is_empty());
    }

    #[test]
    fn the_menu_has_rows_for_collected_skulls_only() {
        let s = store();
        assert!(screen_for(&s, "Chief").rows().is_empty());
        record_pickup(&s, &names(&["Chief"]), 12);
        record_pickup(&s, &names(&["Chief"]), 1);
        let screen = screen_for(&s, "Chief");
        assert_eq!(screen.title, "Skulls");
        let keys: Vec<_> = screen.rows().iter().map(|r| r.kind.clone()).collect();
        assert_eq!(
            keys,
            [
                crate::ui::RowKind::Setting("skulls.grunt_birthday_party"),
                crate::ui::RowKind::Setting("skulls.mythic")
            ]
        );
        assert!(
            screen_for(&s, "Arbiter").rows().is_empty(),
            "another profile has none"
        );
        // A press flips a collected skull; an uncollected one is refused.
        assert_eq!(
            toggle(&s, "Chief", "skulls.mythic").unwrap(),
            "skulls: Mythic off for Chief"
        );
        assert_eq!(collected(&s, "Chief"), [(1, true), (12, false)]);
        assert!(toggle(&s, "Chief", "skulls.iron").is_err());
        assert!(toggle(&s, "Chief", "graphics.fov").is_err());
        assert_eq!(
            screen_for(&s, "Chief").rows().len(),
            2,
            "off rows stay listed"
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

    /// The hooks chain onto whatever is on the function already and run it:
    /// synthetic, with the fixture module's leaf (`eax = 41; ret`) as the
    /// function and an earlier override standing in for the game's stock
    /// (it sets the byte, as `0x1df3e0` does).
    #[test]
    fn the_pickup_hook_runs_the_function_and_records_a_new_skull() {
        use runtime::{Event, Guest, Runner, Runtime, exec::HARNESS_EXIT};
        const LEAF: u32 = 0x40_2000;
        const SHADOW_AT: u32 = 0x40_5000;
        const FRAME: u32 = 0x60_0e00;

        let mut d = Dispatch::default();
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
        g.map_zeroed(SHADOW_AT, 0x1000, Permissions::RX).unwrap();
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
        }
        let original = Arc::new(rt.resolve_original(LEAF).expect("the fixture leaf"));
        let store = Arc::new(store());
        let o = Arc::clone(&original);
        rt.edit_dispatch(|d| {
            d.register_override(LEAF, "stand-in stock", move |cpu, ctx, sel| {
                let i = cpu.esi;
                ctx.runtime().memory().set_u8(FLAGS + i, 1)?;
                Ok(o.invoke(cpu, ctx, sel))
            });
            install_stock(
                d,
                LEAF,
                SHADOW_AT,
                Arc::clone(&original),
                Arc::clone(&store),
            );
        });
        let run = |esi: u32| {
            rt.memory().set_u32(FRAME, HARNESS_EXIT).unwrap();
            let mut cpu = runtime::CpuAbi {
                esp: FRAME,
                esi,
                ..Default::default()
            };
            let mut ctx = runtime::ExecCtx::new(rt.clone(), 1);
            let t = Runner::default().run(&mut cpu, &mut ctx, LEAF);
            assert!(
                matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
                "{:?}",
                ctx.event(t.event_id)
            );
            assert_eq!(cpu.eax, 41, "what was there ran and its result came back");
            assert_eq!(cpu.esp, FRAME + 4);
        };
        run(12);
        assert_eq!(collected(&store, "Chief"), [(12, true)]);
        // The player turns it off in the menu; stocked again while already
        // on (a reload of the level script), nothing changes.
        toggle(&store, "Chief", "skulls.mythic").unwrap();
        run(12);
        assert_eq!(collected(&store, "Chief"), [(12, false)]);
    }
}
