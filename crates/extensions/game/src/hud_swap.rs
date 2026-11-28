//! Draws the HUD style [`crate::hud`] picks on converted Halo 1 maps
//! (docs/halo1/hud.md).
//!
//! A converted map carries both HUDs for its player biped: Halo 1's
//! (`h1objects::h1hud`, which the biped names as built) and Halo 2's with
//! the health strip (`h1objects::health_hud`). The converter records each
//! pair (owner, Halo 1's and Halo 2's `nhdt` datums): a multiplayer
//! package's in its manifest (`[[variant]] hud`), a campaign level's beside
//! it (`<data>/campaigns/<key>/<level>.hud.toml`, `content::LevelHud`). At each game activation this points every
//! owner's HUD references at the HUD the style asks for
//! ([`crate::hud::style_for_map`] of the loaded map and the player's
//! choice), in guest memory only, both ways, so a change of the setting
//! between games takes.
//!
//! **Why a swap works.** The HUD reads its `nhdt` datums from the owner's
//! tag every frame: a weapon's from `weap +0x2b4` (E `0x0013b071`, the
//! datum of the reference at `+0x2b0`), a unit's from its HUD interfaces
//! (`unit +0x1a0`, 8-byte `nhdt` references). Both are tag data, loaded
//! with the map and never copied, so rewriting the datum before the game
//! starts changes what is drawn. No map variant is needed.
//!
//! **When.** Before [`GAME_ACTIVATE`](hooks::points::GAME_ACTIVATE),
//! after the map's tags are loaded and before any HUD draws.
//!
//! **The motion sensor.** Halo 2's `nhdt` does not place the sensor's
//! blips: the game draws them (`0x1a2823`) around a point from its own
//! table, [`SENSOR_CENTRES`] (E `0x00139614`..`0x00139685`: anchor 2,
//! bottom left, plus the table's entry for the screen mode). Halo 1's HUD
//! centres them elsewhere, so while it is shown the table holds Halo 1's
//! centre (the converter's `hud_sensor`) for every screen mode, as the
//! Halo 1 widgets use one offset for all; every other activation puts
//! back what the table held before the first write ([`Sensor`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::{GroupTag, entry_layout, tag_entry_checked, tag_get_checked};

pub use crate::hud::HudStyle;

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

/// Halo 2's motion sensor blip centres (`.data`), two `i16` (x, y from the
/// bottom-left corner) per screen mode: fullscreen, half and quarter
/// screen (V: (41, -42), (31, -31), (31, -31), the centre of the retail
/// 83 x 83 `motion_sensor` background), then an unused fourth.
pub const SENSOR_CENTRES: u32 = 0x0047_ff40;
pub const SENSOR_MODES: usize = 3;

/// The sensor table's entries for the three screen modes.
pub type SensorTable = [[i16; 2]; SENSOR_MODES];

/// One converted map's HUD swaps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapHud {
    /// Whether they hold only while converted multiplayer maps are in the
    /// Halo 1 sandbox (a package's Halo 1 variant; a campaign level's
    /// always do).
    pub sandboxed: bool,
    pub swaps: Vec<content::HudSwap>,
    /// Where Halo 1's HUD centres the motion sensor (`hud_sensor`).
    pub sensor: Option<[i16; 2]>,
}

/// Swaps by map name (the cache header's, lower case).
pub type Table = HashMap<String, MapHud>;

/// Reads the sensor table.
pub fn read_sensor(m: &dyn GuestMemory) -> Result<SensorTable, MemFault> {
    let mut t = [[0; 2]; SENSOR_MODES];
    for (i, e) in t.iter_mut().enumerate() {
        let at = SENSOR_CENTRES + 4 * i as u32;
        *e = [m.u16(at)? as i16, m.u16(at + 2)? as i16];
    }
    Ok(t)
}

fn write_sensor(m: &mut dyn GuestMemory, t: &SensorTable) -> Result<(), MemFault> {
    for (i, [x, y]) in t.iter().enumerate() {
        let at = SENSOR_CENTRES + 4 * i as u32;
        m.set_u16(at, *x as u16)?;
        m.set_u16(at + 2, *y as u16)?;
    }
    Ok(())
}

/// The motion sensor's side of the swap: Halo 2's own table, as read
/// before the first write, and what each activation writes.
#[derive(Debug, Default)]
pub struct Sensor {
    retail: Option<SensorTable>,
}

impl Sensor {
    /// Writes Halo 1's `centre` for every screen mode when `style` is
    /// Halo 1's and the map has one, else Halo 2's own table back.
    /// Returns whether the table changed.
    pub fn activate(
        &mut self,
        m: &mut dyn GuestMemory,
        centre: Option<[i16; 2]>,
        style: HudStyle,
    ) -> Result<bool, MemFault> {
        let now = read_sensor(&*m)?;
        let retail = *self.retail.get_or_insert(now);
        let want = match (style, centre) {
            (HudStyle::Halo1, Some(c)) => [c; SENSOR_MODES],
            _ => retail,
        };
        if want == now {
            return Ok(false);
        }
        write_sensor(m, &want)?;
        Ok(true)
    }

    /// Halo 2's own table, as a machine snapshot keeps it
    /// (docs/runtime/snapshots.md): after a Halo 1 map the guest's table
    /// holds Halo 1's centre, so a load must not read it again as retail.
    pub fn save_state(&self) -> Vec<u8> {
        let w = &mut snapshot::codec::Writer::new();
        w.bool(self.retail.is_some());
        for [x, y] in self.retail.unwrap_or_default() {
            w.u32(x as u16 as u32).u32(y as u16 as u32);
        }
        std::mem::take(w).into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let r = &mut snapshot::codec::Reader::new(bytes, FEATURE);
        let e = |e: snapshot::Error| e.to_string();
        let some = r.bool().map_err(e)?;
        let mut t: SensorTable = [[0; 2]; SENSOR_MODES];
        for entry in &mut t {
            for v in entry.iter_mut() {
                *v = r.u32().map_err(e)? as u16 as i16;
            }
        }
        r.finish().map_err(e)?;
        self.retail = some.then_some(t);
        Ok(())
    }
}

/// Points each swap's owner at the HUD of `style`. Returns how many
/// references changed. An owner that is not a loaded `bipd` or `weap`, or
/// a target that is not a loaded `nhdt`, is left alone.
pub fn apply(
    m: &mut dyn GuestMemory,
    swaps: &[content::HudSwap],
    style: HudStyle,
) -> Result<usize, MemFault> {
    let mut changed = 0;
    for s in swaps {
        let (from, to) = match style {
            HudStyle::Halo1 => (s.halo2, s.halo1),
            HudStyle::Halo2 => (s.halo1, s.halo2),
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
                out.insert(
                    m.base.map.to_ascii_lowercase(),
                    MapHud {
                        sandboxed: true,
                        swaps: v.hud.clone(),
                        sensor: v.hud_sensor,
                    },
                );
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
                out.insert(
                    level.to_owned(),
                    MapHud {
                        sandboxed: false,
                        swaps: h.hud,
                        sensor: h.sensor,
                    },
                );
            }
        }
    }
    out
}

struct HudSwap {
    table: Arc<RwLock<Table>>,
    sensor: Arc<std::sync::Mutex<Sensor>>,
    /// Campaign levels with a Halo 2-assets map ([`campaign_halo2_levels`]):
    /// their pairs are the Halo 1 map's datums, so they hold only while
    /// the campaign's assets are Halo 1's.
    halo2_levels: Arc<HashSet<String>>,
}

/// Levels whose stored Halo 2-assets variant uses different HUD datums.
pub fn campaign_halo2_levels(data_root: &Path) -> HashSet<String> {
    crate::campaigns::halo2_maps(data_root).into_keys().collect()
}

fn locked(s: &std::sync::Mutex<Sensor>) -> std::sync::MutexGuard<'_, Sensor> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

impl hooks::Feature for HudSwap {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Halo 1's or Halo 2's HUD on converted Halo 1 maps")
            .doc("docs/halo1/hud.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use hooks::points::GAME_ACTIVATE;
        let table = Arc::clone(&self.table);
        let sensor = Arc::clone(&self.sensor);
        let halo2_levels = Arc::clone(&self.halo2_levels);
        b.raw()
            .before(&GAME_ACTIVATE, "HUD swap", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let Ok(name) = crate::health::loaded_map_name(&*mem) else {
                    return Ok(Flow::Continue);
                };
                let table = table.read().unwrap_or_else(|e| e.into_inner());
                let key = name.trim().to_ascii_lowercase();
                let hud = table
                    .get(&key)
                    .filter(|h| !h.sandboxed || crate::health::halo1_sandbox())
                    // The Halo 2-assets map of a campaign level is served.
                    .filter(|_| {
                        crate::h1_variants::campaign_assets_halo1() || !halo2_levels.contains(&key)
                    });
                // From the map and the choice directly, not `hud::style()`,
                // so the order against the style's game-start observer does
                // not matter.
                let style = crate::hud::style_for_map(&name, crate::hud::halo1_chosen());
                if let Some(h) = hud {
                    match apply(&mut *mem, &h.swaps, style) {
                        Ok(n) if n > 0 => {
                            eprintln!("hud: {name}: {n} HUD references now {}'s", style.name())
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("hud: {name}: owners unreadable ({e:?})"),
                    }
                }
                // Every activation, so a map after a Halo 1 one gets Halo
                // 2's sensor back.
                let centre = hud.and_then(|h| h.sensor);
                let mut sensor = sensor.lock().unwrap_or_else(|e| e.into_inner());
                match sensor.activate(&mut *mem, centre, style) {
                    Ok(true) => eprintln!(
                        "hud: {name}: motion sensor centred for {}'s HUD",
                        style.name()
                    ),
                    Ok(false) => {}
                    Err(e) => eprintln!("hud: {name}: motion sensor table unwritable ({e:?})"),
                }
                Ok(Flow::Continue)
            })
            // Neither reads what the other writes; the plan wants an order.
            .after_feature(crate::precision::FEATURE)
            .after_feature(crate::aim_assist::FEATURE)
            .after_feature(crate::variant_ext::FEATURE);
        // The table and the Halo 2 levels are read at install; the sensor
        // remembers Halo 2's table, which the guest's no longer shows.
        let (save, load) = (Arc::clone(&self.sensor), Arc::clone(&self.sensor));
        b.snapshot(hooks::SnapshotPolicy::section(
            move || locked(&save).save_state(),
            move |bytes, _| locked(&load).load_state(bytes),
        ));
        Ok(())
    }
}

/// Adds the feature, with the swaps of the packages under `packages` and
/// the campaign levels under `data_root`.
pub fn install(
    registry: &mut hooks::Registry,
    packages: &Path,
    data_root: &Path,
) -> anyhow::Result<bool> {
    let mut table = read_campaign_table(data_root);
    table.extend(read_table(packages));
    if !table.is_empty() {
        eprintln!("hud: HUD swaps for {} converted maps", table.len());
    }
    registry
        .add(&HudSwap {
            table: Arc::new(RwLock::new(table)),
            sensor: Arc::default(),
            halo2_levels: Arc::new(campaign_halo2_levels(data_root)),
        })
        .map_err(|e| anyhow::anyhow!("hud: {e}"))
}

#[cfg(test)]
#[path = "hud_swap_tests.rs"]
mod tests;
