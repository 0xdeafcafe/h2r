//! Stronger aim assist, mainly for the iOS touch controls
//! (docs/game/aim-assist.md).
//!
//! **What Halo 2 reads.** Each weapon tag holds its aim-assist cone:
//! autoaim angle and range, magnetism angle and range (`weap +0x208` to
//! `+0x214`, f32, radians and world units). `0x001a50a0` reads them for
//! the held weapon through the tag entry table, dividing the angles by the
//! zoom magnification and multiplying the ranges by it, and takes the
//! larger of the deviation angle (`+0x218`) and the autoaim angle as how far
//! a shot may bend towards its target. The globals tag's player-control
//! block (`matg +0xf0`, elements of `0x80`) holds magnetism friction (`+0`,
//! how much the crosshair slows over a target) and adhesion (`+4`, how much
//! it follows one); the player-control update `0x00185be0` reads both at
//! `0x0018622b` and caps each at 1.0. Offsets: the engine reads above;
//! names: Assembly's `Plugins/Halo2/weap.xml` and `matg.xml` at
//! eecb829971d3b58d7bd8285d9b4dfe86425dda6f.
//!
//! **What we change.** At each game activation (`0x18f000`, after the
//! scenario's cache is loaded) every loaded `weap` tag's two angles and two
//! ranges, and the globals' friction and adhesion, are set to retail times
//! the level's factors ([`Level::factors`]), in guest memory only. Angles
//! are capped at [`MAX_ANGLE`], friction and adhesion at 1.0 (the engine's
//! own cap), and nothing is ever made weaker than retail.
//!
//! **Idempotent.** We remember, per field, the retail value and what we
//! wrote. A field still holding what we wrote is scaled from the remembered
//! retail value; any other value is a fresh load (a new map or a revert put
//! retail back) and becomes the new retail value. So a reload, a revert or
//! a change of level never compounds, and Default writes retail back.
//!
//! **Multiplayer.** The tag values are shared by every player a machine
//! simulates, so a multiplayer game (options mode 2) keeps the tags retail
//! whatever the setting says. The touch player's stronger aim assist there
//! is per player instead, applied around that local player's own control
//! update and never to a tag ([`player`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use settings::{Setting, Store};

use crate::layout_glue::Gm;

pub mod player;

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "aim-assist";

/// The tag-entry table pointer (16-byte entries: group, datum, address, size).
pub const TAG_ENTRIES: u32 = engine::tags::TAG_ENTRIES_GLOBAL;
/// The tag-index header pointer; its `+0x18` is the tag slot count.
pub const TAG_INDEX: u32 = engine::tags::TAG_INDEX_GLOBAL;
const TAG_COUNT: u32 = engine::tags::index_layout::TAG_COUNT;
const ENTRY_STRIDE: u32 = engine::tags::entry_layout::STRIDE;

/// The weapon group as the entry table stores it.
pub const WEAP: u32 = u32::from_be_bytes(*b"weap");
/// The globals group.
pub const MATG: u32 = u32::from_be_bytes(*b"matg");

/// `weap +0x208`: autoaim angle (radians), full strength inside it.
pub const AUTOAIM_ANGLE: u32 = 0x208;
/// `weap +0x20c`: autoaim range (world units).
pub const AUTOAIM_RANGE: u32 = 0x20c;
/// `weap +0x210`: magnetism angle (radians).
pub const MAGNETISM_ANGLE: u32 = 0x210;
/// `weap +0x214`: magnetism range (world units).
pub const MAGNETISM_RANGE: u32 = 0x214;
/// The weapon's last aim-assist byte: shorter tags are not touched.
const WEAP_MIN_SIZE: u32 = MAGNETISM_RANGE + 4;

/// `matg +0xf0`: the player-control block (count, address).
pub const MATG_PLAYER_CONTROL: u32 = 0xf0;
/// Its element size.
pub const PLAYER_CONTROL_STRIDE: u32 = 0x80;
/// Element `+0`: magnetism friction, `+4`: magnetism adhesion.
pub const MAGNETISM_FRICTION: u32 = 0x0;
pub const MAGNETISM_ADHESION: u32 = 0x4;
/// More player-control elements than this is not a globals tag we know.
const MAX_PLAYER_CONTROLS: u32 = 16;

/// No angle is raised past this (radians, about 28.6 degrees).
pub const MAX_ANGLE: f32 = 0.5;
/// Friction and adhesion: the engine caps both at 1.0 (`0x00186252`).
pub const MAX_STICK: f32 = 1.0;

/// The options mode of a multiplayer game.
pub const MODE_MULTIPLAYER: u32 = 2;

/// The setting's values (`crate::setting_defs::AIM_ASSIST`).
pub const SETTING_AUTO: i64 = 0;
pub const SETTING_DEFAULT: i64 = 1;
pub const SETTING_STRONG: i64 = 2;
pub const SETTING_VERY_STRONG: i64 = 3;

/// How strong the aim assist is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The tags' own values.
    Default,
    Strong,
    VeryStrong,
}

/// Multipliers on the retail values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Factors {
    /// Autoaim and magnetism angles.
    pub angle: f32,
    /// Autoaim and magnetism ranges.
    pub range: f32,
    /// Magnetism friction and adhesion.
    pub stick: f32,
}

impl Level {
    pub fn factors(self) -> Factors {
        match self {
            Level::Default => Factors {
                angle: 1.0,
                range: 1.0,
                stick: 1.0,
            },
            Level::Strong => Factors {
                angle: 1.5,
                range: 1.25,
                stick: 1.25,
            },
            Level::VeryStrong => Factors {
                angle: 2.0,
                range: 1.5,
                stick: 1.5,
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Default => "Default",
            Level::Strong => "Strong",
            Level::VeryStrong => "Very strong",
        }
    }
}

/// The level for setting value `setting`: Auto is Strong while the touch
/// controls show, else Default.
pub fn level_for(setting: i64, touch_shown: bool) -> Level {
    match setting {
        SETTING_STRONG => Level::Strong,
        SETTING_VERY_STRONG => Level::VeryStrong,
        SETTING_AUTO if touch_shown => Level::Strong,
        _ => Level::Default,
    }
}

/// `retail * factor` capped at `max`, but never below `retail`. A factor of
/// 1, or a retail value that is not a finite positive number, returns
/// `retail` bit for bit.
pub fn scale(retail: f32, factor: f32, max: f32) -> f32 {
    if factor == 1.0 || !(retail.is_finite() && retail > 0.0) {
        return retail;
    }
    (retail * factor).min(max).max(retail)
}

/// One field we changed: its retail bits and the bits we wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    retail: u32,
    written: u32,
}

/// What a pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Weapon tags walked.
    pub weapons: u32,
    /// Player-control elements walked.
    pub player_controls: u32,
    /// Fields whose bytes changed in this pass.
    pub writes: u32,
}

/// The fields we hold changed, keyed by (tag datum, field address).
#[derive(Debug, Default)]
pub struct Remembered(HashMap<(u32, u32), Slot>);

impl Remembered {
    /// How many fields are away from retail.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sets every weapon's and the globals' aim-assist fields to retail
    /// times `level`'s factors. Idempotent; Default restores retail.
    pub fn apply<M: Gm>(&mut self, m: &mut M, level: Level) -> Applied {
        let f = level.factors();
        let mut next = HashMap::new();
        let mut out = Applied::default();
        let (Some(entries), Some(index)) = (m.u32(TAG_ENTRIES), m.u32(TAG_INDEX)) else {
            self.0.clear();
            return out;
        };
        let count = m
            .u32(index.wrapping_add(TAG_COUNT))
            .unwrap_or(0)
            .min(0x1_0000);
        for i in 0..count {
            let e = entries.wrapping_add(i * ENTRY_STRIDE);
            let (Some(group), Some(datum), Some(addr), Some(size)) =
                (m.u32(e), m.u32(e + 4), m.u32(e + 8), m.u32(e + 0xc))
            else {
                continue;
            };
            if addr == 0 {
                continue;
            }
            let mut field = |m: &mut M, at: u32, factor: f32, max: f32| {
                self.settle(m, &mut next, (datum, at), |r| scale(r, factor, max))
            };
            if group == WEAP && size >= WEAP_MIN_SIZE {
                out.weapons += 1;
                for (off, factor, max) in [
                    (AUTOAIM_ANGLE, f.angle, MAX_ANGLE),
                    (AUTOAIM_RANGE, f.range, f32::MAX),
                    (MAGNETISM_ANGLE, f.angle, MAX_ANGLE),
                    (MAGNETISM_RANGE, f.range, f32::MAX),
                ] {
                    out.writes += u32::from(field(m, addr + off, factor, max));
                }
            } else if group == MATG && size >= MATG_PLAYER_CONTROL + 8 {
                let pc = addr + MATG_PLAYER_CONTROL;
                let (Some(n), Some(at)) = (m.u32(pc), m.u32(pc + 4)) else {
                    continue;
                };
                if n > MAX_PLAYER_CONTROLS || at == 0 {
                    continue;
                }
                for k in 0..n {
                    out.player_controls += 1;
                    let el = at.wrapping_add(k * PLAYER_CONTROL_STRIDE);
                    for off in [MAGNETISM_FRICTION, MAGNETISM_ADHESION] {
                        out.writes += u32::from(field(m, el + off, f.stick, MAX_STICK));
                    }
                }
            }
        }
        self.0 = next;
        out
    }

    /// One field: its retail value (remembered if it still holds what we
    /// wrote, else what it holds now), scaled, written if it differs.
    /// Returns whether the bytes changed.
    fn settle<M: Gm>(
        &self,
        m: &mut M,
        next: &mut HashMap<(u32, u32), Slot>,
        key: (u32, u32),
        scaled: impl Fn(f32) -> f32,
    ) -> bool {
        let Some(cur) = m.u32(key.1) else {
            return false;
        };
        let retail = match self.0.get(&key) {
            Some(s) if s.written == cur => s.retail,
            _ => cur,
        };
        let want = scaled(f32::from_bits(retail)).to_bits();
        let wrote = want != cur && m.set_u32(key.1, want);
        if want != retail {
            next.insert(
                key,
                Slot {
                    retail,
                    written: want,
                },
            );
        }
        wrote
    }
}

/// The level for this activation: retail tags in a multiplayer game (the
/// touch player's boost there is per player, [`player`]), else the
/// setting's (Auto looks at the touch controls now).
pub fn level_now(setting: i64, mode: Option<u32>) -> Level {
    if mode == Some(MODE_MULTIPLAYER) {
        return Level::Default;
    }
    level_for(setting, input::touch::controls_shown())
}

// ---- the feature ----

/// The activation hook and what it remembers.
pub struct AimAssist {
    setting: Setting<i64>,
    remembered: Arc<Mutex<Remembered>>,
    per_player: Arc<player::PerPlayer>,
}

impl AimAssist {
    pub fn new(store: &Store) -> Self {
        AimAssist {
            setting: store.handle::<i64>(crate::setting_defs::AIM_ASSIST.key),
            remembered: Arc::default(),
            per_player: Arc::default(),
        }
    }
}

impl feature::Feature for AimAssist {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Aim assist strength").doc("docs/game/aim-assist.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::Flow;
        use feature::points::GAME_ACTIVATE;
        let setting = self.setting.clone();
        let remembered = Arc::clone(&self.remembered);
        b.raw()
            .before(&GAME_ACTIVATE, "weapon aim assist", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let mode = Gm::u32(&*mem, c.cpu.eax);
                let level = level_now(setting.get(), mode);
                let mut r = remembered.lock().unwrap_or_else(|e| e.into_inner());
                let had = r.len();
                let done = r.apply(&mut *mem, level);
                if level != Level::Default || had != 0 {
                    eprintln!(
                        "aim-assist: {} ({} weapons, {} player controls, {} fields written)",
                        level.name(),
                        done.weapons,
                        done.player_controls,
                        done.writes
                    );
                }
                Ok(Flow::Continue)
            });

        // Multiplayer, per player: around one local player's control
        // update, and only for the touch player (player.rs).
        let setting = self.setting.clone();
        let pp = Arc::clone(&self.per_player);
        b.raw().before(
            &player::PLAYER_CONTROL_UPDATE,
            "touch player's aim assist (open)",
            move |c| {
                let controller = c.cpu.edx;
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let mode = crate::level_boot::active_mode(&*mem);
                let level = player::player_level(
                    setting.get(),
                    mode,
                    controller,
                    input::touch::controls_shown(),
                );
                pp.enter(&mut *mem, level);
                Ok(Flow::Continue)
            },
        );
        let pp = Arc::clone(&self.per_player);
        b.raw().after(
            &player::AIM_ASSIST_PARAMS,
            "touch player's weapon cone",
            move |c| {
                let out = c.cpu.esi;
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                pp.params(&mut *mem, out);
                Ok(())
            },
        );
        let pp = Arc::clone(&self.per_player);
        b.raw().after(
            &player::PLAYER_CONTROL_UPDATE,
            "touch player's aim assist (close)",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                pp.leave(&mut *mem);
                Ok(())
            },
        );
        Ok(())
    }
}

/// Adds the aim assist to `registry`. Says why when left out.
pub fn install(
    registry: &mut feature::Registry,
    store: Option<Arc<Store>>,
) -> anyhow::Result<bool> {
    let Some(store) = store else {
        eprintln!(
            "aim-assist: off: no settings store (a headless run); weapons keep retail aim assist"
        );
        return Ok(false);
    };
    let added = registry
        .add(&AimAssist::new(&store))
        .map_err(|e| anyhow::anyhow!("aim-assist: {e}"))?;
    if !added {
        eprintln!("aim-assist: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

#[cfg(test)]
mod tests;
