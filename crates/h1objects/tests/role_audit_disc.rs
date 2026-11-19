//! The role audit: every field of a converted Halo 1 sandbox whose value
//! came from its **role** (the base map's retail Halo 2 tags of the same
//! kind) rather than from Halo 1, each classified. Reads `$H1_ISO` and
//! `$H2_DISC`; prints names, paths and values only. Nothing is written
//! unless `ROLE_AUDIT_OUT` names a file for the markdown report.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test role_audit_disc -- --ignored --nocapture
//! H1_MAPS=all ...   # every multiplayer map on its base (default bloodgulch)
//! ```
//!
//! What counts as a leak, per converted tag (`docs/halo1/role-audit.md`):
//!
//! - **value**: a field its builder wrote from a role
//!   (`h1objects::provenance`: role words, a role's named values, retail
//!   values kept in the source) that still holds that value in the built
//!   tag, so no Halo 1 value replaced it;
//! - **copied**: a block or struct taken whole from the Halo 2
//!   counterpart (`rebuild::counterpart_fields`) and still the
//!   counterpart's;
//! - **a reference** to a tag of the base map (not one the conversion
//!   built), by the referenced tag's group: a retail Halo 2 tag in a slot.
//!
//! Each leak is matched against [`ALLOWED`] (Halo 1 has nothing it could
//! come from: allowed, with the reason) and [`KNOWN`] (Halo 1 has an
//! equivalent the converter does not translate yet: a bug, owned by a lane
//! or with what the fix needs). Anything in neither fails the test, so a
//! new leak has to be classified; the failure prints it as a row to paste.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::halo1::{Halo1Objects, ObjectTags, prepare_halo1};
use h1map::H1Map;
use h1objects::Converted;
use h1objects::base::Base;
use h1objects::convert::{local_index, looping, sound};
use h1objects::provenance::RoleField;
use h1objects::sandbox::Objects;
use map::build::{Block, FixupKind};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Kind, Record, StructDef, Target, Val};

/// Leaks allowed: Halo 1 has no equivalent. (group, path, what, why).
/// `*` matches anything and a path ending in `*` matches by prefix;
/// element indices are `[]`. `what` is `value`, `copied`, `none-word`,
/// or the referenced tag's group. Checked before [`KNOWN`].
const ALLOWED: &[Rule] = &[
    (
        "mode",
        "materials[].shader",
        "emblem",
        "(a) the player biped's emblem material: Halo 2's, which draws the player's own emblem and colours (h1models::emblem)",
    ),
    (
        "*",
        "*",
        "none-word",
        "(a) half of a reference the definition does not name, holding none (a group word or -1): no value to take from anywhere",
    ),
    (
        "hlmt",
        "block_60*",
        "*",
        "(a) damage info (vitality, shields, their shaders and effects): Halo 2's damage model wins (campaign.md decision 2; health.md)",
    ),
    (
        "hlmt",
        "shad_ref_*",
        "shad",
        "(a) the active camouflage shader: Halo 1 draws camouflage from its globals, not per model",
    ),
    (
        "prt3",
        "*",
        "value",
        "(a) the static particle encoding (input mask, vertex shader, quad, UV window): Halo 2's particle shader inputs",
    ),
    (
        "prt3",
        "*",
        "copied",
        "(a) the unit colour, alpha, size and rotation constants: Halo 1's values go on the emitter, which multiplies them (particles::particle)",
    ),
    (
        "prt3",
        "*",
        "stem",
        "(a) the particle shader template: Halo 2's shader system, picked by Halo 1's blend mode (particles::particle_stem)",
    ),
    (
        "shad",
        "*",
        "stem",
        "(a) Halo 2 shader templates: Halo 2's shader system; Halo 1's bitmaps go in their slots (h1convert::textures)",
    ),
    (
        "shad",
        "*",
        "bitm",
        "(a) Halo 2's default bitmaps in the template slots Halo 1's shader has nothing for (rasterizer and default_bitmaps)",
    ),
    (
        "nhdt",
        "*",
        "shad",
        "(a) Halo 2's HUD shaders draw the widgets: Halo 1's HUD has no shaders (hud.md)",
    ),
    (
        "nhdt",
        "bitmap_widgets[].bitmap",
        "bitm",
        "(a) the Halo 2 HUD of the HUD pair (masterchief_h1_health), Halo 2's by design (hud.md)",
    ),
    (
        "bipd",
        "unit.ref_16c",
        "jpt!",
        "(a) boarding melee (V: masterchief_boarding_melee): Halo 1 has no boarding",
    ),
    (
        "bipd",
        "unit.ref_174",
        "jpt!",
        "(a) boarding melee response (V: masterchief_boarding_melee_response): Halo 1 has no boarding",
    ),
    (
        "proj",
        "unknown_178",
        "value",
        "(a) Halo 1's own value: arsenal::PROJECTILE_WORDS writes it after the role (water damage range, to), equal to the role's",
    ),
    (
        "proj",
        "unknown_e4",
        "value",
        "(a) Halo 1's detonation noise in the low half (arsenal::projectile), equal to the role's word",
    ),
    (
        "deca",
        "unknown_18",
        "value",
        "(a) the decal scale word: no Halo 1 decal field (rebuild::DecalRole)",
    ),
    (
        "deca",
        "max_overlapping",
        "value",
        "(a) how many decals may overlap: no Halo 1 decal field",
    ),
];

/// Leaks with a Halo 1 equivalent, or in spans not yet identified: each
/// with its owner or what the fix needs. (group, path, what, status).
/// First match wins, so the object header comes before the groups.
///
/// `unidentified` marks a Halo 2 unknown span: whether Halo 1 has its
/// equivalent cannot be told until the span is named (by pairs, as the
/// damage bounds were; docs/formats/tag-definitions.md). The suspects are
/// listed; each is a bug until shown otherwise.
const KNOWN: &[Rule] = &[
    // Owned by other lanes now.
    (
        "weap",
        "barrels[].block_e4[]*",
        "*",
        "owned by h1-weap-snd: firing effects, their sounds and trigger damage (Halo 1 trigger firing effects)",
    ),
    (
        "weap",
        "*",
        "snd!",
        "owned by h1-weap-snd: pickup (ref_264) and zoom (ref_26c, ref_274) sounds",
    ),
    (
        "weap",
        "ref_140",
        "effe",
        "owned by h1-weap-snd: the ready effect (V: needler ready); Halo 1's weap +0x33c",
    ),
    (
        "eqip",
        "*",
        "snd!",
        "owned by h1-weap-snd: the item pickup sound (Halo 1's eqip +0x310)",
    ),
    (
        "bipd",
        "unit.obje.functions*",
        "*",
        "owned by h1-bipd-colour: functions come with the change colours",
    ),
    (
        "bipd",
        "unit.obje.attachments*",
        "*",
        "bug, after h1-bipd-colour: Halo 1's attachments (Converter::halo1_attachments, as projectiles) name the object's functions, which are that lane's",
    ),
    (
        "vehi",
        "block_1c8[]*",
        "value",
        "owned by h1-veh-phys: seat words and flags (the driver seat bit)",
    ),
    (
        "vehi",
        "block_2e8[]*",
        "value",
        "owned by h1-veh-phys: hover points",
    ),
    (
        "vehi",
        "block_2f0[]*",
        "value",
        "owned by h1-veh-phys: friction points",
    ),
    (
        "vehi",
        "block_268[]*",
        "value",
        "owned by h1-veh-phys: engine gears (Halo 1's engine speeds and acceleration)",
    ),
    (
        "vehi",
        "block_2f8[]*",
        "value",
        "owned by h1-veh-phys: the physics element (Halo 1's phys mass, density, gravity)",
    ),
    // The object header of every group.
    (
        "*",
        "object.*",
        "value",
        "unidentified: object header words; +0x6c..+0x8c are the same nine floats on every object of every group (likely (a): Halo 1's obje has one acceleration scale)",
    ),
    (
        "*",
        "item.obje.*",
        "value",
        "unidentified: object header words; +0x6c..+0x8c are the same nine floats on every object of every group (likely (a): Halo 1's obje has one acceleration scale)",
    ),
    (
        "*",
        "unit.obje.*",
        "value",
        "unidentified: object header words; +0x6c..+0x8c as every object's, +0x14..+0x1c the biped's",
    ),
    (
        "*",
        "item.obje.foot_ref_54",
        "foot",
        "unidentified: kept where Halo 1's root names no material effects (Converter::h1_foot takes Halo 1's when it does)",
    ),
    (
        "*",
        "object.foot_ref_54",
        "foot",
        "unidentified: kept where Halo 1's root names no material effects (Converter::h1_foot takes Halo 1's when it does)",
    ),
    (
        "vehi",
        "*",
        "value",
        "owned by h1-veh-phys: the vehicle type's root and unit words (driving, steering, speeds; the Ghost's zeroed words)",
    ),
    // Bugs not fixed here.
    (
        "bipd",
        "unit.block_e8[].ref_0",
        "trak",
        "bug: Halo 1's unit camera track (biped_follow_camera here): convert it as the seats' (Converter::camera_track) once the unit's block is read",
    ),
    (
        "bipd",
        "unit.effe_ref_c4",
        "effe",
        "bug: the flashlight toggle effect (V: masterchief flashlight); Halo 1's unit integrated light toggle effect, not read yet",
    ),
    (
        "bipd",
        "ref_25c",
        "effe",
        "unidentified: a biped area-of-effect blood effect (V: blood_aoe_human_masterchief); no Halo 1 suspect: likely (a)",
    ),
    (
        "eqip",
        "item.ref_10c",
        "jpt!",
        "bug: the grenade's explosion damage when shot (V: *_equipment_explosion); Halo 1's item detonation effect carries it: read the item's detonation fields",
    ),
    (
        "eqip",
        "item.ref_124",
        "effe",
        "bug: the grenade's detonation effect when shot (V: airborne_detonation, equipment_detonation); Halo 1's item detonation effect: read it",
    ),
    (
        "weap",
        "ref_1cc",
        "jpt!",
        "unidentified: a further Halo 2 melee slot (strike_melee); Halo 1 has one melee damage, in ref_1bc",
    ),
    (
        "weap",
        "ref_1d4",
        "jpt!",
        "unidentified: a further Halo 2 melee slot (strike_melee_response); Halo 1 has one melee response, in ref_1c4",
    ),
    (
        "weap",
        "ref_1dc",
        "jpt!",
        "unidentified: a further Halo 2 melee slot (smash/strike_melee); Halo 1 has one melee damage, in ref_1bc",
    ),
    (
        "weap",
        "ref_1e4",
        "jpt!",
        "unidentified: a further Halo 2 melee slot (smash_melee and its response); Halo 1 has one melee damage, in ref_1bc",
    ),
    (
        "weap",
        "ref_1ec",
        "jpt!",
        "unidentified: a further Halo 2 melee slot (smash_melee_response); Halo 1 has one melee response, in ref_1c4",
    ),
    (
        "proj",
        "ref_140",
        "effe",
        "unidentified: the rocket's rocket_detonation only; Halo 1's one detonation effect is in ref_f0 and ref_f8",
    ),
    (
        "jpt!",
        "*",
        "value",
        "unidentified: jpt! +0xa0 (1.0 on 16); Halo 2's jpt! is unnamed past the damage bounds",
    ),
    (
        "weap",
        "*",
        "value",
        "unidentified: weapon words past WEAPON_WORDS, BARREL_WORDS and the magazine; suspects Halo 1's ready time, zoom levels and magnification, autoaim, movement penalty, weapon and trigger flags",
    ),
    (
        "proj",
        "*",
        "value",
        "unidentified: projectile words past PROJECTILE_WORDS; suspects Halo 1's projectile flags",
    ),
    (
        "eqip",
        "*",
        "value",
        "unidentified: equipment and item words; suspects Halo 1's item flags",
    ),
    (
        "bipd",
        "*",
        "value",
        "unidentified: biped and unit words past BIPED_WORDS; suspects Halo 1's unit speeds, jump velocity, seat acceleration, unit and biped flags",
    ),
    (
        "scen",
        "*",
        "value",
        "unidentified: scenery words past the object header (Halo 1's scen has nothing past its obje: likely (a))",
    ),
    (
        "ligh",
        "*",
        "value",
        "unidentified: light words and brightness; suspects Halo 1's light intensity and its falloff function",
    ),
    (
        "effe",
        "events[].particle_systems[]*",
        "value",
        "unidentified: particle-system and emitter words (camera mode, LOD, fade); suspects Halo 1 particle fade times",
    ),
    (
        "deca",
        "layer",
        "value",
        "bug: Halo 1's decal layer (its own enumeration); Halo 2's layer values need naming to map it",
    ),
];

/// One field still holding a role's value or a retail reference.
#[derive(Clone, Debug)]
struct Leak {
    map: String,
    group: String,
    tag: String,
    /// Path with element indices as `[]`.
    path: String,
    /// `value`, `copied`, or the referenced tag's group.
    what: String,
    /// The value, or the referenced tag's name.
    shown: String,
}

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap_or_else(|| panic!("{want}.map on the Halo 2 disc"));
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h1_map(name: &str) -> Option<(H1Map, content::Sha256)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name)?.clone();
    let file = iso.read(&e).unwrap();
    let sha = content::Sha256::of(&file);
    Some((H1Map::from_reader(Cursor::new(file)).unwrap(), sha))
}

/// The converted tags and their role values, as the sandbox hands them
/// over.
#[derive(Default)]
struct Snapshot {
    tags: Vec<(String, Converted)>,
    roles: Vec<(usize, Vec<RoleField>)>,
}

/// The sandbox's objects, keeping a copy of the tree when it is taken.
struct Audit<'a> {
    inner: Objects<'a>,
    snap: Snapshot,
}

impl Halo1Objects for Audit<'_> {
    fn object(&mut self, group: [u8; 4], name: &str) -> Result<usize, String> {
        self.inner.object(group, name)
    }

    fn first_person_arms(&mut self, id: h1map::TagId) -> Result<usize, String> {
        self.inner.first_person_arms(id)
    }

    fn take(&mut self) -> ObjectTags {
        let t = self.inner.tree();
        let first = self.snap.tags.len();
        self.snap.tags.extend(t.tags.iter().cloned());
        self.snap
            .roles
            .extend(t.role_fields.iter().map(|(k, f)| (first + k, f.clone())));
        self.inner.take()
    }
}

/// `path` with every element index (`[3]`, `[*]`) as `[]`.
fn normal(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut skip = false;
    for c in path.chars() {
        match c {
            '[' => {
                out.push_str("[]");
                skip = true;
            }
            ']' => skip = false,
            _ if skip => {}
            _ => out.push(c),
        }
    }
    out
}

fn matches(pattern: &str, s: &str) -> bool {
    pattern == "*" || pattern == s || pattern.strip_suffix('*').is_some_and(|p| s.starts_with(p))
}

type Rule = (&'static str, &'static str, &'static str, &'static str);

/// The rule of `rules` that covers `l`.
fn rule(rules: &[Rule], l: &Leak) -> Option<&'static str> {
    rules
        .iter()
        .find(|(g, p, w, _)| matches(g, &l.group) && matches(p, &l.path) && matches(w, &l.what))
        .map(|r| r.3)
}

/// Every value at `path` in `r`; `[*]` takes every element.
fn values_at<'r>(r: &'r Record, path: &str) -> Vec<&'r Val> {
    let (head, rest) = match path.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };
    let (name, index) = match head.split_once('[') {
        Some((n, i)) => (n, Some(i.trim_end_matches(']'))),
        None => (head, None),
    };
    let Some(v) = r.get(name) else {
        return Vec::new();
    };
    match (index, rest, v) {
        (None, None, v) => vec![v],
        (None, Some(rest), Val::Struct(x)) => values_at(x, rest),
        (Some("*"), Some(rest), Val::Block(e)) => {
            e.iter().flat_map(|x| values_at(x, rest)).collect()
        }
        (Some(i), Some(rest), Val::Block(e)) => i
            .parse::<usize>()
            .ok()
            .and_then(|i| e.get(i))
            .map_or_else(Vec::new, |x| values_at(x, rest)),
        _ => Vec::new(),
    }
}

/// Whether a value says nothing (zero, null, empty): such a role value
/// is the definition's default as well, so it is no leak.
fn empty(v: &Val) -> bool {
    match v {
        Val::Bytes(b) | Val::Data(b) => b.iter().all(|x| *x == 0),
        Val::Int(0) | Val::Enum(0) | Val::Flags(0) | Val::Word(0) => true,
        Val::Float(x) => *x == 0.0,
        Val::List(l) => l.iter().all(empty),
        Val::Block(e) => e.is_empty(),
        Val::Ref { target, .. } | Val::Datum(target) => *target == Target::Null,
        _ => false,
    }
}

/// Whether a 4-byte role value is half of a reference holding none, in a
/// span the definition does not name as one: a tag group's word (V: the
/// `jpt!`, `foot`, `effe` words, each before a `0xffffffff`) or `-1`.
fn none_word(b: &[u8]) -> bool {
    let Ok(w) = <[u8; 4]>::try_from(b) else {
        return false;
    };
    let w = u32::from_le_bytes(w);
    w == u32::MAX || Defs::builtin().group(FourCC(w.to_be_bytes())).is_some()
}

fn shown(v: &Val) -> String {
    let s = match v {
        Val::Bytes(b) if b.len() == 4 => {
            let w = u32::from_le_bytes(b[..4].try_into().unwrap());
            format!("{w:#010x} ({})", f32::from_bits(w))
        }
        Val::Block(e) => format!("{} elements", e.len()),
        Val::Struct(_) => "struct".to_owned(),
        v => defs::show(v),
    };
    cut(&s, 48)
}

/// `s` cut to `n` characters, with `...` when it was longer.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}...", s.chars().take(n - 3).collect::<String>())
    } else {
        s.to_owned()
    }
}

/// Whether `id` names a tag of the tree or stands for one of its sounds.
fn built_here(id: TagId) -> bool {
    id == TagId::NULL
        || id == h1models::SELF_ID
        || local_index(id).is_some()
        || id.0 & 0xffff_0000 == sound(0).0
        || id.0 & 0xffff_0000 == looping(0).0
}

/// Every reference of a record to a tag the tree did not build:
/// (path, datum).
fn record_refs(d: &StructDef, r: &Record, prefix: &str, out: &mut Vec<(String, TagId)>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{prefix}{}", f.name);
        match (&f.kind, v) {
            (
                _,
                Val::Ref {
                    target: Target::Id(id),
                    ..
                }
                | Val::Datum(Target::Id(id)),
            ) if !built_here(*id) => out.push((p, *id)),
            (Kind::Block(s), Val::Block(e)) => {
                for (i, x) in e.iter().enumerate() {
                    record_refs(s, x, &format!("{p}[{i}]."), out);
                }
            }
            (Kind::Struct(s), Val::Struct(x)) => record_refs(s, x, &format!("{p}."), out),
            _ => {}
        }
    }
}

/// The same over a builder tree with no definition: paths are offsets.
fn block_refs(b: &Block, prefix: &str, out: &mut Vec<(String, TagId)>) {
    for f in &b.fixups {
        let p = format!("{prefix}+{:#x}", f.offset);
        match &f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                block_refs(child, &format!("{p}[]."), out)
            }
            FixupKind::TagRef(id) | FixupKind::TagId(id) if !built_here(*id) => out.push((p, *id)),
            _ => {}
        }
    }
}

/// The meaning a definition gives the field at `path` (indices as `[]`).
fn meaning<'d>(d: &'d StructDef, path: &str) -> Option<&'d str> {
    let (head, rest) = match path.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };
    let f = d.field(head.trim_end_matches("[]"))?;
    match (rest, &f.kind) {
        (None, _) => Some(f.meaning.as_str()).filter(|m| !m.is_empty()),
        (Some(rest), Kind::Block(s) | Kind::Struct(s)) => meaning(s, rest),
        _ => None,
    }
}

/// The leaks of one map's sandbox.
fn audit(map: &str, base: &Base, snap: &Snapshot) -> Vec<Leak> {
    let mut out = Vec::new();
    for (k, (tag, conv)) in snap.tags.iter().enumerate() {
        let group = conv.group.to_string();
        let leak = |path: &str, what: String, value: String| Leak {
            map: map.to_owned(),
            group: group.clone(),
            tag: tag.clone(),
            path: normal(path),
            what,
            shown: value,
        };
        let d = Defs::builtin().group(conv.group);
        let record = d.and_then(|d| defs::from_built(d, &conv.root).ok());
        // Retail references.
        let mut refs = Vec::new();
        match (d, &record) {
            (Some(d), Some(r)) => record_refs(d, r, "", &mut refs),
            _ => block_refs(&conv.root, "", &mut refs),
        }
        for (p, id) in refs {
            let (g, name) = base.name(id).unwrap_or_else(|| {
                panic!("{map} {group} {tag} {p}: {id:?} is in neither the tree nor the base")
            });
            // The emblem material is Halo 2's on purpose: it draws the
            // player's own emblem and colours.
            let what = if name == h1objects::convert::EMBLEM_SHADER {
                "emblem".to_owned()
            } else {
                g.to_string()
            };
            out.push(leak(&p, what, name.to_owned()));
        }
        // Role values still there.
        let Some(r) = &record else {
            continue;
        };
        for (_, fields) in snap.roles.iter().filter(|e| e.0 == k) {
            for f in fields {
                if empty(&f.value) || matches!(f.value, Val::Ref { .. } | Val::Datum(_)) {
                    continue;
                }
                if !values_at(r, &f.path)
                    .iter()
                    .any(|v| defs::same(v, &f.value))
                {
                    continue; // Halo 1's value replaced it.
                }
                let what = match &f.value {
                    Val::Block(_) | Val::Struct(_) => "copied",
                    Val::Bytes(b) if none_word(b) => "none-word",
                    _ => "value",
                };
                out.push(leak(&f.path, what.to_owned(), shown(&f.value)));
            }
        }
    }
    out
}

/// One table row: the leaks of one (group, path, what), over tags and
/// maps.
#[derive(Default)]
struct Row {
    tags: std::collections::BTreeSet<String>,
    shown: std::collections::BTreeSet<String>,
}

fn report(leaks: &[Leak]) -> String {
    let mut rows: BTreeMap<(String, String, String), Row> = BTreeMap::new();
    for l in leaks {
        let r = rows
            .entry((l.group.clone(), l.path.clone(), l.what.clone()))
            .or_default();
        r.tags.insert(format!("{} {}", l.map, l.tag));
        r.shown.insert(l.shown.clone());
    }
    let mut s = String::new();
    let mut group = String::new();
    for ((g, p, w), r) in &rows {
        if *g != group {
            group = g.clone();
            s.push_str(&format!(
                "\n### `{g}`\n\n| Field | Leaks | Tags | Meaning | Status |\n|---|---|---|---|---|\n"
            ));
        }
        let l = Leak {
            map: String::new(),
            group: g.clone(),
            tag: String::new(),
            path: p.clone(),
            what: w.clone(),
            shown: String::new(),
        };
        let status = match (rule(ALLOWED, &l), rule(KNOWN, &l)) {
            (Some(why), _) => format!("allowed: {why}"),
            (None, Some(st)) => st.to_owned(),
            (None, None) => "UNCLASSIFIED".to_owned(),
        };
        let examples: Vec<_> = r.shown.iter().take(3).cloned().collect();
        let more = r.shown.len().saturating_sub(3);
        let what = if w == "value" || w == "copied" {
            w.clone()
        } else {
            format!("`{w}` reference")
        };
        let m = Defs::builtin()
            .group(FourCC(g.as_bytes().try_into().unwrap_or(*b"????")))
            .and_then(|d| meaning(d, p))
            .unwrap_or("")
            .replace('|', "/");
        let m = cut(&m, 60);
        s.push_str(&format!(
            "| `{p}` | {what}: {}{} | {} | {m} | {status} |\n",
            examples.join("; ").replace('|', "/"),
            if more > 0 {
                format!(" (+{more})")
            } else {
                String::new()
            },
            r.tags.len(),
        ));
    }
    s
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_role_leak_is_classified() {
    let maps: Vec<String> = match std::env::var("H1_MAPS") {
        Ok(s) if s == "all" => h1convert::convert::MAPS
            .iter()
            .map(|m| m.0.to_owned())
            .collect(),
        Ok(s) => s.split(',').map(str::to_owned).collect(),
        Err(_) => vec!["bloodgulch".to_owned()],
    };
    let mut leaks = Vec::new();
    for name in &maps {
        let Some((h1, sha)) = h1_map(name) else {
            println!("{name}: not on this Halo 1 disc");
            continue;
        };
        let (_, base_name) = h1convert::convert::known(name).expect("a multiplayer map");
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let mut objects = Audit {
            inner: Objects::new(&h1, &base),
            snap: Snapshot::default(),
        };
        let _prepared = prepare_halo1(&h1, sha, &mut base_map, &mut objects).unwrap();
        let snap = objects.snap;
        let noted: usize = snap.roles.iter().map(|e| e.1.len()).sum();
        let got = audit(name, &base, &snap);
        println!(
            "{name} on {base_name}: {} tags, {noted} role values noted, {} leaks",
            snap.tags.len(),
            got.len()
        );
        assert!(
            noted > 0,
            "{name}: no role value noted: is the provenance wired?"
        );
        leaks.extend(got);
    }
    let md = report(&leaks);
    println!("{md}");
    if let Ok(path) = std::env::var("ROLE_AUDIT_OUT") {
        std::fs::write(&path, &md).unwrap();
    }
    // Rules no leak meets any more (fixed, or the lane landed).
    for (g, p, w, st) in ALLOWED.iter().chain(KNOWN) {
        let hit = leaks
            .iter()
            .any(|l| matches(g, &l.group) && matches(p, &l.path) && matches(w, &l.what));
        if !hit {
            println!("rule met by no leak (stale?): ({g:?}, {p:?}, {w:?}): {st}");
        }
    }
    let mut new: Vec<String> = leaks
        .iter()
        .filter(|l| rule(ALLOWED, l).is_none() && rule(KNOWN, l).is_none())
        .map(|l| {
            format!(
                "    ({:?}, {:?}, {:?}, \"\"), // {} {}: {}",
                l.group, l.path, l.what, l.map, l.tag, l.shown
            )
        })
        .collect();
    new.sort();
    new.dedup_by(|a, b| a.split(" // ").next() == b.split(" // ").next());
    assert!(
        new.is_empty(),
        "{} role leaks neither allowed nor known; classify each in ALLOWED or KNOWN:\n{}",
        new.len(),
        new.join("\n")
    );
}

#[test]
fn paths_normalise_and_rules_match_by_prefix() {
    assert_eq!(
        normal("barrels[0].block_e4[12].ref_4"),
        "barrels[].block_e4[].ref_4"
    );
    assert_eq!(
        normal("events[*].particle_systems[*].lod"),
        "events[].particle_systems[].lod"
    );
    let l = Leak {
        map: "m".into(),
        group: "weap".into(),
        tag: "t".into(),
        path: "barrels[].block_e4[].ref_4".into(),
        what: "effe".into(),
        shown: String::new(),
    };
    assert!(rule(KNOWN, &l).unwrap().starts_with("owned by h1-weap-snd"));
    assert!(rule(ALLOWED, &l).is_none());
    assert!(matches("unknown_*", "unknown_1c"));
    assert!(!matches("unknown_1", "unknown_1c"));
}

#[test]
fn values_resolve_through_blocks_and_wildcards() {
    let e = |w: u32| Record::new(vec![("w".into(), Val::Bytes(w.to_le_bytes().to_vec()))]);
    let r = Record::new(vec![("b".into(), Val::Block(vec![e(1), e(2)]))]);
    let w = |v: &Val| match v {
        Val::Bytes(b) => b[0],
        _ => panic!(),
    };
    assert_eq!(
        values_at(&r, "b[1].w")
            .into_iter()
            .map(w)
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(
        values_at(&r, "b[*].w")
            .into_iter()
            .map(w)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(values_at(&r, "b[2].w").is_empty());
}
