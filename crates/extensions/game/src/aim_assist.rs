//! Stronger aim assist, mainly for the iOS touch controls
//! (docs/game/aim-assist.md).
//!
//! **What Halo 2 reads.** Each weapon tag holds its aim-assist cone:
//! aim cone angle and range, pull angle and range (`weap +0x208` to
//! `+0x214`, f32, radians and world units). `0x001a50a0` reads them for
//! the held weapon through the tag entry table, dividing the angles by the
//! zoom magnification and multiplying the ranges by it, and takes the
//! larger of the bend angle (`+0x218`) and the aim cone angle as how far
//! a shot may bend towards its target. The globals tag's player-control
//! block (`matg +0xf0`, elements of `0x80`) holds slowdown (`+0`,
//! how much the crosshair slows over a target) and follow (`+4`, how much
//! it follows one); the player-control update `0x00185be0` reads both at
//! `0x0018622b` and caps each at 1.0. Offsets: the engine reads above;
//! names: ours, after what those reads do.
//!
//! **What we change.** At each game activation (`0x18f000`, after the
//! scenario's cache is loaded) every loaded `weap` tag's two angles and two
//! ranges, and the globals' slowdown and follow, are set to retail times
//! the level's factors ([`Level::factors`]), in guest memory only. Angles
//! are capped at [`MAX_ANGLE`], slowdown and follow at 1.0 (the engine's
//! own cap), and nothing is ever made weaker than retail.
//!
//! **Idempotent.** We remember, per field, the retail value and what we
//! wrote. A field still holding what we wrote is scaled from the remembered
//! retail value; any other value is a fresh load (a new map or a revert put
//! retail back) and becomes the new retail value. So a reload, a revert or
//! a change of level never compounds, and Default writes retail back.
//!
//! **Live edits.** A field the agent's `edit.*` sets is the value the
//! user wants: the hot-edit glue's notice ([`tag_written`]) adopts it as
//! retail, and activations leave it unscaled while it holds the edit's
//! bits.
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
pub const TAG_ENTRIES: u32 = guest_layout::tags::TAG_ENTRIES_GLOBAL;
/// The tag-index header pointer; its `+0x18` is the tag slot count.
pub const TAG_INDEX: u32 = guest_layout::tags::TAG_INDEX_GLOBAL;
const TAG_COUNT: u32 = guest_layout::tags::index_layout::TAG_COUNT;
const ENTRY_STRIDE: u32 = guest_layout::tags::entry_layout::STRIDE;

/// The weapon group as the entry table stores it.
pub const WEAP: u32 = u32::from_be_bytes(*b"weap");
/// The globals group.
pub const MATG: u32 = u32::from_be_bytes(*b"matg");

/// `weap +0x208`: aim cone angle (radians), full strength inside it.
pub const CONE_ANGLE: u32 = 0x208;
/// `weap +0x20c`: aim cone range (world units).
pub const CONE_RANGE: u32 = 0x20c;
/// `weap +0x210`: pull angle (radians).
pub const PULL_ANGLE: u32 = 0x210;
/// `weap +0x214`: pull range (world units).
pub const PULL_RANGE: u32 = 0x214;
/// The weapon's last aim-assist byte: shorter tags are not touched.
const WEAP_MIN_SIZE: u32 = PULL_RANGE + 4;

/// `matg +0xf0`: the player-control block (count, address).
pub const MATG_PLAYER_CONTROL: u32 = 0xf0;
/// Its element size.
pub const PLAYER_CONTROL_STRIDE: u32 = 0x80;
/// Element `+0`: slowdown, `+4`: follow.
pub const SLOWDOWN: u32 = 0x0;
pub const FOLLOW: u32 = 0x4;
/// More player-control elements than this is not a globals tag we know.
const MAX_PLAYER_CONTROLS: u32 = 16;

/// No angle is raised past this (radians, about 28.6 degrees).
pub const MAX_ANGLE: f32 = 0.5;
/// Slowdown and follow: the engine caps both at 1.0 (`0x00186252`).
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
    /// Cone and pull angles.
    pub angle: f32,
    /// Cone and pull ranges.
    pub range: f32,
    /// Slowdown and follow.
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

/// The fields we hold changed, keyed by (tag datum, field address); and
/// the fields a live edit set ([`Remembered::adopt`]), with the bits it
/// left.
#[derive(Debug, Default)]
pub struct Remembered(HashMap<(u32, u32), Slot>, HashMap<(u32, u32), u32>);

impl Remembered {
    /// How many fields are away from retail.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The fields held, as a machine snapshot keeps them
    /// (docs/runtime/snapshots.md): without them the next activation
    /// after a load would take a scaled value for retail and scale again.
    pub fn save_state(&self) -> Vec<u8> {
        let mut held: Vec<((u32, u32), Slot)> = self.0.iter().map(|(k, v)| (*k, *v)).collect();
        held.sort_by_key(|(k, _)| *k);
        let mut w = snapshot::codec::Writer::new();
        w.list(&held, |w, ((datum, at), s)| {
            w.u32(*datum).u32(*at).u32(s.retail).u32(s.written);
        });
        w.into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let e = |e: snapshot::Error| e.to_string();
        let r = &mut snapshot::codec::Reader::new(bytes, "aim-assist");
        let held = r
            .list(16, |r| {
                let key = (r.u32()?, r.u32()?);
                let slot = Slot {
                    retail: r.u32()?,
                    written: r.u32()?,
                };
                Ok((key, slot))
            })
            .map_err(e)?;
        r.finish().map_err(e)?;
        self.0 = held.into_iter().collect();
        Ok(())
    }

    /// Sets every weapon's and the globals' aim-assist fields to retail
    /// times `level`'s factors. Idempotent; Default restores retail.
    pub fn apply<M: Gm>(&mut self, m: &mut M, level: Level) -> Applied {
        let f = level.factors();
        let mut next = HashMap::new();
        let mut edited = HashMap::new();
        let mut out = Applied::default();
        let (Some(entries), Some(index)) = (m.u32(TAG_ENTRIES), m.u32(TAG_INDEX)) else {
            self.0.clear();
            self.1.clear();
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
                self.settle(m, &mut next, &mut edited, (datum, at), |r| scale(r, factor, max))
            };
            if group == WEAP && size >= WEAP_MIN_SIZE {
                out.weapons += 1;
                for (off, factor, max) in [
                    (CONE_ANGLE, f.angle, MAX_ANGLE),
                    (CONE_RANGE, f.range, f32::MAX),
                    (PULL_ANGLE, f.angle, MAX_ANGLE),
                    (PULL_RANGE, f.range, f32::MAX),
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
                    for off in [SLOWDOWN, FOLLOW] {
                        out.writes += u32::from(field(m, el + off, f.stick, MAX_STICK));
                    }
                }
            }
        }
        self.0 = next;
        self.1 = edited;
        out
    }

    /// A live edit (the agent's `edit.*`, docs/editor/realtime-edits.md,
    /// "Realtime coverage") wrote `[start, end)` of tag `datum` of `group`
    /// at `address`: each aim-assist field there now holds the value in
    /// force, taken as retail and kept unscaled by the next activations
    /// while it holds those bits. A field that holds what we wrote (a
    /// revert put our value back) is ours again. How many fields were
    /// adopted.
    pub fn adopt<M: Gm>(&mut self, m: &M, group: u32, datum: u32, address: u32, start: u32, end: u32) -> usize {
        let mut fields = Vec::new();
        if group == WEAP {
            fields.extend([CONE_ANGLE, CONE_RANGE, PULL_ANGLE, PULL_RANGE].map(|o| address.wrapping_add(o)));
        } else if group == MATG {
            let pc = address.wrapping_add(MATG_PLAYER_CONTROL);
            if let (Some(n), Some(at)) = (m.u32(pc), m.u32(pc.wrapping_add(4)))
                && n <= MAX_PLAYER_CONTROLS
                && at != 0
            {
                for k in 0..n {
                    let el = at.wrapping_add(k * PLAYER_CONTROL_STRIDE);
                    fields.extend([el.wrapping_add(SLOWDOWN), el.wrapping_add(FOLLOW)]);
                }
            }
        }
        let mut adopted = 0;
        for at in fields {
            if at < start || at.saturating_add(4) > end {
                continue;
            }
            let Some(cur) = m.u32(at) else { continue };
            let key = (datum, at);
            if self.0.get(&key).is_some_and(|s| s.written == cur) {
                self.1.remove(&key);
            } else {
                self.1.insert(key, cur);
                adopted += 1;
            }
        }
        adopted
    }

    /// One field: its retail value (remembered if it still holds what we
    /// wrote, else what it holds now), scaled, written if it differs.
    /// A field a live edit set and that still holds the edit's bits is
    /// left as it is. Returns whether the bytes changed.
    fn settle<M: Gm>(
        &self,
        m: &mut M,
        next: &mut HashMap<(u32, u32), Slot>,
        edited: &mut HashMap<(u32, u32), u32>,
        key: (u32, u32),
        scaled: impl Fn(f32) -> f32,
    ) -> bool {
        let Some(cur) = m.u32(key.1) else {
            return false;
        };
        if self.1.get(&key) == Some(&cur) {
            edited.insert(key, cur);
            // Kept for a revert to tell our value from the edit's.
            if let Some(s) = self.0.get(&key) {
                next.insert(key, *s);
            }
            return false;
        }
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

/// Why `aim-assist` can't be snapshotted now: the touch player's control
/// update is between its hooks, with slowdown and follow scaled in guest
/// memory and their retail values only in the open window.
pub fn snapshot_live(per_player: &player::PerPlayer) -> Option<String> {
    per_player
        .open()
        .then(|| "the touch player's aim assist window is open".into())
}

/// The running feature's memory, for [`tag_written`].
static LIVE: Mutex<Option<Arc<Mutex<Remembered>>>> = Mutex::new(None);

/// The hot-edit glue's notice of a `weap` or `matg` write: the edited
/// aim-assist fields are adopted ([`Remembered::adopt`]). Nothing when the
/// feature is not installed.
pub fn tag_written<M: Gm>(m: &M, group: u32, datum: u32, address: u32, start: u32, end: u32) -> usize {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    live.map_or(0, |r| r.lock().unwrap_or_else(|e| e.into_inner()).adopt(m, group, datum, address, start, end))
}

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

impl hooks::Feature for AimAssist {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Aim assist strength").doc("docs/game/aim-assist.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use hooks::points::GAME_ACTIVATE;
        let setting = self.setting.clone();
        let remembered = Arc::clone(&self.remembered);
        *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&remembered));
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
            })
            // It reads the mode from the options the host's block was
            // just written into.
            .after_feature(crate::variant_ext::FEATURE);

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
        let pp = Arc::clone(&self.per_player);
        let (save, load) = (Arc::clone(&self.remembered), Arc::clone(&self.remembered));
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || snapshot_live(&pp),
            move || save.lock().unwrap_or_else(|e| e.into_inner()).save_state(),
            move |bytes, _| {
                load.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .load_state(bytes)
            },
        ));
        Ok(())
    }
}

/// Adds the aim assist to `registry`. Says why when left out.
pub fn install(
    registry: &mut hooks::Registry,
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
