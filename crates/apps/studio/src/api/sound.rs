//! Sound audition as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `sound.read`, `sound.play`, `sound.stop` and
//! `sound.mouth`, over the Tags tool's own audition (`app::audition`), so
//! the window and the API are one path. docs/editor/agent-api.md,
//! docs/editor/rich-editors.md.

use std::sync::Arc;

use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::audition::{PlayState, SoundError};

/// Most waveform columns `sound.read` returns.
const MAX_BINS: usize = 4096;

/// The provider.
#[derive(Default)]
pub struct Sound;

const READ: MethodDef = def(
    m::SOUND_READ,
    "A snd! tag of the map the Tags tool shows: its header, every pitch range and permutation, and for one permutation (range 0, permutation 0 by default) its decoded format, length and waveform peaks, or why it is not decoded (a codec stream needs the game's executable from the source), and its mouth curve: one aperture value 0 to 1 per 30 Hz tick, null when the permutation is unvoiced (the curve is read even when the audio is not). Also what plays now. Nothing plays",
    r#"{"tag":"string","range?":"integer","permutation?":"integer","bins?":"integer (waveform columns, default 64, at most 4096)"}"#,
    r#"{"tag":"string","sample_rate":"integer|null","encoding":"string","compression":"string","max_play_time_ms":"integer","ranges":"[{permutations:[{encoded_bytes,chunks}]}]","selected":"{range,permutation,channels,rate,frames,duration_ms,peaks:[[low,high]],mouth:{ticks,curve}|null}|{range,permutation,refusal,mouth}|{range,permutation,error,mouth}","playback":"{playing,tag?,range,permutation,position_ms,output?}"}"#,
    r#"{"tag":"snd!:objects\\test\\blaster","bins":32}"#,
);
const MOUTH: MethodDef = def(
    m::SOUND_MOUTH,
    "A permutation's mouth curve, read from the sound's extra-info resource in the map's ugh!: {ticks, curve} with one aperture value, 0 to 1, per 30 Hz tick. An unvoiced permutation (the sound has no extra info, its descriptor index is negative, or the mouth entry is zero-length) gives a null curve. The Sound window draws the same curve under the waveform. Writing it is a later slice",
    r#"{"tag":"string","range?":"integer","permutation?":"integer"}"#,
    r#"{"ticks":"integer","curve":"number[]|null"}"#,
    r#"{"tag":"snd!:objects\\test\\blaster"}"#,
);
const PLAY: MethodDef = def(
    m::SOUND_PLAY,
    "Play a permutation of a snd! tag through the host's audio output (opened on the first play; an error says why when there is none), from `from_ms` into it, replacing what plays. A codec stream without the game's executable is refused by name. Returns at once; sound.read's `playback` follows it",
    r#"{"tag":"string","range?":"integer","permutation?":"integer","from_ms?":"integer"}"#,
    r#"{"playing":"boolean","frames":"integer","rate":"integer","channels":"integer","duration_ms":"integer","from_ms":"integer","output":"device|headless"}"#,
    r#"{"tag":"snd!:objects\\test\\blaster","from_ms":500}"#,
);
const STOP: MethodDef = def(
    m::SOUND_STOP,
    "Stop what sound.play started; whether anything was playing. Audio already queued in the output (under 60 ms) plays out",
    "{}",
    r#"{"stopped":"boolean"}"#,
    "null",
);

fn failed(e: SoundError) -> agent_proto::RpcError {
    match e {
        SoundError::Refused(m) => err(code::REFUSED, m),
        SoundError::Failed(m) => err(code::FAILED, m),
    }
}

fn playback(s: &PlayState) -> Value {
    json!({
        "playing": s.playing,
        "tag": s.tag.map(|t| format!("{:#010x}", t.0)),
        "range": s.range,
        "permutation": s.permutation,
        "position_ms": s.position_ms,
        "output": s.output,
    })
}

/// One mouth curve as the API gives it: the tick count and one aperture
/// value 0 to 1 per 30 Hz tick, `null` when unvoiced, `error` when it
/// could not be read.
fn mouth(r: &Result<Option<Arc<[u8]>>, SoundError>) -> Value {
    match r {
        Ok(Some(curve)) => json!({
            "ticks": curve.len(),
            "curve": curve.iter().map(|&b| f32::from(b) / 255.0).collect::<Vec<_>>(),
        }),
        Ok(None) => json!({"ticks": 0, "curve": null}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// The `tag` param as a sound of the document the Tags tool shows.
fn sound_tag(app: &mut App, p: &P) -> Result<TagId, agent_proto::RpcError> {
    super::tag_fields::resolve(app, p)
}

fn read(app: &mut App, p: &P) -> Reply {
    let id = sound_tag(app, p)?;
    let range = p.opt_usize("range")?.unwrap_or(0);
    let permutation = p.opt_usize("permutation")?.unwrap_or(0);
    let bins = p.opt_usize("bins")?.unwrap_or(64);
    if bins == 0 || bins > MAX_BINS {
        return Err(err(code::INVALID_PARAMS, format!("bins: 1 to {MAX_BINS}")));
    }
    let info = app.sound_info(id).map_err(failed)?;
    let selected = match app.sound_clip(id, range, permutation) {
        Ok(clip) => json!({
            "range": range,
            "permutation": permutation,
            "channels": clip.channels,
            "rate": clip.rate,
            "frames": clip.frames(),
            "duration_ms": clip.duration_ms(),
            "peaks": clip.peaks(bins),
            "mouth": mouth(&app.sound_mouth(id, range, permutation)),
        }),
        Err(SoundError::Refused(why)) => json!({
            "range": range,
            "permutation": permutation,
            "refusal": why,
            "mouth": mouth(&app.sound_mouth(id, range, permutation)),
        }),
        Err(SoundError::Failed(why)) => json!({
            "range": range,
            "permutation": permutation,
            "error": why,
            "mouth": mouth(&app.sound_mouth(id, range, permutation)),
        }),
    };
    let name = app
        .shown_tags()
        .tag(id)
        .map(|t| t.name.clone())
        .unwrap_or_default();
    Ok(json!({
        "tag": name,
        "sample_rate": info.sample_rate,
        "encoding": info.encoding,
        "compression": info.compression,
        "max_play_time_ms": info.max_play_time_ms,
        "ranges": info.ranges.iter().map(|r| json!({
            "permutations": r.iter().map(|p| json!({
                "encoded_bytes": p.encoded_bytes,
                "chunks": p.chunks,
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>(),
        "selected": selected,
        "playback": playback(&app.sound_state()),
    }))
}

fn play(app: &mut App, p: &P) -> Reply {
    let id = sound_tag(app, p)?;
    let range = p.opt_usize("range")?.unwrap_or(0);
    let permutation = p.opt_usize("permutation")?.unwrap_or(0);
    let from_ms = p.opt_u64("from_ms")?.unwrap_or(0);
    let s = app
        .sound_play(id, range, permutation, from_ms)
        .map_err(failed)?;
    Ok(json!({
        "playing": true,
        "frames": s.frames,
        "rate": s.rate,
        "channels": s.channels,
        "duration_ms": s.duration_ms,
        "from_ms": s.from_ms,
        "output": s.output,
    }))
}

fn mouth_read(app: &mut App, p: &P) -> Reply {
    let id = sound_tag(app, p)?;
    let range = p.opt_usize("range")?.unwrap_or(0);
    let permutation = p.opt_usize("permutation")?.unwrap_or(0);
    let m = app.sound_mouth(id, range, permutation).map_err(failed)?;
    Ok(mouth(&Ok(m)))
}

impl Provider for Sound {
    fn methods(&self) -> Vec<MethodDef> {
        vec![READ, PLAY, STOP, MOUTH]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::SOUND_READ => read(app, &p),
            m::SOUND_PLAY => play(app, &p),
            m::SOUND_STOP => Ok(json!({ "stopped": app.sound_stop() })),
            m::SOUND_MOUTH => mouth_read(app, &p),
            _ => return None,
        })
    }
}

#[cfg(test)]
pub(super) mod tests;
