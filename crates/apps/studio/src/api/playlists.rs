//! `playlist.*`: the open playlist file (`multiplayer_hopper.dat`), edited
//! through [`PlaylistEdit`]s, the same edits as the playlist table's row
//! buttons and weights. Opening is `doc.open {"playlists": ...}`.

use agent_proto::RpcError;
use hopper_edit::model;
use live_proto::hopper::{HOPPERS, defaults};
use serde_json::json;
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::{Action, PlaylistEdit};

fn playlist(app: &App, p: &P) -> Result<usize, RpcError> {
    if !app.hoppers.is_open() {
        return Err(err(
            code::NO_DOCUMENT,
            "no playlist file is open: doc.open {\"playlists\": \"<path>|default|blank\"}",
        ));
    }
    let i = p.usize("playlist")?;
    if i >= HOPPERS {
        return Err(err(
            code::INVALID_PARAMS,
            format!("playlist: 0..{} , not {i}", HOPPERS - 1),
        ));
    }
    Ok(i)
}

fn run(app: &mut App, a: Action) -> Result<Vec<String>, RpcError> {
    app.perform(a, false).map_err(refused)
}

fn entries(app: &App, i: usize) -> usize {
    app.hoppers.hoppers().map_or(0, |h| h[i].records.len())
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::PLAYLIST_LIST => {
            let Some(hs) = app.hoppers.hoppers() else {
                return Err(err(
                    code::NO_DOCUMENT,
                    "no playlist file is open: doc.open {\"playlists\": ...}",
                ));
            };
            Ok(json!({
                "path": app.hoppers.path().map(|p| p.display().to_string()),
                "dirty": app.hoppers.is_dirty(),
                "playlists": hs.iter().enumerate().map(|(i, h)| json!({
                    "playlist": i, "title": h.title, "type": h.hopper_type, "entries": h.records.len(),
                    "unused": model::is_unused(h),
                })).collect::<Vec<_>>(),
                "templates": defaults::templates().iter().map(|t| json!({
                    "template": t.playlist_id, "title": t.title, "type": t.rules.hopper_type().raw(),
                })).collect::<Vec<_>>(),
                "errors": app.hoppers.errors(),
            }))
        }
        m::PLAYLIST_GET => {
            let i = playlist(app, p)?;
            let h = &app.hoppers.hoppers().expect("open")[i];
            let chances = model::chances(h);
            Ok(json!({
                "playlist": i,
                "title": h.title,
                "description": h.description,
                "type": h.hopper_type,
                "entries": h.records.iter().enumerate().map(|(j, r)| json!({
                    "entry": j,
                    "map": r.id,
                    "string": r.string,
                    "weight": r.dwords[0],
                    "chance": chances.get(j),
                    "variant": r.variant.as_ref().map(|v| json!({"name": v.name, "engine": v.engine, "team": v.team})),
                })).collect::<Vec<_>>(),
                "errors": model::hopper_errors(h),
            }))
        }
        m::PLAYLIST_SET => {
            let playlist = playlist(app, p)?;
            let mut edits = Vec::new();
            if let Some(t) = p.opt_str("title")? {
                edits.push(PlaylistEdit::Title {
                    playlist,
                    text: t.to_owned(),
                });
            }
            if let Some(t) = p.opt_str("description")? {
                edits.push(PlaylistEdit::Description {
                    playlist,
                    text: t.to_owned(),
                });
            }
            // The kind goes first, so a new playlist's text can be set in
            // the same call.
            if let Some(t) = p.opt_u64("template")? {
                edits.insert(
                    0,
                    PlaylistEdit::Kind {
                        playlist,
                        template: u32::try_from(t).unwrap_or(u32::MAX),
                    },
                );
            }
            if p.get("clear").is_some() && p.bool("clear")? {
                edits = vec![PlaylistEdit::Clear { playlist }];
            }
            if edits.is_empty() {
                return Err(err(
                    code::INVALID_PARAMS,
                    "playlist.set takes title, description, template (a playlist.list template) or clear",
                ));
            }
            for e in edits {
                run(app, Action::EditPlaylist(e))?;
            }
            let h = &app.hoppers.hoppers().expect("open")[playlist];
            Ok(json!({ "playlist": playlist, "title": h.title, "description": h.description }))
        }
        m::PLAYLIST_ENTRY => {
            let playlist = playlist(app, p)?;
            let entry = || p.usize("entry");
            let edit = match p.str("op")? {
                "add" => PlaylistEdit::Add { playlist },
                "duplicate" => PlaylistEdit::Duplicate {
                    playlist,
                    entry: entry()?,
                },
                "remove" => PlaylistEdit::Remove {
                    playlist,
                    entry: entry()?,
                },
                "move" => PlaylistEdit::Move {
                    playlist,
                    entry: entry()?,
                    to: p.usize("to")?,
                },
                "weight" => PlaylistEdit::Weight {
                    playlist,
                    entry: entry()?,
                    weight: u32::try_from(p.u64("weight")?).unwrap_or(u32::MAX),
                },
                other => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        format!("op: add, duplicate, remove, move or weight, not {other:?}"),
                    ));
                }
            };
            run(app, Action::EditPlaylist(edit))?;
            Ok(json!({ "entries": entries(app, playlist) }))
        }
        m::PLAYLIST_ENTRY_VARIANT => {
            let h = playlist(app, p)?;
            let e = p.usize("entry")?;
            if e >= entries(app, h) {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("playlist {h} has no entry {e}"),
                ));
            }
            app.ensure_variants();
            let v = app
                .variants
                .as_ref()
                .and_then(|e| e.current())
                .cloned()
                .ok_or_else(|| err(code::NO_DOCUMENT, "no variant is open in the variant tool"))?;
            run(app, Action::SetEntryVariant(h, e, Box::new(v.clone())))?;
            Ok(json!({ "variant": v.name() }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
