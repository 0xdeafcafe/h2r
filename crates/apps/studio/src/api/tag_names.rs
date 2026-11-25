//! `tag.names`: how much of each tag group the definitions name, and
//! which fields are still unnamed, so a lane adding names to a group
//! (docs/editor/tag-editing.md, "Where names come from") sees what is left.

use map::FourCC;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::defs::fourcc_text;
use tags::registry::{Kind, Layout, Registry};

use super::{MethodDef, P, Reply, def, err};
use crate::App;

pub(super) const NAMES: MethodDef = def(
    m::TAG_NAMES,
    "How much of each tag group's definition is named: per group its kind title, the fields with a name (`named`), the typed or unknown fields without one (`unnamed`, padding not counted), those with a description (`described`), a `unit=` (`with_unit`) and an `applies=` (`with_effect`), blocks' elements counted once. With `group` (a group code) it also lists that group's unnamed fields, the largest first, with their paths from the root (`+0x14`, `parts[].+0x6`)",
    r#"{"group?":"string (a group code; default every group)"}"#,
    r#"{"groups":"[{group,title,named,unnamed,described,with_unit,with_effect,unnamed_fields?:[{path,type,size}]}]"}"#,
    r#"{"group":"weap"}"#,
);

#[derive(Default)]
struct Counts {
    named: u64,
    unnamed: u64,
    described: u64,
    with_unit: u64,
    with_effect: u64,
    list: Vec<(String, String, u32)>,
}

/// Counts `layout`'s fields and its blocks' elements, collecting the
/// unnamed fields when `list` is on. `prefix` is the path to the element.
fn walk(layout: &Layout, prefix: &str, list: bool, c: &mut Counts) {
    for f in &layout.fields {
        if matches!(f.kind, Kind::Pad { .. }) {
            continue;
        }
        let path = if f.named {
            format!("{prefix}{}", f.name)
        } else {
            format!("{prefix}+{:#x}", f.offset)
        };
        if f.named {
            c.named += 1;
        } else {
            c.unnamed += 1;
            if list {
                c.list.push((path.clone(), f.kind.label(), f.kind.width()));
            }
        }
        c.described += u64::from(f.doc.is_some());
        c.with_unit += u64::from(f.unit.is_some());
        c.with_effect += u64::from(f.applies.is_some());
        if let Kind::Block(child) = &f.kind {
            walk(child, &format!("{path}[]."), list, c);
        }
    }
}

/// One group's line.
fn group_line(group: FourCC, layout: &Layout, list: bool) -> Value {
    let mut c = Counts::default();
    walk(layout, "", list, &mut c);
    let title = match &layout.title {
        Some(tags::defs::Title::Text(t)) => t.clone(),
        _ => fourcc_text(group),
    };
    let mut line = json!({
        "group": fourcc_text(group),
        "title": title,
        "named": c.named,
        "unnamed": c.unnamed,
        "described": c.described,
        "with_unit": c.with_unit,
        "with_effect": c.with_effect,
    });
    if list {
        // Largest first; equal sizes keep the layout's order.
        c.list.sort_by_key(|(_, _, size)| std::cmp::Reverse(*size));
        line["unnamed_fields"] = c
            .list
            .into_iter()
            .map(|(path, ty, size)| json!({"path": path, "type": ty, "size": size}))
            .collect();
    }
    line
}

/// The lines of `registry`: every group, or the one named.
pub(super) fn report(registry: &Registry, group: Option<&str>) -> Result<Value, String> {
    let lines: Vec<Value> = match group {
        Some(g) => {
            let id = tags::defs::parse_fourcc(g).ok_or_else(|| format!("group: `{g}` is not a group code"))?;
            let layout = registry.group(id).ok_or_else(|| format!("no group `{g}`"))?;
            vec![group_line(id, layout, true)]
        }
        None => registry
            .groups
            .iter()
            .map(|(g, l)| group_line(*g, l, false))
            .collect(),
    };
    Ok(json!({ "groups": lines }))
}

pub(super) fn names(app: &mut App, p: &P) -> Reply {
    let group = p.opt_str("group")?;
    report(app.tags.registry(), group).map_err(|e| err(code::INVALID_PARAMS, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented group: three named fields (one described, with a unit
    /// and an effect), two unnamed (one inside a list).
    fn invented() -> Registry {
        let schema = tags::schema::Schema::parse(
            "group zzzz 0x10 align 4\n  0x8 block 0xc align 4\n    0x0 tagref\n",
        )
        .unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into(
            "test",
            "\
group zzzz 0x10 title=\"Invented\"
  0x0 f32 rate unit=seconds doc=\"How fast.\" applies=use -- E: test
  0x4 unknown 0x4
  0x8 block parts 0xc align 4
    0x0 tagref item groups=itmc
    0x8 unknown 0x4
",
        )
        .unwrap();
        Registry::build(&schema, &defs)
    }

    #[test]
    fn counts_and_lists_an_invented_group() {
        let r = invented();
        let all = report(&r, None).unwrap();
        let line = &all["groups"][0];
        assert_eq!(line["group"], "zzzz");
        assert_eq!(line["title"], "Invented");
        assert_eq!(
            (line["named"].as_u64(), line["unnamed"].as_u64()),
            (Some(3), Some(2))
        );
        assert_eq!(
            (
                line["described"].as_u64(),
                line["with_unit"].as_u64(),
                line["with_effect"].as_u64()
            ),
            (Some(1), Some(1), Some(1))
        );
        assert!(line.get("unnamed_fields").is_none(), "only with a group");
        let one = report(&r, Some("zzzz")).unwrap();
        assert_eq!(
            one["groups"][0]["unnamed_fields"],
            json!([
                {"path": "+0x4", "type": "? 4", "size": 4},
                {"path": "parts[].+0x8", "type": "? 4", "size": 4},
            ])
        );
        assert!(report(&r, Some("qqqq")).is_err());
        assert!(report(&r, Some("toolong")).is_err());
    }
}
