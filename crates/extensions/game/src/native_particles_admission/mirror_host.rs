//! R2's particle mirror's host (docs/graphics/retire-d3d-r2.md, "R2's
//! particle mirror"): each batch the mirror holds
//! (`render_bridge::effects_shadow::mirror`) drawn as one GPU particle
//! system through `ParticleDraw`, at the particle draw's return.
//!
//! The batch draws under the cut setup's held material. Its state is the
//! material's guest render words, which no native stage translates yet,
//! so a batch's draw panics by name; [`state`] and [`view`] are what that
//! stage's `ParticleDraw` takes.
//!
//! A system's `Definition` is built from its `prt3` tag and its bitmap's
//! sprite sequences once a map ([`Definitions`]). Every refusal is a named
//! cause: `accepts`' are `scene_kept_15_particle_mirror_<cause>` (the
//! particle keeps its direct draw).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use engine::particle_emission::constant::{ANGLE, SPIN};
use gfx::state::{CompareFunc, DepthBufferMode, RenderState};
use native_render::particle_compute::Blend;
use native_render::particle_definition::{self as pd, Image, Sheet};
use native_render::particle_mirror::Key;
use native_render::particle_system::{Definition, Sprite, View, depth};
use native_render::program::VIEW_CONSTANTS;
use render_bridge::effect_materials::Kind;
use render_bridge::effects_shadow::mirror::{Batch, MirrorHost};
use tags::effects::function::Tables;
use tags::effects::particle::Particle;
use tags::effects::{Ctx, EffectTag};

use super::material_proof::{self, Held};

/// A system's tag-built parts: its definition (the tag's; the callback's
/// tint and size are the [`Key`]'s, [`native_render::particle_mirror::owned`]) and its sprites.
#[derive(Debug, PartialEq)]
pub(crate) struct Built {
    pub definition: Definition,
    pub sprites: Vec<Sprite>,
}

/// The systems built this map, by tag: the definition `[draw]` and the
/// bitmap `[draw + 4]`. A refusal is kept as well, so a tag that cannot
/// be built is read once. A new map (`native_map_work::generation`) lets
/// them all go: the addresses are the map's.
pub(crate) struct Definitions<T = Built> {
    generation: Option<u64>,
    built: HashMap<(u32, u32), Result<Arc<T>, &'static str>>,
}

impl<T> Default for Definitions<T> {
    fn default() -> Self {
        Self { generation: None, built: HashMap::new() }
    }
}

impl<T> Definitions<T> {
    /// `key`'s system, built by `build` the first time this map.
    pub(crate) fn get(
        &mut self,
        generation: u64,
        key: &Key,
        build: impl FnOnce() -> Result<T, &'static str>,
    ) -> Result<Arc<T>, &'static str> {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.built.clear();
        }
        self.built.entry((key.definition, key.bitmap)).or_insert_with(|| build().map(Arc::new)).clone()
    }
}

/// The guest's memory as the effects decoders read a tag.
struct Guest<'a>(&'a dyn GuestMemory);

impl tags::effects::Memory for Guest<'_> {
    fn read(&mut self, address: u32, len: usize) -> tags::Result<Vec<u8>> {
        self.0.bytes(address, len).map_err(|_| tags::Error::Bounds { address, size: len as u64 })
    }
}

/// Sequences a bitmap may hold, and sprites a sequence, before it is
/// taken for a bad read.
const SEQUENCES: u32 = 256;
const SPRITES: u32 = 1024;

/// The sprite sequences of the bitmap at `bitmap` as the callback reads
/// them (`engine::particle_emission`'s step 3): type 3 at `+0`, the
/// sequences at `+0x3c` (0x3c bytes each), each one's sprites at `+0x34`
/// (0x20 bytes each, the window at `+8`, the registration at `+0x18`).
/// `None`: no bitmap, or not a sprites bitmap.
pub(crate) fn sheet(m: &dyn GuestMemory, bitmap: u32) -> Result<Option<Sheet>, &'static str> {
    if bitmap == 0 {
        return Ok(None);
    }
    let unread = |_| "bitmap_unread";
    if m.u16(bitmap).map_err(unread)? != pd::SPRITES {
        return Ok(None);
    }
    let (count, at) = (m.u32(bitmap + 0x3c).map_err(unread)?, m.u32(bitmap + 0x40).map_err(unread)?);
    if count > SEQUENCES {
        return Err("bitmap_sequences");
    }
    let mut sequences = Vec::with_capacity(count as usize);
    for s in 0..count {
        let sequence = at.wrapping_add(s * 0x3c);
        let (n, sprites) = (m.u32(sequence + 0x34).map_err(unread)?, m.u32(sequence + 0x38).map_err(unread)?);
        if n > SPRITES {
            return Err("bitmap_sprites");
        }
        let mut row = Vec::with_capacity(n as usize);
        for k in 0..n {
            let sprite = sprites.wrapping_add(k * 0x20);
            let f = |o: u32| m.f32(sprite + o).map_err(unread);
            row.push(Sprite {
                window: [f(8)?, f(0xc)?, f(0x10)?, f(0x14)?],
                registration: [f(0x18)?, f(0x1c)?],
                _pad: [0.0; 2],
            });
        }
        sequences.push(row);
    }
    Ok(Some(Sheet { sequences }))
}

/// `key`'s system from its tag and bitmap in guest memory, with the
/// image's rotation scale and spin and the engine's function tables.
pub(crate) fn build(m: &dyn GuestMemory, key: &Key) -> Result<Built, &'static str> {
    let tag = Particle::decode_at(&mut Ctx::new(&mut Guest(m)), key.definition).map_err(|_| "tag_unread")?;
    let sheet = sheet(m, key.bitmap)?;
    let image = Image {
        angle_scale: m.f32(ANGLE).map_err(|_| "unreadable")?,
        spin: m.f32(SPIN).map_err(|_| "unreadable")?,
        tables: Tables::engine(),
    };
    let definition = pd::definition(&tag, sheet.as_ref(), &image).map_err(|r| r.name())?;
    Ok(Built { definition, sprites: sheet.map(|s| s.sprites()).unwrap_or_default() })
}

/// Whether particles of `key` can be mirrored with `held` the cut
/// setup's material: there is one (`uncut` otherwise: the setup ran,
/// and the direct draw draws from it), and the system builds.
pub(super) fn accepted<H>(
    held: Option<Arc<H>>,
    definitions: &Mutex<Definitions>,
    generation: u64,
    m: &dyn GuestMemory,
    key: &Key,
) -> Result<(Arc<H>, Arc<Built>), &'static str> {
    let held = held.ok_or("uncut")?;
    let built = definitions.lock().unwrap_or_else(|e| e.into_inner()).get(generation, key, || build(m, key))?;
    Ok((held, built))
}

/// The blend of the cut setup's `render` state as `ParticleDraw` has it,
/// or why it is not one `ParticleDraw` draws: its depth is tested
/// less-equal and never written, it writes every channel, and it has no
/// alpha test. Its stencil is drawn as it is.
pub(super) fn state(render: &RenderState) -> Result<Blend, &'static str> {
    if render.depth_mode == DepthBufferMode::Disabled {
        return Err("depth_off");
    }
    if render.depth_write {
        return Err("depth_write");
    }
    if render.depth_func != CompareFunc::LessEqual {
        return Err("depth_func");
    }
    let w = render.color_write;
    if !(w.red && w.green && w.blue && w.alpha) {
        return Err("color_write");
    }
    if render.alpha_test_enable {
        return Err("alpha_test");
    }
    let colour = gfx::translate::blend_state(render).ok_or("opaque")?.color;
    let additive = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    if colour == wgpu::BlendState::ALPHA_BLENDING.color {
        Ok(Blend::Alpha)
    } else if colour == additive {
        Ok(Blend::Additive)
    } else {
        Err("blend")
    }
}

/// The `ParticleDraw` view of the guest's view constants `c` (hardware
/// slots, docs/graphics/native-world.md, "What a world pass reads") into
/// a whole target of `guest` pixels: `c[0]`, `c[1]` and `c[3]` the
/// view-projection's x, y and w rows (w in W-buffer units), `c[10]` and
/// `c[11]` the scale and offset from clip x, y over w to guest pixels.
/// Clip x and y are those pixels over the target; clip z is 0, as the
/// W-buffered draw stores its own depth: `c[3]`'s W held to `range`
/// (`clamp`ed or culled), floored, over `z_scale`. The billboard axes
/// are the x and y rows without their part along the view, facing the
/// way the pixels run.
pub(super) fn view(
    c: &[[f32; 4]; VIEW_CONSTANTS],
    guest: (u32, u32),
    z_scale: f32,
    range: [f32; 2],
    clamp: bool,
) -> Result<View, &'static str> {
    if guest.0 == 0 || guest.1 == 0 {
        return Err("view_size");
    }
    if !(z_scale > 0.0 && z_scale.is_finite()) {
        return Err("z_scale");
    }
    let (gw, gh) = (guest.0 as f32, guest.1 as f32);
    let (rx, ry, rw, scale, offset) = (c[0], c[1], c[3], c[10], c[11]);
    let row = |a: [f32; 4], ka: f32, kw: f32| -> [f32; 4] { std::array::from_fn(|i| a[i] * ka + rw[i] * kw) };
    let x = row(rx, 2.0 * scale[0] / gw, 2.0 * offset[0] / gw - 1.0);
    let y = row(ry, -2.0 * scale[1] / gh, 1.0 - 2.0 * offset[1] / gh);
    let clip_from_world = std::array::from_fn(|j| [x[j], y[j], 0.0, rw[j]]);

    let xyz = |r: [f32; 4]| [r[0], r[1], r[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let unit = |a: [f32; 3]| -> Option<[f32; 3]> {
        let n = dot(a, a).sqrt();
        (n > 0.0 && n.is_finite()).then(|| a.map(|v| v / n))
    };
    let forward = unit(xyz(rw)).ok_or("view_rows")?;
    let across = |r: [f32; 4], sign: f32| -> Option<[f32; 4]> {
        let r = xyz(r);
        let along = dot(r, forward);
        let a = unit([r[0] - forward[0] * along, r[1] - forward[1] * along, r[2] - forward[2] * along])?;
        Some([a[0] * sign, a[1] * sign, a[2] * sign, 0.0])
    };
    let right = across(rx, scale[0].signum()).ok_or("view_rows")?;
    let up = across(ry, -scale[1].signum()).ok_or("view_rows")?;
    let flags = depth::FLOOR | if clamp { depth::CLAMP } else { 0 };
    Ok(View { clip_from_world, right, up, w_row: rw, depth: [z_scale, range[0], range[1], flags as f32] })
}

/// The game's [`MirrorHost`].
#[derive(Default)]
struct Glue {
    definitions: Mutex<Definitions>,
    /// The cut setup's material the last accepted particle was taken
    /// under, and its key: the batch of that key draws from it.
    setup: Mutex<Option<(Key, Arc<Held>)>>,
}

impl MirrorHost for Glue {
    fn accepts(&self, m: &dyn GuestMemory, key: &Key) -> Result<(), &'static str> {
        let generation = render_bridge::native_map_work::generation();
        let (held, _) = accepted(material_proof::held(Kind::Particle), &self.definitions, generation, m, key)?;
        *self.setup.lock().unwrap_or_else(|e| e.into_inner()) = Some((*key, held));
        Ok(())
    }

    fn draw(&self, _: &mut dyn GuestMemory, batch: &Batch) -> Result<(), &'static str> {
        unimplemented!("particle mirror: {} births of {:#x} drawn from the cut material's render words", batch.spawns.len(), batch.key.definition)
    }
}

/// Installs the mirror's host (with the native particle path's,
/// `super::install`).
pub(super) fn install() {
    render_bridge::effects_shadow::mirror::set_host(Arc::new(Glue::default()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const BASE: u32 = 0x1000;
    const TAG: u32 = 0x1100;
    const BITMAP: u32 = 0x1400;

    fn key(definition: u32, bitmap: u32) -> Key {
        Key { definition, bitmap, tint: [1f32.to_bits(); 4], size: 1f32.to_bits() }
    }

    #[test]
    fn a_system_is_built_once_a_map_by_tag_and_its_refusal_is_kept() {
        let mut d = Definitions::<u32>::default();
        let mut builds = 0;
        let a = key(TAG, BITMAP);
        for _ in 0..3 {
            assert!(d.get(1, &a, || { builds += 1; Ok(7) }).is_ok());
        }
        assert_eq!(builds, 1, "one build a tag");
        // The callback's tint and size are not the tag's: the same system.
        let tinted = Key { tint: [0.5f32.to_bits(); 4], ..a };
        assert!(d.get(1, &tinted, || { builds += 1; Ok(7) }).is_ok());
        assert_eq!(builds, 1);
        // Another bitmap is another system.
        let other = key(TAG, BITMAP + 0x100);
        assert_eq!(d.get(1, &other, || { builds += 1; Err("no_function") }), Err("no_function"));
        assert_eq!(d.get(1, &other, || { builds += 1; Ok(7) }), Err("no_function"), "a refusal is read once");
        assert_eq!(builds, 2);
        // A new map builds again.
        assert!(d.get(2, &other, || { builds += 1; Ok(7) }).is_ok());
        assert_eq!(builds, 3);
    }

    #[test]
    fn a_particle_without_a_cut_setup_or_whose_tag_does_not_build_is_refused_by_name() {
        let m = FlatMemory::new(BASE, 0x1000);
        let defs = Mutex::new(Definitions::default());
        let supplied = Arc::new(());
        let k = key(TAG, 0);
        assert_eq!(accepted::<()>(None, &defs, 1, &m, &k).err(), Some("uncut"));
        // The tag reads, but the image's values are outside this memory.
        let got = accepted(Some(supplied.clone()), &defs, 1, &m, &k);
        assert_eq!(got.err(), Some("unreadable"));
        let unread = key(0x9000_0000, 0);
        assert_eq!(accepted(Some(supplied), &defs, 1, &m, &unread).err(), Some("tag_unread"));
    }

    #[test]
    fn a_zeroed_tag_is_refused_for_its_missing_functions_and_a_bitmap_not_of_sprites_has_no_sheet() {
        let mut m = FlatMemory::new(0, 0x0050_0000);
        m.set_u32(ANGLE, 1f32.to_bits()).unwrap();
        m.set_u32(SPIN, 0).unwrap();
        assert_eq!(build(&m, &key(TAG, 0)).err(), Some("no_function"));
        // Type 0: not a sprites bitmap.
        assert_eq!(sheet(&m, BITMAP), Ok(None));
        // A sprites bitmap with one sequence of one sprite.
        m.set_u16(BITMAP, pd::SPRITES).unwrap();
        m.set_u32(BITMAP + 0x3c, 1).unwrap();
        m.set_u32(BITMAP + 0x40, 0x2000).unwrap();
        m.set_u32(0x2000 + 0x34, 1).unwrap();
        m.set_u32(0x2000 + 0x38, 0x3000).unwrap();
        for (i, v) in [0.0f32, 0.5, 0.25, 0.75, 0.5, 0.5].into_iter().enumerate() {
            m.set_u32(0x3000 + 8 + 4 * i as u32, v.to_bits()).unwrap();
        }
        let s = sheet(&m, BITMAP).unwrap().unwrap();
        assert_eq!(s.sequences.len(), 1);
        assert_eq!(s.sequences[0][0].window, [0.0, 0.5, 0.25, 0.75]);
        assert_eq!(s.sequences[0][0].registration, [0.5, 0.5]);
        // A count past the bound is a bad read.
        m.set_u32(BITMAP + 0x3c, SEQUENCES + 1).unwrap();
        assert_eq!(sheet(&m, BITMAP).err(), Some("bitmap_sequences"));
    }

    fn particle_state() -> RenderState {
        let mut r = RenderState::default();
        r.depth_mode = DepthBufferMode::W;
        r.depth_write = false;
        r.depth_func = CompareFunc::LessEqual;
        r.alpha_test_enable = false;
        r.stencil.enable = false;
        r.color_write = gfx::state::ColorWriteMask { red: true, green: true, blue: true, alpha: true };
        r.blend_enable = true;
        r.blend_op = gfx::state::BlendOp::Add;
        r.src_blend = gfx::state::BlendFactor::SrcAlpha;
        r.dst_blend = gfx::state::BlendFactor::InvSrcAlpha;
        r
    }

    #[test]
    fn a_state_particle_draw_cannot_draw_is_refused_by_name() {
        let r = particle_state();
        assert_eq!(state(&r), Ok(Blend::Alpha));
        assert_eq!(state(&RenderState { dst_blend: gfx::state::BlendFactor::One, ..r }), Ok(Blend::Additive));
        for (s, cause) in [
            (RenderState { depth_mode: DepthBufferMode::Disabled, ..r }, "depth_off"),
            (RenderState { depth_write: true, ..r }, "depth_write"),
            (RenderState { depth_func: CompareFunc::Less, ..r }, "depth_func"),
            (RenderState { color_write: gfx::state::ColorWriteMask { alpha: false, ..r.color_write }, ..r }, "color_write"),
            (RenderState { alpha_test_enable: true, ..r }, "alpha_test"),
            (RenderState { blend_enable: false, ..r }, "opaque"),
            (RenderState { src_blend: gfx::state::BlendFactor::One, ..r }, "blend"),
        ] {
            assert_eq!(state(&s), Err(cause));
        }
        // A stencil test is drawn, not refused.
        let marked = gfx::state::StencilState {
            enable: true,
            func: CompareFunc::Equal,
            reference: 0xff,
            ..Default::default()
        };
        assert_eq!(state(&RenderState { stencil: marked, ..r }), Ok(Blend::Alpha));
        let pipeline = native_render::fog_gpu::layer_stencil(&marked, wgpu::TextureFormat::Depth24PlusStencil8);
        assert_eq!(pipeline.front.compare, wgpu::CompareFunction::Equal);
        assert!(!native_render::fog_gpu::stencil_writes(&marked), "a test that keeps the buffer writes nothing");
    }

    #[test]
    fn the_view_maps_the_guests_rows_to_the_target_and_refuses_a_degenerate_one() {
        // A camera at the origin looking down +y, 640x480, W in units of 2.
        let mut c = [[0.0f32; 4]; VIEW_CONSTANTS];
        c[0] = [1.0, 0.0, 0.0, 0.0];
        c[1] = [0.0, 0.0, 1.0, 0.0];
        c[3] = [0.0, 2.0, 0.0, 0.0];
        c[10] = [320.0, -240.0, 0.0, 0.0];
        c[11] = [320.0, 240.0, 0.0, 0.0];
        let v = view(&c, (640, 480), 100.0, [0.0, 100.0], false).unwrap();
        let clip = |p: [f32; 3]| -> [f32; 4] {
            std::array::from_fn(|i| (0..3).map(|j| v.clip_from_world[j][i] * p[j]).sum::<f32>() + v.clip_from_world[3][i])
        };
        // Straight ahead is the centre; right and up are +x and +z.
        let k = clip([0.0, 5.0, 0.0]);
        assert_eq!((k[0] / k[3], k[1] / k[3], k[2]), (0.0, 0.0, 0.0));
        let k = clip([2.5, 5.0, 2.5]);
        assert!((k[0] / k[3] - 0.25).abs() < 1e-6 && (k[1] / k[3] - 0.25).abs() < 1e-6);
        assert_eq!((v.right, v.up), ([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]));
        assert_eq!(v.w_row, c[3]);
        assert_eq!(v.depth, [100.0, 0.0, 100.0, depth::FLOOR as f32]);
        assert_eq!(view(&c, (640, 480), 100.0, [0.0, 100.0], true).unwrap().depth[3], (depth::FLOOR | depth::CLAMP) as f32);

        assert_eq!(view(&c, (0, 480), 100.0, [0.0, 100.0], false).err(), Some("view_size"));
        assert_eq!(view(&c, (640, 480), 0.0, [0.0, 100.0], false).err(), Some("z_scale"));
        let mut flat = c;
        flat[3] = [0.0; 4];
        assert_eq!(view(&flat, (640, 480), 100.0, [0.0, 100.0], false).err(), Some("view_rows"));
    }
}
