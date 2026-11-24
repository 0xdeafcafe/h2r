//! The voice document (`Doc::Voice`, docs/editor/studio-flow.md, "Voices
//! and the announcer"): one voice, a game character's local clone or the
//! announcer's Fish Audio voice, in two panes. **Setup** is the model, the
//! dataset built from the user's own disc, the fine-tune and the jobs'
//! progress ([`setup`]); **Lines** is the batch of
//! [`crate::app::voice_batch`] with this voice as its default ([`lines`]).
//! A voice that is not set up opens on Setup. The Browser's **Voices** kind
//! lists them, Go to anything finds them and a dialogue sound's **Lines**
//! pane starts a line (`ui::sound`); there is no other way in.
//!
//! A line the voice already said comes back at once from the cache. The
//! batch says one row per frame, the row shown "running" while the model is
//! asked; the clone's install, transcription and fine-tune run in the
//! background; the pane shows their progress. Like the sound editor, the
//! document draws over the Tags tool's surface while its tab is in front.
use dear_imgui_rs::{Condition, ItemHoveredFlags, Ui};

use crate::app::announcer::Said;
use crate::app::voices::{ANNOUNCER, Pane};
use crate::app::{App, Doc};
use crate::theme::icon;
use crate::voice::VoiceError;
use crate::voice::dataset;

pub(super) mod lines;
mod setup;

/// The document's UI-only state.
#[derive(Default)]
pub struct VoicesUi {
    /// The voice the state below was read for: a change resets it.
    shown: Option<String>,
    /// The announcer's line, its clip id and voice id.
    text: String,
    clip: String,
    voice: String,
    /// Lines typed or pasted, one per line, not yet rows.
    paste: String,
    /// The target-sound picker's search, and the tags it lists (read when
    /// the picker first opens, so it is not read every frame).
    tag_filter: String,
    tags: Option<Result<Vec<String>, String>>,
    /// What the last import or save said.
    batch_note: Option<Result<String, String>>,
    /// The clone's reference line, or empty.
    reference: String,
    /// The least minutes of speech a fine-tune starts from; lowered to
    /// train on a small dataset. `None` until Setup first draws.
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

/// The window, when the front document is a voice.
pub fn window(ui: &Ui, app: &mut App, st: &mut VoicesUi, dialogs: bool) {
    if app.front_doc() != Some(Doc::Voice) || app.tool != app.doc_tool(Doc::Voice) {
        return;
    }
    let Some(id) = app.voices.doc.clone() else {
        return;
    };
    let Some(status) = app.voice_status(&id) else {
        return;
    };
    // Another voice: what was typed for the last one does not carry over.
    if st.shown.as_deref() != Some(id.as_str()) {
        *st = VoicesUi {
            shown: Some(id.clone()),
            ..VoicesUi::default()
        };
    }
    ui.window("Voice###voice")
        .size([620.0, 700.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {}", icon::SOUND, status.name));
            ui.same_line();
            ui.text_disabled(status.label());
            if ui
                .selectable_config("Setup##voice-pane-setup")
                .selected(app.voices.pane == Pane::Setup)
                .size([80.0, 0.0])
                .build()
            {
                app.voices.pane = Pane::Setup;
            }
            ui.same_line();
            if ui
                .selectable_config("Lines##voice-pane-lines")
                .selected(app.voices.pane == Pane::Lines)
                .size([80.0, 0.0])
                .build()
            {
                app.voices.pane = Pane::Lines;
            }
            ui.separator();
            match (app.voices.pane, id.as_str()) {
                (Pane::Setup, ANNOUNCER) => setup::announcer(ui),
                (Pane::Setup, _) => {
                    if let Some(c) = dataset::character(&id) {
                        setup::draw(ui, app, st, c);
                    }
                }
                (Pane::Lines, ANNOUNCER) => lines::announcer(ui, app, st),
                (Pane::Lines, _) => lines::character(ui, app, st, &id, dialogs),
            }
        });
}

/// A button that is off, with its reason shown on hover, when `why` is
/// set. True when it was clicked and is on.
fn action(ui: &Ui, label: &str, why: Option<&str>) -> bool {
    let off = ui.begin_disabled_with_cond(why.is_some());
    let clicked = ui.button(label);
    off.end();
    if let Some(w) = why
        && ui.is_item_hovered_with_flags(ItemHoveredFlags::ALLOW_WHEN_DISABLED)
    {
        ui.tooltip_text(w);
    }
    clicked && why.is_none()
}

fn contains(hay: &str, needle: &str) -> bool {
    needle.trim().is_empty() || hay.to_lowercase().contains(&needle.trim().to_lowercase())
}
