//! The role audit: every field of a converted Halo 1 sandbox whose value
//! came from its **role** (the base map's retail Halo 2 tags of the same
//! kind) rather than from Halo 1, each classified. Reads `$H1_ISO` and
//! `$H2_DISC`; prints names, paths and values only. Nothing is written
//! unless `ROLE_AUDIT_OUT` names a file for the markdown report.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it role_audit_disc:: -- --ignored --nocapture
//! H1_MAPS=all ...   # all multiplayer and campaign maps (default bloodgulch)
//! ```
//!
//! What counts as a leak, per converted tag (`docs/halo1/role-audit.md`):
//!
//! - **value**: a field its builder wrote from a role
//!   (`h1objects::provenance`: role words, a role's named values, retail
//!   values kept in the source) that still holds that value in the built
//!   tag, so no Halo 1 value replaced it;
//! - **copied**: a block or struct taken whole from the Halo 2
//!   model damage response and still the
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
        "vehi",
        "*",
        "none-word",
        "(a) half of a reference the definition does not name, holding none (a group word or -1): no value to take from anywhere",
    ),
    ("hlmt", "block_60[].block_bc*", "*", "(a) native damage section response table indexed by object_new"),
    ("hlmt", "block_60[].tagref_e8", "shad", "(a) native first- or third-person shield shader"),
    ("hlmt", "block_60[].tagref_f0", "shad", "(a) native first- or third-person shield shader"),
    (
        "hlmt",
        "shad_ref_94",
        "shad",
        "(a) the active camouflage shader: Halo 1 draws camouflage from its globals, not per model",
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
    ("stem", "postprocess[].passes[].pass", "spas", "(a) native opaque lighting, shadow and fog passes used by the rebuilt model template"),
    (
        "spas",
        "postprocess[].implementations[].vertex_shader",
        "vrtx",
        "(a) the scope and model passes require native screen-effect and model vertex shaders; Halo 1 has no vertex shader tags",
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
    ("vehi", "block_1c8[]*", "value", "seat access thresholds used by the Halo 2 entry test"),
    ("vehi", "block_2f8[]*", "value", "native vehicle phantom"),
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
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
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

    fn fixture(
        &mut self,
        tag: &h1map::Tag,
        lighting: Option<h1convert::fixture::Lighting>,
    ) -> Result<usize, String> {
        self.inner.fixture(tag, lighting)
    }

    fn first_person_arms(&mut self, id: h1map::TagId) -> Result<usize, String> {
        self.inner.first_person_arms(id)
    }

    fn collision(
        &mut self,
        group: [u8; 4],
        name: &str,
    ) -> Result<Vec<tags::collision::Bsp>, String> {
        self.inner.collision(group, name)
    }

    fn take(&mut self) -> ObjectTags {
        let t = self.inner.tree();
        assert!(t.model_shader_error.is_none(), "{:?}", t.model_shader_error);
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
        || id == h1_assets::models::SELF_ID
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

/// Each original slot must point into this tree, to the shader it was
/// built from. Only the extra slot appended to a player can be an emblem.
fn campaign_materials(h1: &H1Map, snap: &Snapshot, name: &str, mode: &Record) {
    let map = &h1.header().name;
    let model = h1.find(h1map::Group::new(b"mode"), name).expect("source render model");
    let source = h1map::Block::read(h1.tag_data(model).unwrap(), 0xdc).unwrap();
    let rows = h1.block(source, 0x20).unwrap();
    let materials = values_at(mode, "materials[*].shader");
    let n = source.count as usize;
    assert!((n..=n+1).contains(&materials.len()), "{map} {name}: shader count");
    for (k, shader) in materials.into_iter().enumerate() {
        let Val::Ref { target: Target::Id(id), .. } = shader else {
            panic!("{map} {name} slot {k}: unresolved shader {shader:?}");
        };
        if k == n {
            let (name, shader) = local_index(*id)
                .and_then(|i| snap.tags.get(i))
                .expect("the emblem instance is rebuilt in this tree");
            assert_eq!(name, h1objects::convert::EMBLEM_SHADER);
            assert_eq!(shader.group, FourCC(*b"shad"));
            continue;
        }
        let source = h1.tag(h1map::TagRef::read(rows, k * 0x20).unwrap().id).unwrap();
        let built = h1objects::convert::local_index(*id).and_then(|i| snap.tags.get(i))
            .unwrap_or_else(|| panic!("{map} {name} slot {k}: {} is not rebuilt", source.name));
        assert_eq!(built.1.group, FourCC(*b"shad"));
        assert!(built.0 == source.name ||
            (built.0.strip_prefix(source.name.as_str()).is_some_and(|s| s.starts_with('_'))
                && h1convert::shad::chicago_numeric(h1, source).is_some()),
            "{map} {name} slot {k}: expected {}, got {}", source.name, built.0);
    }
}

/// The leaks of one map's sandbox.
fn audit(map: &str, h1: &H1Map, base: &Base, snap: &Snapshot) -> Vec<Leak> {
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
        if conv.group == FourCC(*b"mode") && h1map::disc::CAMPAIGN_LEVELS.contains(&map) {
            campaign_materials(h1, snap, tag, record.as_ref().expect("read campaign model"));
        }
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
            out.push(leak(&p, g.to_string(), name.to_owned()));
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

fn audit_maps(selection: &str) -> Vec<String> {
    let mut maps = Vec::new();
    if matches!(selection, "all" | "multiplayer") {
        maps.extend(h1convert::convert::MAPS.iter().map(|m| m.0.to_owned()));
    }
    if matches!(selection, "all" | "campaign") {
        maps.extend(h1map::disc::CAMPAIGN_LEVELS.map(str::to_owned));
    }
    if maps.is_empty() {
        maps.extend(selection.split(',').map(|name| name.trim().to_owned()));
    }
    maps
}

// Include campaign-only and script-spawned objects even when the starting
// BSP has no placement. Their dependent models, effects and damage tags
// share the same conversion tree as the object that names them.
fn campaign_objects(h1: &H1Map, base: &Base) -> Snapshot {
    let mut conv = h1objects::convert::Converter::new(h1, base);
    for tag in h1.tags() {
        match &tag.group.0.to_be_bytes() {
            b"garb" => conv.garbage(tag),
            b"bipd" | b"vehi" | b"weap" | b"eqip" | b"proj" | b"scen"
            | b"mach" | b"ctrl" | b"lifi" => conv.object(tag),
            _ => continue,
        }.unwrap_or_else(|e| panic!("{} {}: {e}", tag.group, tag.name));
    }
    assert!(conv.tree.model_shader_error.is_none(), "{:?}", conv.tree.model_shader_error);
    Snapshot { tags: conv.tree.tags, roles: conv.tree.role_fields }
}

#[test]
fn selection_includes_all_ten_campaign_maps_and_keeps_explicit_order() {
    assert_eq!(audit_maps("campaign"), h1map::disc::CAMPAIGN_LEVELS.map(str::to_owned).to_vec());
    for level in audit_maps("campaign") {
        assert!(audit_maps("all").contains(&level));
    }
    assert_eq!(audit_maps("d40, a10"), ["d40", "a10"]);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_role_leak_is_classified() {
    let selection = std::env::var("H1_MAPS").unwrap_or_else(|_| "bloodgulch".into());
    let maps = audit_maps(&selection);
    let mut leaks = Vec::new();
    for name in &maps {
        let campaign = h1map::disc::CAMPAIGN_LEVELS.contains(&name.as_str());
        let Some((h1, sha)) = h1_map(name) else {
            assert!(!campaign, "campaign level {name} missing from disc");
            println!("{name}: not on this Halo 1 disc");
            continue;
        };
        let base_name = if campaign {
            "05a_deltaapproach"
        } else {
            h1convert::convert::known(name).expect("a known multiplayer map").1
        };
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let snap = if campaign {
            campaign_objects(&h1, &base)
        } else {
            let mut objects = Audit {
                inner: Objects::new(&h1, &base),
                snap: Snapshot::default(),
            };
            let _prepared = prepare_halo1(&h1, sha, &mut base_map, &mut objects).unwrap();
            objects.snap
        };
        assert!(!snap.tags.is_empty(), "{name}: no objects converted");
        let noted: usize = snap.roles.iter().map(|e| e.1.len()).sum();
        let got = audit(name, &h1, &base, &snap);
        println!(
            "{name} on {base_name}: {} tags, {noted} role values noted, {} leaks",
            snap.tags.len(),
            got.len()
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
    assert!(rule(KNOWN, &l).is_none());
    assert!(rule(ALLOWED, &l).is_none());
    let model_shader = Leak {
        group: "mode".into(),
        path: "materials[].shader".into(),
        what: "shad".into(),
        ..l
    };
    assert!(rule(KNOWN, &model_shader).is_none());
    assert!(rule(ALLOWED, &model_shader).is_none());
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
