//! Voices as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `voice.characters`, `voice.dataset.build`,
//! `voice.dataset.status`, `voice.clone.train` and `voice.clone.say`, over
//! `app::voices` and `app::announcer`, so the Voices window and the API
//! are one path. docs/editor/agent-api.md, "Voices";
//! docs/editor/voice-cloning.md.

use std::path::PathBuf;

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::voice::{flag, said_json, voice_error};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::announcer::SayParams;
use crate::app::voices::BuildParams;
use crate::voice::dataset::{Filter, Options};
use crate::voice::sidecar::Backend;
use crate::voice::{Engine, VoiceError};

/// The provider.
#[derive(Default)]
pub struct VoiceClone;

const CHARACTERS: MethodDef = def(
    m::VOICE_CHARACTERS,
    "The game characters Studio can clone (Master Chief, Arbiter, Cortana, Sergeant Johnson, the multiplayer announcer and more): each one's id, name, tag-path rules, how many dialogue sounds (snd! under sound\\dialog\\) of the source's maps the rules pick, its dataset and whether it is fine-tuned; and the speakers the maps have (the code at the end of a tag name, else the first folders under sound\\dialog\\) with a count and a sample tag, to check the rules against. `map` reads one map instead of every map of the source. Nothing is decoded",
    r#"{"map?":"string"}"#,
    r#"{"characters":"[{id,name,rules:{prefixes,folders,codes,paths},tags,dataset:{lines,usable,minutes,from_subtitle,from_asr,pending_asr,no_text,lufs,peak_dbfs}|null,trained}]","dialogue_tags":"integer","speakers":"[{kind:code|folder,key,tags,sample}]","maps_read":"integer","maps_failed":"[{map,why}]","why?":"string"}"#,
    "null",
);
const BUILD: MethodDef = def(
    m::VOICE_DATASET_BUILD,
    "Build a character's voice dataset from the user's own disc, on this machine: the dialogue sounds its rules pick (or `paths`, pieces of tag paths, for any speaker) in every map of the source (or `maps`), each tag once, each permutation decoded, made mono, its silence trimmed; clips shorter than min_ms or longer than max_ms, clipped, silent or radio-filtered (almost no energy under 250 Hz and over 4 kHz, or a path with `radio`) are left out unless keep_radio; the rest are written as 24 kHz mono 16-bit WAVs with a JSONL manifest (path, text, duration, source tag, map, permutation, where the text came from, the source's loudness) under <data root>/voices/<character>. A sound with one permutation whose name has a subtitle in the map's string lists takes it as its text; the rest wait for the local Whisper, which `asr` (default true) starts as a background job (voice.dataset.status). Nothing is uploaded",
    r#"{"character":"string (an id of voice.characters, or a new one with paths)","paths?":"[string]","maps?":"[string]","asr?":"boolean (default true)","min_ms?":"integer (default 1000)","max_ms?":"integer (default 15000)","keep_radio?":"boolean","limit?":"integer (most clips kept)","max_permutations?":"integer (per sound)"}"#,
    r#"{"character":"string","dir":"string","manifest":"string","rules":"object","tags":"integer","decoded":"integer","kept":"integer","dropped":"{why:count}","dropped_examples":"[{tag,why}]","maps_read":"integer","maps_failed":"[{map,why}]","subtitles":"integer","dataset":"{lines,usable,minutes,from_subtitle,from_asr,pending_asr,no_text,lufs,peak_dbfs}","asr_job":"integer|null"}"#,
    r#"{"character":"cortana","asr":false}"#,
);
const STATUS: MethodDef = def(
    m::VOICE_DATASET_STATUS,
    "The local voice model's state (installed, installing or missing, its backend and directory), the background jobs (install, transcribe, train: stage, done of total, the last lines of their log, the result or why they failed), and each character's dataset and fine-tune. `character` shows one; `job` one job",
    r#"{"character?":"string","job?":"integer"}"#,
    r#"{"sidecar":"{backend,backends,state:installed|installing|missing,dir,trains}","job":"{id,kind,character,backend,state:running|done|failed,stage,done,total,text,elapsed_ms,log,result?,why?}|null","jobs":"[job]","datasets":"[{character,dir,built,dataset?,build?,model:{trained,checkpoint?,record?}}]"}"#,
    "null",
);
const TRAIN: MethodDef = def(
    m::VOICE_CLONE_TRAIN,
    "Fine-tune a character's clone on its whole dataset (the lines with text), with MLX on this Mac, as a background job: installs the local model first when it is not. The weights are written under <data root>/voices/<character>/model and used by voice.clone.say from then on. Refused while another job runs, for a backend that does not train, and for a dataset under `min_minutes` of speech (default 10). voice.dataset.status follows it",
    r#"{"character":"string","steps?":"integer","min_minutes?":"number","backend?":"string (f5-mlx)"}"#,
    r#"{"job":"integer","state":"running","character":"string","backend":"string","lines":"integer","minutes":"number"}"#,
    r#"{"character":"cortana","min_minutes":0}"#,
);
const SAY: MethodDef = def(
    m::VOICE_CLONE_SAY,
    "Say a line in a game character's voice with the local clone (nothing leaves the machine): the model copies the delivery of a reference line from the character's dataset (`reference`, a clip id or source tag; default a subtitled line of about 7 s) and uses the character's fine-tune when there is one. The line then goes the way voice.say's do: cached by request, trimmed, measured and brought to the median loudness of the character's own lines on the disc (`sound.measure`'s meter), saved as 44.1 kHz mono 16-bit WAV under <data root>/voices/<character>, played unless `play` is false; with `clip` it is saved as clips/<clip>.wav and voice.commit copies it into the checkout. When the local model is not installed the install starts and the call is REFUSED with step install",
    r#"{"character":"string","text":"string (at most 500 characters)","reference?":"string","speed?":"number (0.5 to 2)","out?":"string","clip?":"string","play?":"boolean (default true)","target_lufs?":"number","fresh?":"boolean","backend?":"string (f5-mlx or qwen3-tts)"}"#,
    r#"{"engine":"clone","character":"string","backend":"string","model":"string","reference":"{id,text,source_tag,duration_ms}|null","text":"string","cached":"boolean","key":"string","file":"string","raw":"string","levels":"levels","raw_levels":"levels","trimmed_ms":"{head,tail}","gain_db":"number","limited":"boolean","target":"{lufs,from:given|character|fallback,lines,why?,lufs_spread,peak_dbfs_spread}","verdict":"{level,over_p90_db,peak_over_p90_db,text}","playback":"{playing,output?,duration_ms?,why?}","ready?":"{clip,repo_path,clips_entry,exists,commit}"}"#,
    r#"{"character":"cortana","text":"Hold on, Chief.","play":false}"#,
);

/// The error, with what to do next for a clone that is not ready.
fn clone_error(app: &App, e: VoiceError) -> RpcError {
    // What is missing: the model (its install has been started), or the
    // character's dataset or its text.
    let step = match &e {
        VoiceError::Setup(w) if w.contains("install") => Some("install"),
        VoiceError::Setup(_) => Some("dataset"),
        _ => None,
    };
    let mut r = voice_error(e);
    if let Some(step) = step {
        r.data = Some(json!({"step": step, "sidecar": app.voices.sidecar_json()}));
    }
    r
}

fn backend(app: &mut App, p: &P) -> Result<(), RpcError> {
    if let Some(b) = p.opt_str("backend")? {
        let b = Backend::parse(b).map_err(voice_error)?;
        app.voices.clone.backend = b;
    }
    Ok(())
}

fn strings(p: &P, k: &str) -> Result<Option<Vec<String>>, RpcError> {
    match p.get(k) {
        None => Ok(None),
        Some(v) => v
            .as_array()
            .and_then(|a| a.iter().map(|s| s.as_str().map(str::to_owned)).collect())
            .map(Some)
            .ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("{k}: expected a list of strings, not {v}"),
                )
            }),
    }
}

fn build(app: &mut App, p: &P) -> Reply {
    let mut filter = Filter::default();
    if let Some(v) = p.opt_u64("min_ms")? {
        filter.min_ms = v;
    }
    if let Some(v) = p.opt_u64("max_ms")? {
        filter.max_ms = v;
    }
    if filter.min_ms >= filter.max_ms {
        return Err(err(code::INVALID_PARAMS, "min_ms must be under max_ms"));
    }
    filter.keep_radio = flag(p, "keep_radio", false)?;
    let params = BuildParams {
        character: p.str("character")?.to_owned(),
        paths: strings(p, "paths")?,
        options: Options {
            maps: strings(p, "maps")?,
            filter,
            limit: p.opt_usize("limit")?,
            max_permutations: p.opt_usize("max_permutations")?,
        },
        asr: flag(p, "asr", true)?,
    };
    app.voice_dataset_build(params)
        .map_err(|e| clone_error(app, e))
}

fn status(app: &mut App, p: &P) -> Reply {
    if let Some(id) = p.opt_u64("job")? {
        let job = app
            .voice_job(id)
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("no job {id}")))?;
        let mut v = app
            .voice_dataset_status(p.opt_str("character")?)
            .map_err(voice_error)?;
        v["job"] = job;
        return Ok(v);
    }
    app.voice_dataset_status(p.opt_str("character")?)
        .map_err(voice_error)
}

fn train(app: &mut App, p: &P) -> Reply {
    backend(app, p)?;
    let min = match p.get("min_minutes") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .ok_or_else(|| {
                    err(
                        code::INVALID_PARAMS,
                        format!("min_minutes: a number of 0 or more, not {v}"),
                    )
                })?,
        ),
    };
    app.voice_clone_train(p.str("character")?, p.opt_u64("steps")?, min)
        .map_err(|e| clone_error(app, e))
}

fn say(app: &mut App, p: &P) -> Reply {
    backend(app, p)?;
    let target_lufs = match p.get("target_lufs") {
        None => None,
        Some(v) => Some(v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("target_lufs: expected a number, not {v}"),
            )
        })?),
    };
    let character = p.str("character")?.to_owned();
    let params = SayParams {
        text: p.str("text")?.to_owned(),
        voice: Some(character.clone()),
        model: None,
        speed: p.opt_f32("speed")?,
        out: p.opt_str("out")?.map(PathBuf::from),
        clip: p.opt_str("clip")?.map(str::to_owned),
        play: flag(p, "play", true)?,
        target_lufs,
        fresh: flag(p, "fresh", false)?,
        engine: Engine::Clone,
        reference: p.opt_str("reference")?.map(str::to_owned),
    };
    let said = app.voice_say(params).map_err(|e| clone_error(app, e))?;
    let mut v = said_json(&said);
    // The reference the model was given: the last one when it was asked
    // now, the one named when the line came from the cache.
    let reference = if said.cached {
        None
    } else {
        app.voices.clone.last_reference.as_ref()
    };
    v["engine"] = json!("clone");
    v["character"] = json!(character);
    v["backend"] = json!(app.voices.backend().id());
    v["model"] = json!(app.voices.clone.model_tag(&character));
    v["reference"] = match reference {
        Some(l) => json!({
            "id": l.id,
            "text": l.text,
            "source_tag": l.source_tag,
            "duration_ms": l.duration_ms,
        }),
        None => Value::Null,
    };
    Ok(v)
}

impl Provider for VoiceClone {
    fn methods(&self) -> Vec<MethodDef> {
        vec![CHARACTERS, BUILD, STATUS, TRAIN, SAY]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::VOICE_CHARACTERS => p.opt_str("map").map(|map| app.voice_characters(map)),
            m::VOICE_DATASET_BUILD => build(app, &p),
            m::VOICE_DATASET_STATUS => status(app, &p),
            m::VOICE_CLONE_TRAIN => train(app, &p),
            m::VOICE_CLONE_SAY => say(app, &p),
            _ => return None,
        })
    }
}

/// For a test: the invented maps as the dataset's source, the fake
/// sidecar (`installed` or not), and with `build` Cortana's dataset built
/// (no transcription).
#[cfg(test)]
pub(crate) fn test_setup(
    app: &mut App,
    installed: bool,
    build: bool,
) -> crate::voice::fake::FakeLauncher {
    use crate::voice::fake::{FakeLauncher, FakeLines};
    let fake = if installed {
        FakeLauncher::ready()
    } else {
        FakeLauncher::default()
    };
    app.voices.set_launcher(std::sync::Arc::new(fake.clone()));
    app.voices.lines = Some(Box::new(FakeLines::sample()));
    app.audition.headless = true;
    if build {
        app.voice_dataset_build(BuildParams {
            character: "cortana".into(),
            asr: false,
            ..BuildParams::default()
        })
        .expect("the invented dataset builds");
    }
    fake
}

#[cfg(test)]
mod tests;
