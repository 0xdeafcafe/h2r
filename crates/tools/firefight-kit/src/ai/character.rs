//! The stage-one Elite: the retail campaign Elite's behaviour rows, with
//! the kit's own tuning set over them. Distances use world units and
//! angles use radians.
use std::io::{Read, Seek};

use map::{FourCC, Map, TagId, TagQuery, build::Block};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};
use tags::geometry::TagView;

const CHAR: FourCC = FourCC(*b"char");

/// The retail campaign Elite whose behaviour rows the authored Elite takes
/// (every campaign map carries it, notes/2026-11-26-firefight-class-census.md).
pub(crate) const DONOR_ELITE: &str = r"objects\characters\elite\ai\elite";

/// The character property blocks the engine looks up for every actor,
/// each through a walk up the parent chain that returns the first row or
/// none (`0x1e4a50`: block count at the property's offset, then the
/// `parent` reference). Some callers read the row without checking, so an
/// empty block faults: the movement row at `+0x5c` was the first, read by
/// `0x1f9119` at an actor's first update (`[0+4]`). The authored Elite has
/// no parent, so it carries the donor Elite's row of each, and a donor
/// without one refuses the build. Not here: `variants` (the donor's string
/// ids), `boss` and `boarding` (a row opts the class in), and `patterns`
/// (authored for the map's weapon).
pub(crate) const PROPERTY_ROWS: &[&str] = &[
    "general",
    "vitality",
    "placement",
    "perception",
    "look",
    "movement",
    "swarm",
    "ready",
    "engage",
    "charge",
    "evasion",
    "cover",
    "retreat",
    "search",
    "pre_search",
    "idle",
    "vocalize",
    "weapons",
];

/// The fields the kit authors over the donor's rows, by property.
pub(crate) fn authored(weapon: TagId) -> Vec<(&'static str, Vec<(&'static str, Val)>)> {
    vec![
        ("general", vec![("actor_type", Val::Int(0))]),
        (
            "vitality",
            vec![
                ("body", Val::Float(72.0)),
                ("shield", Val::Float(84.0)),
                ("legendary_body", Val::Float(90.0)),
                ("legendary_shield", Val::Float(110.0)),
            ],
        ),
        (
            "perception",
            vec![
                ("sight", Val::Float(30.0)),
                ("central_angle", Val::Float(50_f32.to_radians())),
                ("peripheral_angle", Val::Float(95_f32.to_radians())),
                ("peripheral_distance_angle", Val::Float(65_f32.to_radians())),
                ("peripheral_distance", Val::Float(13.0)),
                ("hearing", Val::Float(20.0)),
                ("notice_projectile", Val::Float(1.0)),
                ("notice_vehicle", Val::Float(1.0)),
            ],
        ),
        (
            "weapons",
            vec![
                ("weapon", Val::tag(FourCC(*b"weap"), weapon)),
                ("max_distance", Val::Float(26.0)),
                ("combat_range", Val::floats(&[4.0, 14.0])),
            ],
        ),
    ]
}

/// The donor Elite's first row of each [`PROPERTY_ROWS`] block, as its
/// bytes, and the map it came from.
#[derive(Clone, Debug)]
pub(crate) struct DonorRows {
    pub map: String,
    pub rows: Vec<(&'static str, Vec<u8>)>,
}

fn def() -> &'static StructDef {
    Defs::builtin().group(CHAR).expect("char definition")
}

/// Read [`DONOR_ELITE`]'s rows from `map` (labelled `label` in errors).
/// `Ok(None)` when the map lacks the tag. A property with no row in the
/// Elite or its parents refuses, naming the property and the map.
pub(crate) fn donor_rows<R: Read + Seek>(
    map: &mut Map<R>,
    label: &str,
) -> Result<Option<DonorRows>, String> {
    rows_of(map, DONOR_ELITE, label)
}

/// [`donor_rows`] of the character at `path`.
pub(crate) fn rows_of<R: Read + Seek>(
    map: &mut Map<R>,
    path: &str,
    label: &str,
) -> Result<Option<DonorRows>, String> {
    let Ok(mut id) = map.tag(TagQuery::NameInGroup(path, CHAR)).map(|t| t.id) else {
        return Ok(None);
    };
    let def = def();
    let mut chain = Vec::new();
    // The tag and its parents, nearest first (`parent` is a tag reference
    // at +4, its datum at +8).
    for _ in 0..8 {
        let address = map
            .tag(id)
            .ok()
            .and_then(|t| t.location)
            .map(|l| l.address)
            .ok_or_else(|| format!("{label}'s {path} has no data"))?;
        let bytes = map
            .read_tag_bytes(id)
            .map_err(|e| format!("reading {label}'s {path}: {e}"))?;
        if bytes.len() < def.size as usize {
            return Err(format!("{label}'s {path} is truncated"));
        }
        let parent = TagId(u32::from_le_bytes(bytes[8..12].try_into().unwrap()));
        chain.push((bytes, address));
        if parent == TagId::NULL {
            break;
        }
        id = parent;
    }
    let mut rows = Vec::new();
    for &name in PROPERTY_ROWS {
        let field = def.field(name).expect("character property");
        let stride = def.child(name).expect("character property").size as usize;
        let row = chain.iter().find_map(|(bytes, address)| {
            let v = TagView::new(bytes, *address);
            let root = v.at(0, def.size as usize).ok()?;
            let rows = v.block(root, field.offset as usize, stride).ok()?;
            rows.first().map(|r| r.to_vec())
        });
        let row = row.ok_or_else(|| {
            format!("the campaign Elite {path} in {label} has no {name} row, which Firefight's Elite takes")
        })?;
        rows.push((name, row));
    }
    Ok(Some(DonorRows {
        map: label.to_owned(),
        rows,
    }))
}

/// One donor row as a record of the property's definition. Its tag
/// references name the donor's tags, which the kit map does not have, so
/// each is null.
pub(crate) fn donor_record(name: &str, bytes: &[u8]) -> Result<Record, String> {
    let child = def().child(name).expect("character property");
    let mut row = defs::from_built(child, &Block::new(bytes.to_vec()))
        .map_err(|e| format!("the donor Elite's {name} row: {e}"))?;
    for (_, value) in &mut row.fields {
        if matches!(value, Val::Ref { .. }) {
            *value = Val::Ref {
                group: u32::MAX,
                target: Target::Null,
            };
        }
    }
    Ok(row)
}

pub(super) fn elite(unit: TagId, weapon: TagId, donor: &DonorRows) -> Result<Block, String> {
    let record = elite_record(unit, weapon, donor)?;
    defs::to_built(def(), &record, &mut |_| None).map_err(|e| e.to_string())
}

/// The Elite before it is built: the donor's rows, the authored fields set
/// over them, and the authored firing pattern.
pub(crate) fn elite_record(unit: TagId, weapon: TagId, donor: &DonorRows) -> Result<Record, String> {
    let def = def();
    let err = |e: defs::DefError| e.to_string();
    let mut root = defs::default_record(def);
    root.set("unit", Val::tag(FourCC(*b"bipd"), unit)).map_err(err)?;
    for &name in PROPERTY_ROWS {
        let bytes = donor
            .rows
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b)
            .ok_or_else(|| format!("the donor Elite from {} has no {name} row", donor.map))?;
        let mut row = donor_record(name, bytes)?;
        if let Some((_, fields)) = authored(weapon).into_iter().find(|(n, _)| *n == name) {
            for (key, value) in fields {
                row.set(key, value).map_err(err)?;
            }
        }
        root.set(name, Val::Block(vec![row])).map_err(err)?;
    }
    let pattern_set = def.child("patterns").expect("weapon patterns");
    let mut burst = defs::default_record(pattern_set.child("patterns").expect("firing pattern"));
    burst.set("burst_duration", Val::floats(&[0.35, 0.65])).map_err(err)?;
    let mut patterns = defs::default_record(pattern_set);
    patterns.set("weapon", Val::tag(FourCC(*b"weap"), weapon)).map_err(err)?;
    patterns.set("patterns", Val::Block(vec![burst])).map_err(err)?;
    root.set("patterns", Val::Block(vec![patterns])).map_err(err)?;
    Ok(root)
}

/// Invented donor rows: each property's row filled with its own byte, so
/// a row taken from the wrong property shows.
#[cfg(any(test, feature = "testkit"))]
pub(crate) fn invented_donor() -> DonorRows {
    let def = def();
    DonorRows {
        map: "invented".into(),
        rows: PROPERTY_ROWS
            .iter()
            .enumerate()
            .map(|(i, &name)| {
                let size = def.child(name).unwrap().size as usize;
                (name, vec![0x10 + i as u8; size])
            })
            .collect(),
    }
}

/// [`invented_donor`]'s rows as a built `char`: the properties in `only`
/// (all when empty) but those in `skip`, under the parent `parent`.
#[cfg(any(test, feature = "testkit"))]
pub(crate) fn invented_character(only: &[&str], skip: &[&str], parent: TagId) -> Block {
    let def = def();
    let mut root = defs::default_record(def);
    root.set("parent", Val::tag(CHAR, parent)).unwrap();
    for (name, bytes) in invented_donor().rows {
        if (!only.is_empty() && !only.contains(&name)) || skip.contains(&name) {
            continue;
        }
        root.set(name, Val::Block(vec![donor_record(name, &bytes).unwrap()])).unwrap();
    }
    defs::to_built(def, &root, &mut |_| None).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property row is the donor's, but for the fields the kit
    /// authors and the donor's tag references, which are null in the kit.
    #[test]
    fn each_row_is_the_donors_but_the_authored_fields() {
        let weapon = TagId(0xe1a0_0002);
        let donor = invented_donor();
        let root = elite_record(TagId(0xe1a0_0001), weapon, &donor).unwrap();
        let authored = authored(weapon);
        for (name, bytes) in &donor.rows {
            let Some(Val::Block(rows)) = root.get(name) else {
                panic!("{name}")
            };
            assert_eq!(rows.len(), 1, "{name}");
            let mut want = donor_record(name, bytes).unwrap();
            for (key, value) in authored
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f.clone())
                .unwrap_or_default()
            {
                want.set(key, value).unwrap();
            }
            assert_eq!(rows[0].fields, want.fields, "{name}");
            // The untouched bytes are the donor's own.
            if !authored.iter().any(|(n, _)| n == name) {
                assert!(
                    rows[0].fields.iter().any(|(_, v)| matches!(v, Val::Bytes(b) if b.iter().all(|&x| x == bytes[0]))),
                    "{name} keeps the donor's bytes"
                );
            }
        }
        let Some(Val::Block(vitality)) = root.get("vitality") else {
            panic!("vitality")
        };
        assert_eq!(vitality[0].get("body"), Some(&Val::Float(72.0)));
        for name in ["boss", "boarding", "variants"] {
            assert!(
                !matches!(root.get(name), Some(Val::Block(rows)) if !rows.is_empty()),
                "{name}"
            );
        }
        assert!(defs::to_built(def(), &root, &mut |_| None).is_ok());
    }

    /// A donor without a property's row is refused by the property's name.
    #[test]
    fn a_donor_without_a_row_is_refused_by_name() {
        let mut donor = invented_donor();
        donor.rows.retain(|(n, _)| *n != "movement");
        let error = elite_record(TagId(1), TagId(2), &donor).unwrap_err();
        assert_eq!(error, "the donor Elite from invented has no movement row");
    }
}
