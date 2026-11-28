//! The named gaps: a guest draw no native stage takes is drawn by
//! nothing (docs/graphics/retire-d3d-remainders.md, "The named gaps").
//!
//! The device resolves the draw for the state the guest reads back, then
//! hands it here instead of to a backend. Each is counted
//! `scene_dropped_residual_<gap>` in `render.scene` ([`counters`]) and
//! logged once per gap with the caller that made it.
//!
//! The caller is the return address at `[esp]` of the D3D entry being
//! run (or, for a draw game code wrote inline, of the entry whose drain
//! decoded it): [`crate::runtime_glue`] stores it at every entry
//! ([`set_caller`], through [`caller_at`]). A fan of the screen-quad
//! fan `0x00034420` is drained at the next fan's first entry
//! ([`FAN_DRAIN`]), so there the caller is that fan's own return address.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A guest draw that reaches the device with no native stage to take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gap {
    /// The geometry emitter `0x0004daa0`'s whole-part draw (site
    /// `0x0004dae0`): the items each layer stage refused.
    EmitterRefusal,
    /// Decorator types 1 and 2 (decals) in the transparent layers,
    /// `0x0004ebd0`.
    DecalDecorator,
    /// A sky strip of `0x0004c690` the sky stage refused.
    SkyStrip,
    /// A particle drawn as a model part, the callback `0x00224fd0`.
    ParticleModel,
    /// A swallowed post call whose native pass refused: its packets.
    PostSwallow,
    /// A draw from any other caller.
    Other,
}

const GAPS: [Gap; 6] = [
    Gap::EmitterRefusal,
    Gap::DecalDecorator,
    Gap::SkyStrip,
    Gap::ParticleModel,
    Gap::PostSwallow,
    Gap::Other,
];

impl Gap {
    /// The gap's name in its counter.
    pub const fn name(self) -> &'static str {
        match self {
            Gap::EmitterRefusal => "emitter_refusal",
            Gap::DecalDecorator => "decal_decorator",
            Gap::SkyStrip => "sky_strip",
            Gap::ParticleModel => "particle_model",
            Gap::PostSwallow => "post_swallow",
            Gap::Other => "other",
        }
    }

    /// The gap a draw made from return address `ret` falls in
    /// (docs/graphics/notes/residual-callers.md, the call sites).
    pub const fn of_caller(ret: u32) -> Gap {
        match ret {
            0x0004_daa0..0x0004_dfa0 => Gap::EmitterRefusal,
            0x0004_ebd0..0x0004_ee00 => Gap::DecalDecorator,
            0x0004_c690..0x0004_ca00 => Gap::SkyStrip,
            0x0022_4fd0..0x0022_5700 => Gap::ParticleModel,
            _ => Gap::Other,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The return of the screen-quad fan's first D3D entry (`call 0x00400fa0`
/// at `0x000345b4`): the drain of the fan before it.
pub const FAN_DRAIN: u32 = 0x0003_45b9;
/// The fan's own return address above `[esp]` there: its `sub esp, 0x54`,
/// two pushes, and the entry's return.
pub const FAN_FRAME: u32 = 0x60;

/// The caller a D3D entry at `esp` files its draws under: its return
/// address, or at [`FAN_DRAIN`] the fan's.
pub fn caller_at(read: impl Fn(u32) -> Option<u32>, esp: u32) -> u32 {
    match read(esp) {
        Some(FAN_DRAIN) => read(esp.wrapping_add(FAN_FRAME)).unwrap_or(FAN_DRAIN),
        ret => ret.unwrap_or(0),
    }
}

static CALLER: AtomicU32 = AtomicU32::new(0);
static DROPPED: [AtomicU64; GAPS.len()] = [const { AtomicU64::new(0) }; GAPS.len()];
static LOGGED: Mutex<[bool; GAPS.len()]> = Mutex::new([false; GAPS.len()]);

/// The return address of the D3D entry being run.
#[inline]
pub fn set_caller(ret: u32) {
    CALLER.store(ret, Ordering::Relaxed);
}

/// The return address of the D3D entry being run.
#[inline]
pub fn caller() -> u32 {
    CALLER.load(Ordering::Relaxed)
}

/// One guest draw of `entry` drawn by nothing: counted under the
/// current caller's gap and logged once per gap.
pub fn dropped(entry: &'static str) -> Gap {
    let ret = caller();
    let gap = Gap::of_caller(ret);
    dropped_as(gap, entry, ret);
    gap
}

/// One guest draw of `entry` from `ret`, drawn by nothing, under `gap`.
pub fn dropped_as(gap: Gap, entry: &'static str, ret: u32) {
    DROPPED[gap.index()].fetch_add(1, Ordering::Relaxed);
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    if !std::mem::replace(&mut logged[gap.index()], true) {
        eprintln!(
            "render gap {}: a guest {entry} from {ret:#010x} has no native stage and is drawn by nothing \
             (counted scene_dropped_residual_{})",
            gap.name(),
            gap.name()
        );
    }
}

/// `scene_dropped_residual_<gap>` for every gap, and their sum
/// `scene_dropped_residual`, for `render.scene`.
pub fn counters() -> Vec<(String, u64)> {
    let mut total = 0;
    let mut out: Vec<(String, u64)> = GAPS
        .iter()
        .map(|&g| {
            let n = DROPPED[g.index()].load(Ordering::Relaxed);
            total += n;
            (format!("scene_dropped_residual_{}", g.name()), n)
        })
        .collect();
    out.push(("scene_dropped_residual".to_owned(), total));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(name: &str) -> u64 {
        counters().into_iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap()
    }

    #[test]
    fn every_residual_caller_names_its_gap() {
        assert_eq!(Gap::of_caller(0x0004_dae0), Gap::EmitterRefusal);
        assert_eq!(Gap::of_caller(0x0004_ed25), Gap::DecalDecorator);
        assert_eq!(Gap::of_caller(0x0004_edc8), Gap::DecalDecorator);
        assert_eq!(Gap::of_caller(0x0004_c6bf), Gap::SkyStrip);
        assert_eq!(Gap::of_caller(0x0004_c9e8), Gap::SkyStrip);
        // The planar fog's quad is cut (render_bridge::native_fog_quad),
        // and a fog fan of 0x00025dc0 panics before it draws
        // (render_bridge::native_fog_fans).
        assert_eq!(Gap::of_caller(0x0002_5db3), Gap::Other);
        assert_eq!(Gap::of_caller(0x0002_5e2d), Gap::Other);
        assert_eq!(Gap::of_caller(0x0022_56a8), Gap::ParticleModel);
        assert_eq!(Gap::of_caller(0x0022_56c7), Gap::ParticleModel);
        assert_eq!(Gap::of_caller(0x0010_0000), Gap::Other);
    }

    #[test]
    fn a_fan_drained_at_the_next_fan_is_filed_under_the_fans_caller() {
        let words = |a: u32| match a {
            0x1000 => Some(FAN_DRAIN),
            0x1060 => Some(0x0002_5e2d),
            0x2000 => Some(0x0004_c6bf),
            _ => None,
        };
        assert_eq!(caller_at(words, 0x1000), 0x0002_5e2d);
        assert_eq!(caller_at(words, 0x2000), 0x0004_c6bf);
        assert_eq!(caller_at(words, 0x3000), 0);
    }

    #[test]
    fn a_dropped_draw_counts_its_gap_and_the_total() {
        let (gap, total) = (count("scene_dropped_residual_sky_strip"), count("scene_dropped_residual"));
        dropped_as(Gap::SkyStrip, "DrawIndexedVertices", 0x0004_c976);
        assert_eq!(count("scene_dropped_residual_sky_strip"), gap + 1);
        assert!(count("scene_dropped_residual") > total);
    }
}
