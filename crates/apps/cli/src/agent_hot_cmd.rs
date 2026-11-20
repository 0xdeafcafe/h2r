//! `h2 agent edit ...` and `h2 agent render ...`: short forms of the hot
//! iteration methods (`agent_proto::hot`, docs/graphics/hot-iteration.md).

use agent_proto::hot::method;
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub const USAGE: &str =
    "  edit tags [group] [name]   edit.tags: the loaded map's tags (names need the map file)
  edit get <tag> [path]      edit.get: a field, or an element's fields; tag is a datum
                         (0xe1230045) or a name, path like postprocess[0].colors[1].color
  edit set <tag> <path> <value>  edit.set: write it now, journalled (value is JSON or text)
  edit changes           edit.changes: what was changed, still live or not
  edit revert [id]       edit.revert: put one change (or all) back
  render overrides       render.overrides (hot WGSL overrides; bare `render` is render.stats): the override directory and each override's state
  render reload [dir]    render.reload: read the directory again (dir moves it)
  render programs [microcode|pretransformed]  render.programs: translated program ids
  render dump [id|all] [dir]  render.dump: write translated WGSL to start an override from
  render param [name [value|reset]]   render.param: list, read, set or reset a parameter
  render toggle [name [on|off|reset]] render.toggle: list, read, set or reset a toggle";

/// A JSON word as itself, anything else as a string.
fn value(word: &str) -> Value {
    serde_json::from_str(word).unwrap_or_else(|_| Value::String(word.to_owned()))
}

/// The method and params of an `edit` or `render` short form; `None` when
/// `words` is neither.
pub fn parse(words: &[String]) -> Option<Result<(String, Value)>> {
    let first = words.first()?.as_str();
    // A bare `render` is `render.stats` (agent_cmd's short form).
    if !(first == "edit" || first == "render" && words.len() > 1) {
        return None;
    }
    Some(parse_hot(first, &words[1..]))
}

fn parse_hot(domain: &str, w: &[String]) -> Result<(String, Value)> {
    let verb = w.first().map(String::as_str).unwrap_or("");
    let arg = |i: usize| w.get(i).map(String::as_str);
    let most = |n: usize| -> Result<()> {
        if w.len() > n {
            bail!("{domain} {verb}: unexpected {}", w[n]);
        }
        Ok(())
    };
    let (m, p) = match (domain, verb) {
        ("edit", "tags") => {
            most(3)?;
            let mut p = json!({});
            if let Some(g) = arg(1) {
                p["group"] = json!(g);
            }
            if let Some(n) = arg(2) {
                p["name"] = json!(n);
            }
            (method::EDIT_TAGS, p)
        }
        ("edit", "get") => {
            most(3)?;
            let Some(tag) = arg(1) else {
                bail!("edit get needs a tag (edit tags lists them)")
            };
            (
                method::EDIT_GET,
                json!({"tag": tag, "path": arg(2).unwrap_or("")}),
            )
        }
        ("edit", "set") => {
            most(4)?;
            let (Some(tag), Some(path), Some(v)) = (arg(1), arg(2), arg(3)) else {
                bail!("edit set needs a tag, a path and a value")
            };
            (
                method::EDIT_SET,
                json!({"tag": tag, "path": path, "value": value(v)}),
            )
        }
        ("edit", "changes") => {
            most(1)?;
            (method::EDIT_CHANGES, Value::Null)
        }
        ("edit", "revert") => {
            most(2)?;
            match arg(1) {
                None | Some("all") => (method::EDIT_REVERT, Value::Null),
                Some(id) => {
                    let id: u64 = id
                        .parse()
                        .map_err(|_| anyhow::anyhow!("edit revert: {id} is not a change id"))?;
                    (method::EDIT_REVERT, json!({"id": id}))
                }
            }
        }
        ("render", "overrides" | "status") => {
            most(1)?;
            (method::RENDER_OVERRIDES, Value::Null)
        }
        ("render", "reload") => {
            most(2)?;
            match arg(1) {
                None => (method::RENDER_RELOAD, Value::Null),
                Some(d) => {
                    // The game resolves it, maybe from another directory.
                    let d = std::path::absolute(d).unwrap_or_else(|_| d.into());
                    (
                        method::RENDER_RELOAD,
                        json!({"dir": d.display().to_string()}),
                    )
                }
            }
        }
        ("render", "programs") => {
            most(2)?;
            match arg(1) {
                None => (method::RENDER_PROGRAMS, Value::Null),
                Some(v) => (method::RENDER_PROGRAMS, json!({"vertex": v})),
            }
        }
        ("render", "dump") => {
            most(3)?;
            let mut p = json!({});
            if let Some(id) = arg(1) {
                p["program"] = json!(id);
            }
            if let Some(d) = arg(2) {
                let d = std::path::absolute(d).unwrap_or_else(|_| d.into());
                p["to"] = json!(d.display().to_string());
            }
            (method::RENDER_DUMP, p)
        }
        ("render", "param") => {
            most(3)?;
            let p = match (arg(1), arg(2)) {
                (None, _) => Value::Null,
                (Some(n), None) => json!({"name": n}),
                (Some(n), Some("reset")) => json!({"name": n, "value": null}),
                (Some(n), Some(v)) => {
                    let v: f64 = v
                        .parse()
                        .map_err(|_| anyhow::anyhow!("render param: {v} is not a number"))?;
                    json!({"name": n, "value": v})
                }
            };
            (method::RENDER_PARAM, p)
        }
        ("render", "toggle") => {
            most(3)?;
            let p = match (arg(1), arg(2)) {
                (None, _) => Value::Null,
                (Some(n), None) => json!({"name": n}),
                (Some(n), Some("on" | "true" | "1")) => json!({"name": n, "on": true}),
                (Some(n), Some("off" | "false" | "0")) => json!({"name": n, "on": false}),
                (Some(n), Some("reset")) => json!({"name": n, "on": null}),
                (Some(_), Some(v)) => bail!("render toggle: on, off or reset, not {v}"),
            };
            (method::RENDER_TOGGLE, p)
        }
        _ => bail!("{domain} {verb}: not a short form\n\n{USAGE}"),
    };
    Ok((m.into(), p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> (String, Value) {
        let w: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
        parse(&w).expect("a hot form").unwrap()
    }

    fn fails(s: &str) {
        let w: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
        assert!(parse(&w).expect("a hot form").is_err(), "{s}");
    }

    #[test]
    fn edit_forms() {
        assert_eq!(c("edit tags"), ("edit.tags".into(), json!({})));
        assert_eq!(
            c("edit tags shad metal"),
            (
                "edit.tags".into(),
                json!({"group": "shad", "name": "metal"})
            )
        );
        assert_eq!(
            c("edit get 0xe1230045 postprocess[0]"),
            (
                "edit.get".into(),
                json!({"tag": "0xe1230045", "path": "postprocess[0]"})
            )
        );
        assert_eq!(
            c("edit set 0xe1230045 parameters[0].specular_color [1,0.5,0]"),
            (
                "edit.set".into(),
                json!({"tag": "0xe1230045", "path": "parameters[0].specular_color", "value": [1, 0.5, 0]})
            )
        );
        assert_eq!(
            c("edit set 0xe1230045 postprocess[0].colors[0].color 0xff00ff00"),
            (
                "edit.set".into(),
                json!({"tag": "0xe1230045", "path": "postprocess[0].colors[0].color", "value": "0xff00ff00"})
            )
        );
        assert_eq!(c("edit changes"), ("edit.changes".into(), Value::Null));
        assert_eq!(c("edit revert"), ("edit.revert".into(), Value::Null));
        assert_eq!(c("edit revert 3"), ("edit.revert".into(), json!({"id": 3})));
        fails("edit revert x");
        fails("edit set 0xe1230045 path");
        fails("edit get");
        fails("edit nope");
    }

    #[test]
    fn render_forms() {
        assert_eq!(
            c("render overrides"),
            ("render.overrides".into(), Value::Null)
        );
        assert_eq!(c("render reload"), ("render.reload".into(), Value::Null));
        let (m, p) = c("render reload /tmp/o");
        assert_eq!(m, "render.reload");
        assert_eq!(p["dir"], "/tmp/o");
        assert_eq!(
            c("render programs microcode"),
            ("render.programs".into(), json!({"vertex": "microcode"}))
        );
        assert_eq!(c("render dump"), ("render.dump".into(), json!({})));
        assert_eq!(
            c("render dump 00000000000000ab"),
            ("render.dump".into(), json!({"program": "00000000000000ab"}))
        );
        assert_eq!(c("render param"), ("render.param".into(), Value::Null));
        assert_eq!(
            c("render param tint 0.5"),
            ("render.param".into(), json!({"name": "tint", "value": 0.5}))
        );
        assert_eq!(
            c("render param tint reset"),
            (
                "render.param".into(),
                json!({"name": "tint", "value": null})
            )
        );
        assert_eq!(
            c("render toggle overrides off"),
            (
                "render.toggle".into(),
                json!({"name": "overrides", "on": false})
            )
        );
        assert_eq!(
            c("render toggle overrides reset"),
            (
                "render.toggle".into(),
                json!({"name": "overrides", "on": null})
            )
        );
        fails("render toggle overrides maybe");
        fails("render param tint x");
        assert!(parse(&["status".to_string()]).is_none());
        assert!(parse(&["render".to_string()]).is_none(), "render.stats's form");
    }
}
