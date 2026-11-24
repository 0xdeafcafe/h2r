//! The Voices window: type a line, have a voice say it (Fish Audio's
//! announcer voice, or a game character's local clone), hear it, and see
//! how it sits against the lines it should match. Making a line is the
//! app's ([`crate::app::announcer`], [`crate::app::voices`]); the agent
//! API's `voice.say` and `voice.clone.say` make it the same way. View >
//! Voices opens it.
//!
//! The window waits while the voice is asked (a few seconds); a line the
//! voice already said comes back at once from the cache. The clone's
//! install, transcription and fine-tune run in the background; the
//! window shows their progress.
use dear_imgui_rs::{Condition, Ui};

use crate::app::App;
use crate::app::announcer::{Playback, SayParams, Said};
use crate::app::voices::{BuildParams, MIN_TRAIN_MINUTES};
use crate::theme::{self, icon};
use crate::voice::dataset::CHARACTERS;
use crate::voice::sidecar::Backend;
use crate::voice::{self, Engine, VoiceError};

/// The window's state.
#[derive(Default)]
pub struct AnnouncerUi {
    pub open: bool,
    text: String,
    /// The clip id the line is for, or empty.
    clip: String,
    /// A voice other than the default, or empty.
    voice: String,
    /// Who says it.
    engine: Engine,
    /// The character cloned ([`CHARACTERS`]).
    character: usize,
    /// The clone's reference line, or empty.
    reference: String,
    /// The least minutes of speech a fine-tune starts from; lowered to
    /// train on a small dataset. `None` until the window first draws.
    min_minutes: Option<f32>,
    /// What the last dataset build or fine-tune start said.
    clone_note: Option<Result<String, String>>,
    last: Option<Result<Said, VoiceError>>,
    /// What committing the last line said.
    committed: Option<Result<String, String>>,
}

/// `m:ss.t` of `ms`.
fn clock(ms: u64) -> String {
    format!("{}:{:02}.{}", ms / 60_000, ms / 1000 % 60, ms / 100 % 10)
}

pub fn window(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    if !st.open {
        return;
    }
    ui.window("Voices###announcer")
        .size([460.0, 420.0], Condition::FirstUseEver)
        .build(|| {
            body(ui, app, st);
            ui.separator();
            if ui.button("Close##announcer-close") {
                st.open = false;
            }
        });
}

/// What a backend is called in the picker.
fn backend_label(b: Backend) -> &'static str {
    match b {
        Backend::F5Mlx => "f5-mlx: zero-shot or fine-tune",
        Backend::Qwen3Tts => "qwen3-tts: zero-shot only (no training)",
    }
}

/// The clone's half: the character, its dataset and fine-tune, and the
/// background job's progress.
fn clone_panel(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    let c = &CHARACTERS[st.character.min(CHARACTERS.len() - 1)];
    ui.set_next_item_width(220.0);
    if let Some(_combo) = ui.begin_combo("##voices-character", c.name) {
        for (i, ch) in CHARACTERS.iter().enumerate() {
            if ui.selectable_config(ch.name).selected(i == st.character).build() {
                st.character = i;
            }
        }
    }
    ui.same_line();
    if ui.button("Build dataset##voices-build") {
        let r = app.voice_dataset_build(BuildParams {
            character: c.id.to_owned(),
            asr: true,
            ..BuildParams::default()
        });
        st.clone_note = Some(
            r.map(|v| {
                format!(
                    "{} clips ({} min with text, {} to transcribe) from {} tags",
                    v["kept"], v["dataset"]["minutes"], v["dataset"]["pending_asr"], v["tags"]
                )
            })
            .map_err(|e| e.to_string()),
        );
    }
    let backend = app.voices.backend();
    ui.set_next_item_width(300.0);
    if let Some(_combo) = ui.begin_combo("##voices-backend", backend_label(backend)) {
        for b in Backend::ALL {
            if ui.selectable_config(backend_label(b)).selected(b == backend).build() {
                app.voices.set_backend(b);
            }
        }
    }
    ui.same_line();
    ui.text_disabled("Model");
    let status = app.voice_dataset_status(Some(c.id)).ok();
    let minutes = status
        .as_ref()
        .and_then(|s| s["datasets"][0]["dataset"]["minutes"].as_f64())
        .unwrap_or(0.0);
    let small = minutes < MIN_TRAIN_MINUTES;
    let floor = st.min_minutes.get_or_insert(MIN_TRAIN_MINUTES as f32);
    if app.voices.backend().trains() {
        if small {
            ui.set_next_item_width(90.0);
            if ui.input_float("Least minutes##voices-floor", floor) {
                *floor = if floor.is_finite() { floor.max(0.0) } else { MIN_TRAIN_MINUTES as f32 };
            }
        }
        ui.same_line();
        let need = f64::from(*floor);
        if ui.button(if small {
            "Train anyway##voices-train"
        } else {
            "Fine-tune##voices-train"
        }) {
            st.clone_note = Some(
                app.voice_clone_train(c.id, None, Some(need))
                    .map(|v| format!("Fine-tuning on {} minutes (job {})", v["minutes"], v["job"]))
                    .map_err(|e| e.to_string()),
            );
        }
        if small {
            ui.text_disabled(format!(
                "This dataset has {minutes:.1} min of speech with text; a fine-tune on so little \
                 can give an unstable voice. Lower the minimum to train anyway."
            ));
        }
    } else {
        ui.text_disabled(
            "This model clones from a reference line with no training. Say it needs a \
             dataset with text: Build dataset, then wait for its transcription.",
        );
    }
    if let Some(s) = &status {
        let d = &s["datasets"][0];
        if d["built"] == true {
            ui.text_disabled(format!(
                "Dataset: {} lines, {} min with text ({} subtitles, {} transcribed, {} waiting){}",
                d["dataset"]["lines"],
                d["dataset"]["minutes"],
                d["dataset"]["from_subtitle"],
                d["dataset"]["from_asr"],
                d["dataset"]["pending_asr"],
                if d["model"]["trained"] == true { ", fine-tuned" } else { "" }
            ));
        } else {
            ui.text_disabled("No dataset yet: Build dataset reads the disc's lines.");
        }
        ui.text_disabled(format!(
            "Local model ({}): {}",
            s["sidecar"]["backend"].as_str().unwrap_or_default(),
            s["sidecar"]["state"].as_str().unwrap_or_default()
        ));
        let j = &s["job"];
        if j.is_object() {
            let total = j["total"].as_u64().unwrap_or(0);
            let line = format!(
                "Job {} ({}): {} {}{}",
                j["id"],
                j["kind"].as_str().unwrap_or_default(),
                j["state"].as_str().unwrap_or_default(),
                j["stage"].as_str().unwrap_or_default(),
                if total > 0 {
                    format!(" {}/{}", j["done"], total)
                } else {
                    String::new()
                }
            );
            match j["state"].as_str() {
                Some("failed") => ui.text_colored(
                    theme::WARN,
                    format!("{line}: {}", j["why"].as_str().unwrap_or_default()),
                ),
                _ => ui.text_disabled(line),
            }
            if let Some(t) = j["text"].as_str() {
                ui.text_disabled(t);
            }
        }
    }
    match &st.clone_note {
        Some(Ok(t)) => ui.text_wrapped(t),
        Some(Err(t)) => ui.text_colored(theme::WARN, format!("{} {t}", icon::ALERT)),
        None => {}
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##voices-reference", &mut st.reference)
        .hint("Reference line: a dataset clip id or sound tag (default: picked)")
        .build();
}

fn body(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    if ui.radio_button_bool("Announcer (Fish Audio)", st.engine == Engine::Fish) {
        st.engine = Engine::Fish;
    }
    ui.same_line();
    if ui.radio_button_bool("Game character (local clone)", st.engine == Engine::Clone) {
        st.engine = Engine::Clone;
    }
    let clone = st.engine == Engine::Clone;
    ui.text_disabled(format!(
        "{} lines are saved under {}",
        icon::SOUND,
        if clone {
            app.voices_dir().display().to_string()
        } else {
            app.announcer_dir().display().to_string()
        }
    ));
    if clone {
        clone_panel(ui, app, st);
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##announcer-text", &mut st.text)
        .hint(if clone { "What the character says" } else { "What the announcer says" })
        .build();
    ui.set_next_item_width(160.0);
    ui.input_text("##announcer-clip", &mut st.clip)
        .hint("Clip id, e.g. superbounce")
        .build();
    if !clone {
        ui.same_line();
        ui.set_next_item_width(-1.0);
        ui.input_text("##announcer-voice", &mut st.voice)
            .hint("Voice id (default announcer voice)")
            .build();
    }

    let can = !st.text.trim().is_empty();
    if ui.button(format!("{} Say it##announcer-say", icon::SOUND)) && can {
        let params = if clone {
            SayParams {
                text: st.text.clone(),
                voice: Some(CHARACTERS[st.character.min(CHARACTERS.len() - 1)].id.to_owned()),
                clip: (!st.clip.trim().is_empty()).then(|| st.clip.trim().to_owned()),
                reference: (!st.reference.trim().is_empty())
                    .then(|| st.reference.trim().to_owned()),
                engine: Engine::Clone,
                play: true,
                ..SayParams::default()
            }
        } else {
            SayParams {
                text: st.text.clone(),
                voice: (!st.voice.trim().is_empty()).then(|| st.voice.trim().to_owned()),
                clip: (!st.clip.trim().is_empty()).then(|| st.clip.trim().to_owned()),
                play: true,
                ..SayParams::default()
            }
        };
        st.last = Some(app.voice_say(params));
        st.committed = None;
    }
    ui.same_line();
    if ui.button("Stop##announcer-stop") {
        app.sound_stop();
    }
    ui.separator();
    match &st.last {
        None => ui.text_disabled("Nothing said yet."),
        Some(Err(VoiceError::NoKey)) => {
            ui.text_colored(
                theme::WARN,
                format!("{} Add your Fish Audio key", icon::ALERT),
            );
            ui.text_wrapped(
                "There is no key in the Keychain (service \"fish-audio\"). Make one at \
                 https://fish.audio/app/api-keys/ and run this once in a terminal. Leave \
                 the key off after -w to be asked for it.",
            );
            let mut command = voice::key::ADD_COMMAND.to_owned();
            ui.set_next_item_width(-1.0);
            ui.input_text("##announcer-command", &mut command).build();
        }
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
        }
        Some(Ok(said)) => {
            let said = said.clone();
            said_view(ui, app, st, &said);
        }
    }
}

fn said_view(ui: &Ui, app: &mut App, st: &mut AnnouncerUi, s: &Said) {
    ui.text(format!(
        "{} {}",
        icon::CHECK,
        if s.cached {
            "from the cache (the voice was not asked)"
        } else {
            "made by the voice"
        }
    ));
    ui.text_wrapped(format!("Saved: {}", s.file.display()));
    ui.text(format!(
        "{}  {:.1} LUFS, peak {:.1} dBFS",
        clock(s.levels.duration_ms),
        s.levels.lufs,
        s.levels.peak_dbfs
    ));
    ui.text_disabled(format!(
        "from {:.1} LUFS: trimmed {} ms and {} ms, gain {:+.1} dB{}",
        s.raw_levels.lufs,
        s.trimmed.head * 1000 / s.levels.rate.max(1) as usize,
        s.trimmed.tail * 1000 / s.levels.rate.max(1) as usize,
        s.gain_db,
        if s.limited { " (held by the peak ceiling)" } else { "" }
    ));
    ui.text_wrapped(format!(
        "Target {:.1} LUFS ({}{}): {}",
        s.target.lufs,
        s.target.from,
        if s.target.lines > 0 {
            format!(", median of {} lines", s.target.lines)
        } else {
            String::new()
        },
        s.verdict.text
    ));
    if let Some(why) = &s.target.why {
        ui.text_disabled(format!("Not the retail lines: {why}"));
    }
    match &s.playback {
        Playback::Playing { output, .. } => ui.text_disabled(format!("Playing on {output}")),
        Playback::Silent(why) => ui.text_disabled(format!("Not playing: {why}")),
    }
    if ui.button("Play again##announcer-again") {
        let clip = crate::app::audition::Clip {
            rate: s.levels.rate,
            channels: 1,
            pcm: s.pcm.clone(),
        };
        if let Err(e) = app.sound_play_clip(&clip) {
            st.committed = Some(Err(e.to_string()));
        }
    }
    if let Some(r) = &s.ready {
        ui.separator();
        ui.text_wrapped(format!(
            "As a clip: {} in the checkout (Git LFS)",
            r.repo_path
        ));
        if ui.button("Copy into the checkout##announcer-commit") {
            st.committed = Some(
                app.voice_commit(&r.clip, Some(&s.file), None, r.exists)
                    .map(|(to, r)| {
                        format!(
                            "Copied to {}. git add it{}",
                            to.display(),
                            if r.exists {
                                String::new()
                            } else {
                                format!(", and add to boot::medal_announcer::CLIPS: {}", r.clips_entry)
                            }
                        )
                    })
                    .map_err(|e| e.to_string()),
            );
        }
        if r.exists {
            ui.text_disabled("This clip exists: copying replaces it.");
        }
    }
    match &st.committed {
        Some(Ok(t)) => ui.text_wrapped(t),
        Some(Err(t)) => ui.text_colored(theme::WARN, format!("{} {t}", icon::ALERT)),
        None => {}
    }
}
