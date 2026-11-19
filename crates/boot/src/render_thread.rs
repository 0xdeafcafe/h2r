//! Which of boot's feature hooks the pipelined render thread runs, and why
//! the rest stay on the game thread (docs/graphics/pipelined-render.md,
//! "Render-safe hooks").
//!
//! A chain runs on the render thread when every hook in it is marked
//! [`context_only`](feature::HookRef::context_only) and nothing runs after
//! the body ([`feature::plan::Chain::render_thread`]). The plan marks the
//! chain's override (`Dispatch::mark_context_only`) and the executor's fork
//! keeps it. Every other hook is listed in [`REFUSED`] with the reason it
//! stays off. The test builds the plan from every feature boot adds and
//! fails on a hook that is in neither, so a new hook cannot quietly turn
//! into a fallback.

/// Why a hook stays on the game thread when its chain has an "after" part:
/// the chain calls the body through its shadow with a Rust continuation,
/// which a fallback cannot resume.
const AFTER: &str = "something runs after the body (a Rust continuation a fallback cannot resume)";

/// The hooks that are not context-only, as `(feature, point, why)`. `"*"`
/// stands for every point of a feature. A hook on a game-thread point costs
/// nothing here, because the render never meets it.
pub const REFUSED: &[(&str, &str, &str)] = &[
    // The render path's own points.
    (
        "render-timing",
        "*",
        "off by default; times the front-end against the frame stats the game thread owns, \
         and its timed points need the after continuation",
    ),
    (
        "render-loop",
        "*",
        "off by default; the Rust ports park in Rust continuations around their guest calls",
    ),
    (
        "render-census",
        "*",
        "off by default, and pipelining is off with it (it watches the render on the game thread)",
    ),
    // render_frame_and_present itself: the game thread runs it, and the
    // pipeline's implementation is what hands the rest to the render thread.
    (
        "pipeline-render",
        "RENDER_FRAME",
        "the launch point: runs on the game thread",
    ),
    (
        "map-slots",
        "RENDER_FRAME",
        "after render_frame_and_present, on the game thread",
    ),
    (
        "map-copy",
        "CACHE_COPY_STEP",
        "the loading screen's cache copy on the game thread; steps it with guest calls",
    ),
    // Game-thread points: the simulation, the UI and the level start.
    (
        "variant-vitality",
        "*",
        "a damage instance in the simulation, on the game thread; scales the body and shield parts",
    ),
    ("h1-health", "OBJECT_DAMAGE_UPDATE", AFTER),
    ("h1-health", "BODY_RESTORE", AFTER),
    ("h1-health", "GAME_ACTIVATE", AFTER),
    ("h1-precision", "GAME_ACTIVATE", AFTER),
    ("h1-hud", "GAME_ACTIVATE", AFTER),
    ("level-boot", "GAME_ACTIVATE", AFTER),
    ("aim-assist", "GAME_ACTIVATE", AFTER),
    ("aim-assist", "PLAYER_CONTROL_UPDATE", AFTER),
    ("aim-assist", "AIM_ASSIST_PARAMS", AFTER),
    ("skulls-mp", "GAME_ACTIVATE", AFTER),
    ("skulls-mp", "WEAPON_FROM_PLACEMENT", AFTER),
    ("skulls", "GAME_TICK", AFTER),
    ("skulls", "SKULL_STOCK", AFTER),
    (
        "skulls",
        "OPTIONS_QUEUE",
        "writes the skull selection to the settings store (host state the game thread shares)",
    ),
    ("skulls-custom", "GAME_TICK", AFTER),
    ("touch-glue", "GAME_TICK", AFTER),
    ("cheats", "GAME_TICK", AFTER),
    ("layouts-value", "CONTROLS_ROW_TEXT", AFTER),
    ("controls-text", "CONTROLS_ROW_TEXT", AFTER),
    // Features the test's registry gained when it began calling boot's own
    // registration: all on game-thread points (the simulation's events, the
    // match start, the variant editor and the Game Setup lists), which the
    // render never meets.
    ("hud", "GAME_ACTIVATE", AFTER),
    ("loaded_map", "GAME_ACTIVATE", AFTER),
    ("variant-ext-live", "GAME_ACTIVATE", AFTER),
    (
        "live-telemetry",
        "*",
        "records simulation events (damage, shots, kills, the tick, the match start and end) on the \
         game thread; the samples and the match record run after the body",
    ),
    (
        "variant-more-options-row",
        "*",
        "the variant editor's row, a UI event on the game thread",
    ),
    (
        "h1-options-row",
        "*",
        "the Game Setup list's row, a UI event on the game thread",
    ),
    (
        "clankers-row",
        "*",
        "the Game Setup list's row, a UI event on the game thread",
    ),
    (
        "game-setup-help",
        "SQUAD_SETTINGS_UPDATE",
        "the Game Setup screen's per-frame update (picture and description), UI on the game thread",
    ),
    (
        "clankermatch",
        "*",
        "the OptiMatch playlist list's rows and press, and the Live menu's Quickmatch press: \
         UI events on the game thread",
    ),
    (
        "clankermatch-switch",
        "*",
        "the Game Setup list's Switch To: Clankermatch row, a UI event on the game thread",
    ),
    (
        "live-menu-cursor",
        "LIVE_MENU_CURSOR",
        "the Xbox Live menu's build, a UI event on the game thread",
    ),
    // Local bots: the game thread's frame, sound and players screen. Around
    // RENDER_FRAME, the pipeline's snapshot is taken with the bot views
    // hidden; the render thread never meets these points.
    ("bots", "RENDER_FRAME", AFTER),
    (
        "bots",
        "SIMULATION_SLOT_ACTION",
        "phantoms sample their pad once per simulation tick: the game thread's tick, \
         answered asynchronously",
    ),
    ("bots", "SOUND_UPDATE", AFTER),
    ("bots", "SOUND_LISTENERS_UPDATE", AFTER),
    ("bots", "SOUND_OWNER_USER", AFTER),
    ("bots", "GAME_ACTIVATE", AFTER),
    (
        "bots",
        "WEAPON_FIRE",
        "a shot in the simulation, on the game thread; the bots hear it",
    ),
    (
        "bots",
        "MACHINE_IN_GAME",
        "the simulation's per-tick player update, on the game thread; phantoms' machines answer in the game",
    ),
    (
        "bots",
        "PLAYER_KILLED",
        "a death in the simulation, on the game thread; logs it for the bots' emotes",
    ),
    (
        "bots",
        "PLAYERS_SCREEN_UNJOINED",
        "a players screen event on the game thread; signs a bot in with guest calls",
    ),
    ("bots", "PLAYERS_SCREEN_UPDATE", AFTER),
    (
        "bots",
        "PREGAME_LOBBY_INPUT",
        "a lobby input event on the game thread; notes a Start press for the start-state log",
    ),
    (
        "bots",
        "LOBBY_ROW_PRESS",
        "a lobby list event on the game thread; notes a Start Game press for the start-state log",
    ),
    (
        "bots",
        "SESSION_GAME_START",
        "the session's game start on the game thread; makes this console the host when phantoms \
         are planned",
    ),
];

#[cfg(test)]
mod tests {
    use super::REFUSED;
    use crate::registration::tests::{ALL_ON, Case, registry};

    fn refused(feature: &str, point: &str) -> Option<usize> {
        REFUSED
            .iter()
            .position(|&(f, p, _)| f == feature && (p == "*" || p == point))
    }

    /// Every hook boot's features place, with every switch that adds one
    /// on (and the census in a second build, since pipelining is off with
    /// it), either runs on the pipelined render thread or is listed in
    /// [`REFUSED`] with its reason, and every listed entry is still used.
    /// The ones the render path meets in a default run (the locale lookups,
    /// Blind, the native world's and objects' layer draw) are context-only,
    /// and so are the native transparents' geometry handler and the native
    /// lights' passes.
    #[test]
    fn every_feature_hook_runs_on_the_render_thread_or_says_why_not() {
        let mut unlisted = Vec::new();
        let mut used = vec![false; REFUSED.len()];
        let mut safe = Vec::new();
        // The native world (on by default) and objects at the layer draw,
        // and the transparents' handler, with nothing else there, are
        // render-thread safe together, and so are the lights' skip and view
        // observer (placed only without render-timing).
        let native =
            "impl.render.native_objects=on,impl.render.native_transparents=on,impl.render.native_lights=on,impl.render.native_post=on,impl.render.native_gels=on".to_owned();
        for switches in [String::new(), native, ALL_ON.to_owned(), format!("{ALL_ON},render-census")] {
            let plan = registry(&Case::full(&switches)).build(&feature::Base).unwrap();
            for c in plan.chains.values() {
                match c.render_thread() {
                    Ok(()) => safe.push(c.point.name),
                    Err(hooks) => {
                        for (h, why) in hooks {
                            match refused(h.feature, c.point.name) {
                                Some(i) => used[i] = true,
                                None => unlisted.push(format!(
                                    "{} {} {}:{} ({why})",
                                    c.point.name,
                                    h.kind().as_str(),
                                    h.feature,
                                    h.label
                                )),
                            }
                        }
                    }
                }
            }
        }
        assert!(
            unlisted.is_empty(),
            "hooks the pipelined render would fall back on; mark them context_only or list them in REFUSED:\n{}",
            unlisted.join("\n")
        );
        let stale: Vec<_> = REFUSED
            .iter()
            .zip(&used)
            .filter(|(_, u)| !**u)
            .map(|(r, _)| r)
            .collect();
        assert!(stale.is_empty(), "listed but not placed: {stale:?}");
        for p in [
            "LOCALE_FIND",
            "LOCALE_EXISTS",
            "HUD_SHOWN",
            "FIRST_PERSON_RENDER",
            "LAYER_DRAW",
            "TRANSPARENT_GEOMETRY",
            "RENDER_LIGHT_PASSES",
            "LIGHT_GEOMETRY_DRAW",
            "RENDER_DYNAMIC_LIGHTS",
            "COLOUR_PASS",
            "SCREEN_EFFECT",
        ] {
            assert!(safe.contains(&p), "{p} runs on the render thread");
        }
    }
}
