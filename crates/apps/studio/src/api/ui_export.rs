//! `ui.export_edits`: the open map's retail screen edits as data
//! (docs/game/ui-in-maps.md, "Retail edits as data"), registered in
//! [`super::seams`].
//!
//! A retail screen's edits in the editor (a move, a block hidden or
//! copied, a string ID named) are writes in the map's tag document, saved
//! with the user's map. This reads each screen tag's panes back twice,
//! as loaded and as edited, and writes the edits between them as an edit
//! file (`boot::ui_kit::retail_edits`, `diff_pane`): our own numbers and
//! names, which the game applies at boot. So a concept made in the editor
//! becomes a file under `assets/ui/retail-edits/`.

use std::path::PathBuf;

use boot::ui_kit::retail_edits::{self, BlockKind, PaneBytes};
use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::registry::Kind;

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::tag_api;

/// The export provider.
#[derive(Default)]
pub struct UiExport;

const METHODS: &[MethodDef] = &[MethodDef {
    name: m::UI_EXPORT_EDITS,
    summary: "Write the open map's retail screen edits (moves, hidden and copied blocks, string IDs set by name) as retail edit files, one per edited screen pane, into the directory path (assets/ui/retail-edits in a checkout ships them); screen limits it to one tag. What the format cannot hold is listed in skipped",
    params: r#"{"path":"string","screen?":"string"}"#,
    result: r#"{"map":"string","written":"[{path,screen,pane,edits}]","skipped":"[string]"}"#,
    example: r#"{"path":"$OUT/retail-edits"}"#,
    planned: None,
}];

impl Provider for UiExport {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if method != m::UI_EXPORT_EDITS {
            return None;
        }
        Some(export(app, &P(params)))
    }
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The `(count, address)` pair's address of the block at `path`.
fn pair_of(app: &App, tag: TagId, path: &str) -> Result<u32, String> {
    let (at, _, f) = tag_api::resolve(&app.tags, tag, path)?;
    match &f.kind {
        Kind::Block(_) => Ok(at + f.offset),
        _ => Err(format!("{path} is not a block")),
    }
}

/// How many panes screen tag `tag` has now.
fn panes(app: &App, tag: TagId) -> Result<usize, String> {
    let pair = pair_of(app, tag, "panes")?;
    let (n, _) = app.tags.pair(tag, pair).ok_or("the panes are unreadable")?;
    Ok(n as usize)
}

/// Pane `pane` of screen tag `tag`, as the map was loaded and as the
/// document has it now.
fn pane_bytes(app: &App, tag: TagId, pane: usize) -> Result<(PaneBytes, PaneBytes), String> {
    let (at, _, f) = tag_api::resolve(&app.tags, tag, "header")?;
    let header = at + f.offset;
    let mut source = PaneBytes {
        header: le32(app.tags.read_source(tag, header, 4).ok_or("the header is unreadable")?),
        ..PaneBytes::default()
    };
    let mut now = PaneBytes {
        header: le32(&app.tags.read(tag, header, 4).ok_or("the header is unreadable")?),
        ..PaneBytes::default()
    };
    for kind in BlockKind::ALL {
        let path = format!("panes[{pane}].{}", kind.field_name());
        let pair = pair_of(app, tag, &path)?;
        let size = kind.size() as u32;
        let unreadable = || format!("{path} is unreadable");
        let (n, start) = app.tags.pair(tag, pair).ok_or_else(unreadable)?;
        if n > 0 {
            let bytes = app.tags.read(tag, start, n * size).ok_or_else(unreadable)?;
            now.of_mut(kind).extend_from_slice(&bytes);
        }
        // The block as loaded: the pair's own bytes in the loaded map. A
        // block the edits added to a pane the map had is past it.
        if let Some(p) = app.tags.read_source(tag, pair, 8) {
            let (n, start) = (le32(p), le32(&p[4..]));
            if n > 0 {
                let bytes = app
                    .tags
                    .read_source(tag, start, n * size)
                    .ok_or_else(unreadable)?;
                source.of_mut(kind).extend_from_slice(bytes);
            }
        }
    }
    Ok((source, now))
}

fn export(app: &mut App, p: &P) -> Reply {
    let dir = PathBuf::from(p.str("path")?);
    let only = p.opt_str("screen")?.map(str::to_owned);
    app.sync_tags();
    if !app.tags.is_open() {
        return Err(err(
            code::NO_DOCUMENT,
            "no map's tags are open: open the map whose screens were edited (Maps)",
        ));
    }
    let map = app
        .loaded_map()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let screens: Vec<(TagId, String)> = app
        .tags
        .tags()
        .iter()
        .filter(|t| t.group == tags::ui::SCREEN)
        .filter(|t| only.as_ref().is_none_or(|s| t.name.eq_ignore_ascii_case(s)))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    if let Some(s) = &only
        && screens.is_empty()
    {
        return Err(err(
            code::INVALID_PARAMS,
            format!("screen: the open map has no screen tag {s}"),
        ));
    }
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for (id, name) in screens {
        let n = match panes(app, id) {
            Ok(n) => n,
            Err(e) => {
                skipped.push(format!("{name}: {e}"));
                continue;
            }
        };
        for pane in 0..n {
            let (source, now) = match pane_bytes(app, id, pane) {
                Ok(b) => b,
                Err(e) => {
                    skipped.push(format!("{name} pane {pane}: {e}"));
                    continue;
                }
            };
            if source == now {
                continue;
            }
            let string = |v: u32| app.tags.string(v).map(str::to_owned);
            let d = retail_edits::diff_pane(&name, &map, pane, &source, &now, &string);
            skipped.extend(d.skipped.iter().map(|s| format!("{name} pane {pane}: {s}")));
            let Some(file) = d.file else { continue };
            if let Err(e) = file.check() {
                skipped.push(format!("{name} pane {pane}: not written: {e}"));
                continue;
            }
            let path = dir.join(retail_edits::file_name(&name, pane));
            let text = retail_edits::to_yaml(
                &file,
                &format!(
                    "{name}, pane {pane}: exported by the editor (ui.export_edits) from {map}.map.\n\
                     Applied to the user's map at boot (docs/game/ui-in-maps.md, \"Retail edits as data\")."
                ),
            );
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&path, text))
                .map_err(|e| err(code::FAILED, format!("{}: {e}", path.display())))?;
            written.push(json!({
                "path": path.display().to_string(),
                "screen": name,
                "pane": pane,
                "edits": file.edits.len(),
            }));
        }
    }
    Ok(json!({ "map": map, "written": written, "skipped": skipped }))
}
