//! What a mod contains, in detail: the report `h2 live maps analyze`
//! writes and `h2 live maps details` sends (live_proto::maps::Contents,
//! docs/game/maps.md §5, "Details"). Where [`crate::live_mod_features`]
//! ticks a checklist, this says what changed:
//!
//! - **tag classes**: per group (weapons, render models, shaders,
//!   bitmaps, sounds, BSP geometry, lightmaps, ...), how many tags the mod
//!   adds, changes and removes, and the added ones' names when few;
//! - **swaps**: a reference of a tag both maps have that now names another
//!   tag, classified by what it points at: a weapon's barrel naming
//!   another projectile is a projectile swap ("battle rifle fires fuel
//!   rod"), an object's `model` another model, a render model's material
//!   another shader, a shader's map another bitmap, and so on;
//! - **stats**: the weapon, projectile and damage numbers whose meaning
//!   is established ([`STATS`]: magazine size, rate of fire, projectiles
//!   per shot, velocities, damage bounds), and a count of other numbers
//!   changed per tag;
//! - **placements**: per placed tag, how many the base and the mod place,
//!   how many moved, and the spots where one tag replaced another;
//! - **scripts**: whether the scenario's scripts differ.
//!
//! The fields come from walking each tag of the [`walked`] groups with the
//! built-in field registry (tags::registry: every group's layout, its
//! blocks, references and named numbers): references are kept by the
//! named tag's group and name (never its datum, which shifts when a mod
//! adds tags), numbers by their bytes, blocks by their counts. Tags are
//! matched by group and name, so a tag that moved compares by what it
//! says, not where it is. The comparison ([`contents`]) is pure over two
//! censuses and tested with invented ones; the walk is tested with an
//! invented layout.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use live_proto::maps::{ClassChange, Contents, PlacementChange, StatChange, Swap};
use map::FourCC;
use tags::geometry::TagView;
use tags::registry::{Kind, Layout, Registry};

use crate::live_mod_features::Census;

/// One field's value as compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    /// A tag reference: the named tag's group and name; None when null.
    /// A datum the map has no tag for is named by its hex.
    Ref(Option<(String, String)>),
    /// A number nothing names: 1, 2 or 4 bytes, little-endian.
    Word(u32),
    /// A block's element count.
    Count(u32),
    /// A [`STATS`] number as shown.
    Stat(String),
}

/// One field of a walked tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// Where it is, in words: `barrels[0].projectile`.
    pub label: String,
    /// Whether the registry names the field (not only its offset).
    pub named: bool,
    pub val: Val,
}

/// A walked tag's fields by key: the offsets down to the field, with each
/// block element's index (`2d0.0/8c`); a block's count is `2d0#`; a
/// [`STATS`] value is `!` and the offsets.
pub type Fields = BTreeMap<String, Leaf>;

/// Every tag datum of a map, to its group and name.
pub type Names = HashMap<u32, (String, String)>;

/// Whether `group`'s tags are walked, and if so whether their numbers are
/// kept too (only references and block counts otherwise).
pub fn walked(group: &str) -> Option<bool> {
    Some(match group {
        "weap" | "proj" | "jpt!" | "vehi" | "bipd" | "eqip" | "garb" => true,
        "scen" | "mach" | "ctrl" | "bloc" | "ssce" | "lifi" | "crea" | "hlmt" | "mode" | "coll"
        | "phmo" | "shad" | "itmc" | "vehc" | "effe" | "sky" | "char" | "foot" | "nhdt" => false,
        _ => return None,
    })
}

/// A number whose meaning is established, read by offset.
pub struct Stat {
    pub group: &'static str,
    /// The block it is in, by offsets from the root (`2d0`); empty: the
    /// root.
    pub block: &'static str,
    pub offset: usize,
    pub float: bool,
    pub name: &'static str,
}

const fn stat(
    group: &'static str,
    block: &'static str,
    offset: usize,
    float: bool,
    name: &'static str,
) -> Stat {
    Stat {
        group,
        block,
        offset,
        float,
        name,
    }
}

/// The stats reported. Evidence: `crates/tags/src/defs/item.txt` (weapon
/// magazines), `proj.txt`, `jpt_.txt`, and the Halo 1 converter's barrel
/// and projectile rows (`h1objects::arsenal::BARREL_WORDS`,
/// `PROJECTILE_WORDS`: rounds per second at barrel `+0x4`/`+0x8`,
/// projectiles per shot at `+0x6a`, velocities at proj `+0x17c`/`+0x180`).
pub const STATS: &[Stat] = &[
    stat("weap", "2c0", 0x6, false, "starting ammo"),
    stat("weap", "2c0", 0x8, false, "max ammo"),
    stat("weap", "2c0", 0xa, false, "magazine size"),
    stat("weap", "2c0", 0x10, true, "reload time (s)"),
    stat("weap", "2c0", 0x14, false, "rounds per reload"),
    stat("weap", "2d0", 0x4, true, "rate of fire, min (shots/s)"),
    stat("weap", "2d0", 0x8, true, "rate of fire, max (shots/s)"),
    stat("weap", "2d0", 0x6a, false, "projectiles per shot"),
    stat("proj", "", 0xc8, true, "collision radius"),
    stat("proj", "", 0xe0, true, "max range"),
    stat("proj", "", 0x17c, true, "initial velocity"),
    stat("proj", "", 0x180, true, "final velocity"),
    stat("jpt!", "", 0x1c, true, "damage, min"),
    stat("jpt!", "", 0x20, true, "damage, max"),
];

/// A float as a short number: `0.5`, `12`, `1.234`.
pub fn short_float(v: f32) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

impl Stat {
    fn read(&self, elem: &[u8]) -> Option<String> {
        if self.float {
            let b = elem.get(self.offset..self.offset + 4)?;
            Some(short_float(f32::from_le_bytes(b.try_into().ok()?)))
        } else {
            let b = elem.get(self.offset..self.offset + 2)?;
            Some(i16::from_le_bytes(b.try_into().ok()?).to_string())
        }
    }
}

const MAX_DEPTH: usize = 6;
/// The most elements of one block walked.
const MAX_ELEMENTS: usize = 256;
/// The most fields kept of one tag.
const MAX_LEAVES: usize = 20_000;

fn word(b: &[u8], at: usize, len: usize) -> Option<u32> {
    let s = b.get(at..at + len)?;
    let mut w = [0u8; 4];
    w[..len].copy_from_slice(s);
    Some(u32::from_le_bytes(w))
}

struct Walk<'a> {
    view: TagView<'a>,
    names: &'a Names,
    group: &'a str,
    numbers: bool,
    out: Fields,
}

/// Where the walk is: the key so far, the block offsets without indices
/// (for [`STATS`]), the label so far.
struct At {
    key: String,
    pattern: String,
    label: String,
}

fn walk(cx: &mut Walk<'_>, layout: &Layout, elem: &[u8], at: &At, depth: usize) {
    for s in STATS
        .iter()
        .filter(|s| s.group == cx.group && s.block == at.pattern)
    {
        if let Some(v) = s.read(elem) {
            let label = if at.label.is_empty() {
                s.name.to_string()
            } else {
                format!("{} {}", at.label, s.name)
            };
            cx.out.insert(
                format!("!{}{:x}", at.key, s.offset),
                Leaf {
                    label,
                    named: true,
                    val: Val::Stat(v),
                },
            );
        }
    }
    for f in &layout.fields {
        if cx.out.len() >= MAX_LEAVES {
            return;
        }
        let o = f.offset as usize;
        let key = format!("{}{:x}", at.key, f.offset);
        let label = if at.label.is_empty() {
            f.name.clone()
        } else {
            format!("{}.{}", at.label, f.name)
        };
        let val = match &f.kind {
            Kind::TagRef { .. } => {
                let Some(d) = word(elem, o + 4, 4) else {
                    continue;
                };
                Val::Ref((d != u32::MAX).then(|| {
                    cx.names
                        .get(&d)
                        .cloned()
                        .unwrap_or_else(|| (String::new(), format!("{d:08x}")))
                }))
            }
            Kind::Block(child) => {
                let Ok(els) = cx.view.block(elem, o, child.size as usize) else {
                    continue;
                };
                cx.out.insert(
                    format!("{key}#"),
                    Leaf {
                        label: label.clone(),
                        named: f.named,
                        val: Val::Count(els.len() as u32),
                    },
                );
                if depth >= MAX_DEPTH {
                    continue;
                }
                let pattern = if at.pattern.is_empty() {
                    format!("{:x}", f.offset)
                } else {
                    format!("{}/{:x}", at.pattern, f.offset)
                };
                for (i, e) in els.iter().take(MAX_ELEMENTS).enumerate() {
                    let sub = At {
                        key: format!("{key}.{i}/"),
                        pattern: pattern.clone(),
                        label: format!("{label}[{i}]"),
                    };
                    walk(cx, child, e, &sub, depth + 1);
                }
                continue;
            }
            Kind::Int {
                bytes, count: 1, ..
            }
            | Kind::Enum { bytes, .. }
            | Kind::Flags { bytes, .. }
                if cx.numbers =>
            {
                let Some(w) = word(elem, o, usize::from(*bytes).min(4)) else {
                    continue;
                };
                Val::Word(w)
            }
            Kind::Float { count: 1, .. } if cx.numbers => {
                let Some(w) = word(elem, o, 4) else {
                    continue;
                };
                Val::Word(w)
            }
            Kind::Unknown { len } if cx.numbers && *len <= 4 => {
                let Some(w) = word(elem, o, *len as usize) else {
                    continue;
                };
                Val::Word(w)
            }
            _ => continue,
        };
        cx.out.insert(
            key,
            Leaf {
                label,
                named: f.named,
                val,
            },
        );
    }
}

/// The fields of one tag of `group` (as the census names it: `weap`,
/// `sky`) laid out as `layout`, its payload `bytes` at `address`.
pub fn walk_with(
    layout: &Layout,
    group: &str,
    numbers: bool,
    bytes: &[u8],
    address: u32,
    names: &Names,
) -> Fields {
    let Some(root) = bytes.get(..layout.size as usize) else {
        return Fields::new();
    };
    let mut cx = Walk {
        view: TagView::new(bytes, address),
        names,
        group,
        numbers,
        out: Fields::new(),
    };
    let top = At {
        key: String::new(),
        pattern: String::new(),
        label: String::new(),
    };
    walk(&mut cx, layout, root, &top, 0);
    cx.out
}

/// The fields of one tag of `fourcc`, if its group is [`walked`] and the
/// registry knows it. The scenario is walked for references and counts.
pub fn walk_tag(
    fourcc: FourCC,
    group: &str,
    bytes: &[u8],
    address: u32,
    names: &Names,
) -> Option<Fields> {
    let numbers = if group == "scnr" {
        false
    } else {
        walked(group)?
    };
    let layout = Registry::builtin().group(fourcc)?;
    Some(walk_with(layout, group, numbers, bytes, address, names))
}

// ---------------------------------------------------------------- report

/// A group in words, plural.
pub fn group_label(group: &str) -> String {
    match group {
        "weap" => "weapons",
        "vehi" => "vehicles",
        "bipd" => "bipeds",
        "proj" => "projectiles",
        "eqip" => "equipment",
        "garb" => "garbage objects",
        "scen" => "scenery",
        "mach" => "machines",
        "ctrl" => "controls",
        "bloc" => "crates",
        "ssce" => "sound scenery",
        "lifi" => "light fixtures",
        "crea" => "creatures",
        "hlmt" => "models",
        "mode" => "render models",
        "coll" => "collision models",
        "phmo" => "physics models",
        "jmad" => "animations",
        "shad" => "shaders",
        "stem" => "shader templates",
        "bitm" => "bitmaps",
        "snd!" => "sounds",
        "lsnd" => "looping sounds",
        "effe" => "effects",
        "prt3" => "particles",
        "jpt!" => "damage effects",
        "char" => "AI characters",
        "sbsp" => "BSP geometry",
        "ltmp" => "lightmaps",
        "sky" => "skies",
        "itmc" => "item collections",
        "vehc" => "vehicle collections",
        "nhdt" => "HUDs",
        "ligh" => "lights",
        "lens" => "lens flares",
        "foot" => "material effects",
        "weat" => "weather",
        "fog" => "fog",
        "deca" => "decals",
        "cont" => "contrails",
        "unic" => "strings",
        other => return format!("{other} tags"),
    }
    .into()
}

/// The order classes are listed in: what a player notices first.
const CLASS_ORDER: &[&str] = &[
    "weap", "vehi", "proj", "bipd", "eqip", "hlmt", "mode", "coll", "phmo", "jmad", "shad", "stem",
    "bitm", "sbsp", "ltmp", "sky", "scen", "mach", "ctrl", "bloc", "char", "jpt!", "effe", "prt3",
    "snd!", "lsnd", "itmc", "vehc", "nhdt",
];

fn class_rank(group: &str) -> usize {
    CLASS_ORDER
        .iter()
        .position(|g| *g == group)
        .unwrap_or(CLASS_ORDER.len())
}

/// A tag path's last part in words: `objects\weapons\rifle\battle_rifle\
/// battle_rifle` is "battle rifle".
pub fn words(tag: &str) -> String {
    tag.rsplit(['\\', '/'])
        .find(|p| !p.is_empty())
        .unwrap_or(tag)
        .replace('_', " ")
}

fn plural(n: u32, what: &str) -> String {
    if n == 1 || what.ends_with('s') {
        format!("{n} {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The swap kind of a reference of a `owner` tag that now names a tag of
/// `target`.
fn swap_kind(owner: &str, target: &str) -> &'static str {
    match (owner, target) {
        ("scnr", "sky") => "sky",
        ("scnr", "vehi" | "weap" | "scen" | "eqip" | "mach" | "ctrl" | "bloc" | "bipd") => {
            "palette"
        }
        (_, "proj") => "projectile",
        (_, "hlmt") => "model",
        ("hlmt", "mode") => "render_model",
        ("hlmt", "coll") => "collision",
        ("hlmt", "phmo") => "physics",
        (_, "jmad") => "animation",
        ("shad", "bitm") => "texture",
        (_, "shad" | "stem") => "shader",
        (_, "jpt!") => "damage",
        (_, "effe" | "prt3") => "effect",
        (_, "snd!" | "lsnd") => "sound",
        (_, "weap") => "weapon",
        (_, "vehi") => "vehicle",
        _ => "reference",
    }
}

/// How the swap kinds are listed, first to last.
const SWAP_ORDER: &[&str] = &[
    "projectile",
    "weapon",
    "vehicle",
    "model",
    "render_model",
    "collision",
    "physics",
    "animation",
    "shader",
    "texture",
    "damage",
    "effect",
    "sound",
    "sky",
    "palette",
    "reference",
];

/// One swap in words. `new_to`: the named tag is one the base has not.
fn swap_line(s: &Swap, new_to: bool) -> String {
    let owner = words(&s.tag);
    let to = match &s.to {
        Some(t) => format!("{}{}", words(t), if new_to { " (new)" } else { "" }),
        None => "nothing".into(),
    };
    let was = match &s.from {
        Some(f) => format!(" (was {})", words(f)),
        None => String::new(),
    };
    match s.kind.as_str() {
        "projectile" => format!("projectile swap: {owner} fires {to}{was}"),
        "model" => format!("model swap: {owner} uses the {to} model{was}"),
        "render_model" => format!("render model swap: model {owner} draws {to}{was}"),
        "collision" => format!("collision swap: model {owner} collides as {to}{was}"),
        "physics" => format!("physics swap: model {owner} uses {to} physics{was}"),
        "animation" => format!("animation swap: {owner} animates with {to}{was}"),
        "shader" => format!("shader swap: {owner} uses shader {to}{was}"),
        "texture" => format!("texture swap: shader {owner} uses bitmap {to}{was}"),
        "damage" => format!("damage swap: {owner} deals {to}{was}"),
        "weapon" => format!("weapon swap: {owner} {} is {to}{was}", s.field),
        "vehicle" => format!("vehicle swap: {owner} {} is {to}{was}", s.field),
        "sky" => format!("sky swap: the level's sky is {to}{was}"),
        "palette" => format!("palette swap: the scenario's {} is {to}{was}", s.field),
        k => format!("{k} swap: {owner} {} is {to}{was}", s.field),
    }
}

fn tag_changed(base: &Census, modded: &Census, key: &(String, String)) -> bool {
    let (Some(b), Some(m)) = (base.tags.get(key), modded.tags.get(key)) else {
        return false;
    };
    if b.size != m.size || (b.address == m.address && b.hash != m.hash) {
        return true;
    }
    if key.0 == "bitm"
        && let (Some(pb), Some(pm)) = (base.pixels.get(&key.1), modded.pixels.get(&key.1))
        && pb != pm
    {
        return true;
    }
    match (base.fields.get(key), modded.fields.get(key)) {
        (Some(fb), Some(fm)) => fb != fm,
        _ => false,
    }
}

/// The per-class counts.
fn classes(base: &Census, modded: &Census) -> Vec<ClassChange> {
    let mut by: BTreeMap<&str, ClassChange> = BTreeMap::new();
    for key in modded.tags.keys() {
        let g = key.0.as_str();
        let added = !base.tags.contains_key(key);
        let changed = !added && tag_changed(base, modded, key);
        if !added && !changed {
            continue;
        }
        let c = by.entry(g).or_insert_with(|| ClassChange {
            group: g.to_string(),
            label: group_label(g),
            ..ClassChange::default()
        });
        if added {
            c.added += 1;
            c.added_names.push(key.1.clone());
        } else {
            c.changed += 1;
        }
    }
    for key in base.tags.keys() {
        if modded.tags.contains_key(key) {
            continue;
        }
        let g = key.0.as_str();
        by.entry(g)
            .or_insert_with(|| ClassChange {
                group: g.to_string(),
                label: group_label(g),
                ..ClassChange::default()
            })
            .removed += 1;
    }
    let mut out: Vec<ClassChange> = by.into_values().collect();
    for c in &mut out {
        if c.added_names.len() > 8 {
            c.added_names.clear();
        }
    }
    out.sort_by_key(|c| (class_rank(&c.group), c.group.clone()));
    out
}

fn class_line(c: &ClassChange) -> String {
    let mut parts = Vec::new();
    if c.added > 0 {
        let names: Vec<String> = c.added_names.iter().map(|n| words(n)).collect();
        if !names.is_empty() && names.len() <= 4 {
            parts.push(format!("{} new ({})", c.added, names.join(", ")));
        } else {
            parts.push(format!("{} new", c.added));
        }
    }
    if c.changed > 0 {
        parts.push(format!("{} changed", c.changed));
    }
    if c.removed > 0 {
        parts.push(format!("{} removed", c.removed));
    }
    format!("{}: {}", c.label, parts.join(", "))
}

/// The references of tags both maps have that name another tag now, and
/// the scenario's.
fn swaps(base: &Census, modded: &Census) -> Vec<Swap> {
    let mut out: Vec<Swap> = Vec::new();
    let mut seen: BTreeSet<(String, String, Option<String>, Option<String>)> = BTreeSet::new();
    let mut compare = |owner_group: &str, owner: &str, fb: &Fields, fm: &Fields| {
        for (k, lm) in fm {
            let (Some(lb), Val::Ref(to)) = (fb.get(k), &lm.val) else {
                continue;
            };
            let Val::Ref(from) = &lb.val else {
                continue;
            };
            let (from_name, to_name) = (from.as_ref().map(|f| &f.1), to.as_ref().map(|t| &t.1));
            if from_name == to_name {
                continue;
            }
            let target = to
                .as_ref()
                .or(from.as_ref())
                .map(|t| t.0.as_str())
                .unwrap_or("");
            let kind = swap_kind(owner_group, target);
            let dedupe = (
                kind.to_string(),
                owner.to_string(),
                from_name.cloned(),
                to_name.cloned(),
            );
            if !seen.insert(dedupe) {
                continue;
            }
            out.push(Swap {
                kind: kind.into(),
                tag: owner.into(),
                field: lm.label.clone(),
                from: from_name.cloned(),
                to: to_name.cloned(),
            });
        }
    };
    for (key, fm) in &modded.fields {
        if let Some(fb) = base.fields.get(key) {
            compare(&key.0, &key.1, fb, fm);
        }
    }
    compare("scnr", "scenario", &base.scenario, &modded.scenario);
    out.sort_by_key(|s| {
        SWAP_ORDER
            .iter()
            .position(|k| *k == s.kind)
            .unwrap_or(SWAP_ORDER.len())
    });
    out
}

/// The [`STATS`] that differ, and per tag how many other numbers do.
fn stats(base: &Census, modded: &Census) -> (Vec<StatChange>, Vec<(String, usize)>) {
    let mut out = Vec::new();
    let mut other = Vec::new();
    for (key, fm) in &modded.fields {
        let Some(fb) = base.fields.get(key) else {
            continue;
        };
        let mut n = 0;
        for (k, lm) in fm {
            let Some(lb) = fb.get(k) else {
                continue;
            };
            if lb.val == lm.val {
                continue;
            }
            match (&lb.val, &lm.val) {
                (Val::Stat(from), Val::Stat(to)) => out.push(StatChange {
                    tag: key.1.clone(),
                    stat: lm.label.clone(),
                    from: from.clone(),
                    to: to.clone(),
                }),
                (Val::Word(_), Val::Word(_)) | (Val::Count(_), Val::Count(_)) => n += 1,
                _ => {}
            }
        }
        if n > 0 {
            other.push((key.1.clone(), n));
        }
    }
    (out, other)
}

/// Per feature and tag, what is placed, against the base; and the spots
/// where one tag took another's place (as `placement` swaps).
fn placements(base: &Census, modded: &Census) -> (Vec<PlacementChange>, Vec<Swap>) {
    let mut out = Vec::new();
    let mut replaced: Vec<Swap> = Vec::new();
    let features: BTreeSet<&str> = base
        .placed
        .keys()
        .chain(modded.placed.keys())
        .copied()
        .collect();
    for f in features {
        let b: &[(String, [i32; 3])] = base.placed.get(f).map(Vec::as_slice).unwrap_or(&[]);
        let m: &[(String, [i32; 3])] = modded.placed.get(f).map(Vec::as_slice).unwrap_or(&[]);
        if b == m {
            continue;
        }
        let spots = |v: &[(String, [i32; 3])], name: &str| -> Vec<[i32; 3]> {
            v.iter()
                .filter(|(n, _)| n == name)
                .map(|(_, p)| *p)
                .collect()
        };
        // A spot's tags, to see one replaced by another.
        let at = |v: &[(String, [i32; 3])]| -> BTreeMap<[i32; 3], Vec<String>> {
            let mut a: BTreeMap<[i32; 3], Vec<String>> = BTreeMap::new();
            for (n, p) in v {
                a.entry(*p).or_default().push(n.clone());
            }
            a
        };
        let (at_b, at_m) = (at(b), at(m));
        let mut swaps: BTreeMap<(String, String), u32> = BTreeMap::new();
        for (p, names_b) in &at_b {
            let Some(names_m) = at_m.get(p) else {
                continue;
            };
            let gone: Vec<&String> = names_b.iter().filter(|n| !names_m.contains(n)).collect();
            let came: Vec<&String> = names_m.iter().filter(|n| !names_b.contains(n)).collect();
            for (g, c) in gone.iter().zip(came.iter()) {
                *swaps.entry(((*g).clone(), (*c).clone())).or_default() += 1;
            }
        }
        let mut replaced_from: BTreeMap<&str, u32> = BTreeMap::new();
        let mut replaced_to: BTreeMap<&str, u32> = BTreeMap::new();
        for ((g, c), n) in &swaps {
            *replaced_from.entry(g.as_str()).or_default() += n;
            *replaced_to.entry(c.as_str()).or_default() += n;
            replaced.push(Swap {
                kind: "placement".into(),
                tag: f.to_string(),
                field: plural(*n, "spot"),
                from: Some(g.clone()),
                to: Some(c.clone()),
            });
        }
        let names: BTreeSet<&str> = b.iter().chain(m.iter()).map(|(n, _)| n.as_str()).collect();
        for name in names {
            let (sb, sm) = (spots(b, name), spots(m, name));
            if sb == sm {
                continue;
            }
            // Spots of the base's not in the mod's, and the other way,
            // as multisets; less those replaced in place.
            let minus = |a: &[[i32; 3]], z: &[[i32; 3]]| -> u32 {
                let mut left = z.to_vec();
                let mut n = 0;
                for p in a {
                    if let Some(i) = left.iter().position(|q| q == p) {
                        left.swap_remove(i);
                    } else {
                        n += 1;
                    }
                }
                n
            };
            let lost =
                minus(&sb[..], &sm[..]).saturating_sub(*replaced_from.get(name).unwrap_or(&0));
            let won = minus(&sm[..], &sb[..]).saturating_sub(*replaced_to.get(name).unwrap_or(&0));
            out.push(PlacementChange {
                feature: f.to_string(),
                tag: name.to_string(),
                base: sb.len() as u32,
                now: sm.len() as u32,
                moved: lost.min(won),
            });
        }
    }
    (out, replaced)
}

fn singular(feature: &str) -> &str {
    match feature {
        "vehicles" => "vehicle",
        "weapons" => "weapon",
        "objects" => "object",
        f => f,
    }
}

fn placement_line(p: &PlacementChange) -> Option<String> {
    let what = words(&p.tag);
    let kind = singular(&p.feature);
    let mut line = if p.base == 0 {
        format!("new {kind} placements: {}", plural(p.now, &what))
    } else if p.now == 0 {
        format!("{kind} placements removed: every {what} ({})", p.base)
    } else if p.now > p.base {
        format!(
            "more {kind} placements: {} ({} -> {})",
            plural(p.now - p.base, &what),
            p.base,
            p.now
        )
    } else if p.now < p.base {
        format!(
            "fewer {kind} placements: {} fewer {what} ({} -> {})",
            p.base - p.now,
            p.base,
            p.now
        )
    } else if p.moved > 0 {
        format!("{kind} placements moved: {} moved", plural(p.moved, &what))
    } else {
        return None;
    };
    if p.moved > 0 && p.base != p.now && p.base > 0 && p.now > 0 {
        line.push_str(&format!(", {} moved", p.moved));
    }
    Some(line)
}

/// What `modded` contains over `base`, in words and fields.
pub fn contents(base: &Census, modded: &Census) -> Contents {
    let classes = classes(base, modded);
    let swaps = swaps(base, modded);
    let (stats, other) = stats(base, modded);
    let (placements, replaced) = placements(base, modded);
    let scripts = modded.scripts != base.scripts;

    let mut lines = Vec::new();
    let mut references = 0;
    for s in &swaps {
        if s.kind == "reference" {
            references += 1;
            continue;
        }
        let new_to =
            s.to.as_ref()
                .is_some_and(|t| !base.tags.keys().any(|(_, n)| n == t));
        lines.push(swap_line(s, new_to));
    }
    let mut by_tag: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for s in &stats {
        by_tag
            .entry(s.tag.as_str())
            .or_default()
            .push(format!("{} {} -> {}", s.stat, s.from, s.to));
    }
    for (tag, changes) in by_tag {
        lines.push(format!("{}: {}", words(tag), changes.join(", ")));
    }
    for r in &replaced {
        lines.push(format!(
            "{} replaced: {} -> {} at {}",
            singular(&r.tag),
            words(r.from.as_deref().unwrap_or("")),
            words(r.to.as_deref().unwrap_or("")),
            r.field
        ));
    }
    lines.extend(placements.iter().filter_map(placement_line));
    lines.extend(classes.iter().map(class_line));
    // The scenario's named blocks that grew or shrank (spawns, flags).
    for (k, lm) in &modded.scenario {
        if let (Val::Count(now), Some(lb)) = (&lm.val, base.scenario.get(k))
            && lm.named
            && let Val::Count(was) = lb.val
            && was != *now
        {
            lines.push(format!(
                "scenario {}: {was} -> {now}",
                lm.label.replace('_', " ")
            ));
        }
    }
    if scripts {
        lines.push("scripts changed".into());
    }
    if references > 0 {
        lines.push(format!("{} other references changed", references));
    }
    if !other.is_empty() {
        let mut other = other;
        other.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let shown: Vec<String> = other
            .iter()
            .take(6)
            .map(|(t, n)| format!("{} ({n})", words(t)))
            .collect();
        let more = other.len().saturating_sub(6);
        lines.push(format!(
            "other values changed: {}{}",
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more tags")
            } else {
                String::new()
            }
        ));
    }
    let mut swaps = swaps;
    swaps.extend(replaced);
    Contents {
        lines,
        classes,
        swaps,
        stats,
        placements,
        scripts,
    }
    .bounded()
}

/// What `h2 live maps analyze` writes: the package it is for (when
/// given), the features [`crate::live_mod_features::features`] derives,
/// and the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Analysis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default)]
    pub contents: Contents,
}

/// The features and report of `modded` over `base`, both whole maps.
pub fn analyze(base: &[u8], modded: &[u8]) -> anyhow::Result<(Vec<String>, Contents)> {
    use anyhow::Context;
    use std::io::Cursor;
    let b = crate::live_mod_features::census(
        &mut map::Map::from_reader(Cursor::new(base)).context("reading the base map")?,
    )?;
    let m = crate::live_mod_features::census(
        &mut map::Map::from_reader(Cursor::new(modded)).context("reading the mod")?,
    )?;
    Ok((crate::live_mod_features::features(&b, &m), contents(&b, &m)))
}

#[cfg(test)]
#[path = "live_mod_contents_tests.rs"]
mod tests;
