//! `project.*` and `edits.*` (docs/editor/agent-api.md, "Projects and edit
//! files"): the tag workspace's projects and edit files over the tree
//! (docs/editor/tag-workspace.md, "Projects", "Edits are data"),
//! registered in [`super::seams`]. Each call reads the files again, so an
//! edit file changed in an editor or by `h2 tags edits set` is what the
//! next call sees.
//!
//! The roots are the `tags_root` setting (the tree), the data root, and the
//! checkout Studio was built from (the packages `content/*/`,
//! `assets/tag-edits/`); a project is a package's `map` item. The kits a project can name are
//! `workspace::stage::project_kit::KITS`.
use std::time::Instant;

use serde_json::{Value, json};
use studio_proto::{code, method as m};
use workspace::edits;
use workspace::project::{self, ProjectError, Roots};
use workspace::project_build::{self, Tools};

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;

/// The projects provider.
#[derive(Default)]
pub struct Projects;

const METHODS: &[MethodDef] = &[
    MethodDef {
        name: m::PROJECT_LIST,
        summary: "Every project: each map item of the packages content/*/ in the checkout, then <data>/packages/*/, with its name, mode, base, package and item, or why the package or its map.toml does not read",
        params: "{}",
        result: r#"{"projects":"[{name,path,origin,package,item,mode,base,error}]"}"#,
        example: "null",
        planned: None,
    },
    MethodDef {
        name: m::PROJECT_READ,
        summary: "What a project (a map's name, or a package directory with one map item) resolves to: its layers top first (edits, authored, import, tree), its edit files and the tags they name, the authored tags, its kits with their fingerprints, where a fresh project's scenario comes from, and every problem that would stop its build. Nothing is written. `closure` walks a fresh project's closure for the lossy tags that refuse it",
        params: r#"{"project":"string","closure?":"boolean"}"#,
        result: r#"{"project":"object","file":"path","package":"string","item":"integer","origin":"git|local","layers":"[{kind,name,path}]","authored":"[string]","edit_files":"[string]","edited":"[string]","kits":"[{name,fingerprint,missing}]","scenario_from":"layers|kit|null","lossy":"[string]|null","problems":"[string]"}"#,
        example: r#"{"project":"stage"}"#,
        planned: None,
    },
    MethodDef {
        name: m::PROJECT_BUILD,
        summary: "Build a project from the tree, its layers (the authored layer generated under <data>/workspace/generated/), its edit files, its kits and the other items the map takes, and write its map package <data>/maps/<name>/ with receipt.json; onto adds the tags the base lacks; any problem stops it with its message (an edit's file and line, a lossy or missing tag by name)",
        params: r#"{"project":"string"}"#,
        result: r#"{"dir":"path","map_id":"string","file":"string","unchanged":"boolean","took_ms":"integer","receipt":"{key,project,package,item,mode,base,layers,authored,edit_files,edited,overrides,kits,closure,rebuilt,added,taken,file_size,memory_size,memory_max}"}"#,
        example: r#"{"project":"stage"}"#,
        planned: None,
    },
    MethodDef {
        name: m::EDITS_LIST,
        summary: "Every edit file: assets/tag-edits/*.yaml in the checkout, then <data>/workspace/edits/*.yaml, each with its entries, the tags it edits, and what does not parse or check",
        params: "{}",
        result: r#"{"files":"[{name,origin,path,entries,tags,problems}]"}"#,
        example: "null",
        planned: None,
    },
    MethodDef {
        name: m::EDITS_CHECK,
        summary: "Apply every edit file of a project over its layers once: each problem with its file, line and tag (a tag no layer holds, a dead path, a stale was, a from the field does not hold), and the fields two files set",
        params: r#"{"project":"string"}"#,
        result: r#"{"project":"string","files":"[string]","edited":"[string]","problems":"[{file,line,tag,why}]","overrides":"[{tag,path,earlier,later}]"}"#,
        example: r#"{"project":"stage"}"#,
        planned: None,
    },
    MethodDef {
        name: m::EDITS_SET,
        summary: "Write one set line into local edit file `file` (<data>/workspace/edits/<file>.yaml, made when missing), replacing the line for the same field of the file's entry for `tag`; `project` records the tag's hash under its layers as the entry's was",
        params: r#"{"file":"string","tag":"string (group:path)","path":"string","value":"any","project?":"string"}"#,
        result: r#"{"file":"string","path":"path","line":"integer","replaced":"boolean","was":"string|null"}"#,
        example: r#"{"file":"example","tag":"jpt!:objects\\weapons\\sample","path":"damage_lower_bound","value":12.5}"#,
        planned: None,
    },
];

impl Provider for Projects {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        let roots = app.project_roots();
        Some(match method {
            m::PROJECT_LIST => Ok(json!({ "projects": project::list(&roots) })),
            m::PROJECT_READ => read(&roots, &p),
            m::PROJECT_BUILD => build(&roots, &p),
            m::EDITS_LIST => {
                let files = edits::list(roots.checkout.as_deref(), &roots.workspace());
                Ok(json!({ "files": files }))
            }
            m::EDITS_CHECK => check(&roots, &p),
            m::EDITS_SET => set(&roots, &p),
            _ => return None,
        })
    }
}

fn tools() -> Tools<'static> {
    Tools {
        kits: workspace::stage::project_kit::KITS,
        keep: workspace::stage::project_kit::KEEP,
    }
}

fn load(roots: &Roots, p: &P) -> Result<project::ProjectFile, agent_proto::RpcError> {
    let which = p.str("project")?;
    project::load(roots, which).map_err(|e| err(code::INVALID_PARAMS, e.to_string()))
}

fn failed(e: ProjectError) -> agent_proto::RpcError {
    err(code::FAILED, e.to_string())
}

/// A result as JSON (a macro: this crate names no `serde` trait).
macro_rules! to_json {
    ($v:expr) => {
        serde_json::to_value($v).map_err(|e| err(code::FAILED, e.to_string()))
    };
}

fn read(roots: &Roots, p: &P) -> Reply {
    let file = load(roots, p)?;
    let closure = p.bool("closure")?;
    to_json!(&project_build::describe(roots, &file, &tools(), closure))
}

fn build(roots: &Roots, p: &P) -> Reply {
    let file = load(roots, p)?;
    let started = Instant::now();
    let built = project_build::build(roots, &file, &tools()).map_err(failed)?;
    let wrote = project_build::write(roots, &built).map_err(failed)?;
    Ok(json!({
        "dir": wrote.dir,
        "map_id": wrote.map_id,
        "file": wrote.file,
        "unchanged": wrote.unchanged,
        "took_ms": started.elapsed().as_millis() as u64,
        "receipt": built.receipt,
    }))
}

fn check(roots: &Roots, p: &P) -> Reply {
    let file = load(roots, p)?;
    to_json!(&project::check_edits(roots, &file).map_err(failed)?)
}

fn set(roots: &Roots, p: &P) -> Reply {
    let name = p.str("file")?;
    let tag = p.str("tag")?;
    let path = p.str("path")?;
    // `null` is a value here (a string ID or a reference cleared).
    let value =
        p.0.get("value")
            .ok_or_else(|| err(code::INVALID_PARAMS, "value is required"))?;
    let was = match p.opt_str("project")? {
        None => None,
        Some(which) => {
            let file = project::load(roots, which)
                .map_err(|e| err(code::INVALID_PARAMS, e.to_string()))?;
            let key = edits::parse_key(tag).map_err(|e| err(code::INVALID_PARAMS, e))?;
            Some(project::was_of(roots, &file, &key).map_err(failed)?)
        }
    };
    let wrote = edits::set_line(&roots.workspace(), name, tag, path, value, was)
        .map_err(|e| err(code::REFUSED, e))?;
    to_json!(&wrote)
}
