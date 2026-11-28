//! The `model` builder: a look of the Pogo Stick as the tags a map holds a
//! model by (docs/editor/studio-redesign.md, "Contents").
//!
//! The source is our primitive YAML (`content_kit::source`: nodes, markers and
//! lathes, sweeps and boxes), and the file its `atlas` names, `<atlas>.png`
//! beside it, is the paint. The package names both (the atlas as a `data`
//! item), so the package's identity covers both. The item's `tag` is the
//! look's path; the builder adds, all at that path:
//!
//! - `bitm`: the atlas as a compressed mip chain;
//! - `shad`: written on a shader template the map has (`h1convert::shad`),
//!   with the atlas as its base map;
//! - `mode`: the mesh;
//! - `hlmt`: the render model, the shared physics model and one of the
//!   map's global materials.
//!
//! The look's physics model (`phmo`) and an empty weapon HUD (`nhdt`) are
//! shared by every look of the package: they are named `pogo` in the folder
//! above the look's folder (`<folder>\unsc\pogo_unsc` gives `<folder>\pogo`)
//! and are added by the first model item that needs them. The weapon is a
//! `tag` item of its own.
//!
//! What the map lends (the shader template, the material) is read from it
//! once by [`crate::build_map`]; nothing is copied from a retail tag.
use content_kit::kit::{self, Models, Shared};
use content_kit::source::{self, Source};
use content_kit::texture::{self, Rgba};

use map::FourCC;

use crate::build::{Cx, Failure};
use crate::builders::Job;

/// The folder shared tags are named in, and their name, for a look at
/// `path` (`a\b\c` gives `a\pogo`).
pub fn shared_path(path: &str) -> Result<String, String> {
    let segs: Vec<&str> = path.split('\\').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [folder @ .., _, _] if !folder.is_empty() => Ok(format!("{}\\pogo", folder.join("\\"))),
        _ => Err(format!(
            "a model's tag {path:?} is <folder>\\<look>\\<name>: the shared tags go in <folder>"
        )),
    }
}

/// The package file the source's atlas is: `<atlas>.png` beside the source.
fn atlas_file(item_source: &str, atlas: &str) -> Result<String, String> {
    if atlas.is_empty() || atlas.contains(['/', '\\']) || atlas.contains("..") {
        return Err(format!("atlas {atlas:?} is not a file name"));
    }
    Ok(match item_source.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{atlas}.png"),
        None => format!("{atlas}.png"),
    })
}

struct Parts {
    path: String,
    source: Source,
    atlas: Rgba,
}

fn parts(job: &Job<'_>) -> Result<Parts, Failure> {
    let path = job
        .item
        .tag
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or("a model item needs `tag`, the path of the model it adds")?
        .replace('/', "\\");
    shared_path(&path)?;
    let text = std::str::from_utf8(job.source).map_err(|e| format!("not UTF-8 text: {e}"))?;
    let source = source::parse(text).map_err(|e| Failure {
        why: format!("{e:#}"),
        line: None,
    })?;
    let from = job.item.source.as_deref().unwrap_or("");
    let file = atlas_file(from, &source.atlas)?;
    let png = job.files.get(&file).ok_or_else(|| {
        format!("the model's atlas {file:?} is not in the package (name it as a `data` item)")
    })?;
    let atlas = texture::decode(png).map_err(|e| format!("{file}: {e:#}"))?;
    Ok(Parts {
        path,
        source,
        atlas,
    })
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    let p = parts(job)?;
    content_kit::mesh::build(&p.source).map_err(|e| format!("{}: {e:#}", p.source.name))?;
    let (w, h) = (u32::from(source::atlas_size().0), u32::from(source::atlas_size().1));
    if (p.atlas.width, p.atlas.height) != (w, h) {
        return Err(format!(
            "the atlas is {}x{}, want {w}x{h}",
            p.atlas.width, p.atlas.height
        )
        .into());
    }
    Ok(())
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let Parts {
        path,
        source,
        atlas,
    } = parts(job)?;
    let shading = cx.shading()?;
    let shared_at = shared_path(&path)?;
    for group in [b"bitm", b"shad", b"mode", b"hlmt"] {
        cx.check_free(&Cx::key(FourCC(*group), &path))?;
    }
    // The map's index gains room for the tags (a no-op after the first).
    kit::make_index_room(cx.set);
    let fresh = [b"phmo", b"nhdt"].map(|g| cx.find(&Cx::key(FourCC(*g), &shared_at)).is_none());
    let shared: Shared = kit::install_shared_at(cx.set, &shared_at, &shading.material.0, true)
        .map_err(|e| format!("{e:#}"))?;
    for ((group, id), fresh) in [(b"phmo", shared.physics), (b"nhdt", shared.hud)]
        .into_iter()
        .zip(fresh)
    {
        if fresh {
            cx.note_added(&Cx::key(FourCC(*group), &shared_at), id);
        }
    }
    let models: Models = kit::install_model(cx.set, shading, shared, &path, &source, &atlas)
        .map_err(|e| format!("{path}: {e:#}"))?;
    for (group, id) in [
        (b"bitm", models.bitmap),
        (b"shad", models.shader),
        (b"mode", models.render),
        (b"hlmt", models.model),
    ] {
        cx.note_added(&Cx::key(FourCC(*group), &path), id);
    }
    Ok(())
}
