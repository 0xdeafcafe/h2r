//! The Announcer as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `voice.say`, `voice.list` and `voice.commit`, over
//! `app::announcer`, so the Announcer panel and the API are one path.
//! docs/editor/agent-api.md, "Announcer"; docs/editor/announcer.md.

use std::path::{Path, PathBuf};

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::sound_measure::levels_json;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::announcer::{Playback, Ready, SayParams, Said};
use crate::voice::{VoiceError, key};

/// The provider.
#[derive(Default)]
pub struct Voice;

const SAY: MethodDef = def(
    m::VOICE_SAY,
    "Say a line in an announcer voice (Fish Audio; default voice 86126c4c4dad4911979556b4569802cd) and make it a clip: the same text, voice and settings reuse the earlier file and never call the service again (`fresh` asks again); the silence at both ends is trimmed; the line is measured against the open map's retail announcer lines (`sound.measure`'s reference; our announcer clips, then -14.5 LUFS, without a map) and brought to their median loudness with one gain, its sample peak held under -1 dBFS; it is saved as 44.1 kHz mono 16-bit PCM WAV, the format of art/announcer/*.wav, under <data root>/announcer, never in the checkout; and it plays through the Sound tool's output unless `play` is false. With `clip` (a medal or cue id such as superbounce) it is saved as clips/<clip>.wav and the reply says how to add it. The key is read from the macOS Keychain (service fish-audio) only when the service is called; without it the error is REFUSED with the setup command",
    r#"{"text":"string (at most 500 characters)","voice?":"string (a Fish Audio model id)","model?":"string (a Fish Audio TTS model, default the service's)","speed?":"number (0.5 to 2, default 1)","out?":"string (a .wav path; relative to <data root>/announcer)","clip?":"string (lowercase letters, digits and -)","play?":"boolean (default true)","target_lufs?":"number (-60 to 0; default the retail announcer's median)","fresh?":"boolean"}"#,
    r#"{"text":"string","voice":"string","cached":"boolean","key":"string","file":"string","raw":"string","levels":"{peak_dbfs,rms_dbfs,lufs,max_momentary_lufs,duration_ms,clipped,frames,rate,channels}","raw_levels":"levels","trimmed_ms":"{head,tail}","gain_db":"number","limited":"boolean","target":"{lufs,from:given|retail|our clips|fallback,lines,why?,lufs_spread:{p10,p50,p90,max}|null,peak_dbfs_spread:{p10,p50,p90,max}|null}","verdict":"{level:loud|quiet|ok|no_reference,over_p90_db,peak_over_p90_db,text}","playback":"{playing,output?,duration_ms?,why?}","ready?":"{clip,repo_path,clips_entry,exists,commit}"}"#,
    r#"{"text":"Superbounce","play":false}"#,
);
const LIST: MethodDef = def(
    m::VOICE_LIST,
    "The announcer's voice source: the default voice and any other voice a kept line used, whether the Fish Audio key is in the Keychain (it is not read), where lines are cached and the lines most recently said. Nothing is asked of the service",
    "{}",
    r#"{"source":"string","default":"string","voices":"[{id,default,url,lines}]","key":"{state:present|missing|unreadable,service?,command?,why?}","cache":"{dir,lines}","recent":"[{key,text,voice}]"}"#,
    "null",
);
const COMMIT: MethodDef = def(
    m::VOICE_COMMIT,
    "Copy a saved line to art/announcer/<clip>.wav in the checkout, so it can be committed (the files are in Git LFS). A separate, explicit step: voice.say never writes there. Does not run git and does not edit boot::medal_announcer::CLIPS; the reply gives the CLIPS entry a new clip needs. An existing clip is only replaced with `replace`. `from` is a file voice.say saved (default the last one); `repo` the checkout (default the one the editor runs in)",
    r#"{"clip":"string","from?":"string","repo?":"string","replace?":"boolean"}"#,
    r#"{"file":"string","clip":"string","repo_path":"string","clips_entry":"string","exists":"boolean","next":"string"}"#,
    r#"{"clip":"test-line","repo":"$OUT"}"#,
);

fn voice_error(e: VoiceError) -> RpcError {
    let c = match &e {
        VoiceError::NoKey | VoiceError::Key(_) => code::REFUSED,
        VoiceError::Invalid(_) => code::INVALID_PARAMS,
        _ => code::FAILED,
    };
    let mut r = err(c, e.to_string());
    if e == VoiceError::NoKey {
        r.data = Some(json!({
            "step": "add_key",
            "service": key::SERVICE,
            "command": key::ADD_COMMAND,
        }));
    }
    r
}

fn flag(p: &P, k: &str, default: bool) -> Result<bool, RpcError> {
    match p.get(k) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("{k}: expected true or false, not {v}"))),
    }
}

fn spread(s: Option<super::sound_measure::meter::Spread>) -> Value {
    match s {
        Some(s) => json!({"p10": s.p10, "p50": s.p50, "p90": s.p90, "max": s.max}),
        None => Value::Null,
    }
}

fn ready_json(r: &Ready) -> Value {
    json!({
        "clip": r.clip,
        "repo_path": r.repo_path,
        "clips_entry": r.clips_entry,
        "exists": r.exists,
        "commit": format!(
            "voice.commit {{clip: \"{}\"{}}}, then add the clips_entry to boot::medal_announcer::CLIPS unless the clip exists",
            r.clip,
            if r.exists { ", replace: true" } else { "" }
        ),
    })
}

/// A line made, as the API gives it.
pub fn said_json(s: &Said) -> Value {
    let playback = match &s.playback {
        Playback::Playing { output, duration_ms } => {
            json!({"playing": true, "output": output, "duration_ms": duration_ms})
        }
        Playback::Silent(why) => json!({"playing": false, "why": why}),
    };
    let mut target = json!({
        "lufs": s.target.lufs,
        "from": s.target.from,
        "lines": s.target.lines,
        "lufs_spread": spread(s.target.lufs_spread),
        "peak_dbfs_spread": spread(s.target.peak_spread),
    });
    if let Some(w) = &s.target.why {
        target["why"] = json!(w);
    }
    let rate = f64::from(s.levels.rate.max(1));
    let ms = |f: usize| (f as f64 * 1000.0 / rate).round() as u64;
    let mut v = json!({
        "text": s.text,
        "voice": s.voice,
        "cached": s.cached,
        "key": s.key,
        "file": s.file.display().to_string(),
        "raw": s.raw.display().to_string(),
        "levels": levels_json(&s.levels),
        "raw_levels": levels_json(&s.raw_levels),
        "trimmed_ms": {"head": ms(s.trimmed.head), "tail": ms(s.trimmed.tail)},
        "gain_db": (s.gain_db * 10.0).round() / 10.0,
        "limited": s.limited,
        "target": target,
        "verdict": {
            "level": s.verdict.level,
            "over_p90_db": s.verdict.over_p90_db,
            "peak_over_p90_db": s.verdict.peak_over_p90_db,
            "text": s.verdict.text,
        },
        "playback": playback,
    });
    if let Some(r) = &s.ready {
        v["ready"] = ready_json(r);
    }
    v
}

fn say(app: &mut App, p: &P) -> Reply {
    let target_lufs = match p.get("target_lufs") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("target_lufs: expected a number, not {v}")))?,
        ),
    };
    let params = SayParams {
        text: p.str("text")?.to_owned(),
        voice: p.opt_str("voice")?.map(str::to_owned),
        model: p.opt_str("model")?.map(str::to_owned),
        speed: p.opt_f32("speed")?,
        out: p.opt_str("out")?.map(PathBuf::from),
        clip: p.opt_str("clip")?.map(str::to_owned),
        play: flag(p, "play", true)?,
        target_lufs,
        fresh: flag(p, "fresh", false)?,
    };
    let said = app.voice_say(params).map_err(voice_error)?;
    Ok(said_json(&said))
}

fn commit(app: &mut App, p: &P) -> Reply {
    let (file, ready) = app
        .voice_commit(
            p.str("clip")?,
            p.opt_str("from")?.map(Path::new),
            p.opt_str("repo")?.map(Path::new),
            flag(p, "replace", false)?,
        )
        .map_err(voice_error)?;
    Ok(json!({
        "file": file.display().to_string(),
        "clip": ready.clip,
        "repo_path": ready.repo_path,
        "clips_entry": ready.clips_entry,
        "exists": ready.exists,
        "next": format!(
            "git add {} (Git LFS){}",
            ready.repo_path,
            if ready.exists { "" } else { "; add the clips_entry to boot::medal_announcer::CLIPS" }
        ),
    }))
}

impl Provider for Voice {
    fn methods(&self) -> Vec<MethodDef> {
        vec![SAY, LIST, COMMIT]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::VOICE_SAY => say(app, &p),
            m::VOICE_LIST => Ok(app.voice_list()),
            m::VOICE_COMMIT => commit(app, &p),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests;
