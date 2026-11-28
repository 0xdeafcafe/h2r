//! [`install`]: one definition's tags added to a map's tag set, and what
//! they cost.

use std::io::{Read, Seek};

use anyhow::{Context, Result, anyhow, bail, ensure};
use forge_shapes::{Material, PieceDef, Role};
use h1_assets::models::mode::{ModelOptions, convert_model};
use h1convert::shad::{self, Layout, Library};
use h1objects::rebuild::{self, ModelMaterial};
use map::build::{Block, FixupKind, TagPayload, TagSet};
use map::{FourCC, Map, TagId};

use crate::blockout::mesh::{self, Mesh, Paint, View};
use crate::blockout::{collision, dev, object, physics};

/// What a map supplies: the shader template the dev set and the plates
/// draw with, and the global material a piece is made of.
#[derive(Clone, Debug)]
pub struct Facts {
    pub library: Library,
    /// `shad::TEX_BUMP`.
    pub layout: &'static Layout,
    /// The global material and its index in the globals: the one
    /// `h1objects` gives every built physics piece
    /// (`rebuild::PIECE_MATERIAL`) where the map has it, else the first.
    pub material: (String, i16),
}

impl Facts {
    /// The map's facts. A map without the template or materials is refused
    /// by name.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Facts> {
        let meta = tags::object::Metadata::read(map).map_err(|e| anyhow!("tag memory: {e}"))?;
        let library = Library::read(map);
        let layout = &shad::TEX_BUMP;
        ensure!(
            library.has(layout),
            "the map has no shader template the block-out pieces draw with ({} and its default bitmaps)",
            layout.stem
        );
        let globals =
            h1convert::materials::global_materials(map, &meta).map_err(|e| anyhow!("{e}"))?;
        let index = globals
            .iter()
            .position(|g| g == rebuild::PIECE_MATERIAL.0)
            .or(if globals.is_empty() { None } else { Some(0) })
            .context("the map's globals have no materials")?;
        Ok(Facts {
            library,
            layout,
            material: (globals[index].clone(), index as i16),
        })
    }
}

/// What an install added to a set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// Bytes of tag memory: each added tag's size as the builder lays it
    /// out (its blocks in emission order, each at its alignment).
    pub tag_memory: u64,
    /// Bytes of raw resources (the bitmaps' and render models' data).
    pub raw: u64,
    /// Tags added: each takes an index entry.
    pub tags: u32,
}

impl std::ops::Add for Cost {
    type Output = Cost;
    fn add(self, o: Cost) -> Cost {
        Cost {
            tag_memory: self.tag_memory + o.tag_memory,
            raw: self.raw + o.raw,
            tags: self.tags + o.tags,
        }
    }
}

impl std::iter::Sum for Cost {
    fn sum<I: Iterator<Item = Cost>>(iter: I) -> Cost {
        iter.fold(Cost::default(), |a, b| a + b)
    }
}

/// The size a built tag takes, as `map::build` lays it out: the root at 0,
/// then each block in the order the builder emits them (a block's bytes,
/// then its fixups' children in order), each at its alignment.
pub fn tag_size(root: &Block) -> u64 {
    fn emit(b: &Block, cursor: &mut u64) {
        let align = u64::from(b.align.max(1));
        *cursor = cursor.div_ceil(align) * align + b.bytes.len() as u64;
        for f in &b.fixups {
            match &f.kind {
                FixupKind::Block { count, child } if *count != 0 => emit(child, cursor),
                FixupKind::Data(child) if !child.bytes.is_empty() => emit(child, cursor),
                FixupKind::Pointer(child) => emit(child, cursor),
                _ => {}
            }
        }
    }
    let mut cursor = 0;
    emit(root, &mut cursor);
    cursor
}

/// What the set gained since it had `slots` slots and `raw` raw resources
/// (`set.slots.len()` and `set.raw.len()` taken before the adds).
pub fn cost_since(set: &TagSet, slots: usize, raw: usize) -> Cost {
    let mut cost = Cost {
        raw: set.raw[raw..].iter().map(|r| r.bytes.len() as u64).sum(),
        ..Cost::default()
    };
    for slot in &set.slots[slots..] {
        let Some(tag) = &slot.tag else { continue };
        cost.tags += 1;
        if let TagPayload::Built(root) = &tag.payload {
            cost.tag_memory += tag_size(root);
        }
    }
    cost
}

/// A definition's tags in a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The definition's name (`PieceDef::name`).
    pub name: String,
    pub crate_tag: TagId,
    pub model: TagId,
    /// The render model the `hlmt` names; `None` when no face is drawn.
    pub render: Option<TagId>,
    /// The dev render model (every role `Dev`); the same as `render` when
    /// every role is `Dev`, `None` when no face is drawn.
    pub dev_render: Option<TagId>,
    pub collision: TagId,
    pub physics: TagId,
    /// Everything this install added, the dev set included when it added
    /// it.
    pub cost: Cost,
}

/// Declares every group the kit writes.
pub(crate) fn groups(set: &mut TagSet) {
    for group in [b"bitm", b"shad", b"mode", b"coll", b"phmo", b"hlmt"] {
        set.add_group(FourCC(*group), None, None);
    }
    set.add_group(FourCC(*b"bloc"), Some(FourCC(*b"obje")), None);
}

/// Lets a set decomposed from a map gain tags
/// (`h1_assets::sounds::convert::make_index_room`); a no-op the second
/// time. A set made with `TagSet::from_map` needs it before [`install`].
pub fn make_index_room(set: &mut TagSet) {
    h1_assets::sounds::convert::make_index_room(set);
}

/// The shader of each part of `mesh`, in part order: the dev set's for
/// tiles and the font (added when the set lacks it), the base map's for a
/// `Map` paint.
fn shaders(set: &mut TagSet, facts: &Facts, mesh: &Mesh) -> Result<Vec<TagId>> {
    let needs_dev = mesh
        .parts
        .iter()
        .any(|p| matches!(p.paint, Paint::Tile(_) | Paint::Font));
    let dev = if needs_dev {
        Some(dev::install(set, facts)?)
    } else {
        None
    };
    mesh.parts
        .iter()
        .map(|p| match (&p.paint, dev) {
            (Paint::Tile(role), Some(d)) => Ok(d.tile(*role)),
            (Paint::Font, Some(d)) => Ok(d.font),
            (Paint::Map(path), _) => map_shader(set, path),
            _ => bail!("a dev part without the dev set"),
        })
        .collect()
}

fn map_shader(set: &TagSet, path: &str) -> Result<TagId> {
    set.find(FourCC(*b"shad"), path)
        .with_context(|| format!("the base map has no shader {path}"))
}

/// The `mode` of `mesh` at `path`, found when the set has it (a dev model
/// another definition added).
fn render_model(
    set: &mut TagSet,
    facts: &Facts,
    mesh: &Mesh,
    path: &str,
    name: &str,
) -> Result<TagId> {
    if let Some(id) = set.find(FourCC(*b"mode"), path) {
        return Ok(id);
    }
    let model = object::model(Some(mesh))?;
    let shaders = shaders(set, facts, mesh)?;
    convert_model(
        &model,
        name,
        &ModelOptions {
            shaders,
            strips: false,
        },
    )
    .and_then(|c| c.install(set, path))
    .map_err(|e| anyhow!("{path}.mode: {e}"))
}

/// Adds `def`'s tags to `set` (docs/game/forge-blockout.md, "A definition
/// as tags"). Refused, with nothing added, when the definition does not
/// generate, its collision fails its check, a `Map` material names a
/// shader the set lacks, or the set has the definition already.
pub fn install(set: &mut TagSet, def: &PieceDef, facts: &Facts) -> Result<Installed> {
    let name = def.name();
    let path = def.tag_path();
    let solid = def.generate().map_err(|e| anyhow!("{name}: {e}"))?;
    ensure!(
        set.find(FourCC(*b"bloc"), &path).is_none(),
        "the map already has {path}.bloc"
    );
    let compiled = collision::compile(&solid).with_context(|| name.clone())?;
    let physics = physics::phmo(&solid.hulls).with_context(|| name.clone())?;
    let look = mesh::build(def, &solid, View::Look);
    for part in &look.parts {
        if let Paint::Map(shader) = &part.paint {
            map_shader(set, shader).with_context(|| name.clone())?;
        }
    }
    object::model(Some(&look)).with_context(|| name.clone())?;

    groups(set);
    let (slots, raw) = (set.slots.len(), set.raw.len());
    let collision = set.add_tag(
        FourCC(*b"coll"),
        &path,
        TagPayload::Built(collision::coll(&compiled.bsp)),
    )?;
    let physics = set.add_tag(FourCC(*b"phmo"), &path, TagPayload::Built(physics))?;

    let all_dev = Role::ALL
        .iter()
        .all(|r| *def.materials.get(*r) == Material::Dev);
    let twin = PieceDef::dev(def.shape, def.size, def.param);
    let (render, dev_render) = if look.is_empty() {
        (None, None)
    } else if all_dev {
        let m = render_model(set, facts, &look, &twin.tag_path(), &twin.name())?;
        (Some(m), Some(m))
    } else {
        let r = render_model(set, facts, &look, &path, &name)?;
        let dev_mesh = mesh::build(def, &solid, View::Dev);
        let d = render_model(set, facts, &dev_mesh, &twin.tag_path(), &twin.name())?;
        (Some(r), Some(d))
    };

    let material = ModelMaterial {
        name: crate::blockout::data::data().collision_material.clone(),
        global: facts.material.0.clone(),
        global_index: facts.material.1,
    };
    let hlmt = object::hlmt(render, collision, physics, material)?;
    let model = set.add_tag(FourCC(*b"hlmt"), &path, TagPayload::Built(hlmt))?;
    let (lo, hi) = solid.bounds();
    let bloc = object::bloc(model, lo, hi)?;
    let crate_tag = set.add_tag(FourCC(*b"bloc"), &path, TagPayload::Built(bloc))?;
    Ok(Installed {
        name,
        crate_tag,
        model,
        render,
        dev_render,
        collision,
        physics,
        cost: cost_since(set, slots, raw),
    })
}
