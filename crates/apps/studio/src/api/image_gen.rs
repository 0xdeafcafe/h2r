//! `image.*`: image generation over the agent API ([`super::Provider`],
//! registered in [`super::seams`]; docs/editor/agent-api.md, "Generating
//! images").
//!
//! `image.generate` checks its params, reads the slot and the references
//! from the open map, starts the job and answers with its id at once;
//! `image.result` reads the job. `image.apply`, `image.accept` and
//! `image.discard` run the commands the window's `Action::Image` runs,
//! which write through the Textures tool's `BitmapAction`s
//! (`app::image_gen`).

use std::path::PathBuf;

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::image_gen::{GenInput, RefInput};
use crate::imagegen::Alpha;

/// The `image.*` provider. Its jobs live on the app
/// (`App::imagegen`), shared with the window.
#[derive(Default)]
pub struct ImageGenApi;

const RESULT_SHAPE: &str = r#"{"id":"integer","state":"running|done|failed","why?":"string","error?":"missing|unauthenticated|quota|no_image|failed","generator":"string","elapsed_ms":"integer","prompt":"string","references":"[{path,origin,bitmap?,image?}]","slot":"{bitmap,name,image,width,height,format}|null","size":"[w,h]|null","dir":"string","from":"integer|null","raw?":"{path,width,height}","fitted?":"{path,width,height,format,alpha}","revised_prompt?":"string|null","out?":"string|null","applied":"boolean","accepted":"[path]|null","discarded":"boolean"}"#;

const METHODS: &[MethodDef] = &[
    def(
        m::IMAGE_GENERATE,
        "Generate a picture from a prompt and reference pictures (PNG paths, or {bitmap, image} of the open map, exported for the generator), for image `image` of `bitmap` (fitted to its size, format and alpha) or a `size`. `image: \"new\"` first adds an image to `bitmap` (bitmap.image.add, shaped like image `like`, default 0, or at `size`) and generates into it; `slot` names it. The references are the user's picks. Runs as a job: answers with its id at once; image.result has the outcome",
        r#"{"prompt":"string","references":"[path | {bitmap, image?}]","bitmap?":"string","image?":"integer|\"new\"","like?":"integer","size?":"[w,h]","alpha?":"auto|image|luma|opaque","out?":"string","from?":"integer"}"#,
        r#"{"id":"integer","state":"running","generator":"string","slot":"{bitmap,image}|null"}"#,
        r#"{"prompt":"a round blue badge","references":[],"bitmap":"floor"}"#,
    ),
    def(
        m::IMAGE_RESULT,
        "An image.generate job (the newest without `id`): its state, why it failed, the generator's picture, the picture fitted to the slot, whether it is applied, accepted or discarded",
        r#"{"id?":"integer"}"#,
        RESULT_SHAPE,
        r#"{"id":1}"#,
    ),
    def(
        m::IMAGE_APPLY,
        "Write a finished job's fitted picture into its slot as one Textures undo step, so the canvas, the UI Screens pictures, ui.render and the paired game show it in place",
        r#"{"id":"integer"}"#,
        r#"{"id":"integer","applied":"boolean","line":"string"}"#,
        r#"{"id":1}"#,
    ),
    def(
        m::IMAGE_ACCEPT,
        "Keep a job's picture in its slot (applying it first): it saves with the map like any texture edit. Writes provenance.json (prompt, references, generator, slot) in the job's directory, logs it, and with `save` copies the art and its provenance into that directory. A job made to a size keeps only the art and its provenance",
        r#"{"id":"integer","save?":"string"}"#,
        r#"{"id":"integer","accepted":"boolean","files":"[path]"}"#,
        r#"{"id":1}"#,
    ),
    def(
        m::IMAGE_DISCARD,
        "Take a job's picture out of its slot: its undo step while it is the bitmap's newest, else the slot's original pixels imported back",
        r#"{"id":"integer"}"#,
        r#"{"id":"integer","discarded":"boolean","line":"string"}"#,
        r#"{"id":1}"#,
    ),
];

impl Provider for ImageGenApi {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::IMAGE_GENERATE => generate(app, &p),
            m::IMAGE_RESULT => result(app, &p),
            m::IMAGE_APPLY => {
                let id = id(&p);
                id.and_then(|id| {
                    app.image_apply(id)
                        .map(|line| json!({"id": id, "applied": true, "line": line}))
                        .map_err(refused)
                })
            }
            m::IMAGE_ACCEPT => id(&p).and_then(|id| {
                let save = p.opt_str("save")?.map(PathBuf::from);
                app.image_accept(id, save.as_deref())
                    .map(|files| {
                        json!({
                            "id": id,
                            "accepted": true,
                            "files": files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
                        })
                    })
                    .map_err(refused)
            }),
            m::IMAGE_DISCARD => id(&p).and_then(|id| {
                app.image_discard(id)
                    .map(|line| json!({"id": id, "discarded": true, "line": line}))
                    .map_err(refused)
            }),
            _ => return None,
        })
    }
}

fn bad(e: impl Into<String>) -> RpcError {
    err(code::INVALID_PARAMS, e)
}

/// An app-side refusal: a missing map or bitmap is NO_DOCUMENT, an
/// unknown job or a bad name INVALID_PARAMS, the rest REFUSED.
fn refused(e: String) -> RpcError {
    if e.starts_with("no map") {
        err(code::NO_DOCUMENT, e)
    } else if e.starts_with("no image") || e.contains("not a bitmap") || e.contains("no bitmap") {
        bad(e)
    } else if e.contains("os error") {
        err(code::FAILED, e)
    } else {
        err(code::REFUSED, e)
    }
}

fn id(p: &P) -> Result<u64, RpcError> {
    p.opt_u64("id")?.ok_or_else(|| bad("id is required"))
}

fn reference(v: &Value) -> Result<RefInput, RpcError> {
    if let Some(s) = v.as_str() {
        return Ok(RefInput::File(PathBuf::from(s)));
    }
    let bitmap = v["bitmap"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("a reference is a PNG path or {bitmap, image?}"))?;
    let image = match &v["image"] {
        Value::Null => 0,
        x => x
            .as_u64()
            .ok_or_else(|| bad("a reference's image is a whole number"))? as usize,
    };
    Ok(RefInput::Map {
        bitmap: bitmap.to_owned(),
        image,
    })
}

fn generate(app: &mut App, p: &P) -> Reply {
    let prompt = p.str("prompt")?.to_owned();
    if prompt.trim().is_empty() {
        return Err(bad("prompt is empty"));
    }
    let references = p
        .get("references")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("references is a list (empty for none)"))?
        .iter()
        .map(reference)
        .collect::<Result<Vec<_>, _>>()?;
    if references.len() > 10 {
        return Err(bad("at most 10 references"));
    }
    // `image: "new"` adds an image to the bitmap first (shaped like image
    // `like`, or at `size`) and generates into it.
    let new = p.get("image").and_then(Value::as_str) == Some("new");
    let image = if new {
        None
    } else {
        match p.get("image") {
            Some(Value::String(s)) => {
                return Err(bad(format!("image is a whole number or \"new\", not {s:?}")));
            }
            _ => p.opt_u64("image")?.map(|n| n as usize),
        }
    };
    let like = p.opt_usize("like")?;
    if like.is_some() && !new {
        return Err(bad("like goes with image \"new\""));
    }
    let mut slot = match p.opt_str("bitmap")? {
        Some(b) => Some((b.to_owned(), image.unwrap_or(0))),
        None if image.is_some() || new => return Err(bad("image needs bitmap")),
        None => None,
    };
    let mut size = match p.get("size") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_array().map(Vec::as_slice) {
            Some([w, h]) => match (w.as_u64(), h.as_u64()) {
                (Some(w), Some(h)) if (1..=4096).contains(&w) && (1..=4096).contains(&h) => {
                    Some((w as u32, h as u32))
                }
                _ => return Err(bad("size is [w, h], each 1 to 4096")),
            },
            _ => return Err(bad("size is [w, h]")),
        },
    };
    if slot.is_some() && size.is_some() && !new {
        return Err(bad("give bitmap or size, not both: a slot has its own size"));
    }
    let alpha = match p.opt_str("alpha")? {
        None => Alpha::Auto,
        Some(s) => Alpha::parse(s).ok_or_else(|| bad("alpha is auto, image, luma or opaque"))?,
    };
    let out = p.opt_str("out")?.map(PathBuf::from);
    let from = p.opt_u64("from")?;
    if let Some(f) = from
        && app.image_jobs().job(f).is_none()
    {
        return Err(bad(format!("no image {f} to regenerate (image.result lists the last ones)")));
    }
    if new {
        if app.image_jobs().busy() {
            return Err(err(
                code::REFUSED,
                "an image is still being generated: wait for it (image.result)",
            ));
        }
        let (bitmap, _) = slot.take().expect("new needs bitmap, checked above");
        let index = app
            .image_new_slot(&bitmap, like.unwrap_or(0), size.take())
            .map_err(refused)?;
        slot = Some((bitmap, index));
    }
    let input = GenInput {
        prompt,
        references,
        slot,
        size,
        alpha,
        out,
        from,
    };
    let slot = input
        .slot
        .as_ref()
        .map(|(bitmap, image)| json!({"bitmap": bitmap, "image": image}));
    let id = app.image_generate(input).map_err(refused)?;
    Ok(json!({
        "id": id,
        "state": "running",
        "generator": app.imagegen.source_name(),
        "slot": slot,
    }))
}

fn result(app: &mut App, p: &P) -> Reply {
    let jobs = app.image_jobs();
    let job = match p.opt_u64("id")? {
        Some(id) => jobs
            .job(id)
            .ok_or_else(|| bad(format!("no image {id} (the last {} are kept)", crate::imagegen::KEEP)))?,
        None => jobs
            .last()
            .ok_or_else(|| err(code::REFUSED, "no image has been generated: image.generate starts one"))?,
    };
    Ok(job.report())
}
