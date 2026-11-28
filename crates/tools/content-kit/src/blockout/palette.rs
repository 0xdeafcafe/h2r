//! The palette survey: the surfaces a level is made of, for the Material
//! picker (docs/game/forge-blockout.md, "Materials").
//!
//! [`survey`] reads the scenario's structure BSPs: each drawn part's
//! shader, its triangles and their texture coordinates, and the global
//! material of the collision material that names the same shader.
//! [`rank`] turns those into [`Surface`]s, largest area first: a shader's
//! key, the area drawn with it, the median length of one repeat of its
//! texture (so a piece tiles at the density the level does) and its
//! [`Class`]. Nothing is copied from the map; a `Map` material names the
//! shader by its key.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};

use anyhow::{Result, anyhow};
use map::{Map, TagId};
use scenario_edit::geometry::{scenario_bsps, world_parts};
use tags::geometry::{RawSource, TagView};

/// A structure BSP's collision materials (`sbsp +0x0c`, stride `0x14`):
/// the global material index (`i16`, `+0x08`) and the shader's datum
/// (`+0x10`), as `h1convert::textures` writes them.
const COLLISION_MATERIALS: usize = 0x0c;
const COLLISION_MATERIAL: usize = 0x14;
const GLOBAL_INDEX: usize = 0x08;
const SHADER_DATUM: usize = 0x10;


/// What a surface is, from its global material's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Rock, stone, dirt, sand, grass, snow and the like: what the
    /// natural set starts as.
    RockAndGround,
    Metal,
    Other,
}

/// The class of a global material's name: `Metal` when it names metal,
/// `RockAndGround` when it names rock or ground, else `Other`.
pub fn class(global_material: &str) -> Class {
    let name = global_material.to_ascii_lowercase();
    if name.contains("metal") {
        Class::Metal
    } else if crate::blockout::data::data()
        .palette
        .rock_and_ground
        .iter()
        .any(|w| name.contains(w.as_str()))
    {
        Class::RockAndGround
    } else {
        Class::Other
    }
}

/// One render triangle: its corners in world units and their texture
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub positions: [[f32; 3]; 3],
    pub uvs: [[f32; 2]; 3],
}

impl Triangle {
    /// The area in square world units.
    pub fn area(&self) -> f32 {
        let [a, b, c] = self.positions;
        len(cross(sub(b, a), sub(c, a))) / 2.0
    }

    /// The world length of one repeat of the texture across the triangle:
    /// the mean of the lengths a unit step of `u` and of `v` spans. `None`
    /// when its coordinates are degenerate.
    pub fn repeat(&self) -> Option<f32> {
        let [p0, p1, p2] = self.positions;
        let [t0, t1, t2] = self.uvs;
        let (e1, e2) = (sub(p1, p0), sub(p2, p0));
        let (du1, dv1) = (t1[0] - t0[0], t1[1] - t0[1]);
        let (du2, dv2) = (t2[0] - t0[0], t2[1] - t0[1]);
        let det = du1 * dv2 - du2 * dv1;
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let along_u = len(sub(scale(e1, dv2), scale(e2, dv1))) / det.abs();
        let along_v = len(sub(scale(e2, du1), scale(e1, du2))) / det.abs();
        let repeat = (along_u + along_v) / 2.0;
        (repeat.is_finite() && repeat > 0.0).then_some(repeat)
    }
}

/// What a shader draws in the level, before ranking.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    /// The shader's tag path.
    pub shader: String,
    pub class: Class,
    pub triangles: Vec<Triangle>,
}

/// One surface of the level, as the Material picker lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    /// The shader's tag path (a `shad` key).
    pub shader: String,
    /// Square world units drawn with it.
    pub area: f32,
    /// The median world length of one repeat of its texture.
    pub repeat: f32,
    /// [`Surface::repeat`] in block-out units, rounded to a power of two
    /// (1 to `palette.max_tile`): the tile a piece drawn with it takes.
    pub tile: u16,
    pub class: Class,
}

impl Surface {
    /// The shader's base name, as the picker names the surface.
    pub fn name(&self) -> &str {
        self.shader.rsplit('\\').next().unwrap_or(&self.shader)
    }
}

/// `units` rounded to the nearest power of two, by ratio (a value at or
/// over `p * sqrt 2` goes up to `2p`), within 1 to `palette.max_tile`.
pub fn power_of_two(units: f32) -> u16 {
    let mut p: u16 = 1;
    while p < crate::blockout::data::data().palette.max_tile && units >= f32::from(p) * 2.0 {
        p *= 2;
    }
    let next = f32::from(p) * 2.0;
    if p < crate::blockout::data::data().palette.max_tile && units * units >= f32::from(p) * next {
        p * 2
    } else {
        p
    }
}

/// The surfaces of `drawn`, one a shader (entries naming the same shader
/// are merged), largest area first and then by shader path. A shader whose
/// every triangle has degenerate coordinates is left out.
pub fn rank(drawn: Vec<Drawn>) -> Vec<Surface> {
    let mut by_shader: BTreeMap<String, (Class, Vec<Triangle>)> = BTreeMap::new();
    for d in drawn {
        let entry = by_shader
            .entry(d.shader)
            .or_insert_with(|| (d.class, Vec::new()));
        entry.1.extend(d.triangles);
    }
    let mut out: Vec<Surface> = by_shader
        .into_iter()
        .filter_map(|(shader, (class, triangles))| {
            let area = triangles.iter().map(Triangle::area).sum::<f32>();
            let mut repeats: Vec<f32> = triangles.iter().filter_map(Triangle::repeat).collect();
            if repeats.is_empty() {
                return None;
            }
            repeats.sort_by(f32::total_cmp);
            let repeat = repeats[(repeats.len() - 1) / 2];
            Some(Surface {
                shader,
                area,
                repeat,
                tile: power_of_two(repeat * 100.0),
                class,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.area
            .total_cmp(&a.area)
            .then_with(|| a.shader.cmp(&b.shader))
    });
    out
}

/// The surface the natural set starts as: the largest of the rock and
/// ground class, or `None` (then `Dev`).
pub fn natural(surfaces: &[Surface]) -> Option<&Surface> {
    surfaces.iter().find(|s| s.class == Class::RockAndGround)
}

/// Each collision material's shader and global material index, of one
/// structure BSP.
fn collision_materials<R: Read + Seek>(map: &mut Map<R>, bsp: TagId) -> Result<Vec<(TagId, i16)>> {
    let location = map
        .tag(bsp)?
        .location
        .ok_or_else(|| anyhow!("the structure BSP has no payload"))?;
    let bytes = map.read_tag_bytes(bsp)?;
    let view = TagView::new(&bytes, location.address);
    let root = view
        .at(0, COLLISION_MATERIALS + 8)
        .map_err(|e| anyhow!("structure BSP: {e}"))?;
    let rows = view
        .block(root, COLLISION_MATERIALS, COLLISION_MATERIAL)
        .map_err(|e| anyhow!("collision materials: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let shader = u32::from_le_bytes(
                row[SHADER_DATUM..SHADER_DATUM + 4]
                    .try_into()
                    .expect("a shader datum"),
            );
            let global = i16::from_le_bytes(
                row[GLOBAL_INDEX..GLOBAL_INDEX + 2]
                    .try_into()
                    .expect("a global index"),
            );
            (TagId(shader), global)
        })
        .collect())
}

/// The surfaces of `map`'s structure BSPs, ranked ([`rank`]). `raw` reads
/// the sections' geometry (the map itself and its shared caches).
/// Transparent parts and parts with no shader are left out; a shader no
/// collision material names is `Other`.
pub fn survey<R: Read + Seek>(map: &mut Map<R>, raw: &mut dyn RawSource) -> Result<Vec<Surface>> {
    let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
    let globals = h1convert::materials::global_materials(map, &meta).map_err(|e| anyhow!("{e}"))?;
    let mut classes: HashMap<TagId, Class> = HashMap::new();
    for bsp in scenario_bsps(map).map_err(|e| anyhow!("{e}"))? {
        for (shader, global) in collision_materials(map, bsp)? {
            let name = usize::try_from(global)
                .ok()
                .and_then(|i| globals.get(i))
                .map_or("", String::as_str);
            classes.entry(shader).or_insert_with(|| class(name));
        }
    }
    let (parts, _failed) = world_parts(map, raw).map_err(|e| anyhow!("{e}"))?;
    let mut drawn = Vec::new();
    for part in parts {
        if part.transparent || part.shader == TagId::NULL {
            continue;
        }
        let Ok(tag) = map.tag(part.shader) else {
            continue;
        };
        let Some(shader) = tag.name.clone() else {
            continue;
        };
        let triangles = part
            .tris
            .iter()
            .map(|t| Triangle {
                positions: t.map(|i| part.positions[i as usize]),
                uvs: t.map(|i| part.uvs.get(i as usize).copied().unwrap_or([0.0; 2])),
            })
            .collect();
        drawn.push(Drawn {
            shader,
            class: classes.get(&part.shader).copied().unwrap_or(Class::Other),
            triangles,
        });
    }
    Ok(rank(drawn))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square of `side` world units on the plane z = `z`, its texture
    /// repeating `repeats` times across it, as two triangles.
    fn square(side: f32, z: f32, repeats: f32) -> Vec<Triangle> {
        let p = |x: f32, y: f32| [x * side, y * side, z];
        let t = |x: f32, y: f32| [x * repeats, y * repeats];
        vec![
            Triangle {
                positions: [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)],
                uvs: [t(0.0, 0.0), t(1.0, 0.0), t(1.0, 1.0)],
            },
            Triangle {
                positions: [p(0.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
                uvs: [t(0.0, 0.0), t(1.0, 1.0), t(0.0, 1.0)],
            },
        ]
    }

    fn drawn(shader: &str, class: Class, triangles: Vec<Triangle>) -> Drawn {
        Drawn {
            shader: shader.into(),
            class,
            triangles,
        }
    }

    #[test]
    fn the_survey_ranks_by_area_and_gives_the_repeat_length() {
        let rock = r"invented\shaders\rock";
        let metal = r"invented\shaders\plate";
        let trim = r"invented\shaders\trim";
        // The rock is drawn in two parts: 10 by 10 at 5 repeats (2 world
        // units a repeat) and 4 by 4 at 2 repeats (the same density).
        let surfaces = rank(vec![
            drawn(metal, Class::Metal, square(3.0, 0.0, 6.0)),
            drawn(rock, Class::RockAndGround, square(10.0, 0.0, 5.0)),
            drawn(trim, Class::Other, square(1.0, 2.0, 1.0)),
            drawn(rock, Class::RockAndGround, square(4.0, 1.0, 2.0)),
        ]);
        let order: Vec<&str> = surfaces.iter().map(|s| s.shader.as_str()).collect();
        assert_eq!(order, [rock, metal, trim]);
        assert_eq!(surfaces[0].area, 116.0);
        assert_eq!(surfaces[0].repeat, 2.0);
        // 200 units is nearer 256 than 128 by ratio.
        assert_eq!(surfaces[0].tile, 256);
        assert_eq!(surfaces[1].repeat, 0.5);
        assert_eq!(surfaces[1].tile, 64);
        assert_eq!(surfaces[2].repeat, 1.0);
        assert_eq!(surfaces[2].tile, 128);
        assert_eq!(surfaces[0].name(), "rock");
        assert_eq!(natural(&surfaces).map(|s| s.shader.as_str()), Some(rock));
        assert_eq!(natural(&surfaces[1..]), None);
    }

    #[test]
    fn a_degenerate_surface_is_left_out_and_ties_go_by_path() {
        let flat = Triangle {
            positions: [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: [[0.0; 2]; 3],
        };
        let surfaces = rank(vec![
            drawn("b", Class::Other, square(1.0, 0.0, 1.0)),
            drawn("a", Class::Other, square(1.0, 0.0, 1.0)),
            drawn("smear", Class::Other, vec![flat]),
        ]);
        let order: Vec<&str> = surfaces.iter().map(|s| s.shader.as_str()).collect();
        assert_eq!(order, ["a", "b"]);
    }

    #[test]
    fn classes_and_tiles() {
        assert_eq!(class("hard_metal_thin"), Class::Metal);
        assert_eq!(class("soft_terrain_dirt"), Class::RockAndGround);
        assert_eq!(class("Hard_Rock"), Class::RockAndGround);
        assert_eq!(class("soft_organic_flesh"), Class::Other);
        assert_eq!(power_of_two(0.0), 1);
        assert_eq!(power_of_two(16.0), 16);
        assert_eq!(power_of_two(22.0), 16);
        assert_eq!(power_of_two(23.0), 32);
        assert_eq!(power_of_two(1.0e9), crate::blockout::data::data().palette.max_tile);
    }
}
