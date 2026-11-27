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

/// One item of the list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum EffectItem {
    Quad(EffectQuad),
    Ribbon(Ribbon),
    Particle(ParticleQuad),
}

impl EffectItem {
    /// The datum index the item is built from: the effect's, or the
    /// particle system's.
    pub fn datum(&self) -> u16 {
        match self {
            EffectItem::Quad(q) => q.effect,
            EffectItem::Ribbon(r) => r.effect,
            EffectItem::Particle(p) => p.system,
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

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
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
