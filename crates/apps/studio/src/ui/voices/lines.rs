//! The voice document's **Lines** pane, and the sound document's: a
//! line is a row of the batch ([`crate::app::voice_batch`]), one block per
//! row, said by the voice that is open (docs/editor/studio-flow.md,
//! "Voices and the announcer"). A game character's local clone says rows
//! the model is asked for one per frame; Fish Audio's announcer says one
//! line at a time. A line a sound's pane starts is the same row, with the
//! sound as its target, and its said waveform draws under the sound's own
//! on one axis ([`sound_pane`]). Making a line is the app's
//! ([`crate::app::announcer`], [`crate::app::voices`]); the agent API's
//! `voice.say`, `voice.clone.say` and `voice.say_batch` make it the same
//! way.
use dear_imgui_rs::Ui;

use super::{VoicesUi, action, clock, contains};
use crate::app::announcer::{Playback, SayParams, Said};
use crate::app::audition::Clip;
use crate::app::voice_batch::{Row, RowState, axis_columns};
use crate::app::voices::Pane;
use crate::app::{Action, App};
use crate::theme::{self, icon};
use crate::voice::dataset;
use crate::voice::{self, VoiceError};

/// What one row asks of the pane after the list is drawn.
enum RowAction {
    Play(u64),
    Rerun(u64),
    Remove(u64),
}

/// The rows the voice `id` says: one block each.
fn rows_view(ui: &Ui, app: &mut App, st: &mut VoicesUi, id: &str) {
    let mut acts: Vec<RowAction> = Vec::new();
    let idle_row = |r: &Row| !matches!(r.state, RowState::Running | RowState::Queued);
    let mut shown = 0;
    for i in 0..app.voices.batch.rows.len() {
        if app.voices.batch.voice_of(&app.voices.batch.rows[i]).as_deref() != Some(id) {
            continue;
        }
        shown += 1;
        let row_id = app.voices.batch.rows[i].id;
        let _id = ui.push_id(row_id as usize);
        let blocked = app.voices.batch.blocked(&app.voices.batch.rows[i]);
        {
            let r = &mut app.voices.batch.rows[i];
            ui.text(format!("{shown:>2}"));
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
        // The row's output name and target sound.
        ui.indent_by(24.0);
        ui.set_next_item_width(150.0);
        ui.input_text("##row-clip", &mut app.voices.batch.rows[i].clip)
            .hint("Output name (required)")
            .build();
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Names the sound item. Lowercase letters, digits and -.",
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
            if ui.selectable_config("Standalone sound item").selected(target.is_none()).build() {
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
        row_state(ui, app, i);
        ui.unindent_by(24.0);
        ui.separator();
    }
    if shown == 0 {
        ui.text_disabled("No line yet: add some above, or start one from a sound's Lines pane.");
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

/// What happened to row `i`: its state, what it said, what was kept.
fn row_state(ui: &Ui, app: &App, i: usize) {
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
    if r.state == RowState::Idle
        && let Some(b) = app.voices.batch.blocked(r)
    {
        ui.text_colored(theme::WARN, format!("{} {b}", icon::ALERT));
    }
    match &r.saved {
        Some(Ok(to)) => ui.text_wrapped(format!("Kept: {to}")),
        Some(Err(why)) => ui.text_colored(
            theme::WARN,
            format!("{} Not kept: {}", icon::ALERT, voice::readable(why)),
        ),
        None => {}
    }
}

/// A game character's Lines: add lines, say them, review, keep.
pub(super) fn character(ui: &Ui, app: &mut App, st: &mut VoicesUi, id: &str, dialogs: bool) {
    ui.text_disabled(format!(
        "{} Lines are saved under {}; nothing leaves this machine.",
        icon::SOUND,
        app.voices_dir().display()
    ));
    if let Some(v) = app.voice_status(id)
        && v.needs_setup()
    {
        ui.text_colored(
            theme::WARN,
            format!("{} {} cannot say lines until Setup builds its dataset.", icon::ALERT, v.name),
        );
        ui.same_line();
        if ui.button("Open Setup##voice-to-setup") {
            app.voices.pane = Pane::Setup;
        }
    }

    // Add lines.
    ui.separator_with_text("Lines");
    ui.text_disabled("One line per row. Paste many, one per line (name<Tab>text gives a line its output name).");
    ui.set_next_item_width(-1.0);
    ui.input_text_multiline("##voices-paste", &mut st.paste, [-1.0, 64.0]).build();
    if action(
        ui,
        "Add lines##voices-add",
        st.paste.trim().is_empty().then_some("Type or paste lines above first"),
    ) {
        let n = app.voices.batch.paste(&st.paste, Some(id));
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
                .import(&p, Some(id))
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
    rows_view(ui, app, st, id);

    // Say.
    ui.separator_with_text("Say");
    let mine: Vec<&Row> = app
        .voices
        .batch
        .rows
        .iter()
        .filter(|r| app.voices.batch.voice_of(r).as_deref() == Some(id))
        .collect();
    let waits = |r: &&&Row| matches!(r.state, RowState::Idle | RowState::Failed(_)) && !r.text.trim().is_empty();
    let waiting: Vec<u64> = mine.iter().filter(waits).map(|r| r.id).collect();
    let unnamed = mine
        .iter()
        .filter(waits)
        .any(|r| r.clip.is_empty() && r.tag.is_none());
    let c = app.voices.batch.counts();
    let say_why = if app.voices.batch.active() {
        Some(format!("{} of {} are done", c.done, c.done + c.queued + c.running))
    } else if mine.is_empty() {
        Some("Add a line first".to_owned())
    } else if waiting.is_empty() {
        Some("Every line is said: edit one, or Re-run it".to_owned())
    } else if unnamed {
        Some("Every line needs an output name, or a game line it replaces, first".to_owned())
    } else {
        None
    };
    if action(
        ui,
        &format!("{} Say all##voices-say-all", icon::SOUND),
        say_why.as_deref(),
    ) {
        app.voices.batch.queue(Some(waiting.as_slice()));
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


}

/// The **Lines** pane of a sound's document: the batch's rows whose
/// target is the game sound `tag`, with a button that starts one. A line
/// the voice said draws its waveform under the sound's own on the same
/// time axis (`original_ms` long, the pane's width), with its levels in
/// words. **Save** keeps a line as a `sound` item of the edit target.
pub(crate) fn sound_pane(ui: &Ui, app: &mut App, tag: &str, original_ms: Option<u64>) {
    ui.separator_with_text("Lines");
    let ids: Vec<u64> = app.sound_lines(tag).iter().map(|r| r.id).collect();
    let has_line = !ids.is_empty();
    if action(
        ui,
        "New line##sound-line-new",
        has_line.then_some("This sound has a line already"),
    ) {
        app.sound_new_line(tag);
    }
    ui.same_line();
    ui.text_disabled("A new take of this sound, said by the voice its path maps to");
    for id in ids {
        let _id = ui.push_id(id as usize);
        let Some(i) = app.voices.batch.rows.iter().position(|r| r.id == id) else {
            continue;
        };
        let voice = app.voices.batch.voice_of(&app.voices.batch.rows[i]);
        let status = voice.as_deref().and_then(|v| app.voice_status(v));
        match (&voice, &status) {
            (Some(_), Some(v)) => {
                ui.text_disabled(format!("Said by {}: {}", v.name, v.label()));
            }
            _ => ui.text_colored(
                theme::WARN,
                format!("{} No voice says this sound's path: pick one in the Voices kind", icon::ALERT),
            ),
        }
        if let (Some(v), Some(s)) = (&voice, &status)
            && s.needs_setup()
        {
            ui.text_colored(
                theme::WARN,
                format!("{} {} is not set up", icon::ALERT, s.name),
            );
            ui.same_line();
            if ui.button("Open its Setup##sound-line-setup") {
                app.dispatch(Action::OpenAsset {
                    asset: format!("voice:{v}"),
                    map: None,
                });
            }
        }
        let blocked = app.voices.batch.problem(&app.voices.batch.rows[i]);
        let state = app.voices.batch.rows[i].state.clone();
        {
            let r = &mut app.voices.batch.rows[i];
            ui.set_next_item_width(-190.0);
            ui.input_text("##sound-line-text", &mut r.text)
                .hint("What is said")
                .build();
        }
        ui.same_line();
        let busy = matches!(state, RowState::Running | RowState::Queued);
        let why = if busy {
            Some("This line is being said".to_owned())
        } else {
            blocked.map(|b| format!("Cannot say it yet: {b}"))
        };
        if action(
            ui,
            if state == RowState::Done { "Re-run##sound-line-say" } else { "Say##sound-line-say" },
            why.as_deref(),
        ) {
            app.voices.batch.queue(Some(&[id]));
        }
        ui.same_line();
        if action(ui, "Remove##sound-line-remove", (state == RowState::Running).then_some("It is being said")) {
            app.voices.batch.remove(id);
            continue;
        }
        row_state(ui, app, i);
        if let Some(said) = app.voices.batch.rows[i].said.clone()
            && state == RowState::Done
        {
            said_under_original(ui, &said, original_ms);
            if action(ui, "Play##sound-line-play", None) {
                let _ = app.voice_batch_play(id);
            }
        }
        ui.separator();
    }
}

/// The said line's waveform, on the original's time axis: the pane's
/// width is `original_ms` (the said line's own length when the sound could
/// not be decoded), and a line longer than that is cut where the axis ends.
fn said_under_original(ui: &Ui, said: &Said, original_ms: Option<u64>) {
    let width = ui.content_region_avail()[0].max(64.0);
    let height = 48.0;
    let span = original_ms.unwrap_or(said.levels.duration_ms).max(1);
    let cols = axis_columns(span, said.levels.duration_ms, width as usize);
    let clip = Clip {
        rate: said.levels.rate,
        channels: 1,
        pcm: said.pcm.clone(),
    };
    let peaks = clip.peaks(cols);
    let origin = ui.cursor_screen_pos();
    ui.dummy([width, height]);
    let dl = ui.get_window_draw_list();
    dl.add_rect(origin, [origin[0] + width, origin[1] + height], theme::RAISED)
        .filled(true)
        .build();
    let mid = origin[1] + height * 0.5;
    for (i, [low, high]) in peaks.iter().enumerate() {
        let x = origin[0] + i as f32;
        let top = mid - f32::from(*high) / 32768.0 * height * 0.5;
        let bottom = mid - f32::from(*low) / 32768.0 * height * 0.5;
        dl.add_rect([x, top], [x + 1.0, bottom.max(top + 1.0)], theme::OK)
            .filled(true)
            .build();
    }
    let longer = said.levels.duration_ms.saturating_sub(span);
    ui.text_disabled(format!(
        "said {} at {:.1} LUFS, on the sound's axis ({}){}",
        clock(said.levels.duration_ms),
        said.levels.lufs,
        clock(span),
        if longer > 0 {
            format!(": {} longer than the sound, cut here", clock(longer))
        } else {
            String::new()
        }
    ));
}

/// The announcer's Lines: one line at a time, said by Fish Audio.
pub(super) fn announcer(ui: &Ui, app: &mut App, st: &mut VoicesUi) {
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
        app.voices.announcer_save = None;
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

fn said_view(ui: &Ui, app: &mut App, st: &mut VoicesUi, s: &Said) {
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
    if let Some(ready) = &s.ready {
        app.voices.announcer_save = Some((s.file.clone(), ready.clip.clone()));
    }
    match &st.committed {
        Some(Ok(t)) => ui.text_wrapped(t),
        Some(Err(t)) => ui.text_colored(theme::WARN, format!("{} {t}", icon::ALERT)),
        None => {}
    }
}
