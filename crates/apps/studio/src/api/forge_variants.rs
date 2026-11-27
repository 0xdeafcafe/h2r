//! `forge.*`: Forge map variants as documents and the Forge palette
//! (docs/editor/agent-api.md, "Forge variants and items";
//! `app/forge_variants.rs`, `app/assets/forge.rs`). A variant opens with
//! `shell.open {asset}`, the asset a `map-variant` item's id or a saved
//! file's `forge:<map>:<file>`; a field edit runs the window's own
//! command ([`Action::Forge`]) and a save writes the item the window's
//! Save writes (`App::save_forge_item`), so the document window and the
//! calls leave the same state.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::assets::{self, Query, Root};
use crate::app::forge_variants::{
    ForgeAction, ITEMS_KIND, parse_target, things, value_from_json,
};
use crate::app::{Action, Doc, Refused};

/// The provider.
#[derive(Default)]
pub struct ForgeVariants;

/// The default row count of a list.
const DEFAULT_LIMIT: usize = 200;

const LIST: MethodDef = def(
    m::FORGE_VARIANT_LIST,
    "The Forge map variants, of every map: the rows of the Browser's Forge variants kind (asset.list answers the same with that kind). First the `map-variant` items of the packages that are on (`source` package, `asset` the item's id `item:<package>:<n>`, `package` its package), then the variants a player saved in the game (`source` saved, `asset` `forge:<map>:<file>`) and the ones fetched from Live (`source` live, which do not open: `editable` false). `things` is the number of entries a variant adds or edits. Every word of `query` must match the name, the map, the file or the package",
    r#"{"query?":"string","limit?":"integer"}"#,
    r#"{"root":"forge","rows":"[{asset,root,kind,name,map,file,source,package,editable,badge,id,base,things,maps}]","total":"integer","limit":"integer","truncated":"boolean","maps":"[string]","kinds":"[string]","problems":"[string]"}"#,
    r#"{"query":"castle"}"#,
);

const READ: MethodDef = def(
    m::FORGE_VARIANT_READ,
    "A Forge map variant's things: each entry it adds (`target` added:<op>) and each entry of the base map it edits or removes (`target` base:<block>:<index>), with the fields the variant writes, each with its `type` (bool, int, float, vec2, vec3, vec4, text, tag) and the value its last write gives it (a tag is <group>:<path>, null for none). `item` is the tag the entry places, `piece` its block-out definition, `notes` what the variant says of it beside its fields (a property, an area, a shown flag). `package` is the package the variant is an item of, null for one saved in the game. With no `asset`, the open variant; the open document answers with its unsaved edits (`dirty`)",
    r#"{"asset?":"string (item:<package>:<n> or forge:<map>:<file>)"}"#,
    r#"{"asset":"string","map":"string","file":"string","name":"string","author":"string","description":"string","ops":"integer","things":"[{target,block,added,removed,item,piece,notes,fields:[{field,type,value}]}]","dirty":"boolean","open":"boolean","package":"string|null","selected":"string|null"}"#,
    "{}",
);

const SET: MethodDef = def(
    m::FORGE_VARIANT_SET,
    "Set a field the open Forge map variant writes of one entry: the value of the write that gives the field its value is replaced, so the edit adds no op. The value keeps the field's type (a number for an int or a float, a list of numbers for a vector, a string, <group>:<path> or null for a tag) and the document must still validate (`forge_doc::validate`); a field the variant does not write is refused, since Forge sets it. One undo step of the document (doc.undo). `asset` opens that variant first. Answers the document as forge.variant.read does",
    r#"{"asset?":"string (item:<package>:<n> or forge:<map>:<file>)","target":"string (added:<op> or base:<block>:<index>)","field":"string","value":"any"}"#,
    r#"{"asset":"string","things":"[{target,block,added,removed,item,piece,notes,fields}]","dirty":"boolean","open":"boolean"}"#,
    r#"{"target":"added:1","field":"position","value":[4,5,6]}"#,
);

const SAVE: MethodDef = def(
    m::FORGE_VARIANT_SAVE,
    "Save the open Forge map variant as a `map-variant` item of a package: `package`, else the edit target (NO_TARGET with neither). The item the variant was opened from is written again when it is that package's; a variant of another package, or one saved in the game, becomes a new item at map-variants/<map>/<name>.h2mapvariant, and the document is that item from then on (`asset`). The game lists a package's variants with the ones saved in it; the store's files under <data>/map-variants are never written",
    r#"{"package?":"string (a local package's id)"}"#,
    r#"{"asset":"string","saved":"boolean","dirty":"boolean","edit_target":"string","path":"string","log":"[string]"}"#,
    "{}",
);

const ITEMS: MethodDef = def(
    m::FORGE_ITEMS,
    "The Forge palette, the rows of the Browser's Forge items kind: every tag of the tags tree in a group Forge places (weap, vehi, eqip, scen, bloc, mach, ctrl and the spawner collections itmc and vehc), the tags the packages that are on add in those groups, and the kit's block-out set (bloc under forge\\blockout\\). `forge` on a row is `{menu, custom, source}`: `source` is disc, package or blockout and `custom` marks an item the disc does not have. `custom: true` keeps the custom items; `map` keeps what one map holds; every word of `query` must match the path, the group, the menu, the package or `custom`. asset.where_used on a row's asset lists the map variants that place it",
    r#"{"query?":"string","map?":"string","custom?":"boolean","limit?":"integer"}"#,
    r#"{"root":"tags","rows":"[{asset,root,kind,group,path,name,origin,new,badge,packages,maps,variants,used,forge:{menu,custom,source}}]","total":"integer","custom":"integer","limit":"integer","truncated":"boolean","maps":"[string]","kinds":"[string]","problems":"[string]"}"#,
    r#"{"custom":true}"#,
);

fn run(app: &mut App, action: Action) -> Result<Vec<String>, agent_proto::RpcError> {
    app.perform(action, false).map_err(|r| match r {
        Refused::Failed(e) => err(code::REFUSED, e),
        other => super::scenario::refused(other),
    })
}

fn list(app: &mut App, p: &P) -> Reply {
    app.forge_variant_list(
        p.opt_str("query")?.unwrap_or(""),
        p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT),
    )
    .map_err(|e| err(code::FAILED, e))
}

/// The open variant's id, or the refusal that says how to open one.
fn open_asset(app: &App) -> Result<String, agent_proto::RpcError> {
    app.forge_variants.open.as_ref().map(|o| o.asset()).ok_or_else(|| {
        err(
            code::REFUSED,
            "no Forge variant is open: shell.open {asset} opens one (forge.variant.list lists them)",
        )
    })
}

fn read(app: &mut App, p: &P) -> Reply {
    let asset = match p.opt_str("asset")? {
        Some(asset) => asset.to_owned(),
        None => open_asset(app)?,
    };
    if !asset.starts_with("forge:") && !asset.starts_with("item:") {
        return Err(err(
            code::INVALID_PARAMS,
            format!("{asset:?} is not a Forge variant (item:<package>:<n> or forge:<map>:<file>)"),
        ));
    }
    app.forge_variant_read(&asset).map_err(|e| err(code::REFUSED, e))
}

fn set(app: &mut App, p: &P) -> Reply {
    if let Some(asset) = p.opt_str("asset")? {
        let front = app.forge_variants.open.as_ref().map(|o| o.asset());
        if front.as_deref() != Some(asset) {
            run(app, Action::OpenAsset { asset: asset.to_owned(), map: None })?;
        }
    }
    open_asset(app)?;
    let target = p.str("target")?.to_owned();
    let field = p.str("field")?.to_owned();
    let end = parse_target(&target).map_err(|e| err(code::INVALID_PARAMS, e))?;
    // Null is a value here: a tag set to none.
    let raw = p
        .0
        .get("value")
        .ok_or_else(|| err(code::INVALID_PARAMS, "value is required"))?;
    // The value takes the type the field has now.
    let value = {
        let open = app.forge_variants.open.as_ref().expect("checked above");
        let all = things(&open.doc);
        let now = all
            .iter()
            .find(|t| t.target == end.identity())
            .and_then(|t| t.fields.iter().find(|(name, _)| *name == field))
            .map(|(_, value)| value.clone());
        match now {
            Some(now) => value_from_json(&now, raw).map_err(|e| err(code::INVALID_PARAMS, e))?,
            None => {
                return Err(err(
                    code::REFUSED,
                    format!(
                        "the variant writes no {field:?} of {target} (forge.variant.read lists the fields it writes)"
                    ),
                ));
            }
        }
    };
    run(app, Action::Forge(ForgeAction::Set { target, field, value }))?;
    let asset = open_asset(app)?;
    app.forge_variant_read(&asset).map_err(|e| err(code::FAILED, e))
}

fn save(app: &mut App, p: &P) -> Reply {
    open_asset(app)?;
    let target = super::packages::save_target(app, p.opt_str("package")?)?;
    let path = app.save_forge_item(&target).map_err(super::packages::refuse)?;
    app.sync_tabs();
    let asset = open_asset(app)?;
    Ok(json!({
        "asset": asset,
        "saved": true,
        "dirty": app.doc_dirty(Doc::Forge),
        "edit_target": target,
        "path": path,
        "log": [format!("saved {}", path.display())],
    }))
}

fn items(app: &mut App, p: &P) -> Reply {
    let custom = p.bool("custom")?;
    let mut text = p.opt_str("query")?.unwrap_or("").to_owned();
    if custom {
        // `custom` is a word every custom row matches.
        text.push_str(" custom");
    }
    let q = Query {
        root: Root::Tags,
        package: None,
        kind: Some(ITEMS_KIND.to_owned()),
        map: p.opt_str("map")?.map(str::to_owned),
        text,
        limit: p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT),
    };
    let set = assets::read_set(&app.data_root());
    assets::forge_items(&app.tags_root(), &set, &q).map_err(|e| err(code::REFUSED, e))
}

impl Provider for ForgeVariants {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST, READ, SET, SAVE, ITEMS]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        match method {
            m::FORGE_VARIANT_LIST => Some(list(app, &p)),
            m::FORGE_VARIANT_READ => Some(read(app, &p)),
            m::FORGE_VARIANT_SET => Some(set(app, &p)),
            m::FORGE_VARIANT_SAVE => Some(save(app, &p)),
            m::FORGE_ITEMS => Some(items(app, &p)),
            _ => None,
        }
    }
}
