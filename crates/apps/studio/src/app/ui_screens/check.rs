//! One of our screens in one state, checked as the picture is
//! ([`UiScreens::draw_native`]: the edited docs in scope, the disc or the
//! synthetic source) without drawing it or disturbing the pick shown:
//! `ui.check` and `check.run` over every screen and state.

use anyhow::{Context as _, Result, bail};
use boot::ui_kit::findings::{self, StateReport};
use boot::ui_kit::preview;
use boot::ui_kit::screens;

use super::{UiScreens, edit, presented};

impl UiScreens {
    /// Screen `screen` in state `state`: every check's findings
    /// (`boot::ui_kit::findings::of_state`), with the unsaved edits.
    pub fn check_state(&mut self, screen: &str, state: &str) -> Result<StateReport> {
        let docs = self.shown_docs();
        let disc = self.disc();
        edit::ours::with_docs(&docs, || {
            let mut live =
                screens::live(screen).with_context(|| format!("no screen {screen:?}"))?;
            if !live.go(state) {
                bail!("{screen} has no state {state:?}");
            }
            let spec = presented(screen, live.spec());
            let from = match disc {
                Some(d) => preview::From::Disc(d),
                None => preview::From::Synthetic,
            };
            let view = from.view(&spec, state)?;
            findings::of_state(&from, &spec, &view)
        })
    }

    /// Whether the checks read the disc's art and fonts (else the
    /// synthetic source's).
    pub fn checks_from_disc(&mut self) -> bool {
        self.disc().is_some()
    }
}
