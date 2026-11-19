//! Our settings, one [`Def`] each (docs/game/settings-menu.md, docs/engine/feature-engine.md
//! "Settings"). The file keys and environment variables are the ones the
//! hand-written settings used before the `settings` crate, so an existing
//! `settings.toml` and existing launch scripts keep working.

use std::path::Path;

use settings::{Apply, Choice, Def, Flag, Kind, Menu, Problem, Scope, Store, Tier, Unit, Value};

/// The retail first-person field of view, in degrees.
pub const FOV_RETAIL: i64 = 70;

pub static FOV: Def = Def {
    key: "graphics.fov",
    label: "Field of View",
    help: "First-person horizontal field of view in degrees. 70 is the game's own; \
           0 means 70. The row steps 70 to 120 by 5; the file and H2_FOV take 30 to 150.",
    kind: Kind::Int {
        valid: 30..=150,
        steps: &[70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120],
        unit: Unit::Degrees,
    },
    default: Value::Int(FOV_RETAIL),
    env: Some("H2_FOV"),
    flag: None,
    aliases: &[("0", Value::Int(FOV_RETAIL))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 10,
    }),
};

pub static FPS: Def = Def {
    key: "graphics.fps",
    label: "Frame Rate",
    help: "How often the window presents: 30 (the game's own), 60, 120 or uncapped. \
           A change applies at the window's next turn.",
    kind: Kind::Choice(&[
        Choice {
            value: 30,
            name: "30",
            label: "30",
        },
        Choice {
            value: 60,
            name: "60",
            label: "60",
        },
        Choice {
            value: 120,
            name: "120",
            label: "120",
        },
        Choice {
            value: 0,
            name: "uncapped",
            label: "Uncapped",
        },
    ]),
    default: Value::Int(30),
    env: Some("H2_FPS"),
    flag: Some("--fps"),
    aliases: &[("unlimited", Value::Int(0)), ("max", Value::Int(0))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 20,
    }),
};

/// The guest's back buffer: always 640x480. Halo 2's 480p mode is an
/// upscale and its widescreen is anamorphic 640x480 (docs/graphics/high-resolution.md
/// "Terms" and "Prior art"), so no video mode changes it.
pub const GUEST_FRAME: (u32, u32) = (640, 480);

pub static RENDER_SCALE: Def = Def {
    key: "graphics.render_scale",
    label: "Resolution",
    help: "Internal resolution as a multiple of the game's 640x480; the row shows the \
           size it renders at (1280x960 for 2). The row steps 1 to 4; the file and \
           H2_RENDER_SCALE take up to 6. A change applies at the next frame.",
    kind: Kind::Int {
        valid: 1..=d3d::MAX_SCALE as i64,
        steps: &[1, 2, 3, 4],
        unit: Unit::Pixels {
            width: GUEST_FRAME.0,
            height: GUEST_FRAME.1,
        },
    },
    default: Value::Int(1),
    env: Some("H2_RENDER_SCALE"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 30,
    }),
};

/// [`LIGHTING`]'s values.
pub const LIGHTING_RETAIL: i64 = 0;
pub const LIGHTING_E3: i64 = 1;

pub static LIGHTING: Def = Def {
    key: "graphics.lighting",
    label: "Lighting",
    help: "Retail, or E3: harder, sharper shadows (docs/graphics/e3-lighting.md, phase 1). \
           E3 draws the game's shadow maps at the render scale (at least twice the game's \
           size) and compares them with one tap, in the shown image only; the game itself \
           sees retail. A change applies at the next frame.",
    kind: Kind::Choice(&[
        Choice {
            value: LIGHTING_RETAIL,
            name: "retail",
            label: "Retail",
        },
        Choice {
            value: LIGHTING_E3,
            name: "e3",
            label: "E3",
        },
    ]),
    default: Value::Int(LIGHTING_RETAIL),
    env: Some("H2_LIGHTING"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 35,
    }),
};

/// [`LIGHTING`]'s value as the view backend takes it.
pub fn lighting_of(value: u32) -> d3d::Lighting {
    if value as i64 == LIGHTING_E3 {
        d3d::Lighting::E3
    } else {
        d3d::Lighting::Retail
    }
}

pub static PAD_SPEAKER: Def = Def {
    key: "pad_speaker.enabled",
    label: "Controller Speaker",
    help: "Play the sounds the controller-speaker routes pick on a DualSense's \
           speaker (docs/audio/playstation-audio.md). H2_PAD_SPEAKER may also name a \
           routes file; its `enabled` decides.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_PAD_SPEAKER"),
    flag: None,
    aliases: &[],
    parse_env: Some(pad_speaker_env),
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen (`video_screen::model`).
    menu: None,
};

pub static PAD_MUTE: Def = Def {
    key: "pad.mute_button",
    label: "Mute Button",
    help: "A DualSense's mute button mutes the microphone and its LED shows it. \
           A change reaches the controller thread at once; off unmutes and puts the LED out.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_PAD_MUTE"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 40,
    }),
};

pub static STEAM_SPEAKER: Def = Def {
    key: "steam_pad.speaker",
    label: "Steam Pad Speaker",
    help: "With no DualSense speaker, a Steam Controller's trackpads buzz the pitch \
           and loudness of the sounds the controller-speaker routes pick, as haptics \
           (docs/input/steam-controller.md). Controller Speaker must be on too.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_STEAM_SPEAKER"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen (`video_screen::model`).
    menu: None,
};

pub static STEAM_SENSITIVITY: Def = Def {
    key: "steam_pad.sensitivity",
    label: "Trackpad Aim",
    help: "How far the Steam Controller's right trackpad turns you for a swipe, \
           1 to 10; 5 is the default.",
    kind: Kind::Int {
        valid: 1..=10,
        steps: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        unit: Unit::Plain,
    },
    default: Value::Int(5),
    env: Some("H2_STEAM_SENSITIVITY"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen (`video_screen::model`).
    menu: None,
};

/// The values of [`STEAM_LEFT_PAD`].
pub const LEFT_PAD_DPAD: i64 = 0;
pub const LEFT_PAD_STICK: i64 = 1;

pub static STEAM_LEFT_PAD: Def = Def {
    key: "steam_pad.left_pad",
    label: "Left Trackpad",
    help: "The Steam Controller's left trackpad: the d-pad when clicked, or a \
           second left stick (the controller's own stick still works).",
    kind: Kind::Choice(&[
        Choice {
            value: LEFT_PAD_DPAD,
            name: "dpad",
            label: "D-Pad",
        },
        Choice {
            value: LEFT_PAD_STICK,
            name: "stick",
            label: "Stick",
        },
    ]),
    default: Value::Int(LEFT_PAD_DPAD),
    env: Some("H2_STEAM_LEFT_PAD"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen (`video_screen::model`).
    menu: None,
};

/// The Steam Controller tuning the settings in `store` give, kept current
/// as they change.
pub fn steam_tuning(store: &Store) -> input::steam_pad::SteamTuning {
    let tuning = input::steam_pad::SteamTuning::new(
        left_pad_mode(store.handle::<i64>(STEAM_LEFT_PAD.key).get()),
        store
            .handle::<i64>(STEAM_SENSITIVITY.key)
            .get()
            .clamp(1, 10) as u8,
        store.handle::<bool>(STEAM_SPEAKER.key).get(),
    );
    let t = tuning.clone();
    store.on_change(STEAM_LEFT_PAD.key, move |v| {
        t.set_left_pad(left_pad_mode(v.as_int().unwrap_or(LEFT_PAD_DPAD)))
    });
    let t = tuning.clone();
    store.on_change(STEAM_SENSITIVITY.key, move |v| {
        t.set_sensitivity(v.as_int().unwrap_or(5).clamp(1, 10) as u8)
    });
    let t = tuning.clone();
    store.on_change(STEAM_SPEAKER.key, move |v| {
        t.set_speaker(v.as_bool().unwrap_or(true))
    });
    tuning
}

fn left_pad_mode(v: i64) -> input::trackpad::LeftPadMode {
    if v == LEFT_PAD_STICK {
        input::trackpad::LeftPadMode::Stick
    } else {
        input::trackpad::LeftPadMode::Dpad
    }
}

/// The values of [`LIVE_VISIBILITY`], as `live_glue` hands them to the HLE.
pub const VISIBILITY_PUBLIC: i64 = 0;
pub const VISIBILITY_FRIENDS: i64 = 1;
pub const VISIBILITY_INVITE: i64 = 2;

pub static LIVE_VISIBILITY: Def = Def {
    key: "live.visibility",
    label: "Game Visibility",
    help: "Who finds the Live games you host in the Custom Game Browser and on the \
           web: everyone, your friends, or nobody (invitations only). Invitations \
           work in all three. Changing it updates an open lobby at once \
           (docs/live/live-custom-games.md).",
    kind: Kind::Choice(&[
        Choice {
            value: VISIBILITY_PUBLIC,
            name: "public",
            label: "Public",
        },
        Choice {
            value: VISIBILITY_FRIENDS,
            name: "friends",
            label: "Friends Only",
        },
        Choice {
            value: VISIBILITY_INVITE,
            name: "invite",
            label: "Invite Only",
        },
    ]),
    default: Value::Int(VISIBILITY_PUBLIC),
    env: Some("H2_LIVE_VISIBILITY"),
    flag: None,
    aliases: &[
        ("everyone", Value::Int(VISIBILITY_PUBLIC)),
        ("private", Value::Int(VISIBILITY_INVITE)),
    ],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is the Live party lobby's (`live_glue`), not a screen of ours.
    menu: None,
};

/// The most players a co-op campaign over Live takes, one per console
/// (`crate::coop_probe`, docs/live/live-coop.md section 12). Only the host's
/// value counts: it lifts the session's, the lobby's and the start's
/// limits to it. 2 is retail co-op.
pub static COOP_MAX_PLAYERS: Def = Def {
    key: "coop.max_players",
    label: "Co-op Players",
    help: "The most players a co-op campaign you host over Xbox Live takes, one \
           per console: 2 (the game's own) up to 16. Above 2 is experimental: \
           players 3 and up start beside the others and the levels' scripts \
           only know two players (docs/live/live-coop.md section 13). Applies \
           at the next launch.",
    kind: Kind::Int {
        valid: 2..=16,
        steps: &[2, 3, 4, 8, 16],
        unit: Unit::Plain,
    },
    default: Value::Int(2),
    env: Some("H2_COOP_MAX_PLAYERS"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Restart,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: None,
};

/// [`COOP_MAX_PLAYERS`] in `store`, held to 2 to 16.
pub fn coop_max_players(store: &Store) -> u32 {
    match store.get(COOP_MAX_PLAYERS.key) {
        Some(Value::Int(n)) => crate::coop_probe::clamp_players(n),
        _ => crate::coop_probe::MAX_PLAYERS,
    }
}

/// Whether a co-op campaign this console hosts over Xbox Live lets a friend
/// join while it runs (coop::join, docs/live/live-coop.md section 14).
pub static COOP_JOIN_IN_PROGRESS: Def = Def {
    key: "coop.join_in_progress",
    label: "Join In Progress",
    help: "A friend can join a co-op campaign you host while it runs: the game \
           holds for a second or two while it sends them where everyone is, and \
           nobody goes back to a checkpoint. Off, joining waits for the next \
           level (docs/live/live-coop.md section 14).",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_COOP_JOIN_IN_PROGRESS"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: None,
};

/// Presentation on a synchronous co-op guest. The host always uses its local camera.
pub static COOP_LOOK_PREDICTION: Def = Def {
    key: "coop.look_prediction",
    label: "Co-op Look Prediction",
    help: "Draw your pending turns while waiting for a co-op host's updates.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_COOP_LOOK_PREDICTION"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: None,
};

/// [`COOP_JOIN_IN_PROGRESS`] in `store`.
pub fn coop_join_in_progress(store: &Store) -> bool {
    !matches!(
        store.get(COOP_JOIN_IN_PROGRESS.key),
        Some(Value::Bool(false))
    )
}

/// How strong the weapons' aim assist is (crate::aim_assist,
/// docs/game/aim-assist.md). Read at each map load.
pub static AIM_ASSIST: Def = Def {
    key: "controls.aim_assist",
    label: "Aim Assist",
    help: "How strongly the crosshair is pulled to and held on targets. Auto is Strong \
           while the on-screen touch controls show and the game's own otherwise; Strong and \
           Very strong widen the weapons' aim cone and pull. Applies at the next map \
           load; multiplayer games always use the game's own (docs/game/aim-assist.md).",
    kind: Kind::Choice(&[
        Choice {
            value: crate::aim_assist::SETTING_AUTO,
            name: "auto",
            label: "Auto",
        },
        Choice {
            value: crate::aim_assist::SETTING_DEFAULT,
            name: "default",
            label: "Default",
        },
        Choice {
            value: crate::aim_assist::SETTING_STRONG,
            name: "strong",
            label: "Strong",
        },
        Choice {
            value: crate::aim_assist::SETTING_VERY_STRONG,
            name: "very_strong",
            label: "Very Strong",
        },
    ]),
    default: Value::Int(crate::aim_assist::SETTING_AUTO),
    env: Some("H2_AIM_ASSIST"),
    flag: None,
    aliases: &[
        ("retail", Value::Int(crate::aim_assist::SETTING_DEFAULT)),
        (
            "very-strong",
            Value::Int(crate::aim_assist::SETTING_VERY_STRONG),
        ),
    ],
    parse_env: None,
    // The handle changes at once; the hook reads it at each map load.
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 50,
    }),
};

/// The iOS touch controls' rows (`input::touch`, docs/runtime/ios.md). On
/// the Controls screen on iOS, in place of the Steam Controller's
/// (`video_screen::Switches`); read live by `apple-host`.
pub static TOUCH_LOOK: Def = Def {
    key: "touch.look",
    label: "Touch Look Speed",
    help: "How fast a drag on the right side of the screen turns you, 1 to 10; 5 \
           is the default.",
    kind: Kind::Int {
        valid: 1..=10,
        steps: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        unit: Unit::Plain,
    },
    default: Value::Int(5),
    env: Some("H2_TOUCH_LOOK"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen on iOS (`video_screen::model`).
    menu: None,
};

/// The values of [`TOUCH_CROUCH`].
pub const TOUCH_CROUCH_TOGGLE: i64 = 0;
pub const TOUCH_CROUCH_HOLD: i64 = 1;

pub static TOUCH_CROUCH: Def = Def {
    key: "touch.crouch",
    label: "Touch Crouch",
    help: "The on-screen Crouch button: a tap crouches and the next stands \
           (Toggle), or crouched only while held (Hold).",
    kind: Kind::Choice(&[
        Choice {
            value: TOUCH_CROUCH_TOGGLE,
            name: "toggle",
            label: "Toggle",
        },
        Choice {
            value: TOUCH_CROUCH_HOLD,
            name: "hold",
            label: "Hold",
        },
    ]),
    default: Value::Int(TOUCH_CROUCH_TOGGLE),
    env: Some("H2_TOUCH_CROUCH"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen on iOS (`video_screen::model`).
    menu: None,
};

pub static TOUCH_GYRO: Def = Def {
    key: "touch.gyro",
    label: "Gyro Aim",
    help: "Aim by turning the device as well as by dragging: 0 is off, 1 to 10 \
           the speed (5 is the default speed).",
    kind: Kind::Int {
        valid: 0..=10,
        steps: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        unit: Unit::Plain,
    },
    default: Value::Int(0),
    env: Some("H2_TOUCH_GYRO"),
    flag: None,
    aliases: &[("off", Value::Int(0))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen on iOS (`video_screen::model`).
    menu: None,
};

/// Look Acceleration for direct touch look (`input::touch_look::accel_gain`):
/// 0 to 10, 5 the default (1.0).
pub static TOUCH_ACCEL: Def = Def {
    key: "touch.look_accel",
    label: "Look Acceleration",
    help: "How much farther a fast drag turns than a slow one: 0 is none (every \
           drag turns the same per point), 5 the default, 10 the most.",
    kind: Kind::Int {
        valid: 0..=10,
        steps: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        unit: Unit::Plain,
    },
    default: Value::Int(5),
    env: Some("H2_TOUCH_LOOK_ACCEL"),
    flag: None,
    aliases: &[("off", Value::Int(0))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen on iOS (`video_screen::model`).
    menu: None,
};

/// Touch Auto Fire: the touch controls pull the trigger while the reticle is
/// red over an enemy (`input::touch_layout::fire_mode`).
pub static TOUCH_AUTO_FIRE: Def = Def {
    key: "touch.auto_fire",
    label: "Touch Auto Fire",
    help: "Fire by itself while the reticle is red over an enemy, so both thumbs \
           can move and aim. Semi-automatic weapons fire at their own rate; the \
           sword never lunges by itself.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: Some("H2_TOUCH_AUTO_FIRE"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen on iOS (`video_screen::model`).
    menu: None,
};

/// [`HALO1_SANDBOX`]'s values.
pub const SANDBOX_HALO1: i64 = 1;
pub const SANDBOX_HALO2: i64 = 2;

pub static HALO1_SANDBOX: Def = Def {
    key: "multiplayer.halo1_sandbox",
    label: "Sandbox",
    help: "Which sandbox converted Halo 1 multiplayer maps play with: Halo 1's \
           weapons, vehicles and health packs, or Halo 2's. Chosen on the \
           pregame lobby's Halo 1 Options screen; applies at the next game's map \
           load (docs/halo1/multiplayer.md).",
    kind: Kind::Choice(&[
        Choice {
            value: SANDBOX_HALO1,
            name: "h1",
            label: "Halo 1 Sandbox",
        },
        Choice {
            value: SANDBOX_HALO2,
            name: "h2",
            label: "Halo 2 Sandbox",
        },
    ]),
    default: Value::Int(SANDBOX_HALO1),
    env: Some("H2_HALO1_SANDBOX"),
    flag: None,
    aliases: &[
        ("halo1", Value::Int(SANDBOX_HALO1)),
        ("halo2", Value::Int(SANDBOX_HALO2)),
    ],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    // On the pregame lobby's Halo 1 Options screen (crate::h1_options), not
    // in Settings: it is chosen per game.
    menu: None,
};

/// [`HALO1_ASSETS`]'s and [`HALO1_HUD`]'s values: Halo 1's or Halo 2's.
pub const HALO1: i64 = 1;
pub const HALO2: i64 = 2;

/// Halo 1's or Halo 2's, as a setting's two choices.
const HALO_CHOICES: &[Choice] = &[
    Choice {
        value: HALO1,
        name: "h1",
        label: "Halo 1",
    },
    Choice {
        value: HALO2,
        name: "h2",
        label: "Halo 2",
    },
];

/// Which assets a Halo 1 campaign level is played with (the level variant,
/// docs/halo1/campaign.md §5). On the campaign's Game Options page
/// (crate::h1_options).
pub static HALO1_ASSETS: Def = Def {
    key: "campaign.halo1_assets",
    label: "Assets",
    help: "Which characters, weapons and vehicles a Halo 1 campaign level plays \
           with: Halo 1's own, converted, or Halo 2's. Chosen on the Game Options \
           page before the level; applies at the level's load \
           (docs/halo1/campaign.md).",
    kind: Kind::Choice(HALO_CHOICES),
    default: Value::Int(HALO1),
    env: Some("H2_HALO1_ASSETS"),
    flag: None,
    aliases: &[("halo1", Value::Int(HALO1)), ("halo2", Value::Int(HALO2))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: None,
};

/// Which HUD a Halo 1 map (campaign or multiplayer) shows: Halo 1's or
/// Halo 2's (crate::hud). On both Halo 1 options screens.
pub static HALO1_HUD: Def = Def {
    key: "halo1.hud",
    label: "HUD",
    help: "Which HUD Halo 1 maps show: Halo 1's or Halo 2's. Chosen on the Halo 1 \
           options screens; applies at the next game start.",
    kind: Kind::Choice(HALO_CHOICES),
    default: Value::Int(HALO1),
    env: Some("H2_HALO1_HUD"),
    flag: None,
    aliases: &[("halo1", Value::Int(HALO1)), ("halo2", Value::Int(HALO2))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: None,
};

/// Whether `def` (one of the Halo 1 or Halo 2 choices) is Halo 1's in
/// `store`.
pub fn is_halo1(store: &Store, def: &Def) -> bool {
    match store.get(def.key) {
        Some(Value::Int(v)) => v != HALO2,
        _ => true,
    }
}

/// The content variant [`HALO1_ASSETS`] names in `store`: `h1` or `h2`.
pub fn halo1_assets(store: &Store) -> &'static str {
    if is_halo1(store, &HALO1_ASSETS) { "h1" } else { "h2" }
}

/// The content variant [`HALO1_SANDBOX`] names in `store`: `h1` or `h2`.
pub fn halo1_sandbox(store: &Store) -> &'static str {
    match store.get(HALO1_SANDBOX.key) {
        Some(Value::Int(SANDBOX_HALO2)) => "h2",
        _ => "h1",
    }
}

/// Every setting, in the order `h2 settings list` shows them.
pub static ALL: &[&Def] = &[
    &FOV,
    &FPS,
    &RENDER_SCALE,
    &LIGHTING,
    &PAD_SPEAKER,
    &PAD_MUTE,
    &STEAM_SPEAKER,
    &STEAM_SENSITIVITY,
    &STEAM_LEFT_PAD,
    &LIVE_VISIBILITY,
    &COOP_MAX_PLAYERS,
    &COOP_JOIN_IN_PROGRESS,
    &COOP_LOOK_PREDICTION,
    &AIM_ASSIST,
    &TOUCH_LOOK,
    &TOUCH_CROUCH,
    &TOUCH_GYRO,
    &TOUCH_ACCEL,
    &TOUCH_AUTO_FIRE,
    &HALO1_SANDBOX,
    &HALO1_ASSETS,
    &HALO1_HUD,
    // The campaign skulls, kept per profile (crate::skulls, docs/game/skulls.md).
    &crate::skulls::SKULLS[0],
    &crate::skulls::SKULLS[1],
    &crate::skulls::SKULLS[2],
    &crate::skulls::SKULLS[3],
    &crate::skulls::SKULLS[4],
    &crate::skulls::SKULLS[5],
    &crate::skulls::SKULLS[6],
    &crate::skulls::SKULLS[7],
    &crate::skulls::SKULLS[8],
    &crate::skulls::SKULLS[9],
    &crate::skulls::SKULLS[10],
    &crate::skulls::SKULLS[11],
    &crate::skulls::SKULLS[12],
    &crate::skulls::SKULLS[13],
    &crate::skulls::SKULLS[14],
    // Ours, hidden in the campaign (crate::skulls_custom).
    &crate::skulls::SKULLS[15],
    &crate::skulls::SKULLS[16],
    &crate::skulls::SKULLS[17],
    &crate::skulls::SKULLS[18],
    &crate::skulls::SKULLS[19],
    &crate::skulls::SKULLS[20],
    // Skulls in multiplayer games (crate::skulls_mp, docs/game/skulls.md).
    &crate::skulls_mp::MP_FAMINE,
    &crate::skulls_mp::MP_BLIND,
    &crate::skulls_mp::MP_LOW_GRAVITY,
    // Voice chat (crate::voice_settings, the Voice screen).
    &crate::voice_settings::INPUT_DEVICE,
    &crate::voice_settings::OUTPUT_DEVICE,
    &crate::voice_settings::MODE,
    &crate::voice_settings::PTT_BUTTON,
    &crate::voice_settings::MIC_MUTED,
    &crate::voice_settings::INCOMING_MUTED,
    &crate::voice_settings::INPUT_VOLUME,
    &crate::voice_settings::OUTPUT_VOLUME,
    // The Clankers page of a custom game's Game Setup (crate::clankers,
    // docs/game/bots.md section 7).
    &crate::clankers::ENABLED,
    &crate::clankers::COUNT,
    &crate::clankers::DIFFICULTY,
    &crate::clankers::TEAM,
    &crate::clankers::AGGRESSION,
    &crate::clankers::AIM_STYLE,
    &crate::clankers::TAUNTS,
    &crate::clankers::GLITCHES,
];

/// Every setting of this run: [`ALL`], with the voice device settings
/// offering the host's devices once they are listed
/// ([`crate::voice_settings::use_host_devices`]).
pub fn defs() -> Vec<&'static Def> {
    let [input, output] = crate::voice_settings::device_defs();
    ALL.iter()
        .map(|d| match d.key {
            k if k == input.key => input,
            k if k == output.key => output,
            _ => *d,
        })
        .collect()
}

/// `H2_PAD_SPEAKER` as the controller-speaker settings read it: `0`, `1`,
/// or a routes file whose `enabled` decides.
fn pad_speaker_env(text: &str) -> Result<Value, String> {
    dsound::padroute::PadSpeakerConfig::from_env(|k| {
        (k == "H2_PAD_SPEAKER").then(|| text.to_string())
    })
    .map(|c| Value::Bool(c.enabled))
    .map_err(|e| e.to_string())
}

/// The store of a run with data root `data`: `<data>/settings.toml`, the
/// process environment and `flags`.
pub fn load(data: &Path, flags: &[Flag]) -> (Store, Vec<Problem>) {
    Store::load(
        &defs(),
        Some(&settings::path_in(data)),
        &|k| std::env::var(k).ok(),
        flags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("h2-defs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_env(
        data: &Path,
        vars: &'static [(&'static str, &'static str)],
    ) -> (Store, Vec<Problem>) {
        let env = move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        };
        Store::load(ALL, Some(&settings::path_in(data)), &env, &[])
    }

    #[test]
    fn keys_and_variables_are_unique_and_every_default_is_valid() {
        let mut keys: Vec<_> = ALL.iter().map(|d| d.key).collect();
        let mut envs: Vec<_> = ALL.iter().filter_map(|d| d.env).collect();
        keys.sort();
        keys.dedup();
        envs.sort();
        envs.dedup();
        assert_eq!(keys.len(), ALL.len());
        // Skulls are per-profile only and have no variable.
        assert_eq!(envs.len(), ALL.iter().filter(|d| d.env.is_some()).count());
        for d in ALL {
            assert!(d.key.contains('.'), "{}", d.key);
            assert_eq!(d.check(d.default), Ok(d.default), "{}", d.key);
        }
    }

    #[test]
    fn a_settings_toml_written_by_the_old_code_loads() {
        // As speaker_setting, fov_setting and video_setting wrote it.
        let data = tmp("old");
        std::fs::write(
            settings::path_in(&data),
            "[pad_speaker]\nenabled = false\n\n[graphics]\nfov = 95\nfps = \"uncapped\"\nrender_scale = 2\n",
        )
        .unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.get(FOV.key), Some(Value::Int(95)));
        assert_eq!(s.get(FPS.key), Some(Value::Int(0)));
        assert_eq!(s.get(RENDER_SCALE.key), Some(Value::Int(2)));
        assert_eq!(s.get(PAD_SPEAKER.key), Some(Value::Bool(false)));
        assert_eq!(s.get(PAD_MUTE.key), Some(Value::Bool(true)));
        // `fov = 0` was retail.
        std::fs::write(settings::path_in(&data), "[graphics]\nfov = 0\n").unwrap();
        assert_eq!(with_env(&data, &[]).0.get(FOV.key), Some(Value::Int(70)));
    }

    #[test]
    fn the_variables_read_as_before() {
        let data = tmp("env");
        let (s, p) = with_env(
            &data,
            &[
                ("H2_FOV", "0"),
                ("H2_FPS", "Uncapped"),
                ("H2_RENDER_SCALE", "6"),
                ("H2_PAD_SPEAKER", "0"),
                ("H2_PAD_MUTE", "0"),
                ("H2_LIVE_VISIBILITY", "friends"),
                ("H2_STEAM_SPEAKER", "0"),
                ("H2_STEAM_SENSITIVITY", "8"),
                ("H2_STEAM_LEFT_PAD", "stick"),
                ("H2_LIGHTING", "e3"),
                ("H2_AIM_ASSIST", "strong"),
                ("H2_TOUCH_LOOK", "7"),
                ("H2_TOUCH_CROUCH", "hold"),
                ("H2_TOUCH_GYRO", "off"),
                ("H2_TOUCH_LOOK_ACCEL", "off"),
                ("H2_TOUCH_AUTO_FIRE", "1"),
                ("H2_HALO1_SANDBOX", "h2"),
                ("H2_HALO1_ASSETS", "h2"),
                ("H2_HALO1_HUD", "halo2"),
                ("H2_COOP_MAX_PLAYERS", "4"),
                ("H2_COOP_JOIN_IN_PROGRESS", "0"),
                ("H2_COOP_LOOK_PREDICTION", "0"),
                ("H2_VOICE_INPUT", "default"),
                ("H2_VOICE_OUTPUT", "game"),
                ("H2_VOICE_MODE", "ptt"),
                ("H2_VOICE_PTT_BUTTON", "back"),
                ("H2_VOICE_MIC_MUTED", "1"),
                ("H2_VOICE_INCOMING_MUTED", "0"),
                ("H2_VOICE_INPUT_VOLUME", "50"),
                ("H2_VOICE_OUTPUT_VOLUME", "80"),
            ],
        );
        {
            use crate::voice_settings::*;
            let h = VoiceHandles::new(&s);
            assert_eq!(h.mode.get(), MODE_PUSH);
            assert_eq!(h.push_to_talk_button.get(), PTT_BACK);
            assert!(h.mic_muted.get() && !h.incoming_muted.get());
            assert_eq!((h.input_volume.get(), h.output_volume.get()), (50, 80));
            assert_eq!((h.input_device.get(), h.output_device.get()), (0, 0));
        }
        assert_eq!(coop_max_players(&s), 4);
        assert!(!coop_join_in_progress(&s));
        assert_eq!(s.get(COOP_LOOK_PREDICTION.key), Some(Value::Bool(false)));
        assert_eq!(s.get(TOUCH_ACCEL.key), Some(Value::Int(0)));
        assert_eq!(s.get(TOUCH_AUTO_FIRE.key), Some(Value::Bool(true)));
        assert_eq!(halo1_sandbox(&s), "h2");
        assert_eq!(halo1_assets(&s), "h2");
        assert!(!is_halo1(&s, &HALO1_HUD));
        assert_eq!(s.get(TOUCH_LOOK.key), Some(Value::Int(7)));
        assert_eq!(s.get(TOUCH_CROUCH.key), Some(Value::Int(TOUCH_CROUCH_HOLD)));
        assert_eq!(s.get(TOUCH_GYRO.key), Some(Value::Int(0)));
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.get(STEAM_SPEAKER.key), Some(Value::Bool(false)));
        assert_eq!(s.get(STEAM_SENSITIVITY.key), Some(Value::Int(8)));
        assert_eq!(s.get(STEAM_LEFT_PAD.key), Some(Value::Int(LEFT_PAD_STICK)));
        assert_eq!(s.get(LIGHTING.key), Some(Value::Int(LIGHTING_E3)));
        assert_eq!(
            s.get(LIVE_VISIBILITY.key),
            Some(Value::Int(VISIBILITY_FRIENDS))
        );
        assert_eq!(s.get(FOV.key), Some(Value::Int(70)));
        assert_eq!(s.get(FPS.key), Some(Value::Int(0)));
        assert_eq!(s.get(RENDER_SCALE.key), Some(Value::Int(6)));
        assert_eq!(s.get(PAD_SPEAKER.key), Some(Value::Bool(false)));
        assert_eq!(s.get(PAD_MUTE.key), Some(Value::Bool(false)));
        assert!(
            ALL.iter()
                .filter(|d| d.env.is_some())
                .all(|d| s.state(d.key).unwrap().locked)
        );
        // Bad ones are reported and ignored.
        let (s, p) = with_env(
            &data,
            &[
                ("H2_FOV", "20"),
                ("H2_FPS", "45"),
                ("H2_RENDER_SCALE", "2x"),
                ("H2_PAD_MUTE", "maybe"),
            ],
        );
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(p[1].message.contains("H2_FPS=\"45\""), "{}", p[1]);
        assert!(ALL.iter().all(|d| !s.state(d.key).unwrap().locked));
        // H2_PAD_SPEAKER may name a routes file.
        let routes = data.join("routes.toml");
        std::fs::write(&routes, "enabled = false\n").unwrap();
        let text = routes.display().to_string();
        assert_eq!(pad_speaker_env(&text), Ok(Value::Bool(false)));
        assert_eq!(pad_speaker_env("1"), Ok(Value::Bool(true)));
        assert!(pad_speaker_env(&data.join("missing.toml").display().to_string()).is_err());
    }

    #[test]
    fn the_steam_tuning_follows_its_settings() {
        use input::trackpad::LeftPadMode;
        let data = tmp("steam");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        let t = steam_tuning(&s);
        assert_eq!(t.left_pad(), LeftPadMode::Dpad);
        assert_eq!(t.sensitivity(), 5);
        assert!(t.speaker());
        s.set(STEAM_LEFT_PAD.key, Value::Int(LEFT_PAD_STICK))
            .unwrap();
        s.set(STEAM_SENSITIVITY.key, Value::Int(9)).unwrap();
        s.set(STEAM_SPEAKER.key, Value::Bool(false)).unwrap();
        assert_eq!(t.left_pad(), LeftPadMode::Stick);
        assert_eq!(t.sensitivity(), 9);
        assert!(!t.speaker());
        assert!(s.set(STEAM_SENSITIVITY.key, Value::Int(11)).is_err());
        assert_eq!(t.sensitivity(), 9);
    }

    #[test]
    fn aim_assist_is_auto_unless_asked() {
        use crate::aim_assist::{
            SETTING_AUTO, SETTING_DEFAULT, SETTING_STRONG, SETTING_VERY_STRONG,
        };
        let data = tmp("aim");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(AIM_ASSIST.default, Value::Int(SETTING_AUTO));
        assert_eq!(s.get(AIM_ASSIST.key), Some(Value::Int(SETTING_AUTO)));
        assert_eq!(s.handle::<i64>(AIM_ASSIST.key).get(), SETTING_AUTO);
        assert_eq!(AIM_ASSIST.scope, Scope::Sim);
        assert_eq!(
            AIM_ASSIST.apply,
            Apply::Live,
            "the hook reads it at each map load"
        );
        assert_eq!(AIM_ASSIST.menu.expect("a row").screen, "video");
        // The file takes the names.
        std::fs::write(
            settings::path_in(&data),
            "[controls]\naim_assist = \"very_strong\"\n",
        )
        .unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.get(AIM_ASSIST.key), Some(Value::Int(SETTING_VERY_STRONG)));
        assert!(!s.state(AIM_ASSIST.key).unwrap().locked);
        // The variable takes names, labels and aliases, and locks it.
        for (text, want) in [
            ("default", SETTING_DEFAULT),
            ("retail", SETTING_DEFAULT),
            ("Strong", SETTING_STRONG),
            ("very-strong", SETTING_VERY_STRONG),
            ("auto", SETTING_AUTO),
        ] {
            let env = move |k: &str| (k == "H2_AIM_ASSIST").then(|| text.to_string());
            let (s, p) = Store::load(ALL, Some(&settings::path_in(&data)), &env, &[]);
            assert!(p.is_empty(), "{text}: {p:?}");
            assert_eq!(s.get(AIM_ASSIST.key), Some(Value::Int(want)), "{text}");
            assert!(s.state(AIM_ASSIST.key).unwrap().locked, "{text}");
        }
        let (s, p) = with_env(&data, &[("H2_AIM_ASSIST", "maximum")]);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(
            s.get(AIM_ASSIST.key),
            Some(Value::Int(SETTING_VERY_STRONG)),
            "the file's"
        );
        // A press steps Auto, Default, Strong, Very Strong and round.
        let s = Store::defaults(ALL);
        for want in [
            SETTING_DEFAULT,
            SETTING_STRONG,
            SETTING_VERY_STRONG,
            SETTING_AUTO,
        ] {
            s.step(AIM_ASSIST.key).unwrap();
            assert_eq!(s.get(AIM_ASSIST.key), Some(Value::Int(want)));
        }
    }

    #[test]
    fn co_op_takes_two_players_unless_the_host_asks_for_more() {
        let data = tmp("coop-max");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(coop_max_players(&s), 2, "retail co-op by default");
        assert_eq!(COOP_MAX_PLAYERS.scope, Scope::Sim);
        assert_eq!(COOP_MAX_PLAYERS.apply, Apply::Restart);
        std::fs::write(settings::path_in(&data), "[coop]\nmax_players = 4\n").unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(coop_max_players(&s), 4);
        let (s, _) = with_env(&data, &[("H2_COOP_MAX_PLAYERS", "16")]);
        assert_eq!(coop_max_players(&s), 16);
        assert!(s.state(COOP_MAX_PLAYERS.key).unwrap().locked);
        // Outside 2 to 16: refused, the default holds.
        let (s, p) = with_env(&data, &[("H2_COOP_MAX_PLAYERS", "17")]);
        assert!(!p.is_empty());
        assert!((2..=16).contains(&coop_max_players(&s)));
    }

    #[test]
    fn join_in_progress_is_on_unless_turned_off() {
        let data = tmp("coop-jip");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert!(coop_join_in_progress(&s));
        assert_eq!(COOP_JOIN_IN_PROGRESS.apply, Apply::Live);
        std::fs::write(
            settings::path_in(&data),
            "[coop]\njoin_in_progress = false\n",
        )
        .unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert!(!coop_join_in_progress(&s));
        assert_eq!(s.get(COOP_LOOK_PREDICTION.key), Some(Value::Bool(false)));
        let (s, _) = with_env(&data, &[("H2_COOP_JOIN_IN_PROGRESS", "1")]);
        assert!(coop_join_in_progress(&s));
    }

    #[test]
    fn halo1_maps_play_halo1s_sandbox_unless_asked() {
        let data = tmp("sandbox");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(halo1_sandbox(&s), "h1");
        assert_eq!(HALO1_SANDBOX.apply, Apply::Live, "it counts from the next game");
        std::fs::write(
            settings::path_in(&data),
            "[multiplayer]\nhalo1_sandbox = \"h2\"\n",
        )
        .unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(halo1_sandbox(&s), "h2");
        let (s, _) = with_env(&data, &[("H2_HALO1_SANDBOX", "halo1")]);
        assert_eq!(halo1_sandbox(&s), "h1");
        assert!(s.state(HALO1_SANDBOX.key).unwrap().locked);
    }

    #[test]
    fn lighting_is_retail_unless_asked_and_has_a_video_row() {
        let data = tmp("lighting");
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(LIGHTING.default, Value::Int(LIGHTING_RETAIL));
        assert_eq!(s.get(LIGHTING.key), Some(Value::Int(LIGHTING_RETAIL)));
        assert_eq!(lighting_of(LIGHTING_RETAIL as u32), d3d::Lighting::Retail);
        assert_eq!(lighting_of(LIGHTING_E3 as u32), d3d::Lighting::E3);
        let menu = LIGHTING.menu.expect("a row");
        assert_eq!(menu.screen, "video");
        // Between Resolution and Mute Button.
        assert!(RENDER_SCALE.menu.unwrap().order < menu.order);
        assert!(menu.order < PAD_MUTE.menu.unwrap().order);
        assert_eq!(LIGHTING.apply, Apply::Live, "no restart");
        // The file takes the names; the variable locks it.
        std::fs::write(settings::path_in(&data), "[graphics]\nlighting = \"e3\"\n").unwrap();
        let (s, p) = with_env(&data, &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.get(LIGHTING.key), Some(Value::Int(LIGHTING_E3)));
        assert!(!s.state(LIGHTING.key).unwrap().locked);
        let (s, _) = with_env(&data, &[("H2_LIGHTING", "retail")]);
        assert_eq!(s.get(LIGHTING.key), Some(Value::Int(LIGHTING_RETAIL)));
        assert!(s.state(LIGHTING.key).unwrap().locked);
        let (s, p) = with_env(&data, &[("H2_LIGHTING", "stencil")]);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(
            s.get(LIGHTING.key),
            Some(Value::Int(LIGHTING_E3)),
            "the file's"
        );
    }
}
