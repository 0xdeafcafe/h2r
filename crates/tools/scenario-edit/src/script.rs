//! Edit scripts: a test map from an existing one, headless, as JSON
//! (`h2 testmap`, docs/runtime/agent-api.md phase 5).
//!
//! A script is a list of operations run through a [`Session`], so every
//! edit is validated exactly as the editor's are, then saved as a new map.
//! The usual shape is a template: `clear` every placement block, `keep` one
//! starting location, then `add` the one thing under test.
//!
//! ```json
//! {"ops": [
//!   {"op": "clear", "blocks": "all"},
//!   {"op": "keep", "block": "starting_locations", "indices": [0]},
//!   {"op": "add", "block": "weapons", "set": {"position": [1, 2, 0.5]}},
//!   {"op": "set", "block": "starting_locations", "index": 0,
//!    "set": {"position": [0, 0, 0.5], "facing": 1.57}},
//!   {"op": "remove", "block": "scenery", "index": 3}
//! ]}
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::layout::{self, BlockKind, FieldType};
use crate::{Command, Error, MapSource, Result, Session, Value};

/// Every placement block, with its script name.
pub const BLOCKS: [(&str, BlockKind); 11] = [
    ("scenery", BlockKind::Scenery),
    ("vehicles", BlockKind::Vehicles),
    ("equipment", BlockKind::Equipment),
    ("weapons", BlockKind::Weapons),
    ("machines", BlockKind::Machines),
    ("sound_scenery", BlockKind::SoundScenery),
    ("crates", BlockKind::Crates),
    ("starting_locations", BlockKind::StartingLocations),
    ("netgame_flags", BlockKind::NetgameFlags),
    ("netgame_equipment", BlockKind::NetgameEquipment),
    ("starting_equipment", BlockKind::StartingEquipment),
];

fn kind(name: &str) -> Result<BlockKind> {
    BLOCKS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, k)| *k)
        .ok_or_else(|| Error::NoField(format!("block {name} (one of {})", names())))
}

fn names() -> String {
    BLOCKS.map(|(n, _)| n).join(", ")
}

fn name(k: BlockKind) -> &'static str {
    BLOCKS.iter().find(|(_, b)| *b == k).expect("every kind").0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub ops: Vec<Op>,
    /// Let a block grow past its size in the source (appended to the
    /// metadata; refused by default, as in the editor).
    #[serde(default)]
    pub allow_growth: bool,
}

/// Blocks named in a `clear`: a list, or `"all"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Blocks {
    All(String),
    Some(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Remove every entry of these blocks (a block whose entries can't be
    /// removed is reported and left).
    Clear {
        blocks: Blocks,
    },
    /// Remove every entry but these (the kept ones are renumbered from 0;
    /// with more than one, their order may change).
    Keep {
        block: String,
        indices: Vec<usize>,
    },
    /// Add an entry: a copy of `like`, or a zeroed one with the palette's
    /// first item; then set fields.
    Add {
        block: String,
        #[serde(default)]
        like: Option<usize>,
        #[serde(default)]
        set: BTreeMap<String, serde_json::Value>,
    },
    Set {
        block: String,
        index: usize,
        set: BTreeMap<String, serde_json::Value>,
    },
    Remove {
        block: String,
        index: usize,
    },
}

/// What a script did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// One line per operation.
    pub done: Vec<String>,
    /// Entries per block after the script, non-empty blocks only.
    pub counts: BTreeMap<String, usize>,
    /// Bytes written, when saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

fn value(ty: FieldType, field: &str, v: &serde_json::Value) -> Result<Value> {
    let bad = |what: &'static str| Error::Value(layout_name(field), what);
    Ok(match ty {
        FieldType::Vec3 => {
            let a = v
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or(bad("three numbers"))?;
            let mut out = [0f32; 3];
            for (o, x) in out.iter_mut().zip(a) {
                *o = x.as_f64().ok_or(bad("three numbers"))? as f32;
            }
            Value::Vec3(out)
        }
        FieldType::F32 => Value::Float(v.as_f64().ok_or(bad("a number"))? as f32),
        FieldType::TagRef => return Err(bad("tag references are set through a palette index")),
        _ => Value::Int(v.as_i64().ok_or(bad("an integer"))?),
    })
}

/// A field name as `'static` (the layout's own), for messages.
fn layout_name(field: &str) -> &'static str {
    BLOCKS
        .iter()
        .flat_map(|(_, k)| layout::block(*k).fields.iter())
        .find(|f| f.name == field)
        .map_or("field", |f| f.name)
}

fn set_fields(
    s: &mut Session,
    k: BlockKind,
    index: usize,
    set: &BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    let def = layout::block(k);
    for (field, v) in set {
        let f = def
            .field(field)
            .ok_or_else(|| Error::NoField(field.clone()))?;
        s.apply(Command::set(k, index, f.name, value(f.ty, field, v)?))?;
    }
    Ok(())
}

fn len(s: &Session, k: BlockKind) -> usize {
    s.scenario().map_or(0, |sc| sc.block(k).len())
}

/// Runs `script` on the open map in `s`. Stops at the first refused
/// operation, saying which.
pub fn run(s: &mut Session, script: &Script) -> Result<Report> {
    s.set_allow_growth(script.allow_growth);
    let mut r = Report::default();
    for (i, op) in script.ops.iter().enumerate() {
        let line = apply(s, op).map_err(|e| Error::Package(format!("op {i}: {e}")))?;
        r.done.push(line);
    }
    for (n, k) in BLOCKS {
        let c = len(s, k);
        if c > 0 {
            r.counts.insert(n.into(), c);
        }
    }
    Ok(r)
}

fn apply(s: &mut Session, op: &Op) -> Result<String> {
    Ok(match op {
        Op::Clear { blocks } => {
            let kinds: Vec<BlockKind> = match blocks {
                Blocks::All(a) if a == "all" => BLOCKS.iter().map(|b| b.1).collect(),
                Blocks::All(other) => vec![kind(other)?],
                Blocks::Some(list) => list.iter().map(|n| kind(n)).collect::<Result<_>>()?,
            };
            let mut cleared = Vec::new();
            for k in kinds {
                let n = len(s, k);
                if n == 0 {
                    continue;
                }
                if let Some(why) = s.remove_refusal(k) {
                    cleared.push(format!("{} kept ({why})", name(k)));
                    continue;
                }
                for i in (0..n).rev() {
                    s.remove_entry(k, i)?;
                }
                cleared.push(format!("{} {n}", name(k)));
            }
            format!("clear: {}", cleared.join(", "))
        }
        Op::Keep { block, indices } => {
            let k = kind(block)?;
            let n = len(s, k);
            if let Some(&bad) = indices.iter().find(|&&i| i >= n) {
                return Err(Error::NoEntry(k, bad));
            }
            let keep: Vec<usize> = {
                let mut v = indices.clone();
                v.sort_unstable();
                v.dedup();
                v
            };
            let mut removed = 0;
            for i in (0..n).rev() {
                if !keep.contains(&i) {
                    s.remove_entry(k, i)?;
                    removed += 1;
                }
            }
            format!("keep {}: {} kept, {removed} removed", name(k), keep.len())
        }
        Op::Add { block, like, set } => {
            let k = kind(block)?;
            let i = s.add_entry(k, *like)?;
            set_fields(s, k, i, set)?;
            format!("add {} {i}", name(k))
        }
        Op::Set { block, index, set } => {
            let k = kind(block)?;
            set_fields(s, k, *index, set)?;
            format!(
                "set {} {index}: {}",
                name(k),
                set.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        }
        Op::Remove { block, index } => {
            let k = kind(block)?;
            s.remove_entry(k, *index)?;
            format!("remove {} {index}", name(k))
        }
    })
}

/// Opens `source`, runs `script`, saves to `out`.
pub fn build(source: &Path, script: &Script, out: &Path) -> Result<Report> {
    let mut s = Session::new();
    s.build_scenes = false;
    s.open(MapSource::file(source))?;
    let mut r = run(&mut s, script)?;
    r.bytes = Some(s.save_as(out)?);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scenario;
    use crate::fixture::fixture;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("scenario-script-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn script(j: serde_json::Value) -> Script {
        serde_json::from_value(j).unwrap()
    }

    #[test]
    fn a_template_then_one_weapon_saved_and_read_back() {
        let d = dir("template");
        let src = d.join("source.map");
        std::fs::write(&src, fixture()).unwrap();
        let out = d.join("one-weapon.map");
        let sc = script(serde_json::json!({"ops": [
            {"op": "keep", "block": "weapons", "indices": [1]},
            {"op": "clear", "blocks": ["scenery", "netgame_flags", "netgame_equipment"]},
            {"op": "keep", "block": "starting_locations", "indices": [0]},
            {"op": "set", "block": "starting_locations", "index": 0,
             "set": {"position": [0, 0, 0.5], "facing": 1.5}},
            {"op": "set", "block": "weapons", "index": 0, "set": {"position": [2, 0, 0.5]}},
        ]}));
        let r = build(&src, &sc, &out).unwrap();
        assert_eq!(r.done.len(), 5, "{r:?}");
        assert!(r.bytes.unwrap() > 0);
        let mut m = map::Map::open(&out).unwrap();
        m.validate_checksum().unwrap();
        let s = Scenario::read(&mut m).unwrap();
        assert_eq!(s.block(BlockKind::Weapons).len(), 1);
        assert_eq!(s.block(BlockKind::StartingLocations).len(), 1);
        assert_eq!(s.block(BlockKind::NetgameFlags).len(), 0);
        assert_eq!(
            s.get(BlockKind::StartingLocations, 0, "position").unwrap(),
            Value::Vec3([0.0, 0.0, 0.5])
        );
        assert_eq!(
            s.get(BlockKind::Weapons, 0, "position").unwrap(),
            Value::Vec3([2.0, 0.0, 0.5])
        );
        assert_eq!(r.counts.get("weapons"), Some(&1));
        assert!(!r.counts.contains_key("scenery"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn mistakes_name_the_operation() {
        let d = dir("mistakes");
        let src = d.join("source.map");
        std::fs::write(&src, fixture()).unwrap();
        let out = d.join("out.map");
        for (bad, says) in [
            (
                serde_json::json!({"ops": [{"op": "remove", "block": "tanks", "index": 0}]}),
                "op 0",
            ),
            (
                serde_json::json!({"ops": [{"op": "set", "block": "weapons", "index": 0, "set": {"position": [1, 2]}}]}),
                "three numbers",
            ),
            (
                serde_json::json!({"ops": [{"op": "set", "block": "weapons", "index": 0, "set": {"colour": 1}}]}),
                "colour",
            ),
            (
                serde_json::json!({"ops": [{"op": "keep", "block": "weapons", "indices": [7]}]}),
                "op 0",
            ),
        ] {
            let e = build(&src, &script(bad.clone()), &out)
                .unwrap_err()
                .to_string();
            assert!(e.contains(says), "{bad}: {e}");
        }
        assert!(
            serde_json::from_value::<Script>(serde_json::json!({"ops": [{"op": "fly"}]})).is_err()
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn clearing_everything_leaves_no_placements() {
        let d = dir("clear");
        let src = d.join("source.map");
        std::fs::write(&src, fixture()).unwrap();
        let mut s = Session::new();
        s.build_scenes = false;
        s.open(MapSource::file(&src)).unwrap();
        let r = run(
            &mut s,
            &script(serde_json::json!({"ops": [{"op": "clear", "blocks": "all"}]})),
        )
        .unwrap();
        assert!(r.done[0].starts_with("clear: "), "{r:?}");
        // Whatever the editor refuses to remove is reported, not dropped.
        for (n, c) in &r.counts {
            assert!(r.done[0].contains(&format!("{n} kept")), "{n} {c}: {r:?}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
