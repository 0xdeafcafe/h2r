//! The frame step's entry census (docs/graphics/framescene.md, section 8.2
//! and slice 4): for each guest entry slices 4 and 5 would replace whole,
//! what the guest still does under it that a replacement would have to
//! draw, keep or reproduce. The entries are every `LAYER_DRAW` by its site
//! and layer, `RENDER_LIGHT_PASSES`, `OBJECT_SHADOW`,
//! `RENDER_SCREEN_EFFECTS`, and under it the colour pass `COLOUR_PASS` and
//! each screen effect `SCREEN_EFFECT` (slice 5). The late passes' fog and
//! water calls of the layer draw (layers 16 and 6 from
//! `render_scene_passes`, [`late_site`]) are the scene's. Per entry:
//!
//! - `d3d_draws`: guest draws handed to the D3D backend under it
//!   (`d3d::device::backend_draws`, registered by boot). Rule 1 of 8.2 (no
//!   remainder) needs zero over the census corpus.
//! - `seed_steps`, `effects_seed_steps`: steps of the shared random seeds
//!   (`RANDOM_SEEDS`, `+0` and `+4`) between its entry and its exit, the
//!   first two rows of section 5.2. `seed_unmatched` counts calls after
//!   which a seed is not within 65,536 steps of where it was, or the
//!   pointer moved.
//! - `animation_gets`, `particle_updates`: calls of
//!   `ANIMATION_RESOURCE_GET` (the animation cache's use stamps) and
//!   `PARTICLE_VIEW_UPDATE` (the particle systems' visibility and location
//!   stamps) under it, two more rows of 5.2.
//!
//! An entry that ends with all of these zero over the corpus is one whose
//! replacement is "return"; a non-zero write is a row of the slice's table
//! of writes, kept by calling the guest function or reproduced in Rust.
//!
//! Entries nest (a light's layers inside its passes, a shadow's layers
//! inside the object shadow): every open entry counts what happens under
//! it, so a light pass's numbers include its layers'. A layer draw the
//! layer table does not place (`Site::Other`) is named by the entry around
//! it (`light`, `object_shadow`) when there is one.
//!
//! Off unless `frame-scene.census` is set with the native renderer on: its
//! exit observers give the layer draw and the light, shadow and post entries
//! an after part, which keeps their chains off the pipelined render thread.
//! `render.scene` reports it under `entries` (docs/runtime/agent-api.md).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;
use native_render::skip::CURRENT_LAYER;
use native_render::world_layers::{Site, layer_site};

use hooks::Builder;

/// The option of the `frame-scene` feature that turns the census on.
pub const ID: &str = "frame-scene.census";

/// Open entries kept at most. A deeper stack means exits were missed (a
/// fault skipped an exit observer): it is dropped and counted.
const MAX_DEPTH: usize = 8;

/// Where a layer draw is called from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayerAt {
    /// `render_scene_passes`' calls (`Site::Scene`).
    Scene,
    AlternateView,
    FirstPerson,
    /// Inside `RENDER_LIGHT_PASSES`.
    Light,
    /// Inside `OBJECT_SHADOW`.
    ObjectShadow,
    Other,
}

impl LayerAt {
    pub fn name(self) -> &'static str {
        match self {
            LayerAt::Scene => "scene",
            LayerAt::AlternateView => "alternate",
            LayerAt::FirstPerson => "first_person",
            LayerAt::Light => "light",
            LayerAt::ObjectShadow => "object_shadow",
            LayerAt::Other => "other",
        }
    }
}

/// A guest entry the census watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Entry {
    Layer { at: LayerAt, layer: u8 },
    LightPasses,
    ObjectShadow,
    ScreenEffects,
    ColourPass,
    ScreenEffect,
}

/// An entry without its layer: what an exit observer knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Layer,
    LightPasses,
    ObjectShadow,
    ScreenEffects,
    ColourPass,
    ScreenEffect,
}

impl Entry {
    pub fn kind(self) -> Kind {
        match self {
            Entry::Layer { .. } => Kind::Layer,
            Entry::LightPasses => Kind::LightPasses,
            Entry::ObjectShadow => Kind::ObjectShadow,
            Entry::ScreenEffects => Kind::ScreenEffects,
            Entry::ColourPass => Kind::ColourPass,
            Entry::ScreenEffect => Kind::ScreenEffect,
        }
    }

    /// `layer.scene.3`, `layer.light.12`, `light_passes`, ...
    pub fn name(self) -> String {
        match self {
            Entry::Layer { at, layer } => format!("layer.{}.{layer}", at.name()),
            Entry::LightPasses => "light_passes".into(),
            Entry::ObjectShadow => "object_shadow".into(),
            Entry::ScreenEffects => "screen_effects".into(),
            Entry::ColourPass => "colour_pass".into(),
            Entry::ScreenEffect => "screen_effect".into(),
        }
    }
}

/// The site of a layer draw called with `layer` current, returning to
/// `ret`: the layer table's, and for the late passes' fog (layer 16) and
/// water (layer 6) calls in `render_scene_passes`, which the table does
/// not place, the scene's.
pub fn late_site(layer: u32, ret: u32) -> Site {
    match (layer, ret) {
        (16, native_render::fog::SCENE_FOG_RETURN) => Site::Scene,
        (6, crate::native_water::SCENE_WATER_RETURN) => Site::Scene,
        _ => layer_site(layer, ret),
    }
}

/// A guest function a replaced entry would have to keep calling (or
/// reproduce).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    AnimationGet,
    ParticleUpdate,
}

/// What the census reads at an entry's entry and exit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// The seeds' pointer and its two words, when they read.
    pub seeds: Option<(u32, [u32; 2])>,
    /// `d3d::device::backend_draws` (0 with no counter registered).
    pub draws: u64,
}

/// One entry's totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub calls: u64,
    pub d3d_draws: u64,
    pub seed_steps: u64,
    pub effects_seed_steps: u64,
    pub seed_unmatched: u64,
    pub animation_gets: u64,
    pub particle_updates: u64,
}

struct Open {
    entry: Entry,
    at: Sample,
    animation_gets: u64,
    particle_updates: u64,
}

/// The census's state: the open entries and every entry's totals.
#[derive(Default)]
pub struct Census {
    open: Vec<Open>,
    counts: BTreeMap<Entry, Counts>,
    unbalanced: u64,
}

impl Census {
    /// Where a layer draw called from `site` is: the layer table's site,
    /// or for one it does not place, the entry around it.
    pub fn layer_at(&self, site: Site) -> LayerAt {
        match site {
            Site::Scene => LayerAt::Scene,
            Site::AlternateView => LayerAt::AlternateView,
            Site::FirstPerson => LayerAt::FirstPerson,
            Site::Sky | Site::Other => match self.open.last().map(|o| o.entry) {
                Some(Entry::LightPasses) => LayerAt::Light,
                Some(Entry::ObjectShadow) => LayerAt::ObjectShadow,
                _ => LayerAt::Other,
            },
        }
    }

    pub fn enter(&mut self, entry: Entry, at: Sample) {
        if self.open.len() >= MAX_DEPTH {
            self.unbalanced += self.open.len() as u64;
            self.open.clear();
        }
        self.open.push(Open { entry, at, animation_gets: 0, particle_updates: 0 });
    }

    /// The innermost open entry of `kind` ends. Entries opened inside it
    /// that never ended are dropped and counted; an exit with no entry of
    /// its kind open is counted and ignored.
    pub fn leave(&mut self, kind: Kind, at: Sample) {
        let Some(i) = self.open.iter().rposition(|o| o.entry.kind() == kind) else {
            self.unbalanced += 1;
            return;
        };
        self.unbalanced += (self.open.len() - 1 - i) as u64;
        self.open.truncate(i + 1);
        let Some(o) = self.open.pop() else { return };
        let c = self.counts.entry(o.entry).or_default();
        c.calls += 1;
        c.d3d_draws += at.draws.saturating_sub(o.at.draws);
        c.animation_gets += o.animation_gets;
        c.particle_updates += o.particle_updates;
        let steps = match (o.at.seeds, at.seeds) {
            (Some((p, from)), Some((q, to))) if p == q => crate::pipeline_render::random_steps(from[0], to[0])
                .zip(crate::pipeline_render::random_steps(from[1], to[1])),
            _ => None,
        };
        match steps {
            Some((game, effects)) => {
                c.seed_steps += u64::from(game);
                c.effects_seed_steps += u64::from(effects);
            }
            None => c.seed_unmatched += 1,
        }
    }

    /// A call of `writer`: every open entry is under it.
    pub fn writer(&mut self, writer: Writer) {
        for o in &mut self.open {
            match writer {
                Writer::AnimationGet => o.animation_gets += 1,
                Writer::ParticleUpdate => o.particle_updates += 1,
            }
        }
    }

    /// A snapshot load: entries open across it never end.
    pub fn forget_open(&mut self) {
        self.unbalanced += self.open.len() as u64;
        self.open.clear();
    }

    /// Every entry seen, by name, with its totals.
    pub fn counts(&self) -> Vec<(String, Counts)> {
        self.counts.iter().map(|(e, c)| (e.name(), *c)).collect()
    }

    /// Entries dropped unended, and exits with nothing to end.
    pub fn unbalanced(&self) -> u64 {
        self.unbalanced
    }
}

static CENSUS: Mutex<Option<Census>> = Mutex::new(None);
static DRAWS: OnceLock<fn() -> u64> = OnceLock::new();

/// Registers the counter of guest draws handed to the D3D backend (boot:
/// `d3d::device::backend_draws`).
pub fn set_draw_counter(f: fn() -> u64) {
    let _ = DRAWS.set(f);
}

/// Whether `d3d_draws` counts anything: a draw counter is registered.
pub fn draws_counted() -> bool {
    DRAWS.get().is_some()
}

fn with<R>(f: impl FnOnce(&mut Census) -> R) -> Option<R> {
    CENSUS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(f)
}

/// The census's totals and its unbalanced count; `None` when it is off.
pub fn snapshot() -> Option<(Vec<(String, Counts)>, u64)> {
    with(|c| (c.counts(), c.unbalanced()))
}

/// A snapshot load (the frame step's `forget`).
pub fn forget() {
    with(Census::forget_open);
}

fn sample(mem: &dyn GuestMemory) -> Sample {
    let seeds = (|| {
        let p = mem.u32(points::RANDOM_SEEDS.base).ok().filter(|&p| p != 0)?;
        Some((p, [mem.u32(p).ok()?, mem.u32(p.checked_add(4)?).ok()?]))
    })();
    Sample { seeds, draws: DRAWS.get().map_or(0, |f| f()) }
}

/// Installs the census's observers (the frame step's build, with [`ID`]
/// set).
pub fn install(b: &mut Builder) {
    *CENSUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Census::default());
    b.raw()
        .observe_before(&points::LAYER_DRAW, "entry_census_layer", |o| {
            let g = o.guest();
            let ret = g.u32(o.cpu.esp).unwrap_or(0);
            let layer = g.u32(CURRENT_LAYER).unwrap_or(u32::MAX);
            let at = sample(&*g);
            drop(g);
            let layer = u8::try_from(layer).unwrap_or(u8::MAX);
            let site = late_site(u32::from(layer), ret);
            with(|c| {
                let at_site = c.layer_at(site);
                c.enter(Entry::Layer { at: at_site, layer }, at);
            });
        })
        .after_feature(crate::render_timing::ID)
        .optional();
    for (point, entry, label) in [
        (&points::RENDER_LIGHT_PASSES, Entry::LightPasses, "entry_census_light_passes"),
        (&points::OBJECT_SHADOW, Entry::ObjectShadow, "entry_census_object_shadow"),
        (&points::RENDER_SCREEN_EFFECTS, Entry::ScreenEffects, "entry_census_screen_effects"),
        (&points::COLOUR_PASS, Entry::ColourPass, "entry_census_colour_pass"),
        (&points::SCREEN_EFFECT, Entry::ScreenEffect, "entry_census_screen_effect"),
    ] {
        b.raw()
            .observe_before(point, label, move |o| {
                let at = sample(&*o.guest());
                with(|c| c.enter(entry, at));
            })
            .optional();
    }
    for (point, kind, label) in [
        (&points::LAYER_DRAW, Kind::Layer, "entry_census_layer_end"),
        (&points::RENDER_LIGHT_PASSES, Kind::LightPasses, "entry_census_light_passes_end"),
        (&points::OBJECT_SHADOW, Kind::ObjectShadow, "entry_census_object_shadow_end"),
        (&points::RENDER_SCREEN_EFFECTS, Kind::ScreenEffects, "entry_census_screen_effects_end"),
        (&points::COLOUR_PASS, Kind::ColourPass, "entry_census_colour_pass_end"),
        (&points::SCREEN_EFFECT, Kind::ScreenEffect, "entry_census_screen_effect_end"),
    ] {
        b.raw()
            .observe_after(point, label, move |o| {
                let at = sample(&*o.guest());
                with(|c| c.leave(kind, at));
            })
            .optional();
    }
    for (point, writer, label) in [
        (&points::ANIMATION_RESOURCE_GET, Writer::AnimationGet, "entry_census_animation_get"),
        (&points::PARTICLE_VIEW_UPDATE, Writer::ParticleUpdate, "entry_census_particle_update"),
    ] {
        b.raw()
            .observe_before(point, label, move |_| {
                with(|c| {
                    if !c.open.is_empty() {
                        c.writer(writer);
                    }
                });
            })
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline_render::{RANDOM_A, RANDOM_C};

    fn step(x: u32, n: u32) -> u32 {
        (0..n).fold(x, |x, _| x.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C))
    }

    fn at(game: u32, effects: u32, draws: u64) -> Sample {
        Sample { seeds: Some((0x0100_0000, [game, effects])), draws }
    }

    const SCENE_3: Entry = Entry::Layer { at: LayerAt::Scene, layer: 3 };

    #[test]
    fn an_entry_counts_its_draws_and_the_seed_steps_between_its_entry_and_exit() {
        let mut c = Census::default();
        c.enter(SCENE_3, at(7, 9, 100));
        c.leave(Kind::Layer, at(step(7, 3), 9, 112));
        c.enter(SCENE_3, at(1, 2, 200));
        c.leave(Kind::Layer, at(1, step(2, 5), 200));
        let counts = c.counts();
        assert_eq!(counts.len(), 1);
        let (name, got) = &counts[0];
        assert_eq!(name, "layer.scene.3");
        assert_eq!(
            *got,
            Counts { calls: 2, d3d_draws: 12, seed_steps: 3, effects_seed_steps: 5, ..Counts::default() }
        );
        assert_eq!(c.unbalanced(), 0);
    }

    #[test]
    fn a_seed_that_is_not_a_run_of_steps_or_a_moved_pointer_is_unmatched() {
        let mut c = Census::default();
        // Past the bound (the step's period is 2^32, so it is not met
        // sooner): as good as reseeded.
        let reseeded = step(5, 1 << 17);
        c.enter(Entry::LightPasses, at(5, 0, 0));
        c.leave(Kind::LightPasses, at(reseeded, 0, 0));
        c.enter(Entry::LightPasses, at(5, 0, 0));
        c.leave(Kind::LightPasses, Sample { seeds: Some((0x0200_0000, [5, 0])), draws: 0 });
        c.enter(Entry::LightPasses, at(5, 0, 0));
        c.leave(Kind::LightPasses, Sample { seeds: None, draws: 0 });
        let got = c.counts()[0].1;
        assert_eq!(got.calls, 3);
        assert_eq!(got.seed_unmatched, 3);
        assert_eq!(got.seed_steps, 0);
    }

    #[test]
    fn nested_entries_each_count_what_happens_under_them_and_name_their_layers_by_the_entry_around() {
        let mut c = Census::default();
        c.enter(Entry::LightPasses, at(0, 0, 10));
        let site = c.layer_at(layer_site(12, 0));
        assert_eq!(site, LayerAt::Light);
        c.enter(Entry::Layer { at: site, layer: 12 }, at(0, 0, 11));
        c.writer(Writer::AnimationGet);
        c.leave(Kind::Layer, at(step(0, 1), 0, 15));
        c.writer(Writer::ParticleUpdate);
        c.leave(Kind::LightPasses, at(step(0, 2), 0, 16));
        let counts: BTreeMap<String, Counts> = c.counts().into_iter().collect();
        let layer = counts["layer.light.12"];
        assert_eq!((layer.d3d_draws, layer.seed_steps, layer.animation_gets, layer.particle_updates), (4, 1, 1, 0));
        let light = counts["light_passes"];
        assert_eq!((light.d3d_draws, light.seed_steps, light.animation_gets, light.particle_updates), (6, 2, 1, 1));
        // The table's sites win over the entry around them.
        c.enter(Entry::ObjectShadow, at(0, 0, 0));
        assert_eq!(c.layer_at(layer_site(3, 0x0002_bed5)), LayerAt::Scene);
        assert_eq!(c.layer_at(layer_site(10, 0)), LayerAt::ObjectShadow);
    }

    #[test]
    fn missed_exits_are_dropped_and_counted_not_charged_to_the_entry_around() {
        let mut c = Census::default();
        c.enter(Entry::ScreenEffects, at(0, 0, 0));
        c.enter(Entry::Layer { at: LayerAt::Other, layer: 16 }, at(0, 0, 0));
        // The layer's exit never ran.
        c.leave(Kind::ScreenEffects, at(0, 0, 3));
        let counts: BTreeMap<String, Counts> = c.counts().into_iter().collect();
        assert_eq!(counts.keys().collect::<Vec<_>>(), ["screen_effects"]);
        assert_eq!(counts["screen_effects"].d3d_draws, 3);
        assert_eq!(c.unbalanced(), 1);
        // An exit with nothing open, and a writer with nothing open.
        c.leave(Kind::Layer, at(0, 0, 0));
        c.writer(Writer::AnimationGet);
        assert_eq!(c.unbalanced(), 2);
        // A stack past the bound is dropped whole.
        for _ in 0..MAX_DEPTH {
            c.enter(SCENE_3, at(0, 0, 0));
        }
        c.enter(SCENE_3, at(0, 0, 0));
        assert_eq!(c.unbalanced(), 2 + MAX_DEPTH as u64);
        c.forget_open();
        assert_eq!(c.unbalanced(), 3 + MAX_DEPTH as u64);
    }

    #[test]
    fn entries_are_named_by_site_and_layer() {
        assert_eq!(Entry::Layer { at: LayerAt::FirstPerson, layer: 3 }.name(), "layer.first_person.3");
        assert_eq!(Entry::Layer { at: LayerAt::ObjectShadow, layer: 10 }.name(), "layer.object_shadow.10");
        assert_eq!(Entry::ObjectShadow.name(), "object_shadow");
        assert_eq!(Entry::ScreenEffects.name(), "screen_effects");
        assert_eq!(Entry::ColourPass.name(), "colour_pass");
        assert_eq!(Entry::ScreenEffect.name(), "screen_effect");
    }

    #[test]
    fn the_late_passes_fog_and_water_calls_are_the_scenes() {
        let c = Census::default();
        let fog = late_site(16, native_render::fog::SCENE_FOG_RETURN);
        let water = late_site(6, crate::native_water::SCENE_WATER_RETURN);
        assert_eq!((c.layer_at(fog), c.layer_at(water)), (LayerAt::Scene, LayerAt::Scene));
        assert_eq!(Entry::Layer { at: c.layer_at(fog), layer: 16 }.name(), "layer.scene.16");
        // Another caller, or the other layer at that return, is not.
        assert_eq!(late_site(16, 0), Site::Other);
        assert_eq!(late_site(6, native_render::fog::SCENE_FOG_RETURN), Site::Other);
        // The table's sites are unchanged.
        assert_eq!(late_site(3, 0x0002_bed5), Site::Scene);
    }

    #[test]
    fn the_post_callees_nest_under_the_screen_effects() {
        let mut c = Census::default();
        c.enter(Entry::ScreenEffects, at(0, 0, 0));
        c.enter(Entry::ColourPass, at(0, 0, 0));
        c.leave(Kind::ColourPass, at(0, 0, 4));
        c.enter(Entry::ScreenEffect, at(0, 0, 4));
        c.leave(Kind::ScreenEffect, at(0, 0, 6));
        c.leave(Kind::ScreenEffects, at(0, 0, 7));
        let counts: BTreeMap<String, Counts> = c.counts().into_iter().collect();
        assert_eq!(counts["colour_pass"].d3d_draws, 4);
        assert_eq!(counts["screen_effect"].d3d_draws, 2);
        assert_eq!(counts["screen_effects"].d3d_draws, 7);
        assert_eq!(c.unbalanced(), 0);
    }
}
