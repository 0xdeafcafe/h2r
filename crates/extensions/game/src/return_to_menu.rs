//! Tell the host window to show its loading picture before the guest starts
//! dismantling the match and loading the main menu (docs/runtime/level-boot.md).

use std::sync::{Arc, Mutex};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy, points};

use crate::layout_glue::ReadGm;
/// Frames the frame owner's mailbox may hold ahead of the window.
const FRAMES_IN_FLIGHT: u64 = 2;

use crate::level_boot::{MODE_CAMPAIGN, MODE_MAINMENU, MODE_MULTIPLAYER, OPT_MODE, active_mode};

pub const ID: &str = "return-to-menu";

/// Show the loading picture (`true`) or let the next guest present replace it.
pub type LoadingScreen = Box<dyn Fn(bool) + Send + Sync>;

pub struct ReturnToMenu(pub Arc<dyn Fn(bool) + Send + Sync>);

#[derive(Default)]
struct Exit {
    active: Option<u32>,
    waiting: Option<Handoff>,
}

#[derive(Default)]
struct Handoff {
    activated: bool,
    first_render: Option<u64>,
}

impl Exit {
    fn requested(&mut self, destination: Option<u32>) -> bool {
        if self.waiting.is_some()
            || destination != Some(MODE_MAINMENU)
            || !matches!(self.active, Some(MODE_CAMPAIGN | MODE_MULTIPLAYER))
        {
            return false;
        }
        self.waiting = Some(Handoff::default());
        true
    }

    fn activated(&mut self, mode: Option<u32>) {
        self.active = mode;
        if let Some(wait) = &mut self.waiting {
            wait.activated = mode.is_some();
            wait.first_render = None;
        }
    }

    fn rendered(&mut self, presents: u64) -> bool {
        let Some(wait) = &mut self.waiting else {
            return false;
        };
        if !wait.activated {
            return false;
        }
        // The first ordinary render joins any guest render still in
        // flight. Let the frame owner catch up to that point before
        // allowing a publish to replace the loading picture.
        let first = *wait.first_render.get_or_insert(presents);
        if presents.saturating_sub(first) < FRAMES_IN_FLIGHT {
            return false;
        }
        self.waiting = None;
        true
    }
}

impl Feature for ReturnToMenu {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Show the host loading picture during the return to the menu",
        )
        .doc("docs/runtime/level-boot.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let state = Arc::new(Mutex::new(Exit::default()));
        let screen = Arc::clone(&self.0);

        let exit = Arc::clone(&state);
        let show = Arc::clone(&screen);
        b.raw().observe_before(
            &points::OPTIONS_QUEUE,
            "show return-to-menu loading",
            move |o| {
                let destination = o
                    .arg_u32("options")
                    .and_then(|options| options.checked_add(OPT_MODE))
                    .and_then(|addr| o.guest().u32(addr).ok());
                if exit
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested(destination)
                {
                    show(true);
                }
            },
        );

        let exit = Arc::clone(&state);
        b.game()
            .on_start("remember the active game mode", move |o| {
                let active = active_mode(&ReadGm(&*o.guest()));
                exit.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .activated(active);
            });

        let exit = Arc::clone(&state);
        let show = Arc::clone(&screen);
        // This is the outer render entry on the game thread. The pipeline
        // executes the original frame body on its fork.
        b.raw().observe_after(
            &points::RENDER_FRAME,
            "hand back to the main menu's frames",
            move |_| {
                if exit
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .rendered(crate::presents::presents())
                {
                    show(false);
                }
            },
        );

        b.snapshot(SnapshotPolicy::section(Vec::new, move |_, rt| {
            let active = active_mode(&*rt.memory());
            *state.lock().unwrap_or_else(|e| e.into_inner()) = Exit {
                active,
                waiting: None,
            };
            screen(false);
            Ok(())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_loading_starts_at_the_menu_request_and_waits_for_a_rendered_menu() {
        let mut exit = Exit {
            active: Some(MODE_MULTIPLAYER),
            waiting: None,
        };
        assert!(exit.requested(Some(MODE_MAINMENU)));
        assert!(!exit.requested(Some(MODE_MAINMENU)));
        assert!(
            !exit.rendered(20),
            "a late match frame cannot clear loading"
        );
        exit.activated(Some(MODE_MAINMENU));
        assert!(!exit.rendered(21));
        let last_in_flight = 21 + FRAMES_IN_FLIGHT;
        assert!(!exit.rendered(last_in_flight - 1));
        assert!(exit.rendered(last_in_flight));
        assert!(
            !exit.rendered(last_in_flight + 1),
            "release the loading picture only once"
        );
    }

    #[test]
    fn return_loading_ignores_boot_and_requests_for_a_playable_map() {
        let mut exit = Exit::default();
        assert!(!exit.requested(Some(MODE_MAINMENU)));
        exit.active = Some(MODE_MAINMENU);
        assert!(!exit.requested(Some(MODE_MAINMENU)));
        exit.active = Some(MODE_CAMPAIGN);
        assert!(!exit.requested(None));
        assert!(!exit.requested(Some(MODE_MULTIPLAYER)));
        assert!(exit.requested(Some(MODE_MAINMENU)));
    }

    #[test]
    fn a_second_activation_restarts_the_handoff_for_its_own_frames() {
        let mut exit = Exit {
            active: Some(MODE_MULTIPLAYER),
            waiting: None,
        };
        let in_flight = FRAMES_IN_FLIGHT;
        assert!(exit.requested(Some(MODE_MAINMENU)));
        exit.activated(Some(MODE_MAINMENU));
        assert!(!exit.rendered(21));
        assert!(
            !exit.rendered(21),
            "render calls without presents cannot drain the queue"
        );
        exit.activated(Some(MODE_CAMPAIGN));
        // An alternate level can supersede the menu request.
        assert!(!exit.rendered(21 + in_flight));
        assert!(exit.rendered(21 + 2 * in_flight));
    }

    #[test]
    fn the_loading_signal_brackets_the_menu_transition() {
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        registry.add(&ReturnToMenu(Arc::new(|_| {}))).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        let hooks = plan.text();
        assert!(hooks.contains(
            "OPTIONS_QUEUE: observe_before [return-to-menu:show return-to-menu loading]"
        ));
        assert!(hooks.contains("GAME_ACTIVATE: original -> observe_after [return-to-menu:remember the active game mode]"));
        assert!(hooks.contains("RENDER_FRAME: original -> observe_after [return-to-menu:hand back to the main menu's frames]"));
    }

    #[test]
    fn the_feature_switch_keeps_the_original_transition() {
        let mut registry = hooks::Registry::new(hooks::Switches::parse("-return-to-menu").unwrap());
        assert!(!registry.add(&ReturnToMenu(Arc::new(|_| {}))).unwrap());
        let plan = registry.build(&hooks::Base).unwrap();
        assert!(plan.chains.is_empty());
    }
}
