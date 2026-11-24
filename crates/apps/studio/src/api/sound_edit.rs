//! `sound.edit`, `sound.render` and `sound.permutation` ([`super::Provider`],
//! registered in [`super::seams`]): the waveform edits of a package sound,
//! over `app::sound_edit` and the `sound-edit` crate. The package's WAV is
//! read and never written; the edit list and the permutations are its
//! `<name>.sound.yaml`. docs/editor/agent-api.md, "Sound edits".

use serde_json::{Map, Value, json};
use sound_edit::{Edit, Permutation, PermutationPatch, SoundDoc};
use studio_proto::{code, method as m};

use super::sound_measure::levels_json;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::assets::CopyRefusal;
use crate::app::sound_edit::{self as files, Opened};

/// Most waveform columns `sound.render` returns.
const MAX_BINS: usize = 4096;

/// The provider.
#[derive(Default)]
pub struct SoundEdit;

const EDIT: MethodDef = def(
    m::SOUND_EDIT,
    "Change the edit list of a package sound (the `sound` item whose source WAV is `source`): an edit `op` of trim {start_ms, end_ms?}, fade_in {ms}, fade_out {ms}, gain {db}, normalise {lufs, ceiling_dbfs? default -1} or silence {start_ms, end_ms} is added (at `at`, the end by default), or `remove` {index}, `move` {from, to}, `undo` (drops the last edit), `clear`, or `list` (changes nothing). The list is the `edits` of `<source>.sound.yaml`, added to the package as a `data` item; the WAV is never rewritten and the build renders the list before it encodes. A list that does not render (a trim that leaves nothing, normalising silence) is refused by the place of the edit and nothing is written. `permutation` edits that permutation's file instead. Answers the list and the levels of the render",
    r#"{"package":"string","source":"string (the sound item's WAV, e.g. sounds/boing.wav)","op":"trim|fade_in|fade_out|gain|normalise|silence|remove|move|undo|clear|list","permutation?":"integer (a permutation of sound.permutation; the source's own by default)","at?":"integer (where an edit goes)","start_ms?":"number","end_ms?":"number","ms?":"number","db?":"number","lufs?":"number","ceiling_dbfs?":"number","index?":"integer","from?":"integer","to?":"integer"}"#,
    r#"{"package":"string","source":"string","sidecar":"string","permutation":"integer|null","op":"string","edits":"[{op,...}]","count":"integer","changed":"boolean","removed?":"{op,...}","render":"{peak_dbfs,rms_dbfs,lufs,max_momentary_lufs,duration_ms,clipped,frames,rate,channels}"}"#,
    r#"{"package":"mine","source":"sounds/clip.wav","op":"fade_out","ms":20}"#,
);
const RENDER: MethodDef = def(
    m::SOUND_RENDER,
    "A package sound's WAV through its edit list (the first `upto` edits, all by default), as numbers and, with `out`, a 16-bit PCM WAV file. The source WAV is not changed, and `out` may not be inside the package. Answers the levels before and after, the waveform peaks and what the file is",
    r#"{"package":"string","source":"string","permutation?":"integer","upto?":"integer (edits applied, of the list)","out?":"string (a WAV path outside the package)","bins?":"integer (waveform columns, default 64, at most 4096)"}"#,
    r#"{"package":"string","source":"string","permutation":"integer|null","edits":"integer","before":"{levels}","render":"{levels}","peaks":"[[low,high]]","out?":"string","bytes?":"integer"}"#,
    r#"{"package":"mine","source":"sounds/clip.wav","out":"$OUT/render.wav"}"#,
);
const PERMUTATION: MethodDef = def(
    m::SOUND_PERMUTATION,
    "A package sound's permutations: other WAV files of the package listed in `<source>.sound.yaml`, after the source's own, each with a weight, a gain and a skip fraction. `add` {file, weight?, gain_db?, skip_fraction?} (the file becomes a `sound` item of the package if it is not), `remove` {index}, `set` {index, weight?, gain_db?, skip_fraction?}, `move` {from, to}, or `list`. The rows carry each file's loudness after its edits, as sound.measure reads it. The build writes the source's own permutation; the list is the package's record of the rest",
    r#"{"package":"string","source":"string","op":"add|remove|set|move|list","file?":"string (a WAV of the package)","index?":"integer","from?":"integer","to?":"integer","weight?":"number (above 0)","gain_db?":"number","skip_fraction?":"number (0 to 1)"}"#,
    r#"{"package":"string","source":"string","sidecar":"string","count":"integer","source_row":"{file,levels}","permutations":"[{index,file,weight,gain_db,skip_fraction,edits,levels|error}]"}"#,
    r#"{"package":"mine","source":"sounds/clip.wav","op":"list"}"#,
);

impl Provider for SoundEdit {
    fn methods(&self) -> Vec<MethodDef> {
        vec![EDIT, RENDER, PERMUTATION]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::SOUND_EDIT => edit(app, &p, params),
            m::SOUND_RENDER => render(app, &p),
            m::SOUND_PERMUTATION => permutation(app, &p),
            _ => return None,
        })
    }
}

fn failed(e: CopyRefusal) -> agent_proto::RpcError {
    match e {
        CopyRefusal::Params(m) => err(code::INVALID_PARAMS, m),
        CopyRefusal::Refused(m) => err(code::REFUSED, m),
        CopyRefusal::Failed(m) => err(code::FAILED, m),
    }
}

fn refused(m: String) -> agent_proto::RpcError {
    err(code::REFUSED, m)
}

fn number(p: &P, k: &str) -> Result<Option<f64>, agent_proto::RpcError> {
    p.get(k)
        .map(|v| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("{k}: expected a number, not {v}")))
        })
        .transpose()
}

fn open(app: &App, p: &P) -> Result<Opened, agent_proto::RpcError> {
    files::open(&app.data_root(), p.str("package")?, p.str("source")?).map_err(failed)
}

fn render_json(w: &sound_edit::Wave) -> Value {
    levels_json(&w.levels())
}

/// The edit `op` names, from the params that belong to it. A param the
/// edit does not take is `invalid params` (the edit types refuse unknown
/// fields).
fn edit_of(op: &str, params: &Value) -> Result<Edit, agent_proto::RpcError> {
    let mut o = Map::new();
    o.insert("op".into(), json!(op));
    if let Some(all) = params.as_object() {
        for (k, v) in all {
            let own = matches!(
                k.as_str(),
                "package" | "source" | "op" | "permutation" | "at"
            );
            if !own && !v.is_null() {
                o.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::from_value(Value::Object(o)).map_err(|e| err(code::INVALID_PARAMS, format!("{op}: {e}")))
}

fn edit(app: &mut App, p: &P, params: &Value) -> Reply {
    let mut sound = open(app, p)?;
    let perm = p.opt_usize("permutation")?;
    let op = p.str("op")?;
    let before = sound.doc.clone();
    let mut removed = None;
    {
        let list = sound.doc.edits_mut(perm).map_err(|e| err(code::INVALID_PARAMS, e))?;
        let bad = |e: String| err(code::INVALID_PARAMS, e);
        match op {
            "list" => {}
            "remove" => removed = Some(sound_edit::remove(list, p.usize("index")?).map_err(bad)?),
            "move" => sound_edit::reorder(list, p.usize("from")?, p.usize("to")?).map_err(bad)?,
            "undo" => {
                removed = Some(
                    sound_edit::undo(list)
                        .ok_or_else(|| refused("nothing to undo: the list is empty".into()))?,
                );
            }
            "clear" => list.clear(),
            _ => {
                let e = edit_of(op, params)?;
                sound_edit::insert(list, e, p.opt_usize("at")?).map_err(bad)?;
            }
        }
    }
    let changed = sound.doc != before;
    // The list is rendered before anything is written: a list that does not
    // render leaves the package as it was.
    let wave = sound.rendered(perm, None).map_err(failed)?;
    if changed {
        sound.save().map_err(failed)?;
    }
    let list = sound.doc.edits_mut(perm).map_err(|e| err(code::INVALID_PARAMS, e))?.clone();
    Ok(json!({
        "package": p.str("package")?,
        "source": sound.source,
        "sidecar": sound.sidecar,
        "permutation": perm,
        "op": op,
        "edits": serde_json::to_value(&list).unwrap_or(Value::Null),
        "count": list.len(),
        "changed": changed,
        "removed": removed.map(|e| serde_json::to_value(e).unwrap_or(Value::Null)),
        "render": render_json(&wave),
    }))
}

fn render(app: &mut App, p: &P) -> Reply {
    let sound = open(app, p)?;
    let perm = p.opt_usize("permutation")?;
    let bins = p.opt_usize("bins")?.unwrap_or(64);
    if bins == 0 || bins > MAX_BINS {
        return Err(err(code::INVALID_PARAMS, format!("bins: 1 to {MAX_BINS}")));
    }
    let upto = p.opt_usize("upto")?;
    let before = sound.wave_of(perm).map_err(failed)?;
    let out = sound.rendered(perm, upto).map_err(failed)?;
    let applied = upto.unwrap_or(match perm {
        None => sound.doc.edits.len(),
        Some(i) => sound.doc.permutations[i].edits.len(),
    });
    let mut reply = json!({
        "package": p.str("package")?,
        "source": sound.source,
        "permutation": perm,
        "edits": applied,
        "before": render_json(&before),
        "render": render_json(&out),
        "peaks": out.peaks(bins),
    });
    if let Some(path) = p.opt_str("out")? {
        let target = files::render_target(&sound.dir, std::path::Path::new(path)).map_err(failed)?;
        let bytes = out.to_wav();
        std::fs::write(&target, &bytes)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", target.display())))?;
        reply["out"] = json!(target.display().to_string());
        reply["bytes"] = json!(bytes.len());
    }
    Ok(reply)
}

fn rows(sound: &Opened) -> Value {
    let row = |perm: Option<usize>| match sound.rendered(perm, None) {
        Ok(w) => json!({"levels": render_json(&w)}),
        Err(CopyRefusal::Params(e) | CopyRefusal::Refused(e) | CopyRefusal::Failed(e)) => {
            json!({"error": e})
        }
    };
    let mut perms = Vec::new();
    for (i, q) in sound.doc.permutations.iter().enumerate() {
        let mut r = row(Some(i));
        r["index"] = json!(i);
        r["file"] = json!(q.file);
        r["weight"] = json!(q.weight);
        r["gain_db"] = json!(q.gain_db);
        r["skip_fraction"] = json!(q.skip_fraction);
        r["edits"] = json!(q.edits.len());
        perms.push(r);
    }
    let mut own = row(None);
    own["file"] = json!(sound.source);
    json!({"source_row": own, "permutations": perms, "count": perms.len()})
}

fn permutation(app: &mut App, p: &P) -> Reply {
    let mut sound = open(app, p)?;
    let op = p.str("op")?;
    let before: SoundDoc = sound.doc.clone();
    let bad = |e: String| err(code::INVALID_PARAMS, e);
    match op {
        "list" => {}
        "add" => {
            let file = p.str("file")?;
            sound.check_file(file).map_err(failed)?;
            let mut q = Permutation::new(file);
            q.weight = number(p, "weight")?.unwrap_or(q.weight);
            q.gain_db = number(p, "gain_db")?.unwrap_or(q.gain_db);
            q.skip_fraction = number(p, "skip_fraction")?.unwrap_or(q.skip_fraction);
            sound.doc.add_permutation(q).map_err(bad)?;
        }
        "remove" => {
            sound.doc.remove_permutation(p.usize("index")?).map_err(bad)?;
        }
        "set" => {
            let patch = PermutationPatch {
                weight: number(p, "weight")?,
                gain_db: number(p, "gain_db")?,
                skip_fraction: number(p, "skip_fraction")?,
            };
            sound.doc.set_permutation(p.usize("index")?, &patch).map_err(bad)?;
        }
        "move" => sound
            .doc
            .reorder_permutations(p.usize("from")?, p.usize("to")?)
            .map_err(bad)?,
        other => {
            return Err(err(
                code::INVALID_PARAMS,
                format!("op {other:?}: add, remove, set, move or list"),
            ));
        }
    }
    if sound.doc != before {
        sound.save().map_err(failed)?;
    }
    let mut reply = rows(&sound);
    reply["package"] = json!(p.str("package")?);
    reply["source"] = json!(sound.source);
    reply["sidecar"] = json!(sound.sidecar);
    Ok(reply)
}

