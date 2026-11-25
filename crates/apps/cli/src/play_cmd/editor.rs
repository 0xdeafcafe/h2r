//! `h2 play --editor`: the game started for a tool to hold (Studio's own
//! game). docs/editor/game-view.md, "Own mode: the game in editor mode".
//!
//! The flag is a start-up choice. It boots one level with one default
//! player, shows no window unless `--window` asks for one, and keeps the
//! session local so every edit method is allowed: Live, a session link, a
//! film and scripted input are refused here, before the disc is read.

use anyhow::{Result, bail};
use input::host::HostButton;
use input::{GatedPad, PadGate};

/// What a run started with `--editor` is told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorMode {
    /// `--window`: the game also gets a window of its own.
    pub window: bool,
}

/// What the rest of the command line asked for, as far as editor mode
/// cares.
#[derive(Clone, Copy, Debug, Default)]
pub struct Asked {
    pub editor: bool,
    pub window: bool,
    /// A `--level` or `H2_LEVEL` is given.
    pub level: bool,
    /// Any `--live*`, `--join` or `--gamertag`-style flag chose Live.
    pub live: bool,
    /// `--film`, or `H2_FILM_OUT`.
    pub film: bool,
    /// `--input-timeline`.
    pub timeline: bool,
}

/// The mode for these flags, or why the combination is refused.
pub fn check(a: Asked) -> Result<Option<EditorMode>> {
    if !a.editor {
        if a.window {
            bail!("--window is for --editor: a plain h2 play always has its window");
        }
        return Ok(None);
    }
    if !a.level {
        bail!("--editor needs --level (or H2_LEVEL): it boots one level, with no menu");
    }
    if a.live {
        bail!("--editor is a local game: remove the Live flags (--live, --live-server, --live-local, --join)");
    }
    if a.film {
        bail!("--editor records no film: remove --film (and unset H2_FILM_OUT)");
    }
    if a.timeline {
        bail!("--editor takes its input from the tool that holds it: remove --input-timeline");
    }
    Ok(Some(EditorMode { window: a.window }))
}

/// Editor mode's pad gate: Start never reaches the game, so the in-game
/// pause menu cannot open. Start goes to the tool that holds the game
/// (Studio pauses it). Every pad passes through it, the window's keyboard
/// (Escape is Start) and `input.pad` included. It runs after the debug
/// menu's gate (`InputManager::add_gate`).
pub struct NoStart;

impl PadGate for NoStart {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        for p in pads {
            p.pad.state.buttons.set(HostButton::Start, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::host::{HostPad, HostPadId};

    fn editor() -> Asked {
        Asked { editor: true, level: true, ..Asked::default() }
    }

    #[test]
    fn a_plain_run_is_not_editor_mode() {
        assert_eq!(check(Asked::default()).unwrap(), None);
        let e = check(Asked { window: true, ..Asked::default() }).unwrap_err().to_string();
        assert!(e.contains("--window is for --editor"), "{e}");
    }

    #[test]
    fn editor_mode_boots_a_level_with_no_window_unless_asked() {
        assert_eq!(check(editor()).unwrap(), Some(EditorMode { window: false }));
        assert_eq!(
            check(Asked { window: true, ..editor() }).unwrap(),
            Some(EditorMode { window: true })
        );
    }

    #[test]
    fn editor_mode_refuses_what_would_make_the_session_not_local() {
        let level = check(Asked { level: false, ..editor() }).unwrap_err().to_string();
        assert!(level.contains("--editor needs --level"), "{level}");
        let live = check(Asked { live: true, ..editor() }).unwrap_err().to_string();
        assert!(live.contains("local game"), "{live}");
        let film = check(Asked { film: true, ..editor() }).unwrap_err().to_string();
        assert!(film.contains("no film"), "{film}");
        let timeline = check(Asked { timeline: true, ..editor() }).unwrap_err().to_string();
        assert!(timeline.contains("--input-timeline"), "{timeline}");
    }

    fn pad(id: u64, held: &[HostButton]) -> GatedPad {
        let mut pad = HostPad {
            id: HostPadId(id),
            name: "fake".into(),
            state: Default::default(),
            physical: false,
            supports_rumble: false,
        };
        for b in held {
            pad.state.buttons.set(*b, true);
        }
        GatedPad { key: (0, HostPadId(id)), pad }
    }

    #[test]
    fn start_is_hidden_from_every_pad_and_the_rest_passes() {
        let mut pads = [
            pad(1, &[HostButton::Start, HostButton::South]),
            pad(2, &[HostButton::Start]),
            pad(3, &[HostButton::Back]),
        ];
        NoStart.filter(&mut pads);
        for p in &pads {
            assert!(!p.pad.state.buttons.contains(HostButton::Start));
        }
        assert!(pads[0].pad.state.buttons.contains(HostButton::South));
        assert!(pads[2].pad.state.buttons.contains(HostButton::Back));
        // Filtering again, with nothing held, changes nothing.
        NoStart.filter(&mut pads);
        assert!(pads[0].pad.state.buttons.contains(HostButton::South));
    }
}
