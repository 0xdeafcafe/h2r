//! A view's late passes, typed (docs/graphics/framescene.md, slice 5):
//! the planar fog (layer 16), the water edge blend (layer 6) and the post
//! passes (`render_screen_effects` `0x00013870` and its callees). Each is
//! read where its guest entry runs into the scene's last view of the
//! guest view's key (the first person's view can follow it), by value: the
//! fog words the device holds, each material's words resolved over them,
//! the structure parts the stage draws, and the post call's arguments.
//!
//! The fog and water stages record from what is here: their glue reads
//! the scene's copy back and hands it to the render thread's scene stage
//! (`boot::native_fog_glue`, `boot::native_water_glue`). The post stage
//! still draws through the device's swallow, so [`PostParams`] is what a
//! post stage would draw from, extracted but not yet its input.

use std::ops::Range;

use serde::Serialize;

use crate::builder::{BuildError, SceneBuilder};
use crate::object::Owner;
use crate::scene::{FrameScene, View, ViewKey};

/// The fog render states, by slot: `FOGENABLE`, `FOGTABLEMODE`,
/// `FOGSTART`, `FOGEND`, `FOGDENSITY`, `RANGEFOGENABLE`, `FOGCOLOR`
/// (`native_render::fog_gpu::FOG_WORD_SLOTS`).
pub const FOG_WORDS: usize = 7;

/// The fog words the device holds at a layer's draw, by slot; `None` a
/// slot that does not read.
pub type DeviceFog = [Option<u32>; FOG_WORDS];

/// One material of a late layer: its fog words resolved over the device's
/// (the raw `D3DRS` values: `f32` bits for the float states, an
/// `A8R8G8B8` colour for `FOGCOLOR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LateMaterial {
    /// The world material index (`native_render::world::WorldDraw::material`).
    pub material: u16,
    pub words: [u32; FOG_WORDS],
}

/// A structure part a late layer's stage draws: its owner and its part
/// index, in the layer's item order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LatePart {
    pub owner: Owner,
    pub part: u8,
}

/// A late layer of a view: layer 16 (the fog, [`FogState`]) or layer 6
/// (the water, [`WaterState`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LateLayer {
    pub layer: u8,
    /// The layer's own gate held: for the fog, the layer draw's test
    /// (`0x00044582`); for the water, bit 4 of the water gate clear. A
    /// layer whose gate does not hold draws nothing, natively or not.
    pub draws: bool,
    /// The device's fog words at the layer's draw.
    pub device: DeviceFog,
    /// The layer's items in the current list (its bit set): what the guest
    /// would draw. `None` when the list does not read.
    pub items: Option<u32>,
    /// Into [`FrameScene::late_materials`]: each material the stage draws
    /// with, its words resolved.
    pub materials: Range<u32>,
    /// Into [`FrameScene::late_parts`]: the parts the stage draws, in the
    /// layer's item order.
    pub parts: Range<u32>,
    /// The stage took the parts: the guest was told to skip them and the
    /// stage was queued or reserved. False, the guest draws every item.
    pub drawn: bool,
}

/// The fog of a view: its layer 16.
pub type FogState = LateLayer;
/// The water edge blend of a view: its layer 6.
pub type WaterState = LateLayer;

/// What a stage hands the builder for a late layer.
#[derive(Clone, Copy, Debug)]
pub struct LateInput<'a> {
    pub draws: bool,
    pub device: DeviceFog,
    pub items: Option<u32>,
    pub materials: &'a [LateMaterial],
    pub parts: &'a [LatePart],
}

/// One call of the screen effect `0x0002d140` under `render_screen_effects`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct PostEffect {
    /// The `egor` tag datum (EAX).
    pub datum: u32,
    /// Its four values, read from the address the call names; `None` when
    /// they do not read.
    pub values: Option<[u32; 4]>,
    pub columns: u32,
    pub rows: u32,
}

/// The post passes of a view, by value: what one call of
/// `render_screen_effects` was asked to draw and what its callees were
/// called with.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct PostParams {
    /// The colour pass over the view's target (`0x000214f0`).
    pub colour: bool,
    /// `0x0002df00` at the end.
    pub late: bool,
    /// The view kind (`[0x00485898]`).
    pub view_kind: u32,
    /// The scenario's screen-effect entries.
    pub screen_effects: u32,
    /// The screen-effect loop's gates other than `0x00022710` pass.
    pub screen_effects_gated: bool,
    /// The seven words `render_screen_effects` pushed for the colour pass,
    /// when it called it (`engine::colour_pass::Arguments`).
    pub colour_pass: Option<[u32; 7]>,
    /// Into [`FrameScene::post_effects`]: the screen effects drawn, in
    /// call order.
    pub effects: Range<u32>,
}

impl PostParams {
    /// Whether the call may draw anything at all
    /// (`scene_extract::layout::post::PostFrame::draws`).
    pub fn draws(&self) -> bool {
        self.colour || self.late || (self.screen_effects_gated && self.screen_effects > 0)
    }
}

/// A late layer's sizes, for `render.scene`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LateCounts {
    pub draws: bool,
    pub items: u32,
    pub materials: u32,
    pub parts: u32,
    pub drawn: bool,
    /// Items the guest still draws: every item when the stage did not take
    /// the layer, the items past its parts otherwise.
    pub kept: u32,
    /// Device fog slots that read.
    pub device_words: u32,
}

impl FrameScene {
    pub fn late_materials_of(&self, layer: &LateLayer) -> &[LateMaterial] {
        self.late_materials.get(layer.materials.start as usize..layer.materials.end as usize).unwrap_or(&[])
    }

    pub fn late_parts_of(&self, layer: &LateLayer) -> &[LatePart] {
        self.late_parts.get(layer.parts.start as usize..layer.parts.end as usize).unwrap_or(&[])
    }

    pub fn post_effects_of(&self, post: &PostParams) -> &[PostEffect] {
        self.post_effects.get(post.effects.start as usize..post.effects.end as usize).unwrap_or(&[])
    }

    /// A late layer's sizes.
    pub fn late_counts(&self, layer: &LateLayer) -> LateCounts {
        let items = layer.items.unwrap_or(0);
        let parts = layer.parts.len() as u32;
        LateCounts {
            draws: layer.draws,
            items,
            materials: layer.materials.len() as u32,
            parts,
            drawn: layer.drawn,
            kept: if layer.drawn { items.saturating_sub(parts) } else { items },
            device_words: layer.device.iter().filter(|w| w.is_some()).count() as u32,
        }
    }
}

impl View {
    /// The view's late layer `layer` (16 or 6).
    pub fn late(&self, layer: u8) -> Option<&LateLayer> {
        match layer {
            16 => self.fog.as_ref(),
            6 => self.water.as_ref(),
            _ => None,
        }
    }
}

impl SceneBuilder {
    /// The late layer `layer` (16 the fog, 6 the water) of the scene's last
    /// view of `key`, replacing one set before; not yet drawn
    /// ([`Self::late_drawn`]). The layers run after the view's opaque
    /// block, so the view may no longer be the open one (the first person's
    /// can follow it): the late passes extend no view range, so they are
    /// written into it all the same. The layer as written, a copy.
    pub fn set_late(&mut self, key: ViewKey, layer: u8, input: LateInput<'_>) -> Result<LateLayer, BuildError> {
        if layer != 16 && layer != 6 {
            return Err(BuildError::NotLate);
        }
        let (scene, view) = self.keyed_mut(key)?;
        let m = scene.late_materials.len() as u32;
        scene.late_materials.extend_from_slice(input.materials);
        let p = scene.late_parts.len() as u32;
        scene.late_parts.extend_from_slice(input.parts);
        let state = LateLayer {
            layer,
            draws: input.draws,
            device: input.device,
            items: input.items,
            materials: m..scene.late_materials.len() as u32,
            parts: p..scene.late_parts.len() as u32,
            drawn: false,
        };
        let view = &mut scene.views[view];
        let slot = if layer == 16 { &mut view.fog } else { &mut view.water };
        *slot = Some(state.clone());
        Ok(state)
    }

    /// The late layer `layer` of the last view of `key` was taken by its
    /// stage.
    pub fn late_drawn(&mut self, key: ViewKey, layer: u8) -> Result<(), BuildError> {
        let (scene, view) = self.keyed_mut(key)?;
        let view = &mut scene.views[view];
        let slot = match layer {
            16 => &mut view.fog,
            6 => &mut view.water,
            _ => return Err(BuildError::NotLate),
        };
        slot.as_mut().ok_or(BuildError::NotLate)?.drawn = true;
        Ok(())
    }

    /// The post passes of the last view of `key` at
    /// `render_screen_effects`' entry, replacing any set before; its
    /// callees' calls follow ([`Self::post_colour`],
    /// [`Self::push_post_effect`]). The builder sets `post.colour_pass` and
    /// `post.effects`.
    pub fn set_post(&mut self, key: ViewKey, post: PostParams) -> Result<(), BuildError> {
        let (scene, view) = self.keyed_mut(key)?;
        let e = scene.post_effects.len() as u32;
        scene.views[view].post = Some(PostParams { colour_pass: None, effects: e..e, ..post });
        Ok(())
    }

    /// The colour pass's arguments, under the post passes of the last view
    /// of `key`.
    pub fn post_colour(&mut self, key: ViewKey, args: [u32; 7]) -> Result<(), BuildError> {
        let (scene, view) = self.keyed_mut(key)?;
        scene.views[view].post.as_mut().ok_or(BuildError::NoPost)?.colour_pass = Some(args);
        Ok(())
    }

    /// A screen effect drawn under the post passes of the last view of
    /// `key`. A post's effects are contiguous: an effect of another post
    /// after its last refuses.
    pub fn push_post_effect(&mut self, key: ViewKey, effect: PostEffect) -> Result<(), BuildError> {
        let (scene, view) = self.keyed_mut(key)?;
        let end = scene.post_effects.len() as u32;
        let post = scene.views[view].post.as_mut().ok_or(BuildError::NoPost)?;
        if post.effects.end != end {
            return Err(BuildError::NoPost);
        }
        scene.post_effects.push(effect);
        post.effects.end = end + 1;
        Ok(())
    }
}

/// Every late layer's and post's ranges inside their arrays.
pub(crate) fn check(scene: &FrameScene) -> Result<(), String> {
    fn within(r: &Range<u32>, len: usize, what: &str) -> Result<(), String> {
        if r.start > r.end || r.end as usize > len {
            return Err(format!("{what} {r:?} outside 0..{len}"));
        }
        Ok(())
    }
    for (n, v) in scene.views.iter().enumerate() {
        for l in [&v.fog, &v.water].into_iter().flatten() {
            within(&l.materials, scene.late_materials.len(), &format!("view {n} layer {} materials", l.layer))?;
            within(&l.parts, scene.late_parts.len(), &format!("view {n} layer {} parts", l.layer))?;
        }
        if let Some(p) = &v.post {
            within(&p.effects, scene.post_effects.len(), &format!("view {n} post effects"))?;
        }
    }
    Ok(())
}

/// The words a view's late passes add to the summary hash, in order.
pub(crate) fn summary_words(scene: &FrameScene, v: &View, word: &mut dyn FnMut(u32)) {
    for l in [&v.fog, &v.water] {
        let Some(l) = l else {
            word(0);
            continue;
        };
        word(1);
        word(u32::from(l.layer));
        word(u32::from(l.draws));
        for w in l.device {
            match w {
                Some(w) => {
                    word(1);
                    word(w);
                }
                None => word(0),
            }
        }
        word(l.items.unwrap_or(u32::MAX));
        let materials = scene.late_materials_of(l);
        word(materials.len() as u32);
        for m in materials {
            word(u32::from(m.material));
            for &w in &m.words {
                word(w);
            }
        }
        let parts = scene.late_parts_of(l);
        word(parts.len() as u32);
        for p in parts {
            match p.owner {
                Owner::Cluster(c) => word(c & 0x7fff_ffff),
                Owner::Instance(i) => word(0x8000_0000 | i),
            }
            word(u32::from(p.part));
        }
        word(u32::from(l.drawn));
    }
    let Some(p) = &v.post else {
        word(0);
        return;
    };
    word(1);
    word(u32::from(p.colour));
    word(u32::from(p.late));
    word(p.view_kind);
    word(p.screen_effects);
    word(u32::from(p.screen_effects_gated));
    match p.colour_pass {
        Some(args) => {
            word(1);
            for a in args {
                word(a);
            }
        }
        None => word(0),
    }
    let effects = scene.post_effects_of(p);
    word(effects.len() as u32);
    for e in effects {
        word(e.datum);
        match e.values {
            Some(v) => {
                word(1);
                for x in v {
                    word(x);
                }
            }
            None => word(0),
        }
        word(e.columns);
        word(e.rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::camera;
    use crate::scene::{SceneId, Stamp, ViewKind};
    use crate::summary_hash;

    fn key(view: u8) -> ViewKey {
        ViewKey { kind: ViewKind::Game, player: 0, view }
    }

    const DEVICE: DeviceFog = [Some(1), Some(3), Some(0), Some(0x4120_0000), Some(0), None, Some(0xff80_8080)];

    fn fog_input<'a>(materials: &'a [LateMaterial], parts: &'a [LatePart]) -> LateInput<'a> {
        LateInput { draws: true, device: DEVICE, items: Some(5), materials, parts }
    }

    #[test]
    fn a_views_late_layers_keep_their_words_and_parts_and_count_what_the_guest_keeps() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(b.set_late(key(0), 16, fog_input(&[], &[])), Err(BuildError::NoView), "no view of the key");
        b.open_view(key(0), camera()).unwrap();
        let materials = [LateMaterial { material: 4, words: [1, 3, 0, 0x4120_0000, 0, 0, 0xff80_8080] }];
        let parts = [
            LatePart { owner: Owner::Cluster(7), part: 0 },
            LatePart { owner: Owner::Instance(2), part: 1 },
        ];
        // The first person's view follows the game view's opaque block;
        // its late layers still land in the game view.
        let first_person = ViewKey { kind: ViewKind::FirstPerson, ..key(0) };
        b.open_view(first_person, camera()).unwrap();
        let fog = b.set_late(key(0), 16, fog_input(&materials, &parts)).unwrap();
        assert!(!fog.drawn, "not drawn until the stage takes it");
        b.late_drawn(key(0), 16).unwrap();
        let water = LateInput { draws: false, device: [None; FOG_WORDS], items: Some(2), materials: &[], parts: &[] };
        b.set_late(key(0), 6, water).unwrap();
        assert_eq!(b.set_late(key(0), 3, fog_input(&[], &[])), Err(BuildError::NotLate));
        b.open_view(key(1), camera()).unwrap();
        assert_eq!(b.late_drawn(key(1), 16), Err(BuildError::NotLate), "the second view has no fog");
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let fog = v.late(16).unwrap();
        assert_eq!(s.late_materials_of(fog), &materials);
        assert_eq!(s.late_parts_of(fog), &parts);
        let c = s.late_counts(fog);
        assert_eq!((c.draws, c.items, c.materials, c.parts, c.drawn, c.kept, c.device_words), (true, 5, 1, 2, true, 3, 6));
        let water = v.late(6).unwrap();
        let c = s.late_counts(water);
        assert_eq!((c.draws, c.parts, c.drawn, c.kept), (false, 0, false, 2), "an untaken layer keeps every item");
        assert!(s.views[1].fog.is_none(), "not the first person's");
        assert!(s.views[2].fog.is_none() && s.views[2].late(6).is_none());
    }

    #[test]
    fn a_post_call_keeps_its_colour_pass_and_effects_in_order() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(key(0), camera()).unwrap();
        b.open_view(key(1), camera()).unwrap();
        assert_eq!(b.post_colour(key(0), [0; 7]), Err(BuildError::NoPost), "no call open");
        let post = PostParams {
            colour: true, late: false, view_kind: 1, screen_effects: 2, screen_effects_gated: true,
            colour_pass: Some([9; 7]), effects: 5..9,
        };
        b.set_post(key(0), post.clone()).unwrap();
        b.post_colour(key(0), [1, 2, 3, 4, 5, 6, 7]).unwrap();
        let effect = |datum| PostEffect { datum, values: Some([1, 2, 3, 4]), columns: 2, rows: 1 };
        b.push_post_effect(key(0), effect(0xe000_0001)).unwrap();
        b.push_post_effect(key(0), effect(0xe000_0002)).unwrap();
        b.set_post(key(1), post).unwrap();
        b.push_post_effect(key(1), effect(0xe000_0003)).unwrap();
        assert_eq!(b.push_post_effect(key(0), effect(4)), Err(BuildError::NoPost), "another post's effects followed");
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let p = s.views[0].post.as_ref().unwrap();
        assert!(p.draws());
        assert_eq!(p.colour_pass, Some([1, 2, 3, 4, 5, 6, 7]), "the builder's, not the caller's");
        let datums: Vec<u32> = s.post_effects_of(p).iter().map(|e| e.datum).collect();
        assert_eq!(datums, [0xe000_0001, 0xe000_0002]);
        let q = s.views[1].post.as_ref().unwrap();
        assert_eq!((q.colour_pass, s.post_effects_of(q).len()), (None, 1));
    }

    #[test]
    fn the_late_passes_count_in_the_hash() {
        let build = |device: DeviceFog, drawn: bool, colour: Option<[u32; 7]>| {
            let mut b = SceneBuilder::default();
            b.begin(SceneId::default());
            b.open_view(key(0), camera()).unwrap();
            let parts = [LatePart { owner: Owner::Cluster(1), part: 0 }];
            b.set_late(key(0), 16, LateInput { device, ..fog_input(&[], &parts) }).unwrap();
            if drawn {
                b.late_drawn(key(0), 16).unwrap();
            }
            b.set_post(key(0), PostParams {
                colour: true, late: false, view_kind: 0, screen_effects: 0, screen_effects_gated: false,
                colour_pass: None, effects: 0..0,
            })
            .unwrap();
            if let Some(args) = colour {
                b.post_colour(key(0), args).unwrap();
            }
            summary_hash(&b.seal(Stamp::default()).unwrap())
        };
        let base = build(DEVICE, true, Some([0; 7]));
        assert_eq!(base, build(DEVICE, true, Some([0; 7])));
        let mut other = DEVICE;
        other[2] = Some(0x3f80_0000);
        assert_ne!(base, build(other, true, Some([0; 7])), "a device word");
        assert_ne!(base, build(DEVICE, false, Some([0; 7])), "whether the stage took it");
        assert_ne!(base, build(DEVICE, true, None), "the colour pass");
    }
}
