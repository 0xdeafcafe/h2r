//! The Sound window: audition the `snd!` tag selected in the Tags tool.
//! A list of its permutations, the picked one's waveform, **Play** and
//! **Stop**, and a click on the waveform to play from there. Decoding and
//! playing are the app's ([`crate::app::audition`]); the agent API's
//! `sound.read` and `sound.play` use the same calls.
//!
//! The window is one scrolling body. The waveform has a height of its own;
//! it never takes the leftover height.
use std::sync::Arc;

use dear_imgui_rs::{Condition, MouseButton, Ui};
use map::TagId;

use crate::app::audition::{Clip, SoundError, SoundInfo};
use crate::app::{App, ToolKind};
use crate::theme::{self, icon};

/// The map shown and the tag.
type Key = (Option<String>, TagId);

/// The waveform's height, pixels.
const WAVE_HEIGHT: f32 = 96.0;

/// The window's own state: the permutation picked, and what was decoded for
/// it, so a frame does not decode again (and a refusal is not asked for
/// again each frame).
#[derive(Default)]
pub struct SoundUi {
    info: Option<(Key, Result<SoundInfo, SoundError>)>,
    /// Pitch range and permutation picked.
    pick: (usize, usize),
    clip: Option<((Key, (usize, usize)), Result<Arc<Clip>, SoundError>)>,
    peaks: Option<((Key, (usize, usize), usize), Vec<[i16; 2]>)>,
    problem: Option<String>,
}

/// `m:ss.t` of `ms`.
fn clock(ms: u64) -> String {
    format!("{}:{:02}.{}", ms / 60_000, ms / 1000 % 60, ms / 100 % 10)
}

/// The window, when the Tags tool has a sound selected.
pub fn window(ui: &Ui, app: &mut App, st: &mut SoundUi) {
    if app.tool != ToolKind::Tags {
        return;
    }
    let Some(id) = app.shown_tags().selected else {
        return;
    };
    let Some(entry) = app.shown_tags().tag(id) else {
        return;
    };
    if entry.group.0 != *b"snd!" {
        return;
    }
    let name = entry.name.clone();
    let key: Key = (app.shown_map_name().map(str::to_owned), id);
    ui.window("Sound###sound")
        .size([440.0, 380.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {name}", icon::SOUND));
            body(ui, app, st, id, key);
        });
}

fn body(ui: &Ui, app: &mut App, st: &mut SoundUi, id: TagId, key: Key) {
    if st.info.as_ref().map(|i| &i.0) != Some(&key) {
        st.info = Some((key.clone(), app.sound_info(id)));
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
    for (r, perms) in info.ranges.iter().enumerate() {
        for (p, perm) in perms.iter().enumerate() {
            let label = format!(
                "pitch range {r}, permutation {p}  ({} bytes)##sound-perm-{r}-{p}",
                perm.encoded_bytes
            );
            if ui
                .selectable_config(label)
                .selected(st.pick == (r, p))
                .build()
            {
                st.pick = (r, p);
                st.problem = None;
            }
        }
    }
    ui.separator();
    let (range, permutation) = st.pick;
    let ck = (key.clone(), st.pick);
    if st.clip.as_ref().map(|c| &c.0) != Some(&ck) {
        st.clip = Some((ck, app.sound_clip(id, range, permutation)));
    }
    let clip = match st.clip.as_ref().map(|c| &c.1) {
        Some(Ok(clip)) => clip.clone(),
        Some(Err(e)) => {
            ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
            if ui.button("Try again##sound-clip") {
                st.clip = None;
            }
            return;
        }
        None => return,
    };
    let state = app.sound_state();
    let here = state.tag == Some(id) && (state.range, state.permutation) == st.pick;
    let heard = state.playing && here;
    let duration = clip.duration_ms();

    // The waveform: one column per pixel, the playhead over it, a click
    // plays from there.
    let width = ui.content_region_avail()[0].max(64.0);
    let bins = width as usize;
    let pk = (key, st.pick, bins);
    if st.peaks.as_ref().map(|p| &p.0) != Some(&pk) {
        st.peaks = Some((pk, clip.peaks(bins)));
    }
    let origin = ui.cursor_screen_pos();
    ui.dummy([width, WAVE_HEIGHT]);
    let hovered = ui.is_item_hovered();
    let dl = ui.get_window_draw_list();
    dl.add_rect(
        origin,
        [origin[0] + width, origin[1] + WAVE_HEIGHT],
        theme::RAISED,
    )
    .filled(true)
    .build();
    let mid = origin[1] + WAVE_HEIGHT * 0.5;
    if let Some((_, peaks)) = &st.peaks {
        for (i, [low, high]) in peaks.iter().enumerate() {
            let x = origin[0] + i as f32;
            let top = mid - f32::from(*high) / 32768.0 * WAVE_HEIGHT * 0.5;
            let bottom = mid - f32::from(*low) / 32768.0 * WAVE_HEIGHT * 0.5;
            dl.add_rect([x, top], [x + 1.0, bottom.max(top + 1.0)], theme::ACCENT)
                .filled(true)
                .build();
        }
    }
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
    if let Some(p) = &st.problem {
        ui.text_colored(theme::ERROR, format!("{} {p}", icon::ALERT));
    }
}
