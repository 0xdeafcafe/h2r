//! Halo 1's HUD or Halo 2's on converted Halo 1 maps (docs/game/hud.md).
//!
//! A converted map carries both HUDs for its player biped: Halo 1's
//! (`h1objects::h1hud`, which the biped names as built) and Halo 2's with
//! the health strip (`h1objects::health_hud`). The converter records each
//! pair (owner, Halo 1's and Halo 2's `nhdt` datums): a multiplayer
//! package's in its manifest (`[[variant]] hud`), a campaign level's beside
//! it (`<data>/campaigns/<key>/<level>.hud.toml`, `content::LevelHud`). At each game activation this points every
//! owner's HUD references at the HUD [`mode`] asks for, in guest memory
//! only, both ways, so a change of the setting between games takes.
//!
//! **Why a swap works.** The HUD reads its `nhdt` datums from the owner's
//! tag every frame: a weapon's from `weap +0x2b4` (E `0x0013b071`, the
//! datum of the reference at `+0x2b0`), a unit's from its HUD interfaces
//! (`unit +0x1a0`, 8-byte `nhdt` references). Both are tag data, loaded
//! with the map and never copied, so rewriting the datum before the game
//! starts changes what is drawn. No map variant is needed.
//!
//! **When.** Before [`GAME_ACTIVATE`](feature::points::GAME_ACTIVATE),
//! after the map's tags are loaded and before any HUD draws.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::tags::{GroupTag, entry_layout, tag_entry_checked, tag_get_checked};

/// The feature's id (`features.h1-hud`).
pub const FEATURE: &str = "h1-hud";

/// `unit +0x1a0`: the HUD interfaces (count, address), 8 bytes each, the
/// datum at `+4`.
pub const UNIT_HUD_INTERFACES: u32 = 0x1a0;
pub const HUD_INTERFACE_LEN: u32 = 8;
/// `weap +0x2b0`: the weapon's HUD reference; its datum at `+0x2b4` is
/// what the HUD reads (E `0x0013b071`).
pub const WEAPON_HUD: u32 = 0x2b0;

const BIPD: u32 = u32::from_be_bytes(*b"bipd");
const WEAP: u32 = u32::from_be_bytes(*b"weap");
const NHDT: u32 = u32::from_be_bytes(*b"nhdt");

/// Which HUD a converted Halo 1 map shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudMode {
    /// Halo 1's HUD: the default.
    Halo1,
    /// Halo 2's HUD with Halo 1's health strip.
    Halo2,
}

impl HudMode {
    fn code(self) -> u8 {
        match self {
            HudMode::Halo1 => 1,
            HudMode::Halo2 => 2,
        }
    }
    fn from_code(c: u8) -> Self {
        if c == 2 { HudMode::Halo2 } else { HudMode::Halo1 }
    }
}

static MODE: AtomicU8 = AtomicU8::new(1);

/// Sets the HUD for converted Halo 1 maps (the pre-game option "HUD: Halo
/// 1 / Halo 2"). It stays until set again and is applied at the next game
/// activation. Safe from any thread.
pub fn set_mode(m: HudMode) {
    MODE.store(m.code(), Ordering::SeqCst);
}

/// The HUD converted Halo 1 maps show ([`set_mode`]; Halo 1's by default).
pub fn mode() -> HudMode {
    HudMode::from_code(MODE.load(Ordering::SeqCst))
}

/// Swaps by map name (the cache header's, lower case), and whether they
/// hold only while converted multiplayer maps are in the Halo 1 sandbox
/// (a package's Halo 1 variant; a campaign level's always do).
pub type Table = HashMap<String, (bool, Vec<content::HudSwap>)>;

/// Points each swap's owner at the HUD `mode` asks for. Returns how many
/// references changed. An owner that is not a loaded `bipd` or `weap`, or
/// a target that is not a loaded `nhdt`, is left alone.
pub fn apply(
    m: &mut dyn GuestMemory,
    swaps: &[content::HudSwap],
    mode: HudMode,
) -> Result<usize, MemFault> {
    let mut changed = 0;
    for s in swaps {
        let (from, to) = match mode {
            HudMode::Halo1 => (s.halo2, s.halo1),
            HudMode::Halo2 => (s.halo1, s.halo2),
        };
        if tag_get_checked(&*m, DatumHandle(to), GroupTag(NHDT))?.is_none() {
            continue;
        }
        let Some(entry) = tag_entry_checked(&*m, DatumHandle(s.owner))? else {
            continue;
        };
        let group = m.u32(entry + entry_layout::GROUP)?;
        let owner = m.u32(entry + entry_layout::ADDRESS)?;
        if owner == 0 {
            continue;
        }
        let datums: Vec<u32> = match group {
            BIPD => {
                let count = m.u32(owner + UNIT_HUD_INTERFACES)?.min(0x40);
                let first = m.u32(owner + UNIT_HUD_INTERFACES + 4)?;
                (0..count)
                    .map(|i| first + i * HUD_INTERFACE_LEN + 4)
                    .collect()
            }
            WEAP => vec![owner + WEAPON_HUD + 4],
            _ => continue,
        };
        for at in datums {
            if m.u32(at)? == from {
                m.set_u32(at, to)?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// The HUD swaps of every installed package's Halo 1 sandbox variant, by
/// map name, from `<packages>/<package>/manifest.toml`.
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
        for v in m.variants.iter().filter(|v| v.origin.sandbox == "h1") {
            if !v.hud.is_empty() {
                out.insert(m.base.map.to_ascii_lowercase(), (true, v.hud.clone()));
            }
        }
    }
    out
}

/// The HUD swaps of every converted campaign level under `<data>/campaigns`
/// (`<key>/<level>.hud.toml`), by level name.
pub fn read_campaign_table(data_root: &Path) -> Table {
    let mut out = Table::new();
    let Ok(dirs) = std::fs::read_dir(data_root.join("campaigns")) else {
        return out;
    };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_ascii_lowercase();
            let Some(level) = name.strip_suffix(".hud.toml") else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(f.path()) else {
                continue;
            };
            if let Ok(h) = content::LevelHud::from_toml(&text)
                && !h.hud.is_empty()
            {
                out.insert(level.to_owned(), (false, h.hud));
            }
        }
    }
    out
}

struct Hud {
    table: Arc<RwLock<Table>>,
}

impl feature::Feature for Hud {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Halo 1's or Halo 2's HUD on converted Halo 1 maps")
            .doc("docs/game/hud.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::Flow;
        use feature::points::GAME_ACTIVATE;
        let table = Arc::clone(&self.table);
        b.raw()
            .before(&GAME_ACTIVATE, "HUD swap", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let Ok(name) = crate::health::loaded_map_name(&*mem) else {
                    return Ok(Flow::Continue);
                };
                let table = table.read().unwrap_or_else(|e| e.into_inner());
                let Some((sandboxed, swaps)) = table.get(&name.trim().to_ascii_lowercase()) else {
                    return Ok(Flow::Continue);
                };
                if *sandboxed && !crate::health::halo1_sandbox() {
                    return Ok(Flow::Continue);
                }
                let mode = mode();
                match apply(&mut *mem, swaps, mode) {
                    Ok(n) if n > 0 => eprintln!("hud: {name}: {n} HUD references now {mode:?}'s"),
                    Ok(_) => {}
                    Err(e) => eprintln!("hud: {name}: owners unreadable ({e:?})"),
                }
                Ok(Flow::Continue)
            })
            // Neither reads what the other writes; the plan wants an order.
            .after_feature(crate::precision::FEATURE)
            .after_feature(crate::aim_assist::FEATURE);
        Ok(())
    }
}

/// Adds the feature, with the swaps of the packages under `packages` and
/// the campaign levels under `data_root`.
pub fn install(
    registry: &mut feature::Registry,
    packages: &Path,
    data_root: &Path,
) -> anyhow::Result<bool> {
    let mut table = read_campaign_table(data_root);
    table.extend(read_table(packages));
    if !table.is_empty() {
        eprintln!("hud: HUD swaps for {} converted maps", table.len());
    }
    registry
        .add(&Hud {
            table: Arc::new(RwLock::new(table)),
        })
        .map_err(|e| anyhow::anyhow!("hud: {e}"))
}

#[cfg(test)]
#[path = "hud_tests.rs"]
mod tests;
