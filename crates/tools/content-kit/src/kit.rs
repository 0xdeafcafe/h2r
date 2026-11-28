//! One stick as tags in a map's [`TagSet`]: its atlas (`bitm`), a shader
//! drawing it (`shad`), its render model (`mode`), its model (`hlmt`) and
//! the weapon (`weap`), beside one physics model (`phmo`) and one empty
//! weapon HUD (`nhdt`) the three looks share.
//!
//! Nothing here is copied from a retail tag. What a map must supply is read
//! from it when the kit is built ([`Facts`]) and names tags the map already
//! holds:
//!
//! - **The weapon class.** A unit takes a weapon only when its animation
//!   graph has an `idle` for the weapon's class and type
//!   (docs/halo1/objects.md, "Weapon class and type"). The stick has no
//!   animations of its own, so it is held as a weapon both player graphs
//!   already hold: [`player_class`].
//! - **The shader program.** The `shad` is written from its definition
//!   onto a retail template the map has (`h1convert::shad::Library`), with
//!   our atlas as its base map: [`LAYOUTS`], the first the map can draw.
//! - **The material.** The `hlmt` and `phmo` name one of the map's global
//!   materials: [`MATERIAL`] where the map has it.
//!
//! The weapon is inert: no triggers, no barrels, no magazines, no melee
//! damage, no zoom, no first-person model.

use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek};

use anyhow::{Context, Result, anyhow, bail, ensure};
use h1_assets::models::h1 as src;
use h1_assets::models::mode::{ModelOptions, convert_model, hlmt_nodes};
use h1convert::shad::{self, Bound, Layout, Library, Role};
use h1objects::rebuild;
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use tags::anim::AnimationGraph;
use tags::defs::{self, Defs, Val};

use crate::mesh::{self, Mesh};
use crate::source::{self, Source};
use crate::{Style, texture};

/// The bipeds the players are, by tag path: the Spartan and the Elite.
pub const PLAYER_BIPEDS: [&str; 2] = [
    r"objects\characters\masterchief\masterchief_mp",
    r"objects\characters\elite\elite_mp",
];

/// The weapon class and type the stick takes when both players hold it:
/// the magnum's.
pub const PREFERRED_CLASS: (&str, &str) = ("pistol", "hp");

/// The animation mode a unit on foot holds a weapon in.
pub const MODE_COMBAT: &str = "combat";

/// The shader templates the stick can draw with, the first a map has: the
/// model shader with the black default reflection, then the plain one.
pub const LAYOUTS: [&Layout; 2] = [&shad::TEX_BUMP_ENV, &shad::TEX_BUMP];

/// The global material the stick is made of, where the map has it; else
/// the map's first.
pub const MATERIAL: &str = "hard_metal_thin_hum";

/// `bipd +0x38`: the model reference's datum (`obje.model`).
const OBJECT_MODEL: usize = 0x38;
/// `hlmt +0x14`: the animation graph's datum.
const MODEL_ANIMATION: usize = 0x14;

/// `obje.object_type` of a weapon.
const WEAPON_TYPE: u32 = h1objects::arsenal::WEAPON_TYPE;
/// `weapon_flags` bit 16: no binocular zoom.
const NO_BINOCULAR_ZOOM: i64 = 1 << 16;

/// The stick's bounding sphere, model space: its middle and a radius
/// covering both ends.
pub const BOUNDING_OFFSET: [f32; 3] = [0.0, 0.059, -0.165];
pub const BOUNDING_RADIUS: f32 = 0.22;
/// The physics model's one sphere: on the shaft at the sleeve, a little
/// wider than it.
pub const PHYSICS_CENTRE: [f32; 3] = [0.0, 0.059, -0.2];
pub const PHYSICS_RADIUS: f32 = 0.03;
/// Kilograms.
pub const PHYSICS_MASS: f32 = 8.0;

/// The folder the kit's tags are named under.
pub use pogo::FOLDER;

/// The tag every look shares, by group.
pub fn shared_path() -> String {
    format!(r"{FOLDER}\pogo")
}

/// What a map supplies to the kit.
#[derive(Clone, Debug)]
pub struct Facts {
    /// The weapon class and type both player graphs hold.
    pub class: (String, String),
    /// The first-person view the stick is held in: the map's own weapon
    /// of that class's ([`first_person`]).
    pub first_person: Vec<FirstPerson>,
    pub library: Library,
    pub layout: &'static Layout,
    /// The global material and its index in the globals.
    pub material: (String, i16),
}

/// What the models are drawn with, read from the map: the shader templates
/// and the material. The `model` builder of the package build takes only
/// this ([`Facts`] adds what the weapon needs).
#[derive(Clone, Debug)]
pub struct Shading {
    pub library: Library,
    pub layout: &'static Layout,
    /// The global material and its index in the globals.
    pub material: (String, i16),
}

impl Shading {
    /// The map's shader templates and material.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Shading> {
        let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
        Shading::read_with(map, &meta)
    }

    /// [`Shading::read`] with the map's tag memory already read.
    pub fn read_with<R: Read + Seek>(
        map: &mut Map<R>,
        meta: &tags::object::Metadata,
    ) -> Result<Shading> {
        let library = Library::read(map);
        let layout = LAYOUTS
            .into_iter()
            .find(|l| library.has(l))
            .with_context(|| {
                format!(
                    "the map has no shader template the stick draws with ({} or {}, and their default bitmaps)",
                    LAYOUTS[0].stem, LAYOUTS[1].stem
                )
            })?;
        let globals =
            h1convert::materials::global_materials(map, meta).map_err(|e| anyhow!("{e}"))?;
        let index = globals
            .iter()
            .position(|g| g == MATERIAL)
            .or(if globals.is_empty() { None } else { Some(0) })
            .context("the map's globals have no materials")?;
        Ok(Shading {
            library,
            layout,
            material: (globals[index].clone(), index as i16),
        })
    }
}

/// One element of a weapon's first-person block: the arms and weapon
/// model and the graph that animates them, for one player character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstPerson {
    pub model: TagId,
    pub animations: TagId,
}

/// A `weap`'s class and name (string IDs) and its first-person block
/// (`crates/formats/tags/src/defs/item.txt`).
const WEAPON_CLASS: usize = 0x288;
const WEAPON_NAME: usize = 0x28c;
const WEAPON_FIRST_PERSON: usize = 0x2a8;
/// A first-person element: the model's tag reference, then the graph's.
const FIRST_PERSON_SIZE: u32 = 0x10;
/// The most first-person elements taken: retail has one a player
/// character, two.
pub const FIRST_PERSON_MAX: u32 = 4;

/// The weapon whose first-person view the stick takes, of `weapons`
/// (class, name, tag, in the map's order): the first held as `class`
/// exactly, else the first of its class. A weapon with no first-person
/// view is not offered.
pub fn first_person_source(
    weapons: &[(String, String, TagId)],
    class: &(String, String),
) -> Option<TagId> {
    weapons
        .iter()
        .find(|(c, n, _)| *c == class.0 && *n == class.1)
        .or_else(|| weapons.iter().find(|(c, ..)| *c == class.0))
        .map(|(.., id)| *id)
}

/// The first-person block of the map's weapon of `class`
/// ([`first_person_source`]), so that the stick, held, has the arms and
/// view of a weapon the player graphs already animate. Retail reads the
/// block by player character; a weapon with none leaves the first-person
/// camera and draw with nothing to read.
pub fn first_person<R: Read + Seek>(
    map: &mut Map<R>,
    meta: &tags::object::Metadata,
    class: &(String, String),
) -> Result<Vec<FirstPerson>> {
    let strings = map.strings().to_vec();
    let name = |w: u32| -> String {
        strings
            .get((w & 0xffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_default()
    };
    // Every weapon with a first-person block, its class and name.
    let mut weapons = Vec::new();
    let mut blocks = HashMap::new();
    for t in map.tags().iter().filter(|t| t.group == FourCC(*b"weap")) {
        let Some(at) = t.location.map(|l| l.address) else {
            continue;
        };
        let Ok(head) = meta.slice(at, (WEAPON_FIRST_PERSON + 8) as u64) else {
            continue;
        };
        let (count, first) = (
            word(head, WEAPON_FIRST_PERSON)?,
            word(head, WEAPON_FIRST_PERSON + 4)?,
        );
        if count == 0 || count > FIRST_PERSON_MAX {
            continue;
        }
        weapons.push((
            name(word(head, WEAPON_CLASS)?),
            name(word(head, WEAPON_NAME)?),
            t.id,
        ));
        blocks.insert(t.id, (count, first));
    }
    let source = first_person_source(&weapons, class).with_context(|| {
        format!(
            "no weapon of class {:?} with a first-person view to hold the stick in ({} weapons have one)",
            class.0,
            weapons.len()
        )
    })?;
    let (count, first) = blocks[&source];
    elements(map, meta, source, count, first)
}

/// The `count` first-person elements at `first` of `weapon`, each a
/// model and a graph the map has.
fn elements<R: Read + Seek>(
    map: &Map<R>,
    meta: &tags::object::Metadata,
    weapon: TagId,
    count: u32,
    first: u32,
) -> Result<Vec<FirstPerson>> {
    let group_of = |id: TagId| map.tags().iter().find(|t| t.id == id).map(|t| t.group);
    let mut out = Vec::new();
    for i in 0..count {
        let el = meta
            .slice(first + i * FIRST_PERSON_SIZE, u64::from(FIRST_PERSON_SIZE))
            .map_err(|e| anyhow!("first-person element {i}: {e}"))?;
        let fp = FirstPerson {
            model: TagId(word(el, 4)?),
            animations: TagId(word(el, 0xc)?),
        };
        ensure!(
            group_of(fp.model) == Some(FourCC(*b"mode"))
                && group_of(fp.animations) == Some(FourCC(*b"jmad")),
            "the first-person element {i} of weapon {weapon:?} is not a model and a graph"
        );
        out.push(fp);
    }
    Ok(out)
}

/// The first-person block of the map's `weap` `weapon`, as the game
/// reads it.
pub fn first_person_of<R: Read + Seek>(
    map: &mut Map<R>,
    weapon: TagId,
) -> Result<Vec<FirstPerson>> {
    let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
    let at = map
        .tags()
        .iter()
        .find(|t| t.id == weapon && t.group == FourCC(*b"weap"))
        .and_then(|t| t.location.map(|l| l.address))
        .with_context(|| format!("no weapon {weapon:?}"))?;
    let head = meta
        .slice(at, (WEAPON_FIRST_PERSON + 8) as u64)
        .map_err(|e| anyhow!("weapon {weapon:?}: {e}"))?;
    let (count, first) = (
        word(head, WEAPON_FIRST_PERSON)?,
        word(head, WEAPON_FIRST_PERSON + 4)?,
    );
    ensure!(
        count <= FIRST_PERSON_MAX,
        "weapon {weapon:?}: {count} first-person elements"
    );
    elements(map, &meta, weapon, count, first)
}

/// Whether the graph `id`, its parent or a graph it inherits from has an
/// `idle` in any class and type (in the combat mode where it has one).
pub fn graph_has_idle<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<bool> {
    let mut pairs = BTreeSet::new();
    held(map, id, &mut BTreeSet::new(), &mut pairs)?;
    Ok(!pairs.is_empty())
}

fn word(bytes: &[u8], at: usize) -> Result<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
        .with_context(|| format!("a tag of {} bytes read at {at:#x}", bytes.len()))
}

/// Every (class, type) the graph `id`, its parent or a graph it inherits
/// from holds with an `idle` action, in the combat mode where the graph
/// has one.
fn held<R: Read + Seek>(
    map: &mut Map<R>,
    id: TagId,
    seen: &mut BTreeSet<u32>,
    out: &mut BTreeSet<(String, String)>,
) -> Result<()> {
    if id == TagId::NULL || !seen.insert(id.0) || seen.len() > 16 {
        return Ok(());
    }
    let graph = AnimationGraph::read(map, id).map_err(|e| anyhow!("animation graph: {e}"))?;
    let strings = map.strings().to_vec();
    let name = |s: tags::StringId| s.resolve(&strings).ok().flatten().unwrap_or("").to_owned();
    let combat = graph.modes.iter().any(|m| name(m.name) == MODE_COMBAT);
    for m in &graph.modes {
        if combat && name(m.name) != MODE_COMBAT {
            continue;
        }
        for c in &m.classes {
            for t in &c.types {
                if t.actions.iter().any(|a| name(a.name) == "idle") {
                    out.insert((name(c.name), name(t.name)));
                }
            }
        }
    }
    held(map, graph.parent.id, seen, out)?;
    for i in &graph.inheritance {
        held(map, i.graph.id, seen, out)?;
    }
    Ok(())
}

/// The class to take from what each player graph holds: [`PREFERRED_CLASS`]
/// when every graph has it, else the first pair, in name order, that every
/// graph has.
pub fn choose_class(graphs: &[BTreeSet<(String, String)>]) -> Option<(String, String)> {
    let (first, rest) = graphs.split_first()?;
    let common = |pair: &(String, String)| rest.iter().all(|g| g.contains(pair));
    let preferred = (PREFERRED_CLASS.0.to_owned(), PREFERRED_CLASS.1.to_owned());
    if first.contains(&preferred) && common(&preferred) {
        return Some(preferred);
    }
    first
        .iter()
        .find(|p| !p.0.is_empty() && !p.1.is_empty() && common(p))
        .cloned()
}

/// What each of the map's player bipeds holds: its path and its graph's
/// (class, type) pairs with an `idle`.
pub fn player_graphs<R: Read + Seek>(
    map: &mut Map<R>,
) -> Result<Vec<(&'static str, BTreeSet<(String, String)>)>> {
    let mut out = Vec::new();
    for path in PLAYER_BIPEDS {
        let Some(biped) = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"bipd") && t.name.as_deref() == Some(path))
            .map(|t| t.id)
        else {
            continue;
        };
        let root = map
            .read_tag_bytes(biped)
            .map_err(|e| anyhow!("{path}: {e}"))?;
        let model = TagId(word(&root, OBJECT_MODEL)?);
        let hlmt = map
            .read_tag_bytes(model)
            .map_err(|e| anyhow!("{path}: its model: {e}"))?;
        let graph = TagId(word(&hlmt, MODEL_ANIMATION)?);
        let mut pairs = BTreeSet::new();
        held(map, graph, &mut BTreeSet::new(), &mut pairs).with_context(|| path.to_owned())?;
        out.push((path, pairs));
    }
    Ok(out)
}

/// The weapon class and type every player biped of the map holds.
pub fn player_class<R: Read + Seek>(map: &mut Map<R>) -> Result<(String, String)> {
    let graphs = player_graphs(map)?;
    ensure!(
        !graphs.is_empty(),
        "the map has neither player biped ({} or {})",
        PLAYER_BIPEDS[0],
        PLAYER_BIPEDS[1]
    );
    let sets: Vec<_> = graphs.iter().map(|(_, s)| s.clone()).collect();
    choose_class(&sets).with_context(|| {
        format!(
            "no weapon class and type with an idle in every player graph ({})",
            graphs
                .iter()
                .map(|(p, s)| format!("{p}: {} pairs", s.len()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

impl Facts {
    /// The map's facts.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Facts> {
        let class = player_class(map)?;
        Facts::read_with_class(map, class)
    }

    /// The map's shader templates and material, with the class given.
    pub fn read_with_class<R: Read + Seek>(
        map: &mut Map<R>,
        class: (String, String),
    ) -> Result<Facts> {
        let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
        let shading = Shading::read_with(map, &meta)?;
        let first_person = first_person(map, &meta, &class)?;
        Ok(Facts {
            class,
            first_person,
            library: shading.library,
            layout: shading.layout,
            material: shading.material,
        })
    }

    /// What the models are drawn with.
    pub fn shading(&self) -> Shading {
        Shading {
            library: self.library.clone(),
            layout: self.layout,
            material: self.material.clone(),
        }
    }
}

/// The source and its mesh as the model the `mode` writer takes: one
/// region, one permutation, one section, one part drawn with the one
/// shader, every vertex rigid on its part's node.
pub fn model(source: &Source, mesh: &Mesh) -> Result<src::Model> {
    let world: Vec<[f32; 3]> = source.nodes.iter().map(|n| n.at).collect();
    let parent = |i: usize| -> Result<i16> {
        match &source.nodes[i].parent {
            None => Ok(-1),
            Some(p) => Ok(source.node(p).with_context(|| format!("no node {p}"))? as i16),
        }
    };
    let mut nodes = Vec::with_capacity(source.nodes.len());
    for (i, n) in source.nodes.iter().enumerate() {
        let p = parent(i)?;
        let from = if p < 0 { [0.0; 3] } else { world[p as usize] };
        let translation = mesh::sub(n.at, from);
        nodes.push(src::Node {
            name: n.name.clone(),
            next_sibling: -1,
            first_child: source
                .nodes
                .iter()
                .position(|c| c.parent.as_deref() == Some(n.name.as_str()))
                .map_or(-1, |c| c as i16),
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

/// The one region and permutation of every stick.
pub const REGION: &str = "default";

fn def(group: &[u8; 4]) -> Result<&'static defs::StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .with_context(|| format!("no definition for {}", FourCC(*group)))
}

/// The inert weapon: the object header, the model, the class and type, an
/// empty HUD, and every other field at its definition's default.
pub fn weapon(facts: &Facts, hlmt: TagId, hud: TagId) -> Result<Block> {
    let d = def(b"weap")?;
    let mut r = defs::default_record(d);
    let mut set = |path: &str, v: Val| r.set(path, v).map_err(|e| anyhow!("weap {path}: {e}"));
    set("item.obje.object_type", Val::Enum(WEAPON_TYPE))?;
    set("item.obje.bounding_radius", Val::Float(BOUNDING_RADIUS))?;
    set("item.obje.bounding_offset", Val::floats(&BOUNDING_OFFSET))?;
    set("item.obje.model", Val::tag(FourCC(*b"hlmt"), hlmt))?;
    set("weapon_class", Val::text(&facts.class.0))?;
    set("weapon_name", Val::text(&facts.class.1))?;
    set("weapon_flags", Val::Int(NO_BINOCULAR_ZOOM))?;
    set("magnification_levels", Val::Int(0))?;
    set("hud", Val::tag(FourCC(*b"nhdt"), hud))?;
    // Held, the stick shows the first-person arms and weapon of the
    // map's own weapon of its class; the chase camera hides them.
    let element = d
        .child("first_person")
        .ok_or_else(|| anyhow!("weap: no first_person block"))?;
    let mut list = Vec::new();
    for fp in &facts.first_person {
        let mut x = defs::default_record(element);
        x.set("model", Val::tag(FourCC(*b"mode"), fp.model))
            .map_err(|e| anyhow!("weap first_person.model: {e}"))?;
        x.set("animations", Val::tag(FourCC(*b"jmad"), fp.animations))
            .map_err(|e| anyhow!("weap first_person.animations: {e}"))?;
        list.push(x);
    }
    set("first_person", Val::Block(list))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|e| anyhow!("weap: {e}"))
}

/// The tags the three looks share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shared {
    pub physics: TagId,
    pub hud: TagId,
}

/// One look's tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tags {
    pub weapon: TagId,
    pub model: TagId,
    pub render: TagId,
    pub shader: TagId,
    pub bitmap: TagId,
}

fn groups(set: &mut TagSet) {
    for (group, parent, grandparent) in [
        (b"phmo", None, None),
        (b"nhdt", None, None),
        (b"bitm", None, None),
        (b"shad", None, None),
        (b"mode", None, None),
        (b"hlmt", None, None),
        (b"weap", Some(b"item"), Some(b"obje")),
    ] {
        set.add_group(
            FourCC(*group),
            parent.map(|g: &[u8; 4]| FourCC(*g)),
            grandparent.map(|g: &[u8; 4]| FourCC(*g)),
        );
    }
}

/// Adds the shared tags to `set`.
pub fn install_shared(set: &mut TagSet, facts: &Facts) -> Result<Shared> {
    install_shared_at(set, &shared_path(), &facts.material.0, false)
}

/// The shared tags at `path`: added, or, when `reuse` and both are in the
/// set already (an earlier look of the same package added them), found.
pub fn install_shared_at(
    set: &mut TagSet,
    path: &str,
    material: &str,
    reuse: bool,
) -> Result<Shared> {
    groups(set);
    let held = [b"phmo", b"nhdt"].map(|g| set.find(FourCC(*g), path));
    if reuse && let [Some(physics), Some(hud)] = held {
        return Ok(Shared { physics, hud });
    }
    for (group, id) in [b"phmo", b"nhdt"].into_iter().zip(held) {
        ensure!(
            id.is_none(),
            "the map already has {path}.{}",
            FourCC(*group)
        );
    }
    let physics = rebuild::sphere_physics(PHYSICS_RADIUS, PHYSICS_CENTRE, PHYSICS_MASS, material)
        .map_err(|e| anyhow!("phmo: {e}"))?;
    let physics = set.add_tag(FourCC(*b"phmo"), path, TagPayload::Built(physics))?;
    let hud = h1objects::h1hud::nhdt(&[], &[]);
    let hud = set.add_tag(FourCC(*b"nhdt"), path, TagPayload::Built(hud))?;
    Ok(Shared { physics, hud })
}

/// Adds one look's tags to `set`, from the built-in source and atlas.
pub fn install(set: &mut TagSet, facts: &Facts, shared: Shared, style: Style) -> Result<Tags> {
    let source = Source::builtin(style)?;
    let atlas =
        texture::decode(source::atlas(style)).with_context(|| format!("{}.png", style.id()))?;
    install_from(set, facts, shared, style, &source, &atlas)
}

/// [`install`] from a given source and atlas.
pub fn install_from(
    set: &mut TagSet,
    facts: &Facts,
    shared: Shared,
    style: Style,
    source: &Source,
    atlas: &texture::Rgba,
) -> Result<Tags> {
    groups(set);
    let path = style.tag_path();
    if set.find(FourCC(*b"weap"), &path).is_some() {
        bail!("the map already has {path}.weap");
    }
    let models = install_model(set, &facts.shading(), shared, &path, source, atlas)?;
    let weapon = weapon(facts, models.model, shared.hud)?;
    let weapon = set.add_tag(FourCC(*b"weap"), &path, TagPayload::Built(weapon))?;
    Ok(Tags {
        weapon,
        model: models.model,
        render: models.render,
        shader: models.shader,
        bitmap: models.bitmap,
    })
}

/// One look's tags but its weapon: what the package build's `model`
/// builder adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Models {
    pub model: TagId,
    pub render: TagId,
    pub shader: TagId,
    pub bitmap: TagId,
}

/// Adds one look's `bitm`, `shad`, `mode` and `hlmt`, all named `path`.
pub fn install_model(
    set: &mut TagSet,
    shading: &Shading,
    shared: Shared,
    path: &str,
    source: &Source,
    atlas: &texture::Rgba,
) -> Result<Models> {
    groups(set);
    ensure!(
        (atlas.width, atlas.height) == (u32::from(source::atlas_size().0), u32::from(source::atlas_size().1)),
        "{}: the atlas is {}x{}, want {}x{}",
        source.atlas,
        atlas.width,
        atlas.height,
        source::atlas_size().0,
        source::atlas_size().1
    );
    let mesh = mesh::build(source)?;
    let model = model(source, &mesh)?;

    let bitmap = texture::bitmap(atlas)?
        .install(set, path)
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
    let shader = set.add_tag(FourCC(*b"shad"), path, TagPayload::Built(shader))?;

    let render = convert_model(
        &model,
        &source.name,
        &ModelOptions {
            shaders: vec![shader],
            strips: false,
        },
    )
    .and_then(|c| c.install(set, path))
    .map_err(|e| anyhow!("mode: {e}"))?;

    let material = rebuild::ModelMaterial {
        name: shading.material.0.clone(),
        global: shading.material.0.clone(),
        global_index: shading.material.1,
    };
    let hlmt = rebuild::model(&rebuild::Model {
        render,
        collision: None,
        animation: None,
        physics: Some(shared.physics),
        nodes: hlmt_nodes(&model),
        regions: vec![rebuild::Region {
            name: REGION.to_owned(),
            collision: -1,
            physics: -1,
            permutations: vec![(REGION.to_owned(), -1, -1)],
        }],
        materials: vec![material],
    })
    .map_err(|e| anyhow!("hlmt: {e}"))?;
    let model = set.add_tag(FourCC(*b"hlmt"), path, TagPayload::Built(hlmt))?;
    Ok(Models {
        model,
        render,
        shader,
        bitmap,
    })
}

/// Lets a set decomposed from a map gain tags
/// (`h1_assets::sounds::convert::make_index_room`); a no-op the second time.
pub fn make_index_room(set: &mut TagSet) {
    h1_assets::sounds::convert::make_index_room(set);
}

/// Adds the whole kit to a set decomposed from a map: the shared tags and
/// the three looks, by [`Style::index`]. The set's index gains room for
/// them first (`h1_assets::sounds::convert::make_index_room`).
pub fn install_all(set: &mut TagSet, facts: &Facts) -> Result<[Tags; 3]> {
    h1_assets::sounds::convert::make_index_room(set);
    let shared = install_shared(set, facts)?;
    let mut looks = Vec::with_capacity(Style::ALL.len());
    for style in Style::ALL {
        looks.push(install(set, facts, shared, style).with_context(|| style.id().to_owned())?);
    }
    looks
        .try_into()
        .map_err(|_| anyhow!("a kit of other than three looks"))
}
