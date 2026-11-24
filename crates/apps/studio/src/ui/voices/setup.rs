//! The voice document's **Setup** pane: the model that says the voice's
//! lines, its dataset built from the user's own disc, the fine-tune and
//! the progress of the jobs that make them (docs/editor/studio-flow.md,
//! "Voices and the announcer"). The work is the app's
//! ([`crate::app::voices`]); the agent API's `voice.dataset.*` and
//! `voice.clone.*` run the same calls.
use dear_imgui_rs::Ui;

use super::VoicesUi;
use crate::app::App;
use crate::app::voices::{BuildParams, MIN_TRAIN_MINUTES};
use crate::theme::{self, icon};
use crate::voice::dataset::Character;
use crate::voice::sidecar::Backend;
use crate::voice;

/// What a backend is called in the picker.
fn backend_label(b: Backend) -> &'static str {
    match b {
        Backend::F5Mlx => "f5-mlx: zero-shot or fine-tune",
        Backend::Qwen3Tts => "qwen3-tts: zero-shot only (no training)",
    }
}

/// The announcer's Setup: its key, and nothing else.
pub(super) fn announcer(ui: &Ui) {
    ui.text_wrapped(
        "The announcer is Fish Audio's voice: it needs your API key in the Keychain \
         (service \"fish-audio\") and nothing else. Make a key at \
         https://fish.audio/app/api-keys/ and run this once in a terminal. Leave the \
         key off after -w to be asked for it.",
    );
    let mut command = voice::key::ADD_COMMAND.to_owned();
    ui.set_next_item_width(-1.0);
    ui.input_text("##announcer-command", &mut command).build();
}

/// A character's Setup, the model first.
pub(super) fn draw(ui: &Ui, app: &mut App, st: &mut VoicesUi, c: &'static Character) {
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
    ui.separator();
    character(ui, app, st, c);
}

/// A character's Setup: the model, the dataset, the fine-tune and the
/// jobs' progress.
fn character(ui: &Ui, app: &mut App, st: &mut VoicesUi, c: &'static Character) {
    ui.text(format!("For {}", c.name));
    ui.text_disabled(
        "Needed once per character. The dataset is built from your own disc and stays on this machine.",
    );
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
            .map_err(|e| e.readable()),
        );
    }
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
                    .map_err(|e| e.readable()),
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
            "This model clones from a reference line with no training. It needs a \
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
                    format!("{line}: {}", voice::readable(j["why"].as_str().unwrap_or_default())),
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
    ui.text_disabled("The reference line sets the delivery every line copies.");
}
