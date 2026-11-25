//! `sound.measure`: a sound's levels against a reference set of the open
//! map's sounds ([`super::Provider`], registered in [`super::seams`]).
//! docs/editor/agent-api.md, "Content checks".
//!
//! What is measured is decoded by the Tags tool's audition
//! (`App::sound_clip`, the path `sound.read` takes), or read from one of
//! our WAV clips (the Pogo Stick's `boot::pogo_sound::CLIPS` and the
//! announcer's `boot::medal_announcer::CLIPS`, through the game's own
//! `parse_wav`), or from a WAV file. [`meter`] does the arithmetic. The
//! reference set is the open map's `snd!` tags of the same class (the
//! header's `+2` byte), or of every class for a clip, each one's first
//! permutation decoded.

pub mod meter;

use std::path::Path;

use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use self::meter::{Levels, Spread, round1};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::audition::SoundError;

/// Reference sounds decoded at most by default, and the most `limit` may ask.
pub const DEFAULT_LIMIT: usize = 32;
pub const MAX_LIMIT: usize = 512;

/// The provider.
#[derive(Default)]
pub struct SoundMeasure;

const MEASURE: MethodDef = def(
    m::SOUND_MEASURE,
    "A sound's levels against the open map's sounds of its class: peak and RMS (dBFS), integrated and loudest-block loudness (LUFS, BS.1770-4), duration and full-scale samples, the same numbers' spread over the reference set and a verdict (`loud` past its p90 by more than 1 dB). Measures a snd! tag of the map the Tags tool shows (each permutation of `range`, or one), one of our WAV clips (`clip`: pogo/spring, announcer/superbounce; an unknown one's error lists them all) or a WAV file (`path`). Nothing plays",
    r#"{"tag?":"string","clip?":"string","path?":"string","range?":"integer","permutation?":"integer","class?":"integer (the reference set's class; default the tag's, every class for a clip or file)","limit?":"integer (reference sounds decoded, default 32, at most 512)"}"#,
    r#"{"target":"string","from":"tag|clip|file","class":"integer|null","levels":"{peak_dbfs,rms_dbfs,lufs,max_momentary_lufs,duration_ms,clipped,frames,rate,channels}","permutations?":"[{range,permutation,levels}|{range,permutation,refusal}]","reference":"{class,considered,decoded,skipped,lufs:{p10,p50,p90,max}|null,peak_dbfs:{p10,p50,p90,max}|null,loudest:[{tag,lufs}],why?}","verdict":"{level:loud|quiet|ok|no_reference,over_p90_db,peak_over_p90_db,text}"}"#,
    r#"{"tag":"snd!:objects\\test\\blaster"}"#,
);

impl Provider for SoundMeasure {
    fn methods(&self) -> Vec<MethodDef> {
        vec![MEASURE]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::SOUND_MEASURE).then(|| measure(app, &P(params)))
    }
}

/// Our WAV clips, by id (`pogo/spring`, `announcer/superbounce`): the
/// bytes the game embeds and plays.
pub fn our_clips() -> Vec<(String, &'static [u8])> {
    let pogo = boot::pogo_sound::CLIPS
        .iter()
        .map(|(_, id, bytes)| (format!("pogo/{id}"), *bytes));
    let announcer = boot::medal_announcer::CLIPS
        .iter()
        .map(|(id, bytes)| (format!("announcer/{id}"), *bytes));
    pogo.chain(announcer).collect()
}

/// The levels of a WAV file's bytes (16-bit PCM, one or two channels),
/// read by the game's own reader; why not when it does not read (a Git
/// LFS pointer names `git lfs pull`).
pub fn wav_levels(bytes: &[u8]) -> Result<Levels, String> {
    let c = boot::medal_announcer::parse_wav(bytes).map_err(|e| e.to_string())?;
    let pcm: Vec<i16> = c
        .data
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(meter::measure(&pcm, c.channels, c.rate))
}

/// Levels as the API gives them (one decimal place).
pub fn levels_json(l: &Levels) -> Value {
    json!({
        "peak_dbfs": round1(l.peak_dbfs),
        "rms_dbfs": round1(l.rms_dbfs),
        "lufs": round1(l.lufs),
        "max_momentary_lufs": round1(l.max_momentary_lufs),
        "duration_ms": l.duration_ms,
        "clipped": l.clipped,
        "frames": l.frames,
        "rate": l.rate,
        "channels": l.channels,
    })
}

fn spread_json(s: Option<Spread>) -> Value {
    match s {
        Some(s) => json!({
            "p10": round1(s.p10),
            "p50": round1(s.p50),
            "p90": round1(s.p90),
            "max": round1(s.max),
        }),
        None => Value::Null,
    }
}

/// The open map's sounds measured for comparison.
#[derive(Debug, Default)]
pub struct Reference {
    /// The class compared with; `None` for every class.
    pub class: Option<u8>,
    /// Sounds of the class in the map (the measured tag left out).
    pub considered: usize,
    /// Each sound decoded, by name, and its first permutation's levels.
    pub measured: Vec<(String, Levels)>,
    /// Sounds that did not decode, and why (a codec stream without the
    /// game's executable).
    pub skipped: Vec<(String, String)>,
    /// Why there is no reference at all (no map open).
    pub why: Option<String>,
}

impl Reference {
    pub fn lufs(&self) -> Option<Spread> {
        meter::spread(
            &self
                .measured
                .iter()
                .map(|(_, l)| l.lufs)
                .collect::<Vec<_>>(),
        )
    }

    pub fn peak(&self) -> Option<Spread> {
        meter::spread(
            &self
                .measured
                .iter()
                .map(|(_, l)| l.peak_dbfs)
                .collect::<Vec<_>>(),
        )
    }

    /// What it is, for a verdict's text: "class 4's sounds".
    pub fn of(&self) -> String {
        match self.class {
            Some(c) => format!("the map's class {c} sounds"),
            None => "the map's sounds".into(),
        }
    }

    pub fn json(&self) -> Value {
        let mut loudest: Vec<&(String, Levels)> = self.measured.iter().collect();
        loudest.sort_by(|a, b| b.1.lufs.total_cmp(&a.1.lufs));
        let mut v = json!({
            "class": self.class,
            "considered": self.considered,
            "decoded": self.measured.len(),
            "skipped": self.skipped.len(),
            "lufs": spread_json(self.lufs()),
            "peak_dbfs": spread_json(self.peak()),
            "loudest": loudest.iter().take(3).map(|(n, l)| json!({"tag": n, "lufs": round1(l.lufs)})).collect::<Vec<_>>(),
        });
        if let Some(w) = &self.why {
            v["why"] = json!(w);
        }
        v
    }
}

/// The shown map's tags read, as `sound.read` reads them: the browsed
/// map as it is, or the open map's synced into the tag document.
fn shown_ready(app: &mut App) -> Result<(), String> {
    if app.browsing().is_some() {
        return Ok(());
    }
    super::tag_fields::sync_open_tags(app).map_err(|e| e.message)
}

/// Up to `limit` of the shown map's sounds of `class` (every class for
/// `None`), `exclude` left out, spread evenly over the map's tag order,
/// each one's first permutation measured.
pub fn reference(
    app: &mut App,
    class: Option<u8>,
    exclude: Option<TagId>,
    limit: usize,
) -> Reference {
    reference_where(app, class, exclude, limit, |_| true)
}

/// The shown map's retail announcer lines (`sound\dialog\multiplayer\flavor`,
/// the multi-kill and spree lines), up to `limit` of them measured: what
/// the Announcer brings a new line to (docs/editor/announcer.md).
pub fn retail_announcer(app: &mut App, limit: usize) -> Reference {
    reference_where(app, None, None, limit, |t| {
        t.name
            .to_ascii_lowercase()
            .contains(r"dialog\multiplayer\flavor")
    })
}

fn reference_where(
    app: &mut App,
    class: Option<u8>,
    exclude: Option<TagId>,
    limit: usize,
    keep: impl Fn(&crate::app::audition::SoundTag) -> bool,
) -> Reference {
    let mut r = Reference {
        class,
        ..Reference::default()
    };
    if let Err(e) = shown_ready(app) {
        r.why = Some(e);
        return r;
    }
    let all = match app.sound_tags() {
        Ok(t) => t,
        Err(e) => {
            r.why = Some(e.to_string());
            return r;
        }
    };
    let pool: Vec<_> = all
        .into_iter()
        .filter(|t| Some(t.id) != exclude && class.is_none_or(|c| t.class == c) && keep(t))
        .collect();
    r.considered = pool.len();
    let n = pool.len();
    let take = limit.min(n);
    for i in 0..take {
        let t = &pool[i * n / take];
        match app.sound_clip(t.id, 0, 0) {
            Ok(c) => r
                .measured
                .push((t.name.clone(), meter::measure(&c.pcm, c.channels, c.rate))),
            Err(e) => r.skipped.push((t.name.clone(), e.to_string())),
        }
    }
    r
}

fn limit(p: &P) -> Result<usize, agent_proto::RpcError> {
    let l = p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT);
    if l == 0 || l > MAX_LIMIT {
        return Err(err(
            code::INVALID_PARAMS,
            format!("limit: 1 to {MAX_LIMIT}"),
        ));
    }
    Ok(l)
}

fn class_param(p: &P) -> Result<Option<u8>, agent_proto::RpcError> {
    match p.opt_u64("class")? {
        None => Ok(None),
        Some(c) => u8::try_from(c)
            .map(Some)
            .map_err(|_| err(code::INVALID_PARAMS, "class: 0 to 255")),
    }
}

pub(super) fn measure(app: &mut App, p: &P) -> Reply {
    let given = ["tag", "clip", "path"].iter().filter(|k| p.has(k)).count();
    if given != 1 {
        return Err(err(
            code::INVALID_PARAMS,
            "sound.measure takes one of tag, clip or path",
        ));
    }
    let limit = limit(p)?;
    let class = class_param(p)?;
    if let Some(id) = p.opt_str("clip")? {
        let clips = our_clips();
        let Some((name, bytes)) = clips.iter().find(|(n, _)| n == id) else {
            return Err(err(
                code::INVALID_PARAMS,
                format!(
                    "no clip {id:?} (ours: {})",
                    clips
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };
        let levels = wav_levels(bytes).map_err(|e| err(code::FAILED, format!("{name}: {e}")))?;
        return Ok(answer(app, name, "clip", class, levels, None, None, limit));
    }
    if let Some(path) = p.opt_str("path")? {
        let bytes = std::fs::read(Path::new(path))
            .map_err(|e| err(code::FAILED, format!("{path}: {e}")))?;
        let levels = wav_levels(&bytes).map_err(|e| err(code::FAILED, format!("{path}: {e}")))?;
        return Ok(answer(app, path, "file", class, levels, None, None, limit));
    }
    // A tag of the shown map.
    let id = super::tag_fields::resolve(app, p)?;
    let range = p.opt_usize("range")?.unwrap_or(0);
    let info = app.sound_info(id).map_err(failed)?;
    let perms = info
        .ranges
        .get(range)
        .ok_or_else(|| err(code::INVALID_PARAMS, format!("no pitch range {range}")))?
        .len();
    let which: Vec<usize> = match p.opt_usize("permutation")? {
        Some(i) if i < perms => vec![i],
        Some(i) => {
            return Err(err(
                code::INVALID_PARAMS,
                format!("no permutation {i} in range {range} ({perms})"),
            ));
        }
        None => (0..perms).collect(),
    };
    let mut rows = Vec::new();
    let mut loudest: Option<Levels> = None;
    for i in which {
        match app.sound_clip(id, range, i) {
            Ok(c) => {
                let l = meter::measure(&c.pcm, c.channels, c.rate);
                rows.push(json!({"range": range, "permutation": i, "levels": levels_json(&l)}));
                if loudest.is_none_or(|x| l.lufs > x.lufs) {
                    loudest = Some(l);
                }
            }
            Err(e) => {
                rows.push(json!({"range": range, "permutation": i, "refusal": e.to_string()}))
            }
        }
    }
    let Some(levels) = loudest else {
        let why = rows
            .first()
            .and_then(|r| r["refusal"].as_str())
            .unwrap_or("it has no permutations")
            .to_owned();
        return Err(err(code::REFUSED, format!("nothing of it decoded: {why}")));
    };
    let own = app
        .sound_tags()
        .ok()
        .and_then(|t| t.into_iter().find(|t| t.id == id));
    let name = own.as_ref().map(|t| t.name.clone()).unwrap_or_default();
    let class = class.or(own.map(|t| t.class));
    Ok(answer(
        app,
        &name,
        "tag",
        class,
        levels,
        Some(rows),
        Some(id),
        limit,
    ))
}

fn failed(e: SoundError) -> agent_proto::RpcError {
    match e {
        SoundError::Refused(m) => err(code::REFUSED, m),
        SoundError::Failed(m) => err(code::FAILED, m),
    }
}

#[allow(clippy::too_many_arguments)]
fn answer(
    app: &mut App,
    target: &str,
    from: &str,
    class: Option<u8>,
    levels: Levels,
    permutations: Option<Vec<Value>>,
    exclude: Option<TagId>,
    limit: usize,
) -> Value {
    let reference = reference(app, class, exclude, limit);
    let verdict = meter::verdict(&levels, reference.lufs(), reference.peak(), &reference.of());
    let mut v = json!({
        "target": target,
        "from": from,
        "class": class,
        "levels": levels_json(&levels),
        "reference": reference.json(),
        "verdict": {
            "level": verdict.level,
            "over_p90_db": verdict.over_p90_db,
            "peak_over_p90_db": verdict.peak_over_p90_db,
            "text": verdict.text,
        },
    });
    if let Some(rows) = permutations {
        v["permutations"] = Value::Array(rows);
    }
    v
}

/// `check.run`'s sounds: each of our clips (or `target`: a clip id, a
/// clip id's prefix such as `pogo`, or a snd! tag) against the open map's
/// sounds. A clip louder than the reference's p90 by more than
/// [`meter::LOUD_MARGIN_DB`] is `sound.loud` (an error past
/// [`meter::TOO_LOUD_DB`]); one with full-scale samples `sound.clipping`;
/// one that does not read `sound.unreadable`; no reference
/// `sound.no_reference`.
pub fn check(app: &mut App, target: Option<&str>) -> Result<Vec<super::checks::Finding>, String> {
    use super::checks::{Finding, Severity};
    let clips: Vec<(String, &'static [u8])> = our_clips()
        .into_iter()
        .filter(|(id, _)| {
            target
                .is_none_or(|t| id == t || id.starts_with(&format!("{}/", t.trim_end_matches('/'))))
        })
        .collect();
    let mut subjects: Vec<(
        String,
        Option<String>,
        Result<Levels, String>,
        Option<TagId>,
    )> = clips
        .iter()
        .map(|(id, bytes)| (id.clone(), Some(clip_path(id)), wav_levels(bytes), None))
        .collect();
    let mut class = None;
    if let (Some(t), true) = (target, clips.is_empty()) {
        // Not one of ours: a tag of the shown map.
        let params = json!({"tag": t});
        let id = super::tag_fields::resolve(app, &P(&params)).map_err(|e| e.message)?;
        let tags = app.sound_tags().map_err(|e| e.to_string())?;
        let own = tags
            .iter()
            .find(|x| x.id == id)
            .ok_or_else(|| format!("{t} is not a sound"))?;
        class = Some(own.class);
        let levels = app
            .sound_clip(id, 0, 0)
            .map(|c| meter::measure(&c.pcm, c.channels, c.rate))
            .map_err(|e| e.to_string());
        subjects.push((own.name.clone(), None, levels, Some(id)));
    }
    let exclude = subjects.iter().find_map(|s| s.3);
    let reference = reference(app, class, exclude, DEFAULT_LIMIT);
    let (lufs, peak) = (reference.lufs(), reference.peak());
    let mut out = Vec::new();
    let finding =
        |code: &str, severity, message: String, target: &str, path: &Option<String>| Finding {
            area: "sounds",
            code: code.into(),
            severity,
            message,
            target: target.into(),
            path: path.clone(),
        };
    if lufs.is_none() && !subjects.is_empty() {
        out.push(finding(
            "sound.no_reference",
            Severity::Note,
            format!(
                "nothing to compare with: {}",
                reference
                    .why
                    .clone()
                    .unwrap_or_else(|| format!("{} decoded none", reference.of()))
            ),
            "",
            &None,
        ));
    }
    for (name, path, levels, _) in &subjects {
        let l = match levels {
            Ok(l) => l,
            Err(e) => {
                out.push(finding(
                    "sound.unreadable",
                    Severity::Warning,
                    format!("{name} does not read: {e}"),
                    name.as_str(),
                    path,
                ));
                continue;
            }
        };
        if l.clipped > 0 {
            out.push(finding(
                "sound.clipping",
                Severity::Warning,
                format!(
                    "{name} has {} full-scale samples (peak {:.1} dBFS)",
                    l.clipped, l.peak_dbfs
                ),
                name.as_str(),
                path,
            ));
        }
        let v = meter::verdict(l, lufs, peak, &reference.of());
        if v.level == "loud" {
            let over = v.over_p90_db.unwrap_or(0.0);
            out.push(finding(
                "sound.loud",
                if over > meter::TOO_LOUD_DB {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                format!("{name} ({:.1} LUFS) is {}", l.lufs, v.text),
                name.as_str(),
                path,
            ));
        }
    }
    Ok(out)
}

/// Where clip `id`'s file is in the checkout.
fn clip_path(id: &str) -> String {
    format!("art/{id}.wav")
}

#[cfg(test)]
mod tests;
