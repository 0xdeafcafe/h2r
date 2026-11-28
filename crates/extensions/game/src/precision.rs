//! Precision variants on converted Halo 1 maps (docs/halo1/multiplayer.md,
//! "Precision variants").
//!
//! In the Halo 2 sandbox Halo 1's assault rifle becomes the SMG; in a
//! precision variant (its starting weapon the battle rifle, sniper rifle or
//! carbine) it becomes the battle rifle. The converter places the default
//! and records each swap in the package manifest (`[[variant]] precision`,
//! collection datums: `h1convert::apply::precision_swaps`). At each game
//! activation this rewrites the scenario's netgame equipment items, in guest
//! memory only: `from` becomes `to` in a precision variant, and `to` goes
//! back to `from` in any other.
//!
//! **When.** Before [`GAME_ACTIVATE`](hooks::points::GAME_ACTIVATE)
//! (`0x18f000`, options in `eax`), after the map's tags are loaded. The
//! netgame equipment spawner (`0x15aad0`, reading the item at `+0x58`,
//! `0x15ac83`) has one caller, the per-tick netgame update `0x15ae70`
//! (from `0x15c189` in `0x15c150`, called by the game tick at
//! `0x13803c`), so no item has spawned yet when the game activates.
//!
//! **What it reads.** The options' game type (`+0`, 2 multiplayer) and the
//! variant inline at options `+0x134`, its starting weapon byte at
//! `+0xd6` (`variants::record`, setting 89); the scenario at `0x4e0350`,
//! its netgame equipment at `+0x120` (count, address; `0x90` an element,
//! E `0x15aaf3`), each item reference at `+0x58` (group, datum). A swap's
//! target must be a loaded tag of the same group as what it replaces, or
//! nothing changes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::{GroupTag, tag_get_checked};

/// The feature's id (`features.h1-precision`).
pub const FEATURE: &str = "h1-precision";

/// The scenario pointer.
pub const SCENARIO: u32 = 0x004e_0350;
/// Netgame equipment: count, then address.
pub const NETGAME_EQUIPMENT: u32 = 0x120;
pub const NETGAME_ITEM_LEN: u32 = 0x90;
/// The item reference: group, then datum.
pub const NETGAME_ITEM: u32 = 0x58;
/// Options `+0`: the game type (1 campaign, 2 multiplayer).
pub const OPTIONS_TYPE: u32 = 0x0;
pub const MULTIPLAYER: u32 = 2;
/// The variant inline in the options, and its starting weapon byte.
pub const OPTIONS_VARIANT: u32 = 0x134;
pub const STARTING_WEAPON: u32 = 0xd6;
/// Starting weapons that make a variant a precision one: battle rifle,
/// sniper rifle, carbine (the weapon values of `rules::settings`).
pub const PRECISION_WEAPONS: [u8; 3] = [3, 10, 13];

/// Swaps by map name (the cache header's, lower case).
pub type Table = HashMap<String, Vec<content::Swap>>;

/// Whether the options at `options` start a precision variant.
pub fn is_precision(m: &dyn GuestMemory, options: u32) -> Result<bool, MemFault> {
    if m.u32(options + OPTIONS_TYPE)? != MULTIPLAYER {
        return Ok(false);
    }
    let w = m.u8(options + OPTIONS_VARIANT + STARTING_WEAPON)?;
    Ok(PRECISION_WEAPONS.contains(&w))
}

/// Rewrites the loaded scenario's netgame items for `swaps`, one way or the
/// other. Returns how many items changed.
pub fn apply(
    m: &mut dyn GuestMemory,
    swaps: &[content::Swap],
    precision: bool,
) -> Result<usize, MemFault> {
    let scenario = m.u32(SCENARIO)?;
    if scenario == 0 {
        return Ok(0);
    }
    let count = m.u32(scenario + NETGAME_EQUIPMENT)?;
    let first = m.u32(scenario + NETGAME_EQUIPMENT + 4)?;
    let mut changed = 0;
    for i in 0..count.min(0x1000) {
        let at = first + i * NETGAME_ITEM_LEN + NETGAME_ITEM;
        let group = m.u32(at)?;
        let datum = m.u32(at + 4)?;
        for s in swaps {
            let (old, new) = if precision {
                (s.from, s.to)
            } else {
                (s.to, s.from)
            };
            if datum != old {
                continue;
            }
            // The target is a loaded tag of the item's group.
            if tag_get_checked(&*m, DatumHandle(new), GroupTag(group))?.is_none() {
                continue;
            }
            m.set_u32(at + 4, new)?;
            changed += 1;
            break;
        }
    }
    Ok(changed)
}

/// The swaps of every installed package's Halo 2 sandbox variant, by map
/// name, from `<packages>/<package>/manifest.toml`.
pub fn read_table(packages: &Path) -> Table {
    let mut out = Table::new();
    let Ok(dir) = std::fs::read_dir(packages) else {
        return out;
    };
    for e in dir.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path().join("manifest.toml")) else {
            continue;
        };
        let Ok(m) = content::Manifest::from_toml(&text) else {
            continue;
        };
        for v in m.variants.iter().filter(|v| v.origin.sandbox == "h2") {
            if !v.precision.is_empty() {
                out.insert(m.base.map.to_ascii_lowercase(), v.precision.clone());
            }
        }
    }
    out
}

struct Precision {
    table: Arc<RwLock<Table>>,
}

impl hooks::Feature for Precision {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Precision variants on converted Halo 1 maps")
            .doc("docs/halo1/multiplayer.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use hooks::points::GAME_ACTIVATE;
        let table = Arc::clone(&self.table);
        b.raw()
            .before(&GAME_ACTIVATE, "precision item swap", move |c| {
                if crate::health::halo1_sandbox() {
                    return Ok(Flow::Continue);
                }
                let options = c.cpu.eax;
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let Ok(name) = crate::health::loaded_map_name(&*mem) else {
                    return Ok(Flow::Continue);
                };
                let table = table.read().unwrap_or_else(|e| e.into_inner());
                let Some(swaps) = table.get(&name.trim().to_ascii_lowercase()) else {
                    return Ok(Flow::Continue);
                };
                let precision = options != 0 && is_precision(&*mem, options).unwrap_or(false);
                match apply(&mut *mem, swaps, precision) {
                    Ok(n) if n > 0 => eprintln!(
                        "precision: {name}: {n} item spawns {}",
                        if precision {
                            "swapped for the precision weapon"
                        } else {
                            "back to the default weapon"
                        }
                    ),
                    Ok(_) => {}
                    Err(e) => eprintln!("precision: {name}: items unreadable ({e:?})"),
                }
                Ok(Flow::Continue)
            })
            // Aim assist scales weapon tags and the swap moves item
            // spawns; neither reads the other, but the plan wants an order.
            .after_feature(crate::aim_assist::FEATURE)
            // The precision choice is read from the options the host's
            // block was just written into.
            .after_feature(crate::variant_ext::FEATURE);
        // The swap table is read at install and never changes; the swap
        // itself is in guest memory.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature, with the swaps of the packages under `packages`.
pub fn install(registry: &mut hooks::Registry, packages: &Path) -> anyhow::Result<bool> {
    let table = read_table(packages);
    if !table.is_empty() {
        eprintln!("precision: swaps for {} converted maps", table.len());
    }
    registry
        .add(&Precision {
            table: Arc::new(RwLock::new(table)),
        })
        .map_err(|e| anyhow::anyhow!("precision: {e}"))
}

#[cfg(test)]
#[path = "precision_tests.rs"]
mod tests;
