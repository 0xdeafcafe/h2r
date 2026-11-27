//! The sound editor, the document of a `snd!` tag
//! (docs/editor/studio-redesign.md, "The sound editor"): it draws the
//! front tab's sound while the Tags tool shows it (else the selected one:
//! a browsed map has no tabs). A row for each permutation with its
//! duration and the levels `sound.measure` gives it, the picked one's waveform, **Play** and
//! **Stop**, a click on the waveform to play from there, and under the
//! waveform the permutation's mouth curve as a filled track on the same
//! time axis — read even when the audio itself cannot be decoded. A drag
//! on the track redraws the curve (one stroke, one step of the sound's
//! document), **From the sound** fills it from the sound's loudness at the
//! window's gain and smoothing, **Revert** gives the file's back, and
//! **Undo** takes the document's newest step back, a redraw or a field's
//! write, as Cmd+Z does (`Action::UndoDoc`,
//! docs/editor/studio-flow.md, "Shared patterns"). The redraws are
//! `App::sound_mouth_*` ([`crate::app::sound_mouth`]), which `sound.mouth`
//! shares, and save with the map.
//! Decoding and playing are the app's ([`crate::app::audition`]); the
//! agent API's `sound.read`, `sound.play` and `sound.mouth` use the same
//! calls.
//!
//! Under the track, **Preview on** opens a picker of the map's bipeds; the
//! picked one draws with its `talk` overlay at the weight the curve gives
//! at the playhead (the face preview, docs/editor/rich-editors.md, slice
//! A3, [`crate::app::anim`]'s `face_pose`, which `sound.face` shares).
//!
//! Under the face, the **Lines** pane (`ui::voices::lines::sound_pane`)
//! holds the batch's rows whose target is this sound: **New line** starts
//! one, said by the voice the sound's path maps to, and the said line's
//! waveform draws under the sound's own on the same axis.
//!
//! The window is one scrolling body. The waveform and the mouth track
//! have heights of their own; they never take the leftover height.
use std::sync::Arc;

use dear_imgui_rs::{Condition, MouseButton, Ui};
use map::TagId;
use tags::sound::MOUTH_HZ;

use crate::app::anim::{AnimError, FacePose};
use crate::app::audition::{Clip, SoundError, SoundInfo, SoundRow};
use crate::app::{Action, App, Doc, ToolKind};
use crate::theme::{self, icon};
use crate::ui::anim::{PICTURE, Preview};
use crate::viewport::Gpu;

/// The map shown and the tag.
type Key = (Option<String>, TagId);

/// The waveform's height, pixels.
const WAVE_HEIGHT: f32 = 96.0;
/// The mouth track's height, pixels.
const MOUTH_HEIGHT: f32 = 48.0;

/// The window's own state: the permutation picked, and what was decoded for
/// it, so a frame does not decode again (and a refusal is not asked for
/// again each frame).
#[derive(Default)]
pub struct SoundUi {
    info: Option<(Key, Result<SoundInfo, SoundError>)>,
    /// Each permutation's levels, measured once per sound.
    rows: Option<(Key, Result<Vec<SoundRow>, SoundError>)>,
    /// Pitch range and permutation picked.
    pick: (usize, usize),
    clip: Option<((Key, (usize, usize)), Result<Arc<Clip>, SoundError>)>,
    mouth: Option<((Key, (usize, usize)), Result<Option<Arc<[u8]>>, SoundError>)>,
    peaks: Option<((Key, (usize, usize), usize), Vec<[i16; 2]>)>,
    problem: Option<String>,
    face: Face,
    /// The curve being drawn (one stroke), and the last tick the stroke
    /// touched.
    draft: Option<(Vec<u8>, usize)>,
    /// **From the sound**'s gain and smoothing.
    gain: f32,
    smoothing: f32,
}

impl SoundUi {
    /// The window's state with **From the sound**'s numbers at their
    /// defaults.
    pub fn new() -> Self {
        Self { gain: 1.0, smoothing: 0.3, ..Self::default() }
    }
}

/// The picker that **Preview on** opens.
const FACE_PICK: &str = "##face-pick";

/// The face preview: the object picked, and the picture drawn for one
/// aperture byte (a new byte draws a new one; the same byte keeps it).
#[derive(Default)]
struct Face {
    on: Option<(TagId, String)>,
    picture: Option<FacePicture>,
}

/// The sound, the permutation, the object, and the byte (`None`: no
/// byte, the mouth closed).
type FaceKey = (Key, (usize, usize), TagId, Option<u8>);

struct FacePicture {
    key: FaceKey,
    preview: Option<Preview>,
    pose: Result<FacePose, AnimError>,
}

impl Face {
    fn clear(&mut self, gpu: &mut Gpu<'_>) {
        if let Some(preview) = self.picture.take().and_then(|p| p.preview) {
            let _ = gpu.renderer.unregister_external_texture(preview.id);
        }
    }
}

/// `m:ss.t` of `ms`.
fn clock(ms: u64) -> String {
    format!("{}:{:02}.{}", ms / 60_000, ms / 1000 % 60, ms / 100 % 10)
}

/// A permutation's row: its place, length, levels (or why there are none)
/// and mouth curve, the numbers `sound.measure` and `sound.mouth` give.
pub fn row_text(row: &SoundRow) -> String {
    let head = format!("[{}.{}]", row.range, row.permutation);
    let mouth = row
        .mouth_ticks
        .map_or(String::new(), |t| format!("  mouth {t} ticks"));
    match &row.levels {
        Ok(l) => format!(
            "{head}  {:.2} s  {:.1} LUFS  peak {:.1} dBFS{}{mouth}",
            l.duration_ms as f64 / 1000.0,
            l.lufs,
            l.peak_dbfs,
            if l.clipped > 0 { "  clipped" } else { "" },
        ),
        Err(why) => format!("{head}  {} bytes  {why}{mouth}", row.encoded_bytes),
    }
}

/// The sound the editor draws: the front tab's while the Tags tool shows
/// it, else the one selected there (a browsed map has no tabs). `None` for any
/// other kind.
pub fn target(app: &App) -> Option<(TagId, String)> {
    if app.tool != ToolKind::Tags
        || matches!(app.front_doc(), Some(Doc::Voice | Doc::Forge | Doc::SourceAsset(_)))
    {
        return None;
    }
    let id = app.front_tag_doc().or(app.shown_tags().selected)?;
    let entry = app.shown_tags().tag(id)?;
    (entry.group.0 == *b"snd!").then(|| (id, entry.name.clone()))
}

/// The window, when the front document is a sound.
pub fn window(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut SoundUi) {
    let Some((id, name)) = target(app) else {
        return;
    };
    let key: Key = (app.shown_map_name().map(str::to_owned), id);
    ui.window("Sound###sound")
        .size([440.0, 380.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {name}", icon::SOUND));
            body(ui, app, gpu, st, id, key);
            // The sound's own lines: new takes the voice that says its
            // path makes, under the sound's waveform on one axis
            // (docs/editor/studio-flow.md, "Voices and the announcer").
            let original = st
                .clip
                .as_ref()
                .and_then(|c| c.1.as_ref().ok())
                .map(|c| c.duration_ms());
            super::voices::lines::sound_pane(ui, app, &name, original);
        });
}

fn draw_waveform(ui: &Ui, origin: [f32; 2], width: f32, peaks: &[[i16; 2]]) {
    let dl = ui.get_window_draw_list();
    dl.add_rect(
        origin,
        [origin[0] + width, origin[1] + WAVE_HEIGHT],
        theme::RAISED,
    )
    .filled(true)
    .build();
    let mid = origin[1] + WAVE_HEIGHT * 0.5;
    let column = width / peaks.len().max(1) as f32;
    for (i, [low, high]) in peaks.iter().enumerate() {
        let x = origin[0] + i as f32 * column;
        let top = mid - f32::from(*high) / 32768.0 * WAVE_HEIGHT * 0.5;
        let bottom = mid - f32::from(*low) / 32768.0 * WAVE_HEIGHT * 0.5;
        dl.add_rect([x, top], [x + column, bottom.max(top + 1.0)], theme::ACCENT)
            .filled(true)
            .build();
    }
}

/// A WAV source has a waveform and playback before its map permutations exist.
pub(super) fn source(ui: &Ui, app: &mut App, doc: Doc) {
    let Some(crate::app::assets::SourceView::Sound { clip, peaks }) =
        app.source_asset(doc).map(|asset| &asset.view)
    else {
        return;
    };
    ui.text(format!("{} Hz · {} channels · {}", clip.rate, clip.channels, clock(clip.duration_ms())));
    let width = ui.content_region_avail()[0].max(1.0);
    let origin = ui.cursor_screen_pos();
    ui.dummy([width, WAVE_HEIGHT]);
    draw_waveform(ui, origin, width, peaks);
    if ui.button("Play") {
        let clip = clip.clone();
        if let Err(e) = app.sound_play_clip(&clip) {
            app.error(e.to_string());
        }
    }
    ui.same_line();
    if ui.button("Stop") {
        app.sound_stop();
    }
}

fn body(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut SoundUi, id: TagId, key: Key) {
    let face_key = key.clone();
    if st.info.as_ref().map(|i| &i.0) != Some(&key) {
        st.info = Some((key.clone(), app.sound_info(id)));
        st.rows = Some((key.clone(), app.sound_rows(id)));
        st.pick = (0, 0);
        st.clip = None;
        st.problem = None;
    }
    let info = match st.info.as_ref().map(|i| &i.1) {
        Some(Ok(info)) => info.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            if ui.button("Try again##sound-info") {
                st.info = None;
            }
            return;
        }
        None => return,
    };
    ui.text_disabled(format!(
        "{} Hz, {}, {}",
        info.sample_rate
            .map_or_else(|| "unknown rate".to_owned(), |r| r.to_string()),
        info.encoding,
        info.compression
    ));
    ui.separator();
    if let Some((_, Ok(rows))) = &st.rows {
        for row in rows {
            let pick = (row.range, row.permutation);
            let label = format!(
                "{}##sound-perm-{}-{}",
                row_text(row),
                row.range,
                row.permutation
            );
            if ui
                .selectable_config(label)
                .selected(st.pick == pick)
                .build()
            {
                st.pick = pick;
                st.problem = None;
            }
        }
    }
    ui.separator();
    let (range, permutation) = st.pick;
    let ck = (key.clone(), st.pick);
    if st.clip.as_ref().map(|c| &c.0) != Some(&ck) {
        st.clip = Some((ck.clone(), app.sound_clip(id, range, permutation)));
    }
    // Read each frame: a redraw (here or by the API) shows at once.
    st.mouth = Some((ck, app.sound_mouth(id, range, permutation)));
    let clip = match st.clip.as_ref().map(|c| &c.1) {
        Some(Ok(clip)) => Some(clip.clone()),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            if ui.button("Try again##sound-clip") {
                st.clip = None;
            }
            None
        }
        None => return,
    };
    let state = app.sound_state();
    let here = state.tag == Some(id) && (state.range, state.permutation) == st.pick;
    let heard = state.playing && here;
    let width = ui.content_region_avail()[0].max(64.0);
    if let Some(clip) = &clip {
        let duration = clip.duration_ms();

        // The waveform: one column per pixel, the playhead over it, a click
        // plays from there.
        let bins = width as usize;
        let pk = (key, st.pick, bins);
        if st.peaks.as_ref().map(|p| &p.0) != Some(&pk) {
            st.peaks = Some((pk, clip.peaks(bins)));
        }
        let origin = ui.cursor_screen_pos();
        ui.dummy([width, WAVE_HEIGHT]);
        let hovered = ui.is_item_hovered();
        let peaks = st.peaks.as_ref().map_or(&[][..], |(_, peaks)| peaks.as_slice());
        draw_waveform(ui, origin, width, peaks);
        let dl = ui.get_window_draw_list();
        if here && duration > 0 {
            let at = origin[0] + (state.position_ms.min(duration) as f32 / duration as f32) * width;
            dl.add_rect(
                [at, origin[1]],
                [at + 2.0, origin[1] + WAVE_HEIGHT],
                theme::WIDGET_FOCUS,
            )
            .filled(true)
            .build();
        }
        if hovered && ui.is_mouse_clicked(MouseButton::Left) {
            let x = ((ui.io().mouse_pos()[0] - origin[0]) / width).clamp(0.0, 1.0);
            let from = (x as f64 * duration as f64) as u64;
            match app.sound_play(id, range, permutation, from) {
                Ok(_) => st.problem = None,
                Err(e) => st.problem = Some(e.to_string()),
            }
        }

        if heard {
            if ui.button("Stop##sound-stop") {
                app.sound_stop();
            }
        } else if ui.button(format!("{} Play##sound-play", icon::SOUND)) {
            match app.sound_play(id, range, permutation, 0) {
                Ok(_) => st.problem = None,
                Err(e) => st.problem = Some(e.to_string()),
            }
        }
        ui.same_line();
        let position = if here {
            state.position_ms.min(duration)
        } else {
            0
        };
        ui.text_disabled(format!(
            "{} / {}   {} Hz, {} channel{}",
            clock(position),
            clock(duration),
            clip.rate,
            clip.channels,
            if clip.channels == 1 { "" } else { "s" }
        ));
    }

    // The mouth track, under the waveform on the same time axis: one
    // filled column per 30 Hz tick, the playhead over it, a drag redraws it.
    // A permutation with no curve leaves it out; it shows even
    // when the audio cannot be decoded here.
    let mouth = match st.mouth.as_ref().map(|m| &m.1) {
        Some(Ok(m)) => m.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::ERROR, format!("{} {e}", icon::ALERT));
            return;
        }
        None => return,
    };
    if let Some(curve) = mouth {
        let ticks = curve.len();
        let curve_ms = ticks as u64 * 1000 / u64::from(MOUTH_HZ);
        // The sound's length when the clip is decoded, the curve's own
        // when it is not.
        let span = clip.as_ref().map_or(curve_ms, |c| c.duration_ms()).max(1);
        ui.text_disabled(format!(
            "mouth  {} ticks at {MOUTH_HZ} Hz{}",
            ticks,
            if app.sound_mouth_is_edited(id, range, permutation) { "  redrawn, not saved" } else { "" }
        ));
        let origin = ui.cursor_screen_pos();
        // Drawing: hold the button and drag along the track. The stroke
        // shows as it goes and is one write, one undo step, on release.
        ui.invisible_button("##mouth-draw", [width, MOUTH_HEIGHT]);
        let drawing = ui.is_item_active() && ui.is_mouse_down(MouseButton::Left);
        let tick_w = 1000.0 / MOUTH_HZ as f32 / span as f32 * width;
        if drawing {
            let [mx, my] = ui.io().mouse_pos();
            let tick = (((mx - origin[0]) / tick_w) as isize).clamp(0, ticks as isize - 1) as usize;
            let level = ((1.0 - (my - origin[1]) / MOUTH_HEIGHT).clamp(0.0, 1.0) * 255.0).round() as u8;
            let (draft, last) = st.draft.get_or_insert_with(|| (curve.to_vec(), tick));
            let (lo, hi) = (tick.min(*last), tick.max(*last));
            for t in lo..=hi {
                draft[t] = level;
            }
            *last = tick;
        } else if let Some((draft, _)) = st.draft.take() {
            match app.sound_mouth_write(id, range, permutation, &draft) {
                Ok(_) => st.problem = None,
                Err(e) => st.problem = Some(e.to_string()),
            }
        }
        let shown: &[u8] = st.draft.as_ref().map_or(&curve[..], |(d, _)| d.as_slice());
        let dl = ui.get_window_draw_list();
        dl.add_rect(
            origin,
            [origin[0] + width, origin[1] + MOUTH_HEIGHT],
            theme::RAISED,
        )
        .filled(true)
        .build();
        let base = origin[1] + MOUTH_HEIGHT;
        for (tick, &aperture) in shown.iter().enumerate() {
            let x = origin[0] + tick as f32 * tick_w;
            let top = base - f32::from(aperture) / 255.0 * MOUTH_HEIGHT;
            dl.add_rect([x, top], [x + tick_w, base], theme::OK)
                .filled(true)
                .build();
        }
        if here && span > 0 {
            let at = origin[0] + (state.position_ms.min(span) as f32 / span as f32) * width;
            dl.add_rect(
                [at, origin[1]],
                [at + 2.0, origin[1] + MOUTH_HEIGHT],
                theme::WIDGET_FOCUS,
            )
            .filled(true)
            .build();
        }
        // From the sound: the curve of the sound's own loudness.
        let decoded = clip.is_some();
        ui.slider_f32("Gain##mouth-gain", &mut st.gain, 0.0, 4.0);
        ui.slider_f32("Smoothing##mouth-smooth", &mut st.smoothing, 0.0, 0.95);
        let off = ui.begin_disabled_with_cond(!decoded);
        if ui.button("From the sound##mouth-from") {
            match app.sound_mouth_from_sound(id, range, permutation, st.gain, st.smoothing) {
                Ok(_) => st.problem = None,
                Err(e) => st.problem = Some(e.to_string()),
            }
        }
        drop(off);
        ui.same_line();
        if ui.button("Revert##mouth-revert") {
            match app.sound_mouth_revert(id, range, permutation) {
                Ok(_) => st.problem = None,
                Err(e) => st.problem = Some(e.to_string()),
            }
        }
        ui.same_line();
        // One undo for the sound's document (docs/editor/studio-flow.md,
        // "Shared patterns"): the newest of its steps, the mouth curve's or
        // a field's, as Cmd+Z takes.
        let (can_undo, _) = app.doc_can(Doc::Tag(id));
        let off = ui.begin_disabled_with_cond(!can_undo);
        if ui.button("Undo##mouth-undo") {
            app.dispatch(Action::UndoDoc(Doc::Tag(id)));
        }
        drop(off);
    }
    if let Some(p) = &st.problem {
        ui.text_colored(theme::ERROR, format!("{} {p}", icon::ALERT));
    }
    let ms = if here { state.position_ms } else { 0 };
    face(ui, app, gpu, st, id, face_key, ms);
}

/// The **Preview on** row and the face it draws: the picked biped with its
/// `talk` overlay at the curve's weight `ms` into the permutation.
fn face(ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, st: &mut SoundUi, id: TagId, key: Key, ms: u64) {
    ui.separator();
    ui.text("Preview on");
    ui.same_line();
    let label = st.face.on.as_ref().map_or("None", |(_, name)| name.as_str()).to_owned();
    if ui.button(format!("{label}##face-on")) {
        ui.open_popup(FACE_PICK);
    }
    if let Some(_popup) = ui.begin_popup(FACE_PICK) {
        if ui
            .selectable_config("None##face-none")
            .selected(st.face.on.is_none())
            .build()
        {
            st.face.on = None;
            st.face.clear(gpu);
        }
        let objects = app.face_objects();
        if objects.is_empty() {
            ui.text_disabled("This map has no bipeds.");
        }
        for (object, name) in objects {
            let picked = st.face.on.as_ref().is_some_and(|(o, _)| *o == object);
            if ui
                .selectable_config(format!("{name}##face-{:#010x}", object.0))
                .selected(picked)
                .build()
                && !picked
            {
                st.face.on = Some((object, name));
                st.face.clear(gpu);
            }
        }
    }
    let Some((object, _)) = st.face.on.clone() else {
        return;
    };
    let (range, permutation) = st.pick;
    let weight = match app.face_weight(id, range, permutation, ms) {
        Ok(w) => w,
        Err(e) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            return;
        }
    };
    let fk: FaceKey = (key, st.pick, object, weight.and_then(|w| w.byte));
    if st.face.picture.as_ref().map(|p| &p.key) != Some(&fk) {
        let pose = app.face_pose(object, weight.map_or(0.0, |w| w.weight), None);
        let preview = pose
            .as_ref()
            .ok()
            .and_then(|f| f.pose.model.as_ref())
            .and_then(|m| Preview::render(gpu, m));
        st.face.clear(gpu);
        st.face.picture = Some(FacePicture {
            key: fk,
            preview,
            pose,
        });
    }
    let Some(picture) = &st.face.picture else {
        return;
    };
    let face = match &picture.pose {
        Ok(face) => face,
        Err(e) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            return;
        }
    };
    ui.text_disabled(match weight {
        Some(w) => format!(
            "{}  weight {:.2}  tick {} of {}",
            face.talk, face.weight, w.tick, w.ticks
        ),
        None => format!("{}  unvoiced: the mouth stays closed", face.talk),
    });
    match &picture.preview {
        Some(preview) => {
            let width = ui.content_region_avail()[0].max(64.0);
            ui.image(
                preview.id.texture_id(),
                [width, width * PICTURE[1] as f32 / PICTURE[0] as f32],
            );
        }
        None => {
            for note in &face.pose.notes {
                ui.text_disabled(note);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use studio_proto::method as m;

    use super::*;
    use crate::api::Api;
    use crate::app::{Action, Doc, tag_tests};

    const SND: &str = r"snd!:objects\test\blaster";

    fn open(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("h2s-sound-doc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, ..) = tag_tests::app_with_registry(
            &dir,
            crate::api::sound::tests::sound_map(),
            tag_tests::registry(),
        );
        app.audition.headless = true;
        (app, dir)
    }

    /// An invented sound opens as a tab in the sound editor, which draws
    /// it while the tab is in front and not for another kind of tag.
    #[test]
    fn an_invented_sound_opens_as_a_tab() {
        let (mut app, dir) = open("tab");
        assert_eq!(target(&app), None, "no tab is open");
        app.dispatch(Action::OpenAsset {
            asset: format!("tag:{SND}"),
            map: None,
        });
        let Some(Doc::Tag(id)) = app.front_doc() else {
            panic!("the sound has no tab: {:?}", app.tabs());
        };
        assert_eq!(target(&app).map(|t| t.0), Some(id));
        let tab = app.tab_json(app.tabs().iter().find(|t| t.doc == Doc::Tag(id)).unwrap());
        assert_eq!(
            (tab["editor"].as_str(), tab["group"].as_str()),
            (Some("audition"), Some("snd!"))
        );
        // The map's own tab in front: the sound editor is not drawn.
        app.dispatch(Action::FocusDoc(Doc::Map));
        assert_eq!(target(&app), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A voice document in front draws no sound editor, though a sound is
    /// still the Tags tool's selection; the sound's tab brings it back.
    #[test]
    fn a_voice_in_front_leaves_the_sound_editor_undrawn() {
        let (mut app, dir) = open("voice-front");
        app.dispatch(Action::OpenAsset {
            asset: format!("tag:{SND}"),
            map: None,
        });
        let Some(Doc::Tag(id)) = app.front_doc() else {
            panic!("the sound has no tab: {:?}", app.tabs());
        };
        assert!(target(&app).is_some());
        app.open_voice("cortana");
        assert_eq!(app.front_doc(), Some(Doc::Voice));
        assert_eq!(target(&app), None, "the voice is in front");
        // Undo on the voice's tab says there is nothing of its own to take.
        app.dispatch(Action::UndoDoc(Doc::Voice));
        app.dispatch(Action::FocusDoc(Doc::Tag(id)));
        assert_eq!(target(&app).map(|t| t.0), Some(id));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Each permutation's row carries the numbers `sound.measure` gives
    /// it, and its text says them.
    #[test]
    fn each_permutation_row_carries_sound_measures_numbers() {
        let (mut app, dir) = open("rows");
        let mut api = Api::new();
        let measured = api
            .call(&mut app, m::SOUND_MEASURE, json!({"tag": SND}))
            .unwrap();
        app.dispatch(Action::OpenAsset {
            asset: format!("tag:{SND}"),
            map: None,
        });
        let (id, _) = target(&app).unwrap();
        let rows = app.sound_rows(id).unwrap();
        let perms = measured["permutations"].as_array().unwrap();
        assert_eq!(rows.len(), perms.len());
        for (row, perm) in rows.iter().zip(perms) {
            assert_eq!(
                (row.range as u64, row.permutation as u64),
                (
                    perm["range"].as_u64().unwrap(),
                    perm["permutation"].as_u64().unwrap()
                )
            );
            let l = row.levels.as_ref().unwrap();
            assert_eq!(
                crate::api::sound_measure::levels_json(l),
                perm["levels"],
                "the row's levels are the API's"
            );
            let text = row_text(row);
            assert!(text.contains(&format!("{:.1} LUFS", l.lufs)), "{text}");
            assert!(
                text.contains(&format!("peak {:.1} dBFS", l.peak_dbfs)),
                "{text}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A permutation with a mouth curve says its length in its row.
    #[test]
    fn a_voiced_permutation_row_names_its_mouth_ticks() {
        let dir = std::env::temp_dir().join(format!("h2s-sound-doc-mouth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, ..) = tag_tests::app_with_registry(
            &dir,
            crate::api::sound::tests::mouth_map(),
            tag_tests::registry(),
        );
        app.audition.headless = true;
        app.dispatch(Action::OpenAsset {
            asset: format!("tag:{SND}"),
            map: None,
        });
        let (id, _) = target(&app).unwrap();
        let rows = app.sound_rows(id).unwrap();
        let ticks = app.sound_mouth(id, 0, 0).unwrap().unwrap().len();
        assert_eq!(rows[0].mouth_ticks, Some(ticks));
        assert!(row_text(&rows[0]).contains(&format!("mouth {ticks} ticks")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
