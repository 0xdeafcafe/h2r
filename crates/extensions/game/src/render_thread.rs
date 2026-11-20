//! Which of boot's feature hooks the pipelined render thread runs, and why
//! the rest stay on the game thread (docs/graphics/pipelined-render.md,
//! "Render-safe hooks").
//!
//! A chain runs on the render thread when every hook in it is marked
//! [`context_only`](hooks::HookRef::context_only) and nothing runs after
//! the body ([`hooks::plan::Chain::render_thread`]). The plan marks the
//! chain's override (`Dispatch::mark_context_only`) and the executor's fork
//! keeps it. Every other hook is listed in [`REFUSED`] with the reason it
//! stays off. A test in `registration`'s tests builds the plan from every
//! feature boot adds and fails on a hook that is in neither, so a new hook
//! cannot quietly turn into a fallback.

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
        "render-census",
        "*",
        "off by default, and pipelining is off with it (it watches the render on the game thread)",
    ),
    #[cfg(feature = "gpu")]
    ("native-map-work", "GAME_ACTIVATE", AFTER),
    #[cfg(feature = "gpu")]
    ("native-map-work", "BSP_SWITCH", AFTER),
    #[cfg(feature = "gpu")]
    ("native-map-work", "GAME_STATE_REVERT", AFTER),
    #[cfg(feature = "gpu")]
    (
        "native-objects",
        "GAME_ACTIVATE",
        "the map-load model preload runs on the game thread; the render never meets it",
    ),
    #[cfg(feature = "gpu")]
    (
        "native-transparents",
        "GAME_ACTIVATE",
        "game activation runs on the game thread; the render never meets it",
    ),
    #[cfg(feature = "gpu")]
    (
        "native-post",
        "COLOUR_PASS",
        "on by default; the post call's draws are held until it returns, so the \
         return hook decides commit or replay",
    ),
    #[cfg(feature = "gpu")]
    (
        "native-post",
        "SCREEN_EFFECT",
        "on by default; the post call's draws are held until it returns, so the \
         return hook decides commit or replay",
    ),
    #[cfg(feature = "gpu")]
    (
        "native-shadows",
        "RENDER_LIGHT_PASSES",
        "context-only, but shares the light passes with render-timing (off by default), \
         whose timer runs after the body",
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
    #[cfg(feature = "gpu")]
    (
        "frame-scene",
        "RENDER_VIEWS",
        "context-only, but shares render_views with render-timing (off by default), \
         whose timer runs after the body",
    ),
    #[cfg(feature = "gpu")]
    (
        "frame-scene",
        "RENDER_FRAME",
        "after render_frame_and_present, on the game thread: seals the frame's scene",
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
    (
        "h1-particle-physics",
        "*",
        "native particle simulation on the game thread, with post-step hooks around the guest bodies",
    ),
    ("campaigns", "*", "reads campaign selection and updates profile saves on the game thread"),
    ("campaign-squad-pools", "*", "sizes the guest datum pools as a map loads"),
    ("h1-ai-control", "*", "updates simulation actors and evaluates guest script arguments"),
    ("h1-drive", "VEHICLE_CONTACT_FORCES", AFTER),
    ("h1-flag", "*", "steps source cloth points around the guest solver on the simulation thread"),
    ("h1-command-atoms", "*", "runs command scripts and their actor updates on the simulation thread"),
    ("h1-conversations", "*", "uses the interpreter and tracks actor construction across guest calls"),
    ("h1-recordings", "UNIT_APPLY_CONTROL", "applies recorded input through a guest continuation"),
    ("h1-scope-zoom", "GAME_ACTIVATE", AFTER),
    ("h1-scope-zoom", "SCREEN_EFFECT", AFTER),
    ("h1-script-builtins", "GAME_ACTIVATE", AFTER),
    ("h1-script-builtins", "GAME_TICK", AFTER),
    ("h1-script-builtins", "GAME_UPDATE_FRAME", "runs extra game ticks through the guest simulation"),
    ("h1-script-builtins", "OBJECT_FUNCTION_VALUE", "reads the simulation countdown for its digit shaders"),
    ("h1-script-builtins", "OBJECT_FUNCTION_IMPORT", "reads weapon ownership and unit light power from the simulation"),
    ("h1-script-builtins", "UNIT_LIGHT_UPDATE", "advances the simulation unit's flashlight"),
    ("h1-script-builtins", "HS_UNUSED_EVALUATOR", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_UNIT_FLAG_SET", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_UNIT_FLAG_GET", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_AI_LIST_FLAG_SET", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_SHORT_TO_LONG_EVALUATOR", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_LOOK_ANY_DIRECTION", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_MOVE_ANY_DIRECTION", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_FALSE_EVALUATOR", "evaluates the script on its guest interpreter thread"),
    ("h1-script-builtins", "HS_CONSTANT_LONG_EVALUATOR", "evaluates the script on its guest interpreter thread"),
    ("h1-health", "OBJECT_DAMAGE_UPDATE", AFTER),
    ("h1-hud-text", "GAME_TICK", AFTER),
    ("h1-hud-text", "GAME_ACTIVATE", AFTER),
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
    (
        "h1-hud-text",
        "HS_UNUSED_EVALUATOR",
        "a script builtin on the simulation thread evaluates its Boolean argument in the guest",
    ),
    (
        "h1-hud-text",
        "PLAYER_PRESS_INTERACT",
        "a player's interact press in the simulation, on the game thread; swaps the powerup \
         through guest calls it waits on",
    ),
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
        "ui-back",
        "SCREEN_PUSH",
        "records the replaced dialog in host navigation state on the game thread",
    ),
    (
        "ui-back",
        "SCREEN_BACK_EVENT",
        "reopens the parent with guest calls from the game thread's menu input handler",
    ),
    (
        "ui-back",
        "UI_CHANNEL_FREE",
        "removes the freed dialog from host navigation state on the game thread",
    ),
    (
        "lobby-options",
        "LOBBY_VARIANT_PANEL",
        "updates lobby widgets through guest calls and uses a game-thread reentry guard",
    ),
    (
        "bots",
        "WEAPON_STAT_BUCKET",
        "credits phantom shots by weapon and range into the bots' game-thread tallies",
    ),
    (
        "bots",
        "PARTY_LEAVE",
        "retires the departing lobby's Clankers through guest session calls on the game thread",
    ),
    (
        "bots",
        "MATCHMAKING_CANCEL_CONFIRMED",
        "retires the departing lobby's Clankers through guest session calls on the game thread",
    ),
    (
        "connection-bars",
        "SCOREBOARD_CONNECTION_BARS",
        "fills the scoreboard badge after the body runs, a Rust continuation the fallback cannot resume",
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
    ("clanker-options-route", "RECENT_PLAYERS_BUILD", AFTER),
    (
        "clanker-options-route",
        "PLAYER_OPTIONS_OPEN",
        "opens a dialog with a guest call and may request a Live card",
    ),
    (
        "clanker-options-route",
        "PLAYER_OPTIONS_CHECK_FRIEND",
        "routes the selection through the same dialog and Live lookup",
    ),
    (
        "clanker-options-route",
        "EMBLEM_WIDGET_DRAW",
        "consumes the shared card refresh flag and updates the live widget",
    ),
    // Phantom membership and input run on the game thread.
    ("bots", "RENDER_FRAME", AFTER),
    ("bots", "GAME_ACTIVATE", AFTER),
    ("bots", "SOUND_UPDATE", AFTER),
    ("agent-audio", "SOUND_UPDATE", AFTER),
    ("bots", "SOUND_LISTENERS_UPDATE", AFTER),
    ("bots", "SOUND_OWNER_USER", AFTER),
    (
        "bots",
        "SIMULATION_SLOT_ACTION",
        "phantoms sample their pad once per simulation tick: the game thread's tick, \
         answered asynchronously",
    ),
    (
        "bots",
        "ITEM_SET_UNIT",
        "weapon attachment reads the live placement and player tables before the weapon leaves its pad",
    ),
    (
        "bots",
        "POWERUP_TAKEN",
        "power-up pickup reads the live equipment and its recipient before the simulation deletes the equipment",
    ),
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
        "SESSION_GAME_START",
        "the session's game start on the game thread; makes this console the host when phantoms \
         are planned",
    ),
];
