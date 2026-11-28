//! The stage as a map: a `fresh` build over a [`TagSource`] (the disc map
//! through `tags::repo::MapSource`, or the extracted tag tree), nothing
//! pinned, and its package.
//!
//! [`build`] in order:
//!
//! 1. [`Facts::read`]: the player's size, the floor's global material, the
//!    opaque stem and its default bitmaps, from the base's tags. The
//!    layout is resolved against the player.
//! 2. The base's scenario becomes the stage's ([`edit_scenario`]): its
//!    BSP descriptor names the stage's `sbsp` and `ltmp`, its starting
//!    locations are the layout's spawns and cameras, its placements,
//!    netgame flags and equipment, light fixtures and decorators are
//!    emptied, its base palettes nulled, its trigger volumes parked, every
//!    reference to the base's level renamed to the stage's, and every word
//!    holding a base datum outside a typed field (script nodes, the tag
//!    list) made a reference by key; its skies block emptied. Everything
//!    else (the ambience palettes, the scripts, the AI and the rest) is the base's.
//! 3. A layer over the base ([`StageSource`]) gives that scenario and two
//!    placeholders for the `sbsp` and `ltmp`, and hides the base's
//!    scenario and level. Closure from the globals, the scenario and the
//!    base's kept groups and prefixes (`h1convert::fresh::KEEP_GROUPS`,
//!    `KEEP_PREFIXES`), then `tags::repo::to_tag_set`. A missing tag, a
//!    lossy tag or a dangling reference is an error naming it.
//! 4. [`kit::install`] adds the stage's tags (the sky among them, named by
//!    the scenario's skies block) and writes the structure into the
//!    placeholders; the scenario's descriptor takes that BSP section.
//! 5. The sound gestalt is cut to the sounds kept
//!    (`h1convert::compact::gestalt`), the bitmaps' and gestalt's blocks
//!    kept to their own tags (`h1convert::scratch::scope_shares`), the
//!    scenario's blocks shared with no other tag, and the map built with
//!    the builder's tag-memory check on.
//!
//! The result is a pure function of the base's tags, the YAML, the project
//! and [`crate::stage::version()`]: the [`Report::key`] hashes them, and the same
//! inputs give the same bytes.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use map::build::{BspField, FixupKind, TagPayload, TagSet};
use map::scenario_layout::{self, BlockKind};
use map::{CacheType, FourCC};
use serde::{Deserialize, Serialize};
use tags::repo::{
    Body, BuildOptions, LooseTag, MapSource, Node, RawRes, Region, TagKey, TagRepo, TagSource,
    Tree, Value, closure, to_tag_set,
};

use crate::stage::kit::{self, Facts, Keys, LTMP, SBSP};
use crate::stage::layout::Layout;
use crate::stage::{Error, Result};

/// `scnr +0x210`: the BSP descriptors, `0x44` each: the region's file
/// offset, size and address (`+0`, `+4`, `+8`), the `sbsp` reference
/// (`+0x10`) and the `ltmp` reference (`+0x18`).
pub const BSPS: u32 = 0x210;
pub const BSP: u32 = 0x44;
const BSP_SBSP: u32 = 0x10;
const BSP_LTMP: u32 = 0x18;

/// `scnr +0x130`: BSP switch trigger volumes, which index the descriptors.
const BSP_SWITCHES: u32 = 0x130;

/// A starting location's position and facing.
const START_POSITION: usize = 0x00;
const START_FACING: usize = 0x0c;

/// The root blocks emptied: the base's placements
/// (`h1convert::apply::BASE_OBJECTS`), its netgame flags and equipment,
/// its light fixtures and decorators (`h1convert::convert::BASE_BLOCKS`),
/// all placed for its own level, its BSP switches, and its skies (the
/// stage's own is named once the structure is written, [`kit::install`]).
fn cleared() -> Vec<(u32, &'static str)> {
    let mut out: Vec<(u32, &'static str)> = [
        BlockKind::Scenery,
        BlockKind::Vehicles,
        BlockKind::Equipment,
        BlockKind::Weapons,
        BlockKind::Machines,
        BlockKind::SoundScenery,
        BlockKind::Crates,
        BlockKind::NetgameFlags,
        BlockKind::NetgameEquipment,
    ]
    .iter()
    .map(|&k| (scenario_layout::block(k).offset, k.name()))
    .collect();
    for (at, what) in h1convert::convert::BASE_BLOCKS {
        out.push((at as u32, what));
    }
    out.push((BSP_SWITCHES, "bsp_switches"));
    out.push((crate::stage::sky::SKIES, "skies"));
    out
}

// The project.

/// The stage's `map` item (`content/stage/map.toml`, a
/// [`workspace::MapSpec`]) as this build makes it
/// (docs/editor/tag-workspace.md, "Projects").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// The package directory and map name (`maps\<name>.map`).
    pub name: String,
    /// `fresh`: closure over the base, nothing pinned.
    pub mode: String,
    /// The disc map whose tags the stage is built over.
    pub base: String,
    /// The stage's scenario path.
    pub scenario: String,
    /// Content kits, in order.
    pub kits: Vec<String>,
}

fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 31
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The fields of a `map.toml` ([`workspace::MapSpec`], parsed by
/// `workspace::stage::project`) that the stage build checks.
pub struct Spec {
    pub fresh: bool,
    pub base: String,
    pub name: String,
    pub scenario: Option<String>,
    pub kits: Vec<String>,
}

impl Project {
    /// The map item this build can make: the stage kit alone, `fresh`.
    /// Anything else is an error naming it. Its edit files and import
    /// layers are the tree build's stack, not this build's.
    pub fn from_spec(spec: &Spec) -> Result<Project> {
        if !plain_name(&spec.name) {
            return Err(Error::Project(format!(
                "name {:?}: letters, digits, '_' and '-', at most 31",
                spec.name
            )));
        }
        if !plain_name(&spec.base) {
            return Err(Error::Project(format!(
                "base {:?} is not a map name",
                spec.base
            )));
        }
        if !spec.fresh {
            return Err(Error::Project(
                "mode \"onto\": the stage builds \"fresh\"".into(),
            ));
        }
        if spec.kits != ["stage"] {
            return Err(Error::Project(format!(
                "kits {:?}: this build runs the stage kit alone",
                spec.kits
            )));
        }
        let scenario = spec.scenario.clone().unwrap_or_default();
        let ok = !scenario.is_empty()
            && scenario.len() < 256
            && !scenario.contains('/')
            && scenario.rsplit('\\').next().is_some_and(|n| !n.is_empty());
        if !ok {
            return Err(Error::Project(format!(
                "scenario {scenario:?} is not a tag path"
            )));
        }
        Ok(Project {
            name: spec.name.clone(),
            mode: "fresh".into(),
            base: spec.base.clone(),
            scenario,
            kits: spec.kits.clone(),
        })
    }

}

// The base.

/// What a build takes from the base map besides its tags.
#[derive(Clone, Debug)]
pub struct Base {
    /// The map's file stem (`lockout`).
    pub map: String,
    pub cache_type: CacheType,
    pub scenario: TagKey,
    pub globals: TagKey,
    /// Every tag of the base map, in slot order.
    pub keys: Vec<TagKey>,
    /// The string-ID table to start from.
    pub strings: Vec<Option<String>>,
    /// The `0x800`-byte header: the fields the builder does not own.
    pub header: Vec<u8>,
    pub low_detail: Option<Vec<u8>>,
    /// Index header word `+0x14`.
    pub index_word_14: u32,
    /// Each tag's datum on the base map, for words the scenario holds
    /// outside its typed fields.
    pub datums: HashMap<u32, TagKey>,
}

fn key_of(t: &map::Tag) -> TagKey {
    TagKey::new(t.group, t.name.clone().unwrap_or_default())
}

impl Base {
    /// The base read from its map.
    pub fn from_map<R: Read + Seek>(src: &mut MapSource<'_, R>, stem: &str) -> Result<Base> {
        let fail =
            |what: &str, e: &dyn std::fmt::Display| Error::Base(format!("{stem}: {what}: {e}"));
        let (h, scenario, globals) = {
            let m = src.map();
            (
                m.header().clone(),
                m.scenario().map(|t| t.id),
                m.globals().map(|t| t.id),
            )
        };
        let header = src
            .map()
            .read_file_range(0, 0x800)
            .map_err(|e| fail("the header", &e))?;
        let word = src
            .map()
            .read_file_range(u64::from(h.index_offset) + 0x14, 4)
            .map_err(|e| fail("the index header", &e))?;
        let scenario = scenario
            .and_then(|id| src.key_of(id))
            .ok_or_else(|| Error::Base(format!("{stem}: no scenario")))?;
        let globals = globals
            .and_then(|id| src.key_of(id))
            .ok_or_else(|| Error::Base(format!("{stem}: no globals")))?;
        let low = src.low_detail()?;
        let keys = src.tags().iter().map(key_of).collect();
        let datums = src.tags().iter().map(|t| (t.id.0, key_of(t))).collect();
        Ok(Base {
            map: stem.to_owned(),
            cache_type: h.cache_type,
            scenario,
            globals,
            keys,
            strings: src.strings().to_vec(),
            header,
            low_detail: (!low.is_empty()).then_some(low),
            index_word_14: u32::from_le_bytes(word[..4].try_into().expect("4")),
            datums,
        })
    }

    /// The base read from a tag tree's manifest of `stem`, and the tree
    /// opened for that map's variants. Datums are the retail numbering of
    /// the manifest's slots (`salt` in `content/stage/kit.yaml`: every disc map numbers slot `n` `(salt + n) << 16 | n`, `map::build::TagSet::salt_base`).
    pub fn from_repo(root: &Path, stem: &str) -> Result<(Base, TagRepo)> {
        let top = TagRepo::open(root)?;
        let m = top.manifest(stem)?;
        let repo = TagRepo::for_map(root, stem)?;
        let scenario = m
            .scenario
            .clone()
            .ok_or_else(|| Error::Base(format!("{stem}: the manifest names no scenario")))?;
        let globals = m
            .globals
            .clone()
            .ok_or_else(|| Error::Base(format!("{stem}: the manifest names no globals")))?;
        let low_detail = match &m.low_detail {
            Some(h) => Some(repo.raw_by_hash(h)?),
            None => None,
        };
        let keys: Vec<TagKey> = m.tags.iter().map(|(k, _)| k.clone()).collect();
        let datums = keys
            .iter()
            .enumerate()
            .map(|(slot, k)| {
                let salt = u32::from(crate::stage::data::data().salt.wrapping_add(slot as u16));
                (salt << 16 | slot as u32, k.clone())
            })
            .collect();
        let base = Base {
            map: stem.to_owned(),
            cache_type: m.cache_type(),
            scenario,
            globals,
            keys,
            strings: repo.strings()?,
            header: repo.header(stem)?,
            low_detail,
            index_word_14: m.index_word_14,
            datums,
        };
        Ok((base, repo))
    }
}

// The scenario.

/// What [`edit_scenario`] did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ScenarioEdit {
    /// The base's structure BSPs and lightmaps, renamed to the stage's.
    pub base_level: Vec<String>,
    /// Root blocks emptied: (name, elements dropped).
    pub cleared: Vec<(String, u32)>,
    /// Base palette entries nulled.
    pub palette_entries: usize,
    /// Trigger volumes moved out of the level.
    pub trigger_volumes: usize,
    /// Starting locations written.
    pub starting_locations: usize,
    /// References renamed from the base's level to the stage's.
    pub renamed: usize,
    /// Words holding a base datum outside a typed field, made references.
    pub datum_words: usize,
    /// What the base's scenario lost on extraction that the stage does not
    /// empty anyway: bytes no array of the tag reaches (so neither does
    /// the engine). A lost block the stage keeps is an error instead.
    pub gaps: Vec<String>,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4"))
}

fn put(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The child node of the block field at `offset` of `node`, if any.
pub fn child(tree: &Tree, node: u32, offset: u32) -> Option<u32> {
    tree.nodes
        .get(node as usize)?
        .fields
        .iter()
        .find_map(|(o, v)| match v {
            Value::Block(c) if *o == offset => Some(*c),
            _ => None,
        })
}

/// Empties the block field at `offset` of `node`; returns its old count.
pub fn empty_block(tree: &mut Tree, node: u32, offset: u32) -> u32 {
    let n = &mut tree.nodes[node as usize];
    let at = offset as usize;
    let count = word(&n.bytes, at);
    n.bytes[at..at + 8].fill(0);
    n.fields.retain(|(o, _)| *o != offset);
    count
}

/// Points the block field at `offset` of `node` at a new child; returns
/// the child's node.
pub fn set_block(tree: &mut Tree, node: u32, offset: u32, child: Node) -> u32 {
    let id = tree.nodes.len() as u32;
    let count = child.count;
    tree.nodes.push(child);
    let n = &mut tree.nodes[node as usize];
    let at = offset as usize;
    put(&mut n.bytes, at, count);
    put(&mut n.bytes, at + 4, 0);
    n.fields.retain(|(o, _)| *o != offset);
    n.fields.push((offset, Value::Block(id)));
    n.fields.sort_by_key(|(o, _)| *o);
    id
}

/// The nodes reached from the root, each once.
fn reachable(tree: &Tree) -> Vec<u32> {
    let mut seen = vec![false; tree.nodes.len()];
    let mut out = Vec::new();
    let mut todo = vec![0u32];
    while let Some(n) = todo.pop() {
        if std::mem::replace(&mut seen[n as usize], true) {
            continue;
        }
        out.push(n);
        for (_, v) in &tree.nodes[n as usize].fields {
            if let Value::Block(c) | Value::Data(c) = v {
                todo.push(*c);
            }
        }
    }
    out.sort_unstable();
    out
}

/// The bytes a field takes.
fn span(v: &Value) -> u32 {
    match v {
        Value::Block(_) | Value::Data(_) => 8,
        Value::Bsp => 12,
        Value::Ref(_) | Value::String(_) | Value::Raw(_) => 4,
    }
}

/// The offset a loss line names (`block +0x… (…) left empty: …`).
pub fn lost_block(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("block +0x")?;
    let hex: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u32::from_str_radix(&hex, 16).ok()
}

/// The base's scenario made the stage's (module docs, step 2).
pub fn edit_scenario(
    mut tag: LooseTag,
    base: &Base,
    keys: &Keys,
    layout: &Layout,
) -> Result<(LooseTag, ScenarioEdit)> {
    let name = tag.key.to_string();
    let Body::Tree(tree) = &mut tag.body else {
        return Err(Error::Base(format!("{name} is an alias")));
    };
    let mut edit = ScenarioEdit::default();
    let cleared = cleared();
    let start = scenario_layout::block(BlockKind::StartingLocations);

    // What extraction lost: a block the stage empties or rewrites anyway,
    // or bytes nothing reaches; any other lost block is an error.
    for line in std::mem::take(&mut tree.losses) {
        match lost_block(&line) {
            Some(o) if cleared.iter().any(|(at, _)| *at == o) || o == start.offset => {}
            Some(o) => {
                return Err(Error::Base(format!(
                    "{name}: block +{o:#x} could not be taken from the base ({line})"
                )));
            }
            None => edit.gaps.push(line),
        }
    }

    // The BSP descriptor: one element, naming the stage's level.
    let desc = child(tree, 0, BSPS)
        .ok_or_else(|| Error::Base(format!("{name}: no BSP descriptor (+{BSPS:#x})")))?;
    let mut level: HashMap<TagKey, TagKey> = HashMap::new();
    for (_, v) in &tree.nodes[desc as usize].fields {
        if let Value::Ref(k) = v {
            if k.group == SBSP {
                level.insert(k.clone(), keys.sbsp.clone());
            } else if k.group == LTMP {
                level.insert(k.clone(), keys.ltmp.clone());
            }
        }
    }
    level.insert(base.scenario.clone(), keys.scenario.clone());
    let mut base_level: Vec<String> = level
        .keys()
        .filter(|k| **k != base.scenario)
        .map(ToString::to_string)
        .collect();
    base_level.sort();
    edit.base_level = base_level;
    {
        let n = &mut tree.nodes[desc as usize];
        if n.count == 0 || n.size != BSP {
            return Err(Error::Base(format!(
                "{name}: {} BSP descriptors of {:#x} bytes",
                n.count, n.size
            )));
        }
        n.count = 1;
        n.bytes.truncate(BSP as usize);
        // The builder's section fixups take the place of `Value::Bsp`
        // once the structure is written (`attach_bsp`).
        n.fields
            .retain(|(o, v)| *o < BSP && !matches!(v, Value::Bsp));
        for (at, group) in [(BSP_SBSP, SBSP), (BSP_LTMP, LTMP)] {
            let named = n
                .fields
                .iter()
                .any(|(o, v)| *o == at + 4 && matches!(v, Value::Ref(k) if k.group == group));
            if !named || word(&n.bytes, at as usize) != group.as_u32() {
                return Err(Error::Base(format!(
                    "{name}: the BSP descriptor has no {group} reference at +{at:#x}"
                )));
            }
        }
    }
    put(&mut tree.nodes[0].bytes, BSPS as usize, 1);

    // The base's placements and what it placed for its own level.
    for (at, what) in &cleared {
        let dropped = empty_block(tree, 0, *at);
        edit.cleared.push(((*what).to_owned(), dropped));
    }
    for &(at, size) in &h1convert::fresh::BASE_PALETTES {
        let Some(c) = child(tree, 0, at) else {
            continue;
        };
        let n = &mut tree.nodes[c as usize];
        for e in 0..n.count as usize {
            let at = (e * size) as u32;
            let before = n.fields.len();
            n.fields
                .retain(|(o, v)| !(*o == at + 4 && matches!(v, Value::Ref(_))));
            if n.fields.len() != before {
                n.bytes[e * size..e * size + 8].fill(0xff);
                edit.palette_entries += 1;
            }
        }
    }
    if let Some(c) = child(tree, 0, h1convert::convert::TRIGGER_VOLUMES as u32) {
        let n = &mut tree.nodes[c as usize];
        let size = h1convert::convert::TRIGGER_VOLUME;
        for e in 0..n.count as usize {
            for (k, v) in h1convert::convert::PARKED.iter().enumerate() {
                let at = e * size + 0x24 + 4 * k;
                n.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            edit.trigger_volumes += 1;
        }
    }

    // Starting locations: each bay's spawn, then its cameras, each a copy
    // of the base's first with its position and facing.
    let first = child(tree, 0, start.offset)
        .filter(|&c| tree.nodes[c as usize].count > 0)
        .ok_or_else(|| Error::Base(format!("{name}: no starting location to copy")))?;
    let (template, template_fields, align) = {
        let n = &tree.nodes[first as usize];
        let size = start.stride;
        let fields: Vec<(u32, Value)> = n
            .fields
            .iter()
            .filter(|(o, _)| *o < size)
            .cloned()
            .collect();
        if fields
            .iter()
            .any(|(_, v)| matches!(v, Value::Block(_) | Value::Data(_)))
        {
            return Err(Error::Base(format!(
                "{name}: the first starting location holds a block"
            )));
        }
        (n.bytes[..size as usize].to_vec(), fields, n.align)
    };
    let mut places: Vec<([f32; 3], f32)> = Vec::new();
    for bay in &layout.bays {
        places.push((bay.spawn.point, bay.spawn.facing));
        for c in &bay.cameras {
            places.push((c.eye, c.yaw));
        }
    }
    let mut node = Node {
        bytes: Vec::with_capacity(places.len() * template.len()),
        count: places.len() as u32,
        size: start.stride,
        align,
        fields: Vec::new(),
    };
    for (k, (p, facing)) in places.iter().enumerate() {
        let mut e = template.clone();
        for (i, v) in p.iter().enumerate() {
            e[START_POSITION + 4 * i..START_POSITION + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        e[START_FACING..START_FACING + 4].copy_from_slice(&facing.to_le_bytes());
        node.bytes.extend_from_slice(&e);
        let at = k as u32 * start.stride;
        node.fields
            .extend(template_fields.iter().map(|(o, v)| (o + at, v.clone())));
    }
    edit.starting_locations = places.len();
    set_block(tree, 0, start.offset, node);

    // References to the base's level and scenario, by key and by bare
    // datum word.
    let live = reachable(tree);
    for &n in &live {
        let node = &mut tree.nodes[n as usize];
        for (_, v) in &mut node.fields {
            if let Value::Ref(k) = v
                && let Some(to) = level.get(&*k)
            {
                *k = to.clone();
                edit.renamed += 1;
            }
        }
        let mut taken: Vec<(u32, u32)> =
            node.fields.iter().map(|(o, v)| (*o, o + span(v))).collect();
        taken.sort_unstable();
        let mut found = Vec::new();
        for at in (0..node.bytes.len() as u32 / 4 * 4).step_by(4) {
            if taken.iter().any(|&(a, b)| at + 4 > a && at < b) {
                continue;
            }
            let w = word(&node.bytes, at as usize);
            if w == u32::MAX {
                continue;
            }
            if let Some(k) = base.datums.get(&w) {
                let k = level.get(k).unwrap_or(k).clone();
                found.push((at, Value::Ref(k)));
            }
        }
        for (at, _) in &found {
            put(&mut node.bytes, *at as usize, 0);
        }
        edit.datum_words += found.len();
        node.fields.extend(found);
        node.fields.sort_by_key(|(o, _)| *o);
    }
    tag.key = keys.scenario.clone();
    Ok((tag, edit))
}

// The build.

/// A one-node placeholder for a tag [`kit::install`] writes.
fn stub(key: TagKey) -> LooseTag {
    LooseTag {
        key,
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![0; 4],
                count: 1,
                size: 4,
                align: 4,
                fields: Vec::new(),
            }],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

/// The base's tags with the stage's scenario and placeholders over them
/// and the base's scenario and level hidden.
pub struct StageSource<'a> {
    pub base: &'a mut dyn TagSource,
    pub own: HashMap<TagKey, LooseTag>,
    pub hidden: HashSet<TagKey>,
}

impl TagSource for StageSource<'_> {
    fn keys(&self) -> Vec<TagKey> {
        let mut k: Vec<TagKey> = self
            .base
            .keys()
            .into_iter()
            .filter(|k| !self.hidden.contains(k) && !self.own.contains_key(k))
            .collect();
        k.extend(self.own.keys().cloned());
        k.sort();
        k
    }

    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        if let Some(t) = self.own.get(key) {
            return Ok(Some(t.clone()));
        }
        if self.hidden.contains(key) {
            return Ok(None);
        }
        self.base.read(key)
    }

    fn raw(&mut self, raw: &RawRes) -> tags::repo::Result<Vec<u8>> {
        self.base.raw(raw)
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        self.base.groups()
    }
}

/// The built stage.
#[derive(Clone, Debug)]
pub struct Built {
    pub bytes: Vec<u8>,
    pub layout: Layout,
    pub report: Report,
}

/// The build report (docs/editor/content-stage.md, "The map").
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// SHA-256 over the stage's version, the YAML, the project and the
    /// base's name and header: equal keys, equal bytes.
    pub key: String,
    pub version: u32,
    pub name: String,
    pub base: String,
    pub scenario: String,
    pub layout: Layout,
    /// The stage's own tags.
    pub own: Vec<String>,
    /// Tags in the map, and of them the base's.
    pub tags: usize,
    pub from_base: usize,
    /// Local raw resources copied (count, bytes); shared-cache pointers.
    pub raw: (usize, u64),
    pub external: usize,
    pub bsps: usize,
    pub triangles: usize,
    pub vertices: usize,
    pub collision_surfaces: usize,
    /// The lightmap page's side in texels, and its texels to the unit on
    /// a day face.
    pub lightmap_side: u32,
    pub lightmap_density: f32,
    pub file_size: u32,
    /// Tag memory used and the bound (`map::writer::max_metadata_size`).
    pub memory_size: u32,
    pub memory_max: u32,
    pub scenario_edit: ScenarioEdit,
    /// The sound gestalt cut to the sounds kept, or why it stays whole.
    pub gestalt: Option<String>,
    pub gestalt_whole: Option<String>,
}

/// The build key (see [`Report::key`]).
pub fn build_key(base: &Base, project: &Project) -> String {
    let mut text = Vec::new();
    text.extend_from_slice(&crate::stage::version().to_le_bytes());
    for part in [
        crate::stage::SOURCE.as_bytes(),
        toml::to_string(project).unwrap_or_default().as_bytes(),
    ] {
        text.extend_from_slice(&(part.len() as u64).to_le_bytes());
        text.extend_from_slice(part);
    }
    text.extend_from_slice(base.map.as_bytes());
    text.push(0);
    text.extend_from_slice(&base.header);
    tags::repo::hex(&tags::repo::sha256(&text))
}

fn list(what: &str, items: Vec<String>) -> Error {
    let n = items.len();
    let shown: Vec<String> = items.into_iter().take(8).collect();
    Error::Build(format!("{n} {what}: {}", shown.join("; ")))
}

/// Points the scenario's BSP descriptor at built section `section`.
fn attach_bsp(set: &mut TagSet, section: usize) -> Result<()> {
    let scenario = set.scenario;
    let Some(TagPayload::Built(root)) = set.tag_mut(scenario).map(|t| &mut t.payload) else {
        return Err(Error::Build("the scenario is not built".into()));
    };
    let desc = root
        .fixups
        .iter_mut()
        .find_map(|f| match &mut f.kind {
            FixupKind::Block { count, child } if f.offset == BSPS && *count == 1 => Some(child),
            _ => None,
        })
        .ok_or_else(|| Error::Build("the scenario's BSP descriptor is not one element".into()))?;
    desc.share = None;
    for (k, field) in [
        (0, BspField::FileOffset),
        (4, BspField::Size),
        (8, BspField::Address),
    ] {
        desc.fixup(k, FixupKind::Bsp { section, field });
    }
    Ok(())
}

/// No block of the scenario is placed once for it and another tag: the
/// scenario editor reads a block inside another tag as foreign
/// (`scenario_edit::Scenario::read`).
fn unshare(b: &mut map::build::Block) {
    b.share = None;
    for f in &mut b.fixups {
        if let FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) =
            &mut f.kind
        {
            unshare(child);
        }
    }
}

/// Builds the stage (module docs) over `src`, the base's tags.
pub fn build(src: &mut dyn TagSource, base: &Base, project: &Project) -> Result<Built> {
    let facts = Facts::read(src, &base.globals)?;
    let layout = Layout::builtin(facts.player)?;
    let keys = Keys::new(&project.scenario);
    keys_free(base, &keys)?;
    let scnr = src
        .read(&base.scenario)?
        .ok_or_else(|| Error::Base(format!("{}: no {}", base.map, base.scenario)))?;
    let (scnr, scenario_edit) = edit_scenario(scnr, base, &keys, &layout)?;
    let key = build_key(base, project);
    build_over(
        src,
        base,
        project,
        Over {
            layout,
            facts,
            keys,
            scenario: scnr,
            scenario_edit,
            roots: Vec::new(),
            key,
        },
    )
}

/// Refuses a base that already holds one of the keys the build adds.
pub fn keys_free(base: &Base, keys: &Keys) -> Result<()> {
    let held: HashSet<&TagKey> = base.keys.iter().collect();
    if let Some(k) = keys.all().iter().find(|k| held.contains(k)) {
        return Err(Error::Base(format!(
            "{}: the base already holds {k}; give the map another scenario path",
            base.map
        )));
    }
    Ok(())
}

/// What [`build_over`] lays out: the layout and the facts it was resolved
/// with, the keys, the scenario already made the map's ([`edit_scenario`],
/// and whatever a caller wrote into it after), more closure roots, and the
/// build key the report carries. The stage's own [`build`] makes one; a new
/// map (`starter-map`, docs/editor/editor.md, "New maps") makes another
/// from its template.
pub struct Over {
    pub layout: Layout,
    pub facts: Facts,
    pub keys: Keys,
    pub scenario: LooseTag,
    pub scenario_edit: ScenarioEdit,
    /// Tags the scenario names nowhere the closure walks (a weapon a later
    /// placement edit names, an AI character), each kept with what it
    /// reaches. A root the base does not hold is an error naming it.
    pub roots: Vec<TagKey>,
    pub key: String,
}

/// Steps 3 to 5 of the module docs over a prepared scenario ([`Over`]).
pub fn build_over(
    src: &mut dyn TagSource,
    base: &Base,
    project: &Project,
    over: Over,
) -> Result<Built> {
    let Over {
        layout,
        facts,
        keys,
        scenario: scnr,
        scenario_edit,
        roots: extra_roots,
        key,
    } = over;
    let held: HashSet<&TagKey> = base.keys.iter().collect();
    if let Some(k) = extra_roots.iter().find(|k| !held.contains(k)) {
        return Err(Error::Base(format!("{}: the base has no {k}", base.map)));
    }

    let mut hidden: HashSet<TagKey> = HashSet::from([base.scenario.clone()]);
    for k in &base.keys {
        if k.group == SBSP || k.group == LTMP {
            hidden.insert(k.clone());
        }
    }
    let own = HashMap::from([
        (keys.scenario.clone(), scnr),
        (keys.sbsp.clone(), stub(keys.sbsp.clone())),
        (keys.ltmp.clone(), stub(keys.ltmp.clone())),
    ]);
    let mut stage = StageSource {
        base: src,
        own,
        hidden,
    };

    // Roots: the globals, the scenario, the base's kept groups and
    // prefixes, and what the stage's shaders bind.
    let built_here = [kit::SCNR, SBSP, LTMP];
    let mut roots = vec![base.globals.clone(), keys.scenario.clone()];
    for k in &base.keys {
        if built_here.contains(&k.group) {
            continue;
        }
        let group = h1convert::fresh::KEEP_GROUPS
            .iter()
            .any(|g| k.group == FourCC(**g));
        let prefix = h1convert::fresh::KEEP_PREFIXES
            .iter()
            .any(|p| k.path.starts_with(p));
        if group || prefix {
            roots.push(k.clone());
        }
    }
    roots.push(facts.stem.clone());
    roots.extend(facts.fixed.iter().map(|(_, k, _)| k.clone()));
    roots.extend(extra_roots);
    let mut seen = HashSet::new();
    roots.retain(|k| seen.insert(k.clone()));

    let c = closure(&mut stage, &roots)?;
    if !c.missing.is_empty() {
        let items = c
            .missing
            .iter()
            .map(|(to, from)| format!("{to} (named by {from})"))
            .collect();
        return Err(list("tags the base does not hold", items));
    }
    let lossy: Vec<String> = c
        .tags
        .iter()
        .filter_map(|t| {
            let tree = t.tree()?;
            (!tree.losses.is_empty()).then(|| format!("{} ({})", t.key, tree.losses.join("; ")))
        })
        .collect();
    if !lossy.is_empty() {
        return Err(list("tags not taken exactly from the base", lossy));
    }
    for k in [&keys.scenario, &keys.sbsp, &keys.ltmp] {
        if c.get(k).is_none() {
            return Err(Error::Build(format!("the closure does not reach {k}")));
        }
    }
    let opts = BuildOptions {
        cache_type: base.cache_type,
        name: project.name.clone(),
        scenario: keys.scenario.clone(),
        globals: base.globals.clone(),
        strings: base.strings.clone(),
        header_template: Some(base.header.clone()),
        low_detail: base.low_detail.clone(),
        index_word_14: base.index_word_14,
    };
    let (mut set, built) = to_tag_set(&mut stage, &c.tags, &opts)?;
    if !built.dangling.is_empty() {
        let items = built
            .dangling
            .iter()
            .map(|(from, to)| format!("{from} -> {to}"))
            .collect();
        return Err(list("references to tags not in the map", items));
    }
    if !built.orphan_bsps.is_empty() || built.bsps != 0 {
        return Err(Error::Build(format!(
            "the base's level reached the set: {} BSP regions, orphans {:?}",
            built.bsps, built.orphan_bsps
        )));
    }

    let installed = kit::install(&mut set, &layout, &facts, &keys)?;
    attach_bsp(&mut set, installed.section)?;
    if set.bsps.len() != 1 {
        return Err(Error::Build(format!(
            "{} BSP regions, not one",
            set.bsps.len()
        )));
    }
    let scenario = set.scenario;
    if let Some(TagPayload::Built(root)) = set.tag_mut(scenario).map(|t| &mut t.payload) {
        unshare(root);
    }
    let (gestalt, gestalt_whole) = match h1convert::compact::gestalt(&mut set) {
        Ok(c) => (
            Some(format!(
                "{} sounds; {} to {} chunks; {} to {} bytes",
                c.sounds, c.chunks.0, c.chunks.1, c.bytes.0, c.bytes.1
            )),
            None,
        ),
        Err(e) => (None, Some(e.to_string())),
    };
    h1convert::scratch::scope_shares(&mut set);
    set.scenario_path = project.scenario.clone();

    let (bytes, out) = set
        .build()
        .map_err(|e| Error::Build(format!("laying out the map: {e}")))?;
    let memory_max = map::writer::max_metadata_size(base.cache_type)
        .map_err(|e| Error::Build(format!("the tag memory bound: {e}")))?;
    let tags = set.slots.iter().filter(|s| s.tag.is_some()).count();
    let own = keys.all();
    let report = Report {
        key,
        version: crate::stage::version(),
        name: project.name.clone(),
        base: base.map.clone(),
        scenario: project.scenario.clone(),
        layout: layout.clone(),
        own: own.iter().map(ToString::to_string).collect(),
        tags,
        from_base: c.tags.len().saturating_sub(3),
        raw: built.raw,
        external: built.external,
        bsps: set.bsps.len(),
        triangles: installed.triangles,
        vertices: installed.vertices,
        collision_surfaces: installed.surfaces,
        lightmap_side: installed.lightmap_side,
        lightmap_density: installed.lightmap_density,
        file_size: out.file_size,
        memory_size: out.memory_size,
        memory_max,
        scenario_edit,
        gestalt,
        gestalt_whole,
    };
    Ok(Built {
        bytes,
        layout,
        report,
    })
}

// The package.

/// The Git checkout `path` is in, if any.
pub fn git_checkout(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    abs.ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The package's manifest: a derived map whose one layer is the map, in
/// the map list as the base's entry gives it.
pub fn manifest(built: &Built, project: &Project, base: &Base) -> Result<content::Manifest> {
    let text = format!(
        "[package]\nformat = 1\nname = \"Content stage\"\n\
         description = \"Studio's test map: a floor marked in world units, scale and movement references, fixed cameras.\"\n\
         [base]\nmap = \"{name}\"\nderived = true\n\
         [[patch]]\nfile = \"{name}.map\"\nsha256 = \"{sha}\"\n\
         [list]\nbase = \"{base}\"\n",
        name = project.name,
        sha = content::Sha256::of(&built.bytes),
        base = base.map,
    );
    content::Manifest::from_toml(&text).map_err(|e| Error::Build(format!("the manifest: {e}")))
}

/// Writes the stage's package, `<data>/maps/<name>/`: the manifest, the
/// map and `report.json`, replacing an older stage. A data directory in a
/// Git checkout is refused: nothing the build writes is committed.
pub fn package(built: &Built, project: &Project, base: &Base, data_root: &Path) -> Result<PathBuf> {
    if let Some(repo) = git_checkout(data_root) {
        return Err(Error::Project(format!(
            "{} is in the Git checkout {}; the stage is written to the user's data directory",
            data_root.display(),
            repo.display()
        )));
    }
    let manifest = manifest(built, project, base)?;
    let report = serde_json::to_vec_pretty(&built.report)
        .map_err(|e| Error::Build(format!("the report: {e}")))?;
    let file = format!("{}.map", project.name);
    content::store::write_package(
        &content::store::maps_dir(data_root),
        &project.name,
        &manifest,
        &[
            (file.as_str(), built.bytes.as_slice()),
            ("report.json", report.as_slice()),
        ],
        true,
    )
    .map_err(|e| Error::Build(format!("writing the package: {e}")))
}
