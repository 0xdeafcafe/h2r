//! The Announcer window: type a line, have the voice say it, hear it, and
//! see how it sits against the retail announcer. Making a line is the
//! app's ([`crate::app::announcer`]); the agent API's `voice.say` makes it
//! the same way. View > Announcer opens it.
//!
//! The window waits while the voice is asked (a few seconds); a line the
//! voice already said comes back at once from the cache.
use dear_imgui_rs::{Condition, Ui};

use crate::app::App;
use crate::app::announcer::{Playback, SayParams, Said};
use crate::theme::{self, icon};
use crate::voice::{self, VoiceError};

/// The window's state.
#[derive(Default)]
pub struct AnnouncerUi {
    pub open: bool,
    text: String,
    /// The clip id the line is for, or empty.
    clip: String,
    /// A voice other than the default, or empty.
    voice: String,
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
    ui.window("Announcer###announcer")
        .size([460.0, 420.0], Condition::FirstUseEver)
        .build(|| {
            body(ui, app, st);
            ui.separator();
            if ui.button("Close##announcer-close") {
                st.open = false;
            }
        });
}

fn body(ui: &Ui, app: &mut App, st: &mut AnnouncerUi) {
    ui.text_disabled(format!(
        "{} lines are saved under {}",
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

    let can = !st.text.trim().is_empty();
    if ui.button(format!("{} Say it##announcer-say", icon::SOUND)) && can {
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
