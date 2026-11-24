//! Save authored animation source into a local package.

use serde_json::json;
use studio_proto::{code, method as m};

use super::{MethodDef, P, Reply, def, err};
use crate::App;
use crate::app::anim::AnimationPlacement;
use crate::app::assets::CopyRefusal;

pub(super) const SAVE: MethodDef = def(
    m::ANIM_SAVE,
    "Save an animation document as a map-scoped animation item of a local package. A document opened on a source clip replaces that clip. A new or duplicated document needs an action",
    r#"{"doc":"integer","package":"string (a local package id)","mode?":"string (default any for a new clip)","class?":"string (default any for a new clip)","weapon_type?":"string (default any for a new clip)","action?":"string (required for a new or duplicated clip)"}"#,
    r#"{"doc":"integer","package":"string","tag":"string (tag:jmad:<path>)","map":"string","source":"string (relative to package)","item":"integer"}"#,
    r#"{"doc":1,"package":"mine","action":"wave"}"#,
);

pub(super) fn save(app: &mut App, p: &P) -> Reply {
    let doc = p.u64("doc")?;
    let placement = AnimationPlacement {
        mode: p.opt_str("mode")?,
        class: p.opt_str("class")?,
        weapon_type: p.opt_str("weapon_type")?,
        action: p.opt_str("action")?,
    };
    let saved = app
        .anim_save(doc, p.str("package")?, placement)
        .map_err(|e| match e {
            CopyRefusal::Params(m) => err(code::INVALID_PARAMS, m),
            CopyRefusal::Refused(m) => err(code::REFUSED, m),
            CopyRefusal::Failed(m) => err(code::FAILED, m),
        })?;
    Ok(json!({
        "doc": doc,
        "package": saved.package,
        "tag": saved.tag,
        "map": saved.map,
        "source": saved.source,
        "item": saved.item,
    }))
}
