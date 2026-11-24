//! The Voices window: a voice says lines, you hear them, and see how they
//! sit against the lines they should match. Fish Audio's announcer voice
//! says one line at a time. A game character's local clone says a batch:
//! one flow top to bottom (1 voice, 2 lines, 3 say, 4 review, 5 use), the
//! rows of [`crate::app::voice_batch`], with the dataset and the fine-tune
//! under a collapsed "Voice setup". Making a line is the app's
//! ([`crate::app::announcer`], [`crate::app::voices`]); the agent API's
//! `voice.say`, `voice.clone.say` and `voice.say_batch` make it the same
//! way. View > Voices opens it.
//!
//! A line the voice already said comes back at once from the cache. The
//! batch says one row per frame, the row shown "running" while the model is
//! asked; the clone's install, transcription and fine-tune run in the
//! background; the window shows their progress.
use dear_imgui_rs::{Condition, ItemHoveredFlags, TreeNodeFlags, Ui};

use crate::app::App;
use crate::app::announcer::{Playback, SayParams, Said};
use crate::app::voice_batch::{Row, RowState};
use crate::app::voices::{BuildParams, MIN_TRAIN_MINUTES};
use crate::theme::{self, icon};
use crate::voice::dataset::{self, CHARACTERS};
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
    /// The character cloned ([`CHARACTERS`]): the batch's voice.
    character: usize,
    /// Lines typed or pasted, one per line, not yet rows.
    paste: String,
    /// The voice picker's search.
    voice_filter: String,
    /// The search of a row's own voice picker.
    row_filter: String,
    /// The target-sound picker's search, and the tags it lists (read when
    /// the picker first opens, so it is not read every frame).
    tag_filter: String,
    tags: Option<Result<Vec<String>, String>>,
    /// What the last import or save said.
    batch_note: Option<Result<String, String>>,
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

pub fn window(ui: &Ui, app: &mut App, st: &mut AnnouncerUi, dialogs: bool) {
    if !st.open {
        return;
    }
    ui.window("Voices###announcer")
        .size([620.0, 700.0], Condition::FirstUseEver)
        .build(|| {
            body(ui, app, st, dialogs);
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

/// What the picker shows of a character: its dataset and fine-tune.
struct VoiceInfo {
    built: bool,
    minutes: f64,
    pending: usize,
    tuned: bool,
}

impl VoiceInfo {
    fn of(app: &App, id: &str) -> VoiceInfo {
        let dir = app.voices_dir().join(id);
        let lines = dataset::read_manifest(&dir.join(dataset::MANIFEST)).ok();
        let s = lines.as_deref().map(dataset::summarize);
        VoiceInfo {
            built: lines.is_some(),
            minutes: s.as_ref().map_or(0.0, |s| s.minutes()),
            pending: s.as_ref().map_or(0, |s| s.pending_asr),
            tuned: crate::voice::clone::trained(&dir, app.voices.backend()).is_some(),
        }
    }

    /// `12.3 min, fine-tuned`, or why it is not ready.
    fn label(&self) -> String {
        if !self.built {
            return "no dataset yet (Voice setup)".into();
        }
        format!(
            "{:.1} min of speech{}{}",
            self.minutes,
            if self.tuned { ", fine-tuned" } else { "" },
            if self.pending > 0 {
                format!(", {} lines to transcribe", self.pending)
            } else {
                String::new()
            }
        )
    }
}

fn contains(hay: &str, needle: &str) -> bool {
    needle.trim().is_empty() || hay.to_lowercase().contains(&needle.trim().to_lowercase())
}

/// A character picker: a combo with a search, each entry showing its
/// dataset minutes and whether it is fine-tuned. `none` adds a first entry
/// that clears the choice (a row follows the batch's voice). `current` is
/// an index into [`CHARACTERS`], or `None`.
fn voice_picker(
    ui: &Ui,
    app: &App,
    id: &str,
    width: f32,
    current: Option<usize>,
    none: Option<&str>,
    filter: &mut String,
) -> Option<Option<usize>> {
    let preview = match (current, none) {
        (Some(i), _) => CHARACTERS[i.min(CHARACTERS.len() - 1)].name,
        (None, Some(n)) => n,
        (None, None) => "Pick a voice",
    };
    let mut picked = None;
    ui.set_next_item_width(width);
    if let Some(_combo) = ui.begin_combo(id, preview) {
        ui.set_next_item_width(-1.0);
        ui.input_text("##voice-search", filter).hint("Search voices").build();
        if let Some(n) = none
            && ui.selectable_config(n).selected(current.is_none()).build()
        {
            picked = Some(None);
        }
        for (i, ch) in CHARACTERS.iter().enumerate() {
            if !contains(ch.name, filter) && !contains(ch.id, filter) {
                continue;
            }
            let info = VoiceInfo::of(app, ch.id);
            if ui
                .selectable_config(format!("{}  ({})##v{i}", ch.name, info.label()))
                .selected(current == Some(i))
                .build()
            {
                picked = Some(Some(i));
            }
        }
    }
    picked
}

/// The sound tags of the shown map, for the target picker.
fn load_tags(app: &App) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = app
        .sound_tags()
        .map_err(|e| format!("{e}: open a map first"))?
        .into_iter()
        .filter(|t| dataset::is_dialogue(&t.name))
        .map(|t| t.name)
        .collect();
    names.sort();
    Ok(names)
}

/// What one row asks of the window after the list is drawn.
enum RowAction {
    Play(u64),
    Rerun(u64),
    Remove(u64),
}

/// The batch: one block per row.
fn rows_view(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    let mut acts: Vec<RowAction> = Vec::new();
    let idle_row = |r: &Row| !matches!(r.state, RowState::Running | RowState::Queued);
    for i in 0..app.voices.batch.rows.len() {
        let row_id = app.voices.batch.rows[i].id;
        let _id = ui.push_id(row_id as usize);
        let current = app.voices.batch.rows[i]
            .character
            .as_deref()
            .and_then(|c| CHARACTERS.iter().position(|x| x.id == c));
        let blocked = app.voices.batch.blocked(&app.voices.batch.rows[i]);
        {
            let r = &mut app.voices.batch.rows[i];
            ui.text(format!("{:>2}", i + 1));
            ui.same_line();
            ui.set_next_item_width(-190.0);
            ui.input_text("##row-text", &mut r.text)
                .hint("What the character says")
                .build();
            ui.same_line();
            let busy = !idle_row(r);
            let has_said = r.said.is_some();
            if action(
                ui,
                "Play##row-play",
                (!has_said || busy).then_some("Say the line first"),
            ) {
                acts.push(RowAction::Play(row_id));
            }
            ui.same_line();
            let say_off = if busy {
                Some("This line is being said".to_owned())
            } else {
                blocked.map(|b| format!("Cannot say it yet: {b}"))
            };
            if action(
                ui,
                if r.state == RowState::Done { "Re-run##row-rerun" } else { "Say##row-rerun" },
                say_off.as_deref(),
            ) {
                acts.push(RowAction::Rerun(row_id));
            }
            ui.same_line();
            if action(ui, "Remove##row-remove", (r.state == RowState::Running).then_some("It is being said")) {
                acts.push(RowAction::Remove(row_id));
            }
        }
        // The row's voice, output name and target sound.
        ui.indent_by(24.0);
        let mut f = std::mem::take(&mut st.row_filter);
        let picked = voice_picker(ui, app, "##row-voice", 170.0, current, Some("Batch voice"), &mut f);
        st.row_filter = f;
        if let Some(p) = picked {
            app.voices.batch.rows[i].character = p.map(|i| CHARACTERS[i].id.to_owned());
        }
        ui.same_line();
        ui.set_next_item_width(150.0);
        ui.input_text("##row-clip", &mut app.voices.batch.rows[i].clip)
            .hint("Output name (required)")
            .build();
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Names the saved clip: art/announcer/<name>.wav. Lowercase letters, digits and -.",
            );
        }
        ui.same_line();
        let target = app.voices.batch.rows[i].tag.clone();
        ui.set_next_item_width(-1.0);
        let preview = target
            .as_deref()
            .map_or_else(
                || "Replaces: nothing (saved as a new clip)".to_owned(),
                |t| format!("Replaces: {t}"),
            );
        if let Some(_combo) = ui.begin_combo("##row-tag", preview) {
            if st.tags.is_none() {
                st.tags = Some(load_tags(app));
            }
            ui.set_next_item_width(-1.0);
            ui.input_text("##tag-search", &mut st.tag_filter)
                .hint("Search the open map's dialogue sounds")
                .build();
            ui.text_disabled(
                "Replaces: the game sound this line stands in for. Kept lines go into the \
                 edit target package over it.",
            );
            if ui.selectable_config("Nothing: save only as the output name's clip").selected(target.is_none()).build() {
                app.voices.batch.rows[i].tag = None;
            }
            match st.tags.clone() {
                Some(Ok(names)) => {
                    let shown: Vec<&String> = names.iter().filter(|n| contains(n, &st.tag_filter)).collect();
                    if shown.len() > 200 {
                        ui.text_disabled(format!("{} match: the first 200 are shown, search to narrow", shown.len()));
                    }
                    for n in shown.into_iter().take(200) {
                        if ui.selectable_config(n).selected(target.as_deref() == Some(n.as_str())).build() {
                            app.voices.batch.rows[i].tag = Some(n.clone());
                        }
                    }
                    if ui.button("Read the map again##tags-refresh") {
                        st.tags = None;
                    }
                }
                Some(Err(why)) => ui.text_disabled(why),
                None => {}
            }
        }
        // What happened.
        let r = &app.voices.batch.rows[i];
        match &r.state {
            RowState::Idle => ui.text_disabled("Not said yet"),
            RowState::Queued => ui.text_disabled("Queued"),
            RowState::Running => ui.text_colored(theme::ACCENT, "Running: the model is saying it"),
            RowState::Failed(why) => ui.text_colored(
                theme::WARN,
                format!("{} {}", icon::ALERT, voice::readable(why)),
            ),
            RowState::Done => {
                if let Some(s) = &r.said {
                    ui.text_colored(
                        theme::OK,
                        format!(
                            "{} {}  {:.1} LUFS{}: {}",
                            icon::CHECK,
                            clock(s.levels.duration_ms),
                            s.levels.lufs,
                            if s.cached { ", from the cache" } else { "" },
                            s.verdict.text
                        ),
                    );
                }
            }
        }
        if r.state == RowState::Idle {
            if let Some(b) = app.voices.batch.blocked(r) {
                ui.text_colored(theme::WARN, format!("{} {b}", icon::ALERT));
            }
        }
        match &r.saved {
            Some(Ok(to)) => ui.text_wrapped(format!("Kept: {to}")),
            Some(Err(why)) => ui.text_colored(
                theme::WARN,
                format!("{} Not kept: {}", icon::ALERT, voice::readable(why)),
            ),
            None => {}
        }
        ui.unindent_by(24.0);
        ui.separator();
    }
    for a in acts {
        match a {
            RowAction::Play(id) => {
                if let Err(e) = app.voice_batch_play(id) {
                    st.batch_note = Some(Err(e));
                }
            }
            RowAction::Rerun(id) => {
                app.voices.batch.queue(Some(&[id]));
            }
            RowAction::Remove(id) => app.voices.batch.remove(id),
        }
    }
}

/// The clone's flow: voice, lines, say, review, use.
fn clone_flow(ui: &Ui, app: &mut App, st: &mut AnnouncerUi, dialogs: bool) {
    ui.text_disabled(format!(
        "{} Lines are saved under {}; nothing leaves this machine.",
        icon::SOUND,
        app.voices_dir().display()
    ));

    // 1. Voice.
    ui.separator_with_text("1  Voice");
    let current = CHARACTERS
        .iter()
        .position(|c| c.id == app.voices.batch.default_character);
    // Voice setup is for the voice picked here, however it was picked.
    if let Some(i) = current {
        st.character = i;
    }
    let mut filter = std::mem::take(&mut st.voice_filter);
    if let Some(Some(i)) = voice_picker(ui, app, "##voices-character", 300.0, current, None, &mut filter) {
        st.character = i;
        app.voices.batch.default_character = CHARACTERS[i].id.to_owned();
    }
    st.voice_filter = filter;
    ui.same_line();
    ui.text_disabled("Voice for every line without its own");
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
    match current {
        Some(i) => {
            let info = VoiceInfo::of(app, CHARACTERS[i].id);
            ui.text_disabled(format!("{}: {}", CHARACTERS[i].name, info.label()));
            if !info.built {
                ui.text_colored(
                    theme::WARN,
                    format!("{} It cannot say lines until Voice setup (below) builds its dataset.", icon::ALERT),
                );
            }
        }
        None => ui.text_disabled("Pick the voice the lines are said in."),
    }

    // 2. Lines.
    ui.separator_with_text("2  Lines");
    ui.text_disabled("One line per row. Paste many, one per line (name<Tab>text gives a line its output name).");
    ui.set_next_item_width(-1.0);
    ui.input_text_multiline("##voices-paste", &mut st.paste, [-1.0, 64.0]).build();
    if action(
        ui,
        "Add lines##voices-add",
        st.paste.trim().is_empty().then_some("Type or paste lines above first"),
    ) {
        let n = app.voices.batch.paste(&st.paste, None);
        st.paste.clear();
        st.batch_note = Some(Ok(format!("Added {n} lines")));
    }
    ui.same_line();
    if action(
        ui,
        &format!("{} Import a text file...##voices-import", icon::FOLDER),
        (!dialogs).then_some("File dialogs are off here: use voice.say_batch {file}"),
    ) && let Some(p) = rfd::FileDialog::new()
        .set_title("A text file, one line per row")
        .add_filter("Text", &["txt"])
        .pick_file()
    {
        st.batch_note = Some(
            app.voices
                .batch
                .import(&p, None)
                .map(|n| format!("Added {n} lines from {}", p.display()))
                .map_err(|e| e.to_string()),
        );
    }
    ui.same_line();
    if ui.button("Clear done##voices-clear") {
        app.voices.batch.clear_done();
    }
    if let Some(n) = &st.batch_note {
        match n {
            Ok(t) => ui.text_disabled(t),
            Err(t) => ui.text_colored(theme::WARN, format!("{} {t}", icon::ALERT)),
        }
    }
    rows_view(ui, app, st);

    // 3. Say.
    ui.separator_with_text("3  Say");
    let c = app.voices.batch.counts();
    let waits = |r: &&Row| matches!(r.state, RowState::Idle | RowState::Failed(_)) && !r.text.trim().is_empty();
    let waiting = app.voices.batch.rows.iter().any(|r| waits(&r));
    let unnamed = app
        .voices
        .batch
        .rows
        .iter()
        .filter(waits)
        .position(|r| r.clip.is_empty() && r.tag.is_none());
    let say_why = if app.voices.batch.active() {
        Some(format!("{} of {} are done", c.done, c.done + c.queued + c.running))
    } else if app.voices.batch.rows.is_empty() {
        Some("Add a line first".to_owned())
    } else if !waiting {
        Some("Every line is said: edit one, or Re-run it".to_owned())
    } else if unnamed.is_some() {
        Some("Every line needs an output name, or a game line it replaces, first".to_owned())
    } else {
        None
    };
    if action(
        ui,
        &format!("{} Say all##voices-say-all", icon::SOUND),
        say_why.as_deref(),
    ) {
        app.voices.batch.queue(None);
    }
    ui.same_line();
    if ui.button("Stop##voices-stop") {
        app.voices.batch.cancel();
        app.sound_stop();
    }
    ui.same_line();
    ui.text_disabled(format!(
        "{} done, {} running, {} queued, {} failed, {} not said",
        c.done, c.running, c.queued, c.failed, c.idle
    ));
    if let Some(w) = &say_why {
        ui.text_disabled(w);
    }

    // 4 and 5. Review, use.
    ui.separator_with_text("4  Review: play each line above, Re-run one you do not like");
    ui.separator_with_text("5  Use");
    ui.checkbox("Replace clips the checkout already has##voices-replace", &mut app.voices.batch.replace);
    let save_why = app.voice_batch_save_problem();
    if action(
        ui,
        "Keep all done lines##voices-save",
        save_why.as_deref(),
    ) {
        let r = app.voice_batch_save(None, None, None);
        let ok = r.iter().filter(|(_, r)| r.is_ok()).count();
        let said = format!("Kept {ok} of {} lines (each line says where, or why not)", r.len());
        st.batch_note = Some(if ok == 0 && !r.is_empty() { Err(said) } else { Ok(said) });
    }
    if let Some(w) = &save_why {
        ui.text_disabled(w);
    } else {
        ui.text_disabled(format!(
            "A line with an output name is copied to art/announcer/<name>.wav of the checkout (git add it); \
             one with a target sound becomes a sound item of the edit target ({}).",
            app.edit_target().unwrap_or_else(|| "none set".into())
        ));
    }

    // Setup.
    ui.separator();
    if ui.collapsing_header("Voice setup: dataset, fine-tune, model##voices-setup", TreeNodeFlags::empty()) {
        setup(ui, app, st);
    }
}

/// Building a character's dataset, its fine-tune, and the model's jobs.
fn setup(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    let c = &CHARACTERS[st.character.min(CHARACTERS.len() - 1)];
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

fn body(ui: &Ui, app: &mut App, st: &mut AnnouncerUi, dialogs: bool) {
    if ui.radio_button_bool("Game character (local clone, many lines)", st.engine == Engine::Clone) {
        st.engine = Engine::Clone;
    }
    ui.same_line();
    if ui.radio_button_bool("Announcer (Fish Audio, one line)", st.engine == Engine::Fish) {
        st.engine = Engine::Fish;
    }
    if st.engine == Engine::Clone {
        clone_flow(ui, app, st, dialogs);
        return;
    }
    ui.text_disabled(format!(
        "{} Lines are saved under {}",
        icon::SOUND,
        app.announcer_dir().display()
    ));
    ui.set_next_item_width(-1.0);
    ui.input_text("##announcer-text", &mut st.text)
        .hint("What the announcer says")
        .build();
    ui.set_next_item_width(160.0);
    ui.input_text("##announcer-clip", &mut st.clip)
        .hint("Clip id, e.g. superbounce")
        .build();
    ui.same_line();
    ui.set_next_item_width(-1.0);
    ui.input_text("##announcer-voice", &mut st.voice)
        .hint("Voice id (default announcer voice)")
        .build();

    let why = st.text.trim().is_empty().then_some("Type a line first");
    if action(ui, &format!("{} Say it##announcer-say", icon::SOUND), why) {
        let params = SayParams {
            text: st.text.clone(),
            voice: (!st.voice.trim().is_empty()).then(|| st.voice.trim().to_owned()),
            clip: (!st.clip.trim().is_empty()).then(|| st.clip.trim().to_owned()),
            play: true,
            ..SayParams::default()
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
            ui.text_colored(theme::WARN, format!("{} {}", icon::ALERT, e.readable()));
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
