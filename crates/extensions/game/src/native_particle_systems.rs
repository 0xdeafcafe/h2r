//! The particle walk's host (docs/graphics/native-particles.md, "The
//! frame graph's particle stage"): each batch the walk answered in Rust
//! (`render_bridge::native_particle_walk`) as one GPU particle system,
//! published for the frame graph's slot 9 stage
//! (`native_render::particle_set`).
//!
//! A system is built once a map by its definition and bitmap
//! (`mirror_host::Definitions`): the `Definition` and sprites from the
//! `prt3` tag and its bitmap in guest memory (`mirror_host::build`), the
//! blend from the tag's shader template, and the sprites' image decoded
//! from the map (`SpriteBitmap`). A walk with a system that does not
//! build is refused by name (`layer_plan_15_particle_walk_refused_<name>`)
//! and the guest walks it. The refusal is kept (`Refused`), so the next
//! walk of that system is refused at the walk's entry by one lookup
//! (`Host::refused`), before the walk is modelled.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use native_render::particle_compute::Blend;
use native_render::particle_definition as pd;
use native_render::particle_mirror::{self as nm, Key};
use native_render::particle_set::{self, Set, SpriteBitmap, System};
use native_render::particle_system::{Definition, Sprite};
use render_bridge::particle_births::Batch;
use tags::effects::{Ctx, EffectTag};
use tags::effects::particle::Particle;

use crate::native_map::NativeMap;
use crate::native_particles_admission::mirror_host::{self, Definitions};

/// The particle templates the draw matches exactly, with the blend each
/// draws with (docs/graphics/native-particles.md, "Templates").
pub const TEMPLATES: [(&str, Blend); 4] = [
    ("shaders\\shader_templates\\transparent\\particle_alpha_blend", Blend::Alpha),
    ("shaders\\shader_templates\\transparent\\particle_additive", Blend::Additive),
    ("shaders\\shader_templates\\transparent\\particle_alpha_multiply_add", Blend::MultiplyAdd),
    ("shaders\\shader_templates\\transparent\\particle_alpha_test_no_lighting_fixed_for_glass", Blend::Glass),
];

/// A built system: all of it but the frame's tint, size and births.
struct Part {
    definition: Definition,
    sprites: Vec<Sprite>,
    blend: Blend,
    bitmap: Arc<SpriteBitmap>,
}

/// The running map, opened for its bitmaps and tag names.
struct Opened {
    generation: u64,
    map: NativeMap,
    files: assets::ResourceFiles,
    /// By bitmap tag and image: one `SpriteBitmap` a texture.
    bitmaps: HashMap<(u32, i16), Arc<SpriteBitmap>>,
}

/// The systems refused this map, by definition and bitmap, with the
/// name. A new map (`native_map_work::generation`) lets them go.
#[derive(Default)]
struct Refused {
    generation: Option<u64>,
    names: HashMap<(u32, u32), &'static str>,
}

impl Refused {
    fn keep(&mut self, generation: u64, key: (u32, u32), name: &'static str) {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.names.clear();
        }
        self.names.insert(key, name);
    }

    fn get(&self, generation: u64, key: (u32, u32)) -> Option<&'static str> {
        if self.generation != Some(generation) {
            return None;
        }
        self.names.get(&key).copied()
    }
}

#[derive(Default)]
pub struct Systems {
    parts: Mutex<Definitions<Part>>,
    refused: Mutex<Refused>,
    map: Mutex<Option<Opened>>,
}

/// The guest's memory as the effects decoders read a tag.
struct Guest<'a>(&'a dyn GuestMemory);

impl tags::effects::Memory for Guest<'_> {
    fn read(&mut self, address: u32, len: usize) -> tags::Result<Vec<u8>> {
        self.0.bytes(address, len).map_err(|_| tags::Error::Bounds { address, size: len as u64 })
    }
}

/// The blend of a particle drawn with the template named `name`. Another
/// template is refused by its name's last part (`template_<part>`), one
/// with no name as `template_unnamed`.
pub fn blend(name: Option<&str>) -> Result<Blend, &'static str> {
    let name = name.ok_or("template_unnamed")?;
    TEMPLATES
        .iter()
        .find(|(t, _)| name.eq_ignore_ascii_case(t))
        .map(|&(_, b)| b)
        .ok_or_else(|| refused_template(name))
}

/// `template_<the name's last path part>`, lowercase, made once a name
/// for the process: a refusal is a `&'static str` and the disc has a few
/// dozen templates.
fn refused_template(name: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let part = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let wanted = format!("template_{}", part.to_ascii_lowercase());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&n) = names.iter().find(|n| **n == wanted) {
        return n;
    }
    let n: &'static str = Box::leak(wanted.into_boxed_str());
    names.push(n);
    n
}

/// The image every sprite of the bitmap at `bitmap` cuts from (sprite
/// `+0`, i16): 0 for a bitmap that is not of sprites, or has none.
/// Sprites on two images are refused (`sprite_pages`): a system samples
/// one texture.
pub fn sprite_page(m: &dyn GuestMemory, bitmap: u32) -> Result<i16, &'static str> {
    let unread = |_| "bitmap_unread";
    if m.u16(bitmap).map_err(unread)? != pd::SPRITES {
        return Ok(0);
    }
    let (count, at) = (m.u32(bitmap + 0x3c).map_err(unread)?, m.u32(bitmap + 0x40).map_err(unread)?);
    if count > 256 {
        return Err("bitmap_sequences");
    }
    let mut page = None;
    for s in 0..count {
        let sequence = at.wrapping_add(s * 0x3c);
        let (n, sprites) = (m.u32(sequence + 0x34).map_err(unread)?, m.u32(sequence + 0x38).map_err(unread)?);
        if n > 1024 {
            return Err("bitmap_sprites");
        }
        for k in 0..n {
            let image = m.u16(sprites.wrapping_add(k * 0x20)).map_err(unread)? as i16;
            match page {
                None => page = Some(image),
                Some(p) if p != image => return Err("sprite_pages"),
                Some(_) => {}
            }
        }
    }
    Ok(page.unwrap_or(0))
}

/// The running map's name (the cache header's, `CACHE_MAP_NAME`).
fn map_name(m: &dyn GuestMemory) -> Result<String, &'static str> {
    let bytes = m.bytes(guest_layout::global::CACHE_MAP_NAME.base, 32).map_err(|_| "map_name")?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let name = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if name.is_empty() { Err("map_name") } else { Ok(name) }
}

impl Systems {
    /// `key`'s system from guest memory and the running map.
    fn build(&self, m: &dyn GuestMemory, generation: u64, key: &Key) -> Result<Part, &'static str> {
        if key.bitmap == 0 {
            return Err("no_bitmap");
        }
        let built = mirror_host::build(m, key)?;
        let tag = Particle::decode_at(&mut Ctx::new(&mut Guest(m)), key.definition).map_err(|_| "tag_unread")?;
        let page = sprite_page(m, key.bitmap)?;
        let mut opened = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if opened.as_ref().is_none_or(|o| o.generation != generation) {
            let (map, files) = crate::native_map::open(&map_name(m)?).map_err(|_| "map_unopened")?;
            *opened = Some(Opened { generation, map, files, bitmaps: HashMap::new() });
        }
        let o = opened.as_mut().ok_or("map_unopened")?;
        let template = o.map.tag(tag.shader_template.id).ok().and_then(|t| t.name.clone());
        let blend = blend(template.as_deref())?;
        let id = o
            .map
            .tags()
            .iter()
            .find(|t| t.group == map::FourCC(*b"bitm") && t.location.is_some_and(|l| l.address == key.bitmap))
            .map(|t| t.id)
            .ok_or("bitmap_tag")?;
        let bitmap = match o.bitmaps.get(&(id.0, page)) {
            Some(b) => Arc::clone(b),
            None => {
                let image = crate::native_world_glue::decode_image(&mut o.map, &mut o.files, id, page.max(0) as usize)
                    .ok_or("bitmap_decode")?;
                let b = Arc::new(SpriteBitmap { tag: id.0, image });
                o.bitmaps.insert((id.0, page), Arc::clone(&b));
                b
            }
        };
        Ok(Part { definition: built.definition, sprites: built.sprites, blend, bitmap })
    }
}

impl render_bridge::native_particle_walk::Host for Systems {
    fn accept(&self, m: &dyn GuestMemory, batches: &[Batch]) -> Result<(), &'static str> {
        let generation = render_bridge::native_map_work::generation();
        let mut parts = self.parts.lock().unwrap_or_else(|e| e.into_inner());
        for b in batches {
            if let Err(name) = parts.get(generation, &b.key, || self.build(m, generation, &b.key)) {
                let mut refused = self.refused.lock().unwrap_or_else(|e| e.into_inner());
                refused.keep(generation, (b.key.definition, b.key.bitmap), name);
                return Err(name);
            }
        }
        Ok(())
    }

    fn refused(&self, definition: u32, bitmap: u32) -> Option<&'static str> {
        let generation = render_bridge::native_map_work::generation();
        self.refused.lock().unwrap_or_else(|e| e.into_inner()).get(generation, (definition, bitmap))
    }

    fn publish(&self, batches: Vec<Batch>) {
        let generation = render_bridge::native_map_work::generation();
        let mut parts = self.parts.lock().unwrap_or_else(|e| e.into_inner());
        let mut unbuilt = 0;
        let mut systems = Vec::with_capacity(batches.len());
        for b in batches {
            // Accepted this map: built. A map change since lets them go.
            let Ok(part) = parts.get(generation, &b.key, || Err("unbuilt")) else {
                unbuilt += 1;
                continue;
            };
            systems.push(System {
                bitmap: Arc::clone(&part.bitmap),
                definition: nm::owned(&part.definition, &b.key),
                sprites: part.sprites.clone(),
                blend: part.blend,
                births: b.spawns,
            });
        }
        drop(parts);
        render_bridge::native_layers::count(15, "particle_walk_unbuilt", unbuilt);
        particle_set::publish(Some(Set { systems }));
    }
}

/// Installs the walk's host (with the native particle path's,
/// `native_particles_glue::install`).
pub fn install() {
    render_bridge::native_particle_walk::set_host(Arc::new(Systems::default()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn each_matched_template_has_its_blend_and_others_are_refused_by_name() {
        let t = |part: &str| format!("shaders\\shader_templates\\transparent\\{part}");
        assert_eq!(blend(Some(&t("particle_alpha_blend"))), Ok(Blend::Alpha));
        assert_eq!(blend(Some(&t("particle_additive").to_uppercase())), Ok(Blend::Additive));
        assert_eq!(blend(Some(&t("particle_alpha_multiply_add"))), Ok(Blend::MultiplyAdd));
        assert_eq!(blend(Some(&t("particle_alpha_test_no_lighting_fixed_for_glass"))), Ok(Blend::Glass));
        for part in ["particle_additive_tint", "particle_plasma", "particle_alpha_test_no_lighting"] {
            assert_eq!(blend(Some(&t(part))), Err(format!("template_{part}").as_str()), "{part}");
        }
        assert_eq!(
            blend(Some("shaders\\shader_templates\\particle\\Particle_Alpha_Blend_Distortion")),
            Err("template_particle_alpha_blend_distortion"),
            "the last part, lowercase, not the transparent one of that name"
        );
        let a = blend(Some(&t("particle_plasma"))).unwrap_err();
        assert!(std::ptr::eq(a, blend(Some(&t("particle_plasma"))).unwrap_err()), "one name a template");
        assert_eq!(blend(None), Err("template_unnamed"));
        assert_eq!(TEMPLATES.map(|(_, b)| b), Blend::ALL, "every blend has its template");
    }

    #[test]
    fn a_refusal_is_kept_for_its_system_until_the_map_changes() {
        let mut r = Refused::default();
        assert_eq!(r.get(1, (2, 3)), None);
        r.keep(1, (2, 3), "input_the_gpu_lacks_12");
        assert_eq!(r.get(1, (2, 3)), Some("input_the_gpu_lacks_12"));
        assert_eq!(r.get(1, (2, 4)), None, "another bitmap is another system");
        assert_eq!(r.get(2, (2, 3)), None, "a new map's addresses are its own");
        r.keep(2, (5, 6), "template_particle_plasma");
        assert_eq!(r.get(2, (2, 3)), None, "the last map's let go");
    }

    const BITMAP: u32 = 0x1000;
    const SEQUENCES: u32 = 0x1100;
    const SPRITES: u32 = 0x1200;

    /// A sprites bitmap of one sequence whose sprites cut from `images`.
    fn sheet(images: &[i16]) -> FlatMemory {
        let mut m = FlatMemory::new(0x1000, 0x1000);
        m.set_u16(BITMAP, pd::SPRITES).unwrap();
        m.set_u32(BITMAP + 0x3c, 1).unwrap();
        m.set_u32(BITMAP + 0x40, SEQUENCES).unwrap();
        m.set_u32(SEQUENCES + 0x34, images.len() as u32).unwrap();
        m.set_u32(SEQUENCES + 0x38, SPRITES).unwrap();
        for (k, &i) in images.iter().enumerate() {
            m.set_u16(SPRITES + 0x20 * k as u32, i as u16).unwrap();
        }
        m
    }

    #[test]
    fn a_systems_sprites_cut_from_one_image() {
        assert_eq!(sprite_page(&sheet(&[2, 2, 2]), BITMAP), Ok(2));
        assert_eq!(sprite_page(&sheet(&[]), BITMAP), Ok(0));
        assert_eq!(sprite_page(&sheet(&[0, 1]), BITMAP), Err("sprite_pages"));
        let mut plain = sheet(&[1]);
        plain.set_u16(BITMAP, 0).unwrap();
        assert_eq!(sprite_page(&plain, BITMAP), Ok(0), "not a sprites bitmap");
    }
}
