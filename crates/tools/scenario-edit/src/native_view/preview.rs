//! One object, or a render model alone, drawn offscreen through the native
//! renderer the viewport draws placed objects with ([`NativeObjects`]),
//! with no level around it: Studio's `model.render`
//! (docs/editor/agent-api.md, "`model.*`").
//!
//! - [`object`]: an object tag's model at a variant (a collection stands
//!   for its first entry, as the viewport draws it).
//! - [`render_model`]: a `mode` no `hlmt` names, such as a weapon's
//!   first-person model or the globals' first-person hands.
//! - [`markers`]: a `mode`'s markers (`mode +0x58`), and [`marker_frame`]
//!   where one sits in the rest pose, so a weapon can be put in a hand.
//! - [`render`]: the pieces, each placed by a node matrix, into a picture.
//!
//! A material whose shader names no base map the renderer decodes is
//! drawn [`UNTEXTURED`] rather than left out, so a preview shows every
//! triangle the model has; the piece lists those materials.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::Arc;

use assets::ResourceFiles;
use map::{Map, TagId};
use native_render::gpu::Image;
use native_render::load;
use native_render::model::{Model, NodeMatrix};
use native_render::model_load::{self, VariantRegion};
use tags::StringId;
use tags::model::RenderModelGeometry;
use tags::object::{Context, PermutationChoice};

use super::{Clear, Files, Lighting, NativeModel, NativeModels, NativeObjects, invert};

/// The node matrix a piece is placed by (the renderer's), so callers
/// need no dependency on the renderer crate.
pub use native_render::model::NodeMatrix as Matrix;
use crate::geometry::{Camera, Exact, Marker};
use crate::layout::BlockKind;

/// The grey a material with no decoded base map is drawn in.
pub const UNTEXTURED: [u8; 4] = [150, 150, 150, 255];

/// `mode +0x58`: the marker groups, 0xc bytes each (a name, then a block
/// of 0x24-byte instances: node at `+2`, translation at `+4`, rotation
/// quaternion at `+0x10`).
const MARKER_GROUPS: usize = 0x58;
const GROUP_SIZE: usize = 0xc;
const MARKER_SIZE: usize = 0x24;

/// A model ready to draw, with what the preview made of it.
#[derive(Clone, Debug)]
pub struct Piece {
    pub model: NativeModel,
    /// The render model drawn.
    pub mode: TagId,
    /// Materials drawn [`UNTEXTURED`]: no base map decoded.
    pub untextured: Vec<u16>,
    /// The permutation drawn per region of the variant, `(region,
    /// permutation)`; empty for a render model alone (each region's first).
    pub picked: Vec<(String, String)>,
}

impl Piece {
    /// The model's rest-pose bounds in its own space.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.model.model.bounds.map(|b| (b.min, b.max))
    }

    pub fn triangles(&self) -> usize {
        self.model.model.indices.len() / 3
    }
}

/// The string ID the map has for `name`.
pub fn string_id(ctx: &Context, name: &str) -> Option<StringId> {
    let index = ctx
        .catalog
        .strings
        .iter()
        .position(|s| s.as_deref() == Some(name))?;
    let len = u32::try_from(name.len()).ok().filter(|n| *n < 0x100)?;
    Some(StringId((len << 24) | (index as u32 & 0x00ff_ffff)))
}

/// The permutation drawn for a variant region: the likeliest (the first
/// of equals), so a region the game leaves to chance still draws one.
fn likeliest(perms: &[PermutationChoice]) -> Option<&PermutationChoice> {
    perms.iter().fold(None, |best: Option<&PermutationChoice>, p| match best {
        Some(b) if b.probability >= p.probability => Some(b),
        _ => Some(p),
    })
}

/// The model object tag `object` draws (a collection's first entry) at
/// the variant named `variant`, or the object's default variant.
pub fn object<R: Read + Seek>(
    map: &mut Map<R>,
    ctx: &Context,
    files: &mut ResourceFiles,
    object: TagId,
    variant: Option<&str>,
) -> Result<Piece, String> {
    let (definition, resolved) = crate::object_collision::resolve(ctx, object)?;
    let resolved = match variant {
        None => resolved.ok_or("the object names no model (obje.model is null)")?,
        Some(name) => {
            let header = ctx
                .resolve_object(definition)
                .map_err(|e| e.to_string())?
                .header;
            let sid = string_id(ctx, name)
                .ok_or_else(|| format!("the model has no variant {name:?}"))?;
            let r = ctx
                .resolve_model(header.model.id, sid)
                .map_err(|e| e.to_string())?;
            if r.variant.index().is_none() {
                return Err(format!("the model has no variant {name:?}"));
            }
            r
        }
    };
    let mode = resolved
        .render_model
        .as_ref()
        .ok_or("the hlmt names no render model")?
        .id;
    let geo = RenderModelGeometry::read(map, mode, &mut Files(files)).map_err(|e| e.to_string())?;
    if resolved.node_count as usize != geo.nodes.len() {
        return Err(format!(
            "the hlmt has {} nodes, its render model {}",
            resolved.node_count,
            geo.nodes.len()
        ));
    }
    let mut picked = Vec::new();
    let variant: Vec<VariantRegion<'_>> = resolved
        .regions
        .iter()
        .filter_map(|v| {
            let p = likeliest(&v.permutations)?;
            picked.push((
                v.region.clone().unwrap_or_default(),
                p.name.clone().unwrap_or_default(),
            ));
            Some(VariantRegion {
                name: v.region.as_deref(),
                permutations: vec![p.name.as_deref()],
            })
        })
        .collect();
    let built = model_load::variant_model(&geo, &variant).map_err(|e| e.to_string())?;
    let mut piece = finish(map, files, definition, mode, built.model, &geo)?;
    piece.picked = picked;
    Ok(piece)
}

/// The render model `mode` alone, each region's first permutation, keyed
/// by its own tag.
pub fn render_model<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    mode: TagId,
) -> Result<Piece, String> {
    let geo = RenderModelGeometry::read(map, mode, &mut Files(files)).map_err(|e| e.to_string())?;
    let built = model_load::variant_model(&geo, &[]).map_err(|e| e.to_string())?;
    finish(map, files, mode, mode, built.model, &geo)
}

fn finish<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    key: TagId,
    mode: TagId,
    model: Model,
    geo: &RenderModelGeometry,
) -> Result<Piece, String> {
    if model.draws.is_empty() {
        return Err("the render model has nothing opaque to draw".into());
    }
    let mut base = HashMap::new();
    let mut untextured = Vec::new();
    for m in model.materials() {
        let picture = geo
            .materials
            .get(usize::from(m))
            .map(|x| x.shader.id)
            .and_then(|shader| load::base_map(map, shader))
            .and_then(|bitmap| load::decode_first(map, files, bitmap));
        let image = picture.unwrap_or_else(|| {
            untextured.push(m);
            Image::solid(UNTEXTURED)
        });
        base.insert(m, image);
    }
    Ok(Piece {
        model: NativeModel {
            object: key,
            model,
            base,
        },
        mode,
        untextured,
        picked: Vec::new(),
    })
}

/// One marker of a render model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMarker {
    pub name: String,
    pub node: usize,
    pub translation: [f32; 3],
    /// A unit quaternion, `[i, j, k, w]`.
    pub rotation: [f32; 4],
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn float(b: &[u8], at: usize) -> f32 {
    f32::from_bits(word(b, at))
}

/// The elements of the block whose count and pointer are at `b[at..]`.
fn block<'a>(ctx: &'a Context, b: &[u8], at: usize, size: usize) -> Result<&'a [u8], String> {
    let count = word(b, at) as usize;
    if count == 0 {
        return Ok(&[]);
    }
    ctx.meta
        .slice(word(b, at + 4), (count * size) as u64)
        .map_err(|e| e.to_string())
}

/// The markers of render model `mode`, every instance of every group, in
/// the tag's order.
pub fn markers(ctx: &Context, mode: TagId) -> Result<Vec<ModelMarker>, String> {
    let (address, size) = ctx
        .catalog
        .get(mode)
        .and_then(|t| t.location)
        .ok_or("the render model has no tag data")?;
    if (size as usize) < MARKER_GROUPS + 8 {
        return Err("the render model is too short for its marker groups".into());
    }
    let root = ctx
        .meta
        .slice(address, u64::from(size))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for g in block(ctx, root, MARKER_GROUPS, GROUP_SIZE)?.chunks_exact(GROUP_SIZE) {
        let name = StringId(word(g, 0))
            .resolve(&ctx.catalog.strings)
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned();
        for m in block(ctx, g, 4, MARKER_SIZE)?.chunks_exact(MARKER_SIZE) {
            out.push(ModelMarker {
                name: name.clone(),
                node: usize::from(m[2]),
                translation: [float(m, 4), float(m, 8), float(m, 12)],
                rotation: [float(m, 0x10), float(m, 0x14), float(m, 0x18), float(m, 0x1c)],
            });
        }
    }
    Ok(out)
}

/// A unit quaternion `[i, j, k, w]` as forward, left and up axes.
fn axes(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    let n = (x * x + y * y + z * z + w * w).sqrt();
    if !n.is_finite() || n < 1e-6 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)],
        [2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)],
        [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Where `marker` sits in `piece`'s model space in the rest pose: its
/// node's rest frame, then the marker's own translation and rotation.
pub fn marker_frame(piece: &Piece, marker: &ModelMarker) -> NodeMatrix {
    let node = piece
        .model
        .model
        .inverse_binds
        .get(marker.node)
        .map_or(NodeMatrix::IDENTITY, invert);
    let [forward, left, up] = axes(marker.rotation);
    node.compose(&NodeMatrix {
        scale: 1.0,
        forward,
        left,
        up,
        position: marker.translation,
    })
}

/// The bounds of the pieces as placed.
pub fn placed_bounds(pieces: &[(&Piece, NodeMatrix)]) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    for (piece, at) in pieces {
        let Some((min, max)) = piece.bounds() else {
            continue;
        };
        for corner in 0..8 {
            let c = [
                if corner & 1 == 0 { min[0] } else { max[0] },
                if corner & 2 == 0 { min[1] } else { max[1] },
                if corner & 4 == 0 { min[2] } else { max[2] },
            ];
            let p = at.point(c);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            any = true;
        }
    }
    any.then_some((lo, hi))
}

/// An orbit camera at `yaw` and `pitch` (radians) that fits `bounds` in
/// its view.
pub fn framing(bounds: ([f32; 3], [f32; 3]), yaw: f32, pitch: f32) -> Camera {
    let (lo, hi) = bounds;
    let target: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]).powi(2))
        .sum::<f32>()
        .sqrt()
        .max(1e-3)
        * 0.5;
    let fov_y = 40f32.to_radians();
    let distance = radius / (fov_y * 0.5).sin() * 1.1;
    Camera {
        target,
        yaw,
        pitch,
        distance,
        fov_y,
        exact: None,
    }
}

/// The first-person camera: the eye at the first-person models' origin,
/// looking along their +x with +z up, at the game's default horizontal
/// field of view `fov_x` (radians) for a `size` picture.
pub fn first_person_camera(fov_x: f32, [width, height]: [u32; 2]) -> Camera {
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let fov_y = 2.0 * ((fov_x * 0.5).tan() / aspect).atan();
    Camera {
        target: [1.0, 0.0, 0.0],
        yaw: std::f32::consts::PI,
        pitch: 0.0,
        distance: 1.0,
        fov_y,
        exact: Some(Exact {
            eye: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            near: 0.01,
            far: 100.0,
        }),
    }
}

/// Draws `pieces`, each placed by its node matrix, seen by `camera`, over
/// `background` (sRGB-encoded), into a new `size` texture in the format
/// the viewport's native passes write (`render::UI_FORMAT`, RGBA8),
/// readable by a copy.
pub fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pieces: &[(&Piece, NodeMatrix)],
    camera: &Camera,
    size: [u32; 2],
    background: [f64; 3],
) -> wgpu::Texture {
    let models = NativeModels {
        models: pieces
            .iter()
            .map(|(p, _)| (p.model.object, Arc::new(p.model.clone())))
            .collect(),
        errors: Vec::new(),
    };
    let markers: Vec<Marker> = pieces
        .iter()
        .enumerate()
        .map(|(index, (p, at))| Marker {
            block: BlockKind::Scenery,
            index,
            position: at.position,
            axes: [at.forward, at.left, at.up],
            scale: at.scale,
            object: Some(p.model.object),
        })
        .collect();
    let mut objects = NativeObjects::new(device, queue, &models, Lighting::Flat);
    objects.place(&models, &markers, &[]);
    let extent = wgpu::Extent3d {
        width: size[0].max(1),
        height: size[1].max(1),
        depth_or_array_layers: 1,
    };
    let color_format = crate::render::UI_FORMAT;
    let depth_format = crate::render::DEPTH_FORMAT;
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model preview colour"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model preview depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    objects.render(
        device,
        queue,
        &color_view,
        color_format,
        &depth_view,
        depth_format,
        [extent.width, extent.height],
        camera,
        Clear::all(background),
    );
    color
}

/// How many pixels of `rgba` differ from `background` (8-bit sRGB): what
/// the pieces cover.
pub fn covered(rgba: &[u8], background: [f64; 3]) -> usize {
    let bg: [u8; 3] = std::array::from_fn(|k| (background[k].clamp(0.0, 1.0) * 255.0).round() as u8);
    rgba.chunks_exact(4)
        .filter(|p| (0..3).any(|k| p[k].abs_diff(bg[k]) > 2))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity quaternion is the identity frame; a half turn about z
    /// turns forward to back and left to right.
    #[test]
    fn a_quaternion_turns_the_axes() {
        assert_eq!(
            axes([0.0, 0.0, 0.0, 1.0]),
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        let half = axes([0.0, 0.0, 1.0, 0.0]);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6);
        assert!(close(half[0], [-1.0, 0.0, 0.0]), "{half:?}");
        assert!(close(half[1], [0.0, -1.0, 0.0]), "{half:?}");
        assert!(close(half[2], [0.0, 0.0, 1.0]), "{half:?}");
        // A zero quaternion is read as no turn.
        assert_eq!(axes([0.0; 4]), axes([0.0, 0.0, 0.0, 1.0]));
    }

    /// The likeliest permutation wins, the first of equals.
    #[test]
    fn the_likeliest_permutation_is_drawn() {
        let p = |name: &str, probability| PermutationChoice {
            name: Some(name.into()),
            probability,
            model_permutation: None,
        };
        let perms = [p("a", 0.2), p("b", 0.5), p("c", 0.5)];
        assert_eq!(likeliest(&perms).unwrap().name.as_deref(), Some("b"));
        assert!(likeliest(&[]).is_none());
    }

    /// The first-person camera looks along +x from the origin with +z up.
    #[test]
    fn the_first_person_camera_looks_along_x() {
        let c = first_person_camera(70f32.to_radians(), [160, 90]);
        assert_eq!(c.eye(), [0.0; 3]);
        assert_eq!(c.forward(), [1.0, 0.0, 0.0]);
        // A wide picture: the vertical angle is the narrower.
        assert!(c.fov_y < 70f32.to_radians());
    }

    #[test]
    fn covered_counts_pixels_off_the_background() {
        let bg = [0.0, 0.0, 0.0];
        let rgba = [0, 0, 0, 255, 200, 10, 10, 255, 1, 1, 1, 255];
        assert_eq!(covered(&rgba, bg), 1);
    }
}
