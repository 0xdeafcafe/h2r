//! Colour fields as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `tag.colours`, every colour row of a tag with its
//! form and hex, over the field panel's own rows (`fields::find`'s walk
//! of every element of every list, `fields::colour::row_json`). Colours
//! are written by `tag.set`, which takes `"#aarrggbb"` or `{rgba,
//! intensity?}` for a colour field (`app::tag_api::set_field`), the same
//! write the picker dialog makes. docs/editor/rich-editors.md,
//! "Colours"; docs/editor/agent-api.md.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::tag_fields;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::fields as panel;

/// The provider.
#[derive(Default)]
pub struct Colours;

const COLOURS: MethodDef = def(
    m::TAG_COLOURS,
    "Every colour row of a tag, in the field panel's order over every element of every list (to 12 lists deep and 5,000 list items, as tag.find walks): its tag.set path, label, the stored form (rgb, argb, rgb8, argb8, xrgb8 or hdr_rgb), `by` (`view` for a definition's view= word, `name` for a three-float field whose name says colour), the colour as `#aarrggbb` (alpha ff for a form without alpha; an hdr_rgb colour at intensity 1), its red, green, blue and alpha in 0 to 1, and an hdr_rgb colour's intensity. `truncated` says the walk stopped at the budget",
    r#"{"tag":"string|integer"}"#,
    r#"{"colours":"[{path, label, form, by, hex, rgba:[r,g,b,a], intensity?}]","truncated":"boolean"}"#,
    r#"{"tag":"weap"}"#,
);

fn colours(app: &mut App, p: &P) -> Reply {
    let tag = tag_fields::resolve(app, p)?;
    let (link, issues) = tag_fields::linked(app);
    let found = panel::find::find(app.shown_tags(), tag, "", link, issues, "", false, true)
        .map_err(|e| err(code::REFUSED, e))?;
    let colours: Vec<Value> = found
        .hits
        .iter()
        .filter_map(|h| {
            let c = h.row.get("colour").filter(|c| !c.is_null())?;
            let mut out = json!({
                "path": h.row["path"],
                "label": h.row["label"],
                "form": c["form"],
                "by": c["by"],
                "hex": c["hex"],
                "rgba": c["rgba"],
            });
            if let Some(i) = c.get("intensity") {
                out["intensity"] = i.clone();
            }
            Some(out)
        })
        .collect();
    Ok(json!({ "colours": colours, "truncated": found.truncated }))
}

impl Provider for Colours {
    fn methods(&self) -> Vec<MethodDef> {
        vec![COLOURS]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::TAG_COLOURS => colours(app, &p),
            _ => return None,
        })
    }
}

#[cfg(test)]
pub(super) mod tests;
