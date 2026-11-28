//! The pod as tags in a map's [`TagSet`]: its atlas (`bitm`), a shader
//! drawing it (`shad`), its render model (`mode`), a physics model
//! (`phmo`), its model (`hlmt`) and the object (`vehi`), all named
//! [`super::TAG_PATH`].
//!
//! Nothing here is copied from a retail tag. What a map must supply is read
//! from it when the kit is built ([`Facts`]) and names tags the map already
//! holds:
//!
//! - **The shader program.** The `shad` is written from its definition
//!   onto a retail template the map has (`crate::kit::LAYOUTS`, the
//!   first the map can draw), with our atlas as its base map.
//! - **The material.** The `hlmt` and `phmo` name one of the map's global
//!   materials: [`MATERIAL`] where the map has it.
//!
//! **The object is a vehicle**, the class Halo 2's own insertion pod is
//! (docs/game/firefight.md, "The Flood pod"), so the arrival code that
//! parks, moves and reparks a pod treats ours as it would that one. It is
//! the stationary kind (`h1objects::rebuild::ROLE_VEHICLE_TYPE`, the
//! turrets') with no seats, no weapons and no drive, written by the
//! writer the Halo 1 props seated by scripts go through
//! (`h1objects::rebuild::vehicle`): nobody rides it, the squad is placed
//! beside it.

use std::collections::HashMap;
use std::io::{Read, Seek};

use anyhow::{Context, Result, anyhow, bail, ensure};
use h1_assets::models::h1 as src;
use h1_assets::models::mode::{ModelOptions, convert_model, hlmt_nodes};
use h1convert::shad::{self, Bound, Role};
use h1objects::rebuild::{self, Predicted, VehicleSpec};
use map::build::{TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use crate::kit::Shading;
use crate::mesh::{self, Mesh};
use crate::texture::{self, Rgba};

use super::TAG_PATH;
use super::source::{self, Source};

/// The global material the pod is made of, where the map has it; else the
/// map's first.
pub const MATERIAL: &str = "soft_organic_flesh";

/// The pod's bounding sphere, model space: its middle, and a radius
/// covering the tendrils and the petals laid open.
pub const BOUNDING_OFFSET: [f32; 3] = [0.0, 0.0, 0.6];
pub const BOUNDING_RADIUS: f32 = 1.3;
/// The physics model's one sphere: the root bulb.
pub const PHYSICS_CENTRE: [f32; 3] = [0.0, 0.0, 0.38];
pub const PHYSICS_RADIUS: f32 = 0.38;
/// Kilograms.
pub const PHYSICS_MASS: f32 = 600.0;
/// The unit's field of view, radians (70 degrees): no seat shows it.
pub const FIELD_OF_VIEW: f32 = 1.221_730_5;

/// The one region and permutation of the pod.
pub const REGION: &str = "default";

/// What a map supplies to the kit.
#[derive(Clone, Debug)]
pub struct Facts {
    /// The shader templates, and the global material with its index in the
    /// globals.
    pub shading: Shading,
}

impl Facts {
    /// The map's facts.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Facts> {
        let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
        let mut shading = Shading::read_with(map, &meta)?;
        let globals =
            h1convert::materials::global_materials(map, &meta).map_err(|e| anyhow!("{e}"))?;
        if let Some(index) = globals.iter().position(|g| g == MATERIAL) {
            shading.material = (MATERIAL.to_owned(), index as i16);
        }
        Ok(Facts { shading })
    }
}

/// The source and its mesh as the model the `mode` writer takes: one
/// region, one permutation, one section, one part drawn with the one
/// shader, every vertex rigid on its part's node. The nodes are linked as
/// a tree: a node's first child, then that child's next sibling.
pub fn model(source: &Source, mesh: &Mesh) -> Result<src::Model> {
    let world: Vec<[f32; 3]> = source.nodes.iter().map(|n| n.at).collect();
    let mut parents = Vec::with_capacity(source.nodes.len());
    for n in &source.nodes {
        parents.push(match &n.parent {
            None => -1i16,
            Some(p) => source.node(p).with_context(|| format!("no node {p}"))? as i16,
        });
    }
    let mut nodes = Vec::with_capacity(source.nodes.len());
    for (i, n) in source.nodes.iter().enumerate() {
        let p = parents[i];
        let from = if p < 0 { [0.0; 3] } else { world[p as usize] };
        let translation = mesh::sub(n.at, from);
        let child = parents.iter().position(|q| *q == i as i16);
        let sibling = (p >= 0)
            .then(|| (i + 1..parents.len()).find(|j| parents[*j] == p))
            .flatten();
        nodes.push(src::Node {
            name: n.name.clone(),
            next_sibling: sibling.map_or(-1, |j| j as i16),
            first_child: child.map_or(-1, |j| j as i16),
            parent: p,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            distance_from_parent: mesh::length(translation),
        });
    }
    let mut markers = Vec::with_capacity(source.markers.len());
    for m in &source.markers {
        let node = source
            .node(&m.node)
            .with_context(|| format!("no node {}", m.node))?;
        markers.push(src::Marker {
            name: m.name.clone(),
            instances: vec![src::MarkerInstance {
                region: 0,
                permutation: 0,
                node: node as i8,
                translation: mesh::sub(m.at, world[node]),
                rotation: [0.0, 0.0, 0.0, 1.0],
            }],
        });
    }
    let mut vertices = Vec::with_capacity(mesh.vertices());
    let mut triangles: Vec<[u16; 3]> = Vec::with_capacity(mesh.triangles());
    for p in &mesh.parts {
        let base = vertices.len();
        ensure!(
            base + p.vertices.len() <= usize::from(u16::MAX),
            "{}: more than 65535 vertices",
            source.name
        );
        vertices.extend(p.vertices.iter().map(|v| src::Vertex {
            position: v.position,
            normal: v.normal,
            binormal: v.binormal,
            tangent: v.tangent,
            texcoord: v.uv,
            nodes: [p.node as i16, -1],
            weights: [1.0, 0.0],
        }));
        triangles.extend(p.triangles.iter().map(|t| t.map(|i| i + base as u16)));
    }
    let (lo, hi) = mesh.bounds();
    let centroid = mesh::mul(mesh::add(lo, hi), 0.5);
    Ok(src::Model {
        flags: 0,
        node_list_checksum: 0,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0, 1.0],
        markers,
        nodes,
        regions: vec![src::Region {
            name: REGION.to_owned(),
            permutations: vec![src::Permutation {
                name: REGION.to_owned(),
                flags: 0,
                lods: [0; 5],
            }],
        }],
        geometries: vec![src::Geometry {
            flags: 0,
            parts: vec![src::Part {
                flags: 0,
                shader: 0,
                centroid_nodes: [0, -1],
                centroid_weights: [1.0, 0.0],
                centroid,
                vertices,
                // The triangles as given, in the strip form the writer
                // unrolls back into this list.
                strip: src::triangles_strip(&triangles),
            }],
        }],
        // An opaque part: the writer draws a `soso` part opaque.
        shaders: vec![src::ShaderRef {
            group: h1map::Group::new(b"soso"),
            tag: h1map::TagId(0),
            permutation: 0,
        }],
    })
}

/// The pod's tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tags {
    pub vehicle: TagId,
    pub model: TagId,
    pub render: TagId,
    pub physics: TagId,
    pub shader: TagId,
    pub bitmap: TagId,
}

/// The groups of [`Tags`], in the order the kit adds them.
pub const GROUPS: [&[u8; 4]; 6] = [b"bitm", b"shad", b"mode", b"phmo", b"hlmt", b"vehi"];

fn groups(set: &mut TagSet) {
    for group in GROUPS {
        let (parent, grandparent) = if group == b"vehi" {
            (Some(FourCC(*b"unit")), Some(FourCC(*b"obje")))
        } else {
            (None, None)
        };
        set.add_group(FourCC(*group), parent, grandparent);
    }
}

/// Adds the pod's tags to `set`, from the built-in source and atlas.
pub fn install(set: &mut TagSet, facts: &Facts) -> Result<Tags> {
    install_from(set, facts, &source::builtin()?, &source::atlas()?)
}

/// [`install`] from a given source and atlas.
pub fn install_from(
    set: &mut TagSet,
    facts: &Facts,
    source: &Source,
    atlas: &Rgba,
) -> Result<Tags> {
    groups(set);
    for group in GROUPS {
        if set.find(FourCC(*group), TAG_PATH).is_some() {
            bail!("the map already has {TAG_PATH}.{}", FourCC(*group));
        }
    }
    ensure!(
        (atlas.width, atlas.height) == (u32::from(source::atlas_size().0), u32::from(source::atlas_size().1)),
        "{}: the atlas is {}x{}, want {}x{}",
        source.atlas,
        atlas.width,
        atlas.height,
        source::atlas_size().0,
        source::atlas_size().1
    );
    let mesh = source::build(source)?;
    let model = model(source, &mesh)?;
    let shading = &facts.shading;

    let bitmap = texture::bitmap(atlas)?
        .install(set, TAG_PATH)
        .map_err(|e| anyhow!("bitm: {e}"))?;
    let maps = HashMap::from([(
        Role::Base,
        Bound {
            id: bitmap,
            scale: [1.0, 1.0],
            lod_offset: shad::lod_offset(source::atlas_size().0, source::atlas_size().1),
        },
    )]);
    let shader = shading
        .library
        .build(shading.layout, &maps)
        .map_err(|e| anyhow!("shad: {e}"))?;
    let shader = set.add_tag(FourCC(*b"shad"), TAG_PATH, TagPayload::Built(shader))?;

    let render = convert_model(
        &model,
        &source.name,
        &ModelOptions {
            shaders: vec![shader],
            strips: false,
        },
    )
    .and_then(|c| c.install(set, TAG_PATH))
    .map_err(|e| anyhow!("mode: {e}"))?;

    let physics = rebuild::sphere_physics(
        PHYSICS_RADIUS,
        PHYSICS_CENTRE,
        PHYSICS_MASS,
        &shading.material.0,
    )
    .map_err(|e| anyhow!("phmo: {e}"))?;
    let physics = set.add_tag(FourCC(*b"phmo"), TAG_PATH, TagPayload::Built(physics))?;

    let hlmt = rebuild::model(&rebuild::Model {
        render,
        collision: None,
        animation: None,
        physics: Some(physics),
        nodes: hlmt_nodes(&model),
        regions: vec![rebuild::Region {
            name: REGION.to_owned(),
            collision: -1,
            physics: -1,
            permutations: vec![(REGION.to_owned(), -1, -1)],
        }],
        materials: vec![rebuild::ModelMaterial {
            name: shading.material.0.clone(),
            global: shading.material.0.clone(),
            global_index: shading.material.1,
        }],
    })
    .map_err(|e| anyhow!("hlmt: {e}"))?;
    let hlmt = set.add_tag(FourCC(*b"hlmt"), TAG_PATH, TagPayload::Built(hlmt))?;

    let vehicle = vehicle(hlmt, render, bitmap)?;
    let vehicle = set.add_tag(FourCC(*b"vehi"), TAG_PATH, TagPayload::Built(vehicle))?;
    Ok(Tags {
        vehicle,
        model: hlmt,
        render,
        physics,
        shader,
        bitmap,
    })
}

/// The `vehi`: a stationary vehicle with the pod's model and bounding
/// sphere and nothing else, every other field at its definition's default.
pub fn vehicle(hlmt: TagId, render: TagId, bitmap: TagId) -> Result<map::build::Block> {
    rebuild::vehicle(&VehicleSpec {
        object: &h1objects::h1::Object::default(),
        source: &[],
        references: &[],
        bounds: Some((BOUNDING_RADIUS, BOUNDING_OFFSET)),
        hlmt: Some(hlmt),
        seats: &[],
        predicted: &[Predicted::RenderModel(render), Predicted::Bitmap(bitmap)],
        camera_field_of_view: FIELD_OF_VIEW,
        vehicle_type: rebuild::ROLE_VEHICLE_TYPE as u16,
        flags: 0,
        weapons: &[],
        points: &[],
        attachments: &[],
        functions: &[],
        creation_effect: None,
    })
    .map_err(|e| anyhow!("vehi: {e}"))
}
