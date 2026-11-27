//! The effects a view draws, typed (docs/graphics/framescene.md, 5.2 and
//! 10): particles, effect quads and contrail ribbons as items of one list
//! in the guest's visit order. An item carries the sim-owned state it was
//! built from (datum indices) and the vertex registers its draw sets, so
//! `native_render`'s effect stage turns the list into its `particles` and
//! `effect_geometry` instance streams without reading the guest.
//!
//! The list holds what the draws produce. The effects random word and the
//! `0x00174420` visibility and location stamps are not here: the guest
//! keeps making them (framescene.md, 5.2).

use serde::Serialize;

use crate::builder::{BuildError, SceneBuilder};

/// Vertex registers an item sets (`v0` is the corner stream).
pub const REGISTERS: usize = 16;

/// One vertex register, as four words that keep NaN payloads and signed zero.
pub type Register = [u32; 4];

/// A particle's quad, as the emitter `0x0023ed30` writes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParticleQuad {
    /// The `particle_system` datum index the particle belongs to.
    pub system: u16,
    /// The particle's index in the system's particles.
    pub particle: u16,
    /// The registers the emitter sets; unset ones keep the draw's defaults.
    pub registers: [Option<Register>; REGISTERS],
}

/// One effect quad (`0x0004b252`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectQuad {
    /// The `effect` datum index.
    pub effect: u16,
    /// Its four corners' vertices, `stride` bytes each, as the builder fills them.
    pub vertices: Vec<u8>,
    pub stride: u32,
}

/// One contrail ribbon (`0x0004b28c`): a strip of `stride`-byte vertices.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ribbon {
    /// The `effect` datum index the contrail belongs to.
    pub effect: u16,
    pub vertices: Vec<u8>,
    pub stride: u32,
}

/// One weather plate (`0x00037a60`): a view-sized sheet of the `weat`
/// tag's block (stride `0x3a8`), three scrolling bitmap layers, depth
/// tested GREATER against the scene's depth and tinted per player. It
/// draws in slot 9 ([`WeatherPlate::SLOT`]). Every field is what the
/// guest function reads, so the draw needs no guest memory.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WeatherPlate {
    /// The block element's index.
    pub element: u16,
    /// The view's player (`[0x00485600]`): the per-player state at
    /// `element + 0xb8 * player` is the plate's scroll and tint.
    pub player: u8,
    /// The view window in pixels: left, top, right, bottom
    /// (`0x0048564a`, `0x00485648`, `0x0048564e`, `0x0048564c`).
    pub window: [i16; 4],
    /// The three layers' bitmap tag datums (element `+4`, `+0xc`, `+0x14`).
    pub bitmaps: [u32; 3],
    /// Each layer's scale (element `+0x48 + 4 * layer`).
    pub layer_scale: [f32; 3],
    /// Each layer's scroll offset in the sheet's texture space: the player
    /// state's `+0x11c + 8 * layer` and `+0x120 + 8 * layer`, times the
    /// layer's scale (what the guest sets as vertex constant `0x15 + layer`).
    pub scroll: [[f32; 2]; 3],
    /// The placement inputs the guest hands `0x0003a5c0` (element `+0x80`, `+0x84`).
    pub extent: [f32; 2],
    /// The player state's per-layer sheet size (`+0x134`, `+0x138`, `+0x13c`).
    pub size: [f32; 3],
    /// The sheet's colour: the player state's `+0xec..+0xf4` times its
    /// `+0x148 * +0x14c`, each clamped to `[0, 1]`.
    pub tint: [f32; 3],
    /// The element's own colour (`+0x98`, `+0x9c`, `+0xa0`), each clamped to
    /// `[0, 1]`: the plate's fourth definition colour.
    pub fill: [f32; 3],
}

impl WeatherPlate {
    /// The slot every plate draws in: 9, with the transparents and effects.
    pub const SLOT: crate::ViewSlot = crate::ViewSlot::Transparents;
}

/// One item of the list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum EffectItem {
    Quad(EffectQuad),
    Ribbon(Ribbon),
    Particle(ParticleQuad),
    WeatherPlate(WeatherPlate),
}

impl EffectItem {
    /// The datum index the item is built from: the effect's, or the
    /// particle system's.
    pub fn datum(&self) -> u16 {
        match self {
            EffectItem::Quad(q) => q.effect,
            EffectItem::Ribbon(r) => r.effect,
            EffectItem::Particle(p) => p.system,
            EffectItem::WeatherPlate(w) => w.element,
        }
    }
}

/// A view's effects, in the guest's order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Effects {
    items: Vec<EffectItem>,
}

impl Effects {
    pub fn push(&mut self, item: EffectItem) {
        self.items.push(item);
    }

    pub fn items(&self) -> &[EffectItem] {
        &self.items
    }

    /// The weather plates, in order.
    pub fn plates(&self) -> impl Iterator<Item = &WeatherPlate> {
        self.items.iter().filter_map(|i| match i {
            EffectItem::WeatherPlate(p) => Some(p),
            _ => None,
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The view's effects as summary-hash words, in order: the weather
/// plates (the other items join with the effects arm).
pub(crate) fn summary_words(effects: &Effects, word: &mut dyn FnMut(u32)) {
    let plates = effects.plates().count();
    word(plates as u32);
    for p in effects.plates() {
        word(u32::from(p.element) | u32::from(p.player) << 16);
        p.window.iter().for_each(|&x| word(x as u16 as u32));
        p.bitmaps.iter().for_each(|&x| word(x));
        p.layer_scale.iter().for_each(|x| word(x.to_bits()));
        p.scroll.iter().flatten().for_each(|x| word(x.to_bits()));
        p.extent.iter().for_each(|x| word(x.to_bits()));
        p.size.iter().for_each(|x| word(x.to_bits()));
        p.tint.iter().for_each(|x| word(x.to_bits()));
        p.fill.iter().for_each(|x| word(x.to_bits()));
    }
}

impl SceneBuilder {
    /// Adds an effect item to the open view, after its effects so far.
    pub fn push_effect(&mut self, item: EffectItem) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].effects.push(item);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn particle(system: u16, particle: u16) -> EffectItem {
        EffectItem::Particle(ParticleQuad { system, particle, registers: [None; REGISTERS] })
    }

    #[test]
    fn effect_items_keep_the_datum_order() {
        let mut list = Effects::default();
        list.push(particle(7, 0));
        list.push(EffectItem::Ribbon(Ribbon { effect: 3, vertices: vec![0; 32], stride: 16 }));
        list.push(particle(2, 5));
        list.push(EffectItem::Quad(EffectQuad { effect: 9, vertices: vec![0; 64], stride: 16 }));
        list.push(particle(7, 1));
        // Pushed order, not sorted by datum or kind.
        let datums: Vec<u16> = list.items().iter().map(EffectItem::datum).collect();
        assert_eq!(datums, [7, 3, 2, 9, 7]);
        assert!(matches!(list.items()[1], EffectItem::Ribbon(_)));
        assert!(matches!(&list.items()[4], EffectItem::Particle(p) if p.particle == 1));
        assert_eq!(list.len(), 5);
    }

    fn plate(element: u16, player: u8) -> WeatherPlate {
        WeatherPlate {
            element,
            player,
            window: [0, 0, 640, 480],
            bitmaps: [0xe600_0001, 0, 0],
            layer_scale: [1.0; 3],
            scroll: [[0.25, 0.5]; 3],
            extent: [1.0, 2.0],
            size: [1.0; 3],
            tint: [1.0; 3],
            fill: [0.0; 3],
        }
    }

    #[test]
    fn a_views_plates_follow_its_effects_in_order() {
        use crate::scene::{SceneId, Stamp, ViewKey, ViewKind};
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(b.push_effect(particle(1, 1)).unwrap_err(), BuildError::NoView);
        let key = |view| ViewKey { kind: ViewKind::Game, player: 0, view };
        b.open_view(key(0), crate::test_camera::camera()).unwrap();
        b.push_effect(EffectItem::WeatherPlate(plate(2, 0))).unwrap();
        b.push_effect(particle(5, 0)).unwrap();
        b.push_effect(EffectItem::WeatherPlate(plate(0, 1))).unwrap();
        b.open_view(key(1), crate::test_camera::camera()).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        let plates: Vec<_> = s.views[0].effects.plates().map(|p| (p.element, p.player)).collect();
        assert_eq!(plates, [(2, 0), (0, 1)]);
        assert_eq!(s.views[0].effects.len(), 3);
        assert!(s.views[1].effects.is_empty());
        assert_eq!(WeatherPlate::SLOT.number(), Some(9));
    }
}

/// What a visual effect part made, as the sim's effects phase reports it
/// (docs/engine/effects-parts.md): a decal, a light or a lens flare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PartKind {
    Decal,
    Light,
    Flare,
}

/// One visual effect part created since the last scene, in creation order.
/// The sim keeps these out of guest memory, so they never reach the arena
/// hash; decals and lights are also in the guest's arrays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PartEvent {
    /// Creation order, from 1; never reused.
    pub seq: u64,
    pub tick: u32,
    pub kind: PartKind,
    /// The part's tag and its effect's definition tag.
    pub tag: u32,
    pub effect: u32,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub scale: f32,
    /// The guest made it too.
    pub guest_made: bool,
}
