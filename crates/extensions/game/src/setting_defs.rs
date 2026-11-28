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
        order: 30,
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

pub(crate) const LEGACY_SCALE: settings::LegacyScale = settings::LegacyScale {
    key: "graphics.render_scale",
    env: "H2_RENDER_SCALE",
    frame: GUEST_FRAME,
    max: gfx::external::MAX_SCALE,
};

pub static RESOLUTION: Def = Def {
    key: "graphics.resolution",
    label: "Resolution",
    help: "Native follows the window in pixels. Fullscreen Native uses up to 4K pixels. Pick a size to fix the resolution.",
    kind: Kind::Resolution(LEGACY_SCALE),
    default: Value::Int(0),
    env: Some("H2_RESOLUTION"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 10,
    }),
};

/// Resolution for headless graphics paths that have no settings store.
pub fn resolution_from_env() -> Result<settings::Resolution, String> {
    let vars = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(v) = vars("H2_RESOLUTION") {
        return settings::Resolution::parse(&v)
            .ok_or_else(|| format!("H2_RESOLUTION={v:?}: expected {}", RESOLUTION.expected()));
    }
    match vars(LEGACY_SCALE.env) {
        Some(v) => LEGACY_SCALE
            .parse(&v)
            .map_err(|e| format!("{}={v:?}: {e}", LEGACY_SCALE.env)),
        None => Ok(settings::Resolution::Native),
    }
}

pub const HUD_PINNED: u32 = 1;

pub static HUD_POSITION: Def = Def {
    key: "graphics.hud_position",
    label: "HUD Position",
    help: "Centered keeps the HUD in the middle safe area. Pinned keeps edge and corner indicators near the edges of the screen.",
    kind: Kind::Choice(&[
        Choice {
            value: 0,
            name: "centered",
            label: "Centered",
        },
        Choice {
            value: HUD_PINNED as i64,
            name: "pinned",
            label: "Pinned",
        },
    ]),
    default: Value::Int(HUD_PINNED as i64),
    env: Some("H2_HUD_POSITION"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 40,
    }),
};

/// The distance choices use half units. Highest is a separate selection.
pub const LOD_HIGHEST: u32 = 9;
pub static LOD_DISTANCE: Def = Def {
    key: "graphics.lod_distance",
    label: "LOD Distance",
    help: "Highest selects the finest model detail at any distance. 1x matches the rendering resolution.",
    kind: Kind::Choice(&[
        Choice { value: 0, name: "retail", label: "Retail" },
        Choice { value: 1, name: "0.5", label: "0.5x" },
        Choice { value: 2, name: "1.0", label: "1x" },
        Choice { value: 3, name: "1.5", label: "1.5x" },
        Choice { value: 4, name: "2.0", label: "2x" },
        Choice { value: 6, name: "3.0", label: "3x" },
        Choice { value: 8, name: "4.0", label: "4x" },
        Choice { value: LOD_HIGHEST as i64, name: "highest", label: "Highest" },
    ]),
    default: Value::Int(LOD_HIGHEST as i64),
    env: Some("H2_LOD_DISTANCE"),
    flag: None,
    aliases: &[
        ("1", Value::Int(2)), ("2", Value::Int(4)),
        ("3", Value::Int(6)), ("4", Value::Int(8)),
    ],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "video",
        order: 50,
    }),
};

/// Convert one pixel of the guest's projected size to output pixels for LOD.
pub fn lod_pixel_multiplier(output_height: u32, half_units: u32) -> f32 {
    if half_units == 0 {
        return 1.0;
    }
    (output_height as f32 / GUEST_FRAME.1 as f32) * (half_units as f32 * 0.5)
}

/// Select the readback and draw policy when rendering above guest resolution.
pub static SINGLE_COPY: Def = Def {
    key: "graphics.single_copy",
    label: "Single-copy scaled rendering",
    help: "In offline play, draw once and sample that image for guest readbacks. Disable for exact Xbox readbacks. Network play, films and lockstep always use exact readbacks.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: Some("H2_SINGLE_COPY"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Restart,
    scope: Scope::Sim,
    tier: Tier::Advanced,
    menu: None,
};

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

pub static PAD_LIGHTBAR: Def = Def {
    key: "controls.lightbar",
    label: "Lightbar",
    help: "Match each PlayStation controller's lightbar to its player's team or primary armour colour.",
    kind: Kind::Bool,
    default: Value::Bool(true),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    // Its row is on the Controls screen (video_screen::model).
    menu: None,
};

pub static PAD_MUTE: Def = Def {
    key: "voice.mute_button",
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
        screen: "voice",
        order: 90,
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
/// join while it runs, and so starts with the host alone
/// (docs/live/live-coop-join.md 13.4 and 14).
pub static COOP_JOIN_IN_PROGRESS: Def = Def {
    key: "coop.join_in_progress",
    label: "Join In Progress",
    help: "A friend can join a co-op campaign you host while it runs, so you can \
           start alone: the game holds for a second or two while it sends them \
           where everyone is, and nobody goes back to a checkpoint. Off, joining \
           waits for the next level (docs/live/live-coop.md section 14).",
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
/// docs/game/aim-assist.md). Campaign reads it at each map load.
pub static AIM_ASSIST: Def = Def {
    key: "controls.aim_assist",
    label: "Aim Assist",
    help: "How strongly the crosshair is pulled to and held on targets. Auto is Strong \
           while the on-screen touch controls show and the game's own otherwise; Strong and \
           Very strong widen the weapons' aim cone and pull. Campaign changes apply at the next map \
           load. Local multiplayer touch input uses the setting during play (docs/game/aim-assist.md).",
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
    // Campaign reads the handle at map load. Multiplayer touch reads it live.
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "controls",
        order: 60,
    }),
};

/// The iOS touch controls' rows (`input::touch`, docs/runtime/ios.md). On
/// the Controls screen on iOS, in place of the Steam Controller's
/// (`video_screen::Switches`); read live by `h2host`.
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
/// docs/halo1/campaign.md §5). On the campaign's Skulls page
/// (crate::h1_options).
pub static HALO1_ASSETS: Def = Def {
    key: "campaign.halo1_assets",
    label: "Assets",
    help: "Which characters, weapons and vehicles a Halo 1 campaign level plays \
           with: Halo 1's own, converted, or Halo 2's. Chosen on the Skulls page \
           before the level; applies at the level's load \
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

/// Whether the superbounce HUD's tutorial steps have been seen
/// (docs/game/superbounce.md, "The superbounce game type").
/// `superbounce_hud` sets it when the last step closes; `false` shows the
/// steps again the next time a game starts, and
/// `H2_SUPERBOUNCE_TUTORIAL=1` shows them whatever it says.
pub static SUPERBOUNCE_TUTORIAL_SEEN: Def = Def {
    key: "superbounce.tutorial_seen",
    label: "Superbounce Tutorial Seen",
    help: "The superbounce HUD's four tutorial steps have been shown to the end. \
           Set it to false to see them again.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::Diagnostic,
    menu: None,
};

/// The superbounce fall assist, opt in (docs/game/superbounce.md, "Hold X:
/// the fall assist"): holding X in the air steers the player onto a
/// strip. Off, nothing steers and nothing on the map or the HUD asks for
/// X. Its row is on the Controls screen (`video_screen::model`);
/// `superbounce_assist` reads it live.
pub static SUPERBOUNCE_FALL_ASSIST: Def = Def {
    key: "superbounce.fall_assist",
    label: "Fall Assist",
    help: "Superbounce: hold X while falling and the stick is steered so you \
           land on the strip. Off by default.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: "controls",
        order: 70,
    }),
};

/// `H2_SUPERBOUNCE_TUTORIAL=1` shows the superbounce tutorial steps even
/// when [`SUPERBOUNCE_TUTORIAL_SEEN`] is set.
pub const SUPERBOUNCE_TUTORIAL_ENV: &str = "H2_SUPERBOUNCE_TUTORIAL";

/// Whether the superbounce tutorial shows this run: never seen, or forced
/// by `env` (the value of [`SUPERBOUNCE_TUTORIAL_ENV`]).
pub fn superbounce_tutorial_shows(seen: bool, env: Option<&str>) -> bool {
    !seen || env.is_some_and(|v| v.trim() == "1")
}

/// A Clanker brain plays `player.0`, the local human seat, through the pad
/// tap, as `player.bot {"on": true}` turns it on
/// (docs/runtime/agent-api-sim.md, "`player.bot`: a Clanker plays the
/// player's seat"): for a lab run that starts with it on (`h2 play --set
/// player.bot=true`). `player_bot` follows it live; a real press hands
/// the seat back. Host scope: what it changes is the pad, which a film
/// records, so a film never lends it to its playback.
pub static PLAYER_BOT: Def = Def {
    key: "player.bot",
    label: "Clanker Plays For You",
    help: "A Clanker brain plays your seat in multiplayer games, for benchmarks and \
           spectating. Any press on your pad takes it back.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::Diagnostic,
    menu: None,
};

/// Every setting, in the order `h2 settings list` shows them.
pub static ALL: &[&Def] = &[
    &FOV,
    &FPS,
    &RESOLUTION,
    &HUD_POSITION,
    &LOD_DISTANCE,
    &SINGLE_COPY,
    &PAD_SPEAKER,
    &PAD_LIGHTBAR,
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
    &SUPERBOUNCE_TUTORIAL_SEEN,
    &SUPERBOUNCE_FALL_ASSIST,
    &PLAYER_BOT,
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
    // The Forge monitor's look, kept per profile (forge::look::saved,
    // docs/game/forge-monitor-appearance.md).
    &forge::look::saved::DEFS[0],
    &forge::look::saved::DEFS[1],
    &forge::look::saved::DEFS[2],
    &forge::look::saved::DEFS[3],
    &forge::look::saved::DEFS[4],
    // The Forge body D-pad Up swaps into, kept per profile (forge::body,
    // docs/game/forge-design-slices.md, "The monitor body").
    &forge::body::DEF,
    // The Forge monitor's block-out grid and dev view (forge::piece,
    // docs/game/forge-blockout.md).
    &forge::piece::GRID,
    &forge::piece::DEV_VIEW,
    // Skulls in multiplayer games (crate::skulls_mp, docs/game/skulls.md).
    &crate::skulls_mp::MP_FAMINE,
    &crate::skulls_mp::MP_BLIND,
    &crate::skulls_mp::MP_LOW_GRAVITY,
    // Voice chat (crate::voice_settings, the Voice screen).
    &crate::voice_settings::INPUT_DEVICE,
    &crate::voice_settings::OUTPUT_DEVICE,
    &crate::voice_settings::MODE,
    &crate::voice_settings::PTT_BUTTON,
    &crate::voice_settings::INPUT_VOLUME,
    &crate::voice_settings::OUTPUT_VOLUME,
    &crate::voice_settings::MIC_MUTED,
    &crate::voice_settings::INCOMING_MUTED,
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
    // The Firefight page of a custom game's Game Setup
    // (crate::firefight::options, docs/game/firefight.md).
    &crate::firefight::options::ENABLED,
    &crate::firefight::options::WAVE_SET,
    &crate::firefight::options::TEAMS,
    &crate::firefight::options::DIFFICULTY,
    &crate::firefight::options::LIVES,
    &crate::firefight::options::SKULLS,
    &crate::firefight::options::PODS,
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
        // Some settings are only changed through the store.
        assert_eq!(envs.len(), ALL.iter().filter(|d| d.env.is_some()).count());
        for d in ALL {
            assert!(d.key.contains('.'), "{}", d.key);
            assert_eq!(d.check(d.default), Ok(d.default), "{}", d.key);
        }
    }

    #[test]
    fn the_superbounce_tutorial_shows_until_seen_or_when_forced() {
        assert!(superbounce_tutorial_shows(false, None));
        assert!(!superbounce_tutorial_shows(true, None));
        assert!(!superbounce_tutorial_shows(true, Some("0")));
        assert!(superbounce_tutorial_shows(true, Some("1")));
        let store = Store::defaults(ALL);
        assert_eq!(
            store.get(SUPERBOUNCE_TUTORIAL_SEEN.key),
            Some(Value::Bool(false))
        );
        store
            .set(SUPERBOUNCE_TUTORIAL_SEEN.key, Value::Bool(true))
            .unwrap();
        assert!(store.handle::<bool>(SUPERBOUNCE_TUTORIAL_SEEN.key).get());
    }

    #[test]
    fn the_fall_assist_is_opt_in_with_a_controls_row() {
        let store = Store::defaults(ALL);
        assert_eq!(
            store.get(SUPERBOUNCE_FALL_ASSIST.key),
            Some(Value::Bool(false))
        );
        assert_eq!(
            SUPERBOUNCE_FALL_ASSIST.menu.expect("a row").screen,
            "controls"
        );
        store
            .set(SUPERBOUNCE_FALL_ASSIST.key, Value::Bool(true))
            .unwrap();
        assert!(store.handle::<bool>(SUPERBOUNCE_FALL_ASSIST.key).get());
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
        assert_eq!(
            s.get(RESOLUTION.key),
            Some(settings::Resolution::Fixed(1280, 960).value())
        );
        assert_eq!(s.get(SINGLE_COPY.key), Some(Value::Bool(true)));
        assert_eq!(s.get(PAD_SPEAKER.key), Some(Value::Bool(false)));
        assert_eq!(s.get(PAD_MUTE.key), Some(Value::Bool(true)));
        // `fov = 0` was retail.
        std::fs::write(settings::path_in(&data), "[graphics]\nfov = 0\n").unwrap();
        assert_eq!(with_env(&data, &[]).0.get(FOV.key), Some(Value::Int(70)));
    }

    #[test]
    fn explicit_resolution_wins_at_its_own_precedence() {
        use settings::Resolution::{Fixed, Native};

        let data = tmp("resolution");
        let path = settings::path_in(&data);
        std::fs::write(
            &path,
            "[graphics]\nrender_scale = 3\nresolution = \"native\"\n",
        )
        .unwrap();
        let load = |s: &Store| s.handle::<settings::Resolution>(RESOLUTION.key).get();
        let (file, p) = with_env(&data, &[]);
        assert!(p.is_empty());
        assert_eq!(load(&file), Native);
        let (legacy, p) = with_env(&data, &[("H2_RENDER_SCALE", "2")]);
        assert!(p.is_empty());
        assert_eq!(load(&legacy), Fixed(1280, 960));
        assert_eq!(
            legacy.state(RESOLUTION.key).unwrap().source,
            settings::Source::Env("H2_RENDER_SCALE")
        );

        let (env, p) = with_env(
            &data,
            &[("H2_RESOLUTION", "3440X1440"), ("H2_RENDER_SCALE", "2")],
        );
        assert!(p.is_empty());
        assert_eq!(load(&env), Fixed(3440, 1440));
        assert!(env.state(RESOLUTION.key).unwrap().locked);
        assert_eq!(lod_pixel_multiplier(1050, 2), 2.1875);
        assert_eq!(lod_pixel_multiplier(2880, 2), 6.0);
        for bad in [
            "2",
            "2x",
            "0x1080",
            "5120x0",
            "8193x480",
            "300x-1",
            "800x600x1",
        ] {
            assert!(RESOLUTION.parse(bad).is_err(), "{bad}");
        }
        let flags = [Flag::parse_set("graphics.resolution=native").unwrap()];
        let (flag, p) = Store::load(
            ALL,
            Some(&path),
            &|k| (k == "H2_RENDER_SCALE").then(|| "2".into()),
            &flags,
        );
        assert!(p.is_empty());
        assert_eq!(load(&flag), Native);
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
                ("H2_HUD_POSITION", "centered"),
                ("H2_LOD_DISTANCE", "1.5"),
                ("H2_SINGLE_COPY", "0"),
                ("H2_PAD_SPEAKER", "0"),
                ("H2_PAD_MUTE", "0"),
                ("H2_LIVE_VISIBILITY", "friends"),
                ("H2_STEAM_SPEAKER", "0"),
                ("H2_STEAM_SENSITIVITY", "8"),
                ("H2_STEAM_LEFT_PAD", "stick"),
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
        assert_eq!(
            s.get(LIVE_VISIBILITY.key),
            Some(Value::Int(VISIBILITY_FRIENDS))
        );
        assert_eq!(s.get(FOV.key), Some(Value::Int(70)));
        assert_eq!(s.get(FPS.key), Some(Value::Int(0)));
        assert_eq!(
            s.get(RESOLUTION.key),
            Some(settings::Resolution::Fixed(3840, 2880).value())
        );
        assert_eq!(s.get(SINGLE_COPY.key), Some(Value::Bool(false)));
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
        assert_eq!(AIM_ASSIST.menu.expect("a row").screen, "controls");
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
    fn lod_distance_defaults_to_highest_and_persists_the_distance_choices() {
        let data = tmp("lod-distance");
        let (store, problems) = with_env(&data, &[]);
        assert!(problems.is_empty());
        let setting = store.handle::<u32>(LOD_DISTANCE.key);
        assert_eq!(setting.get(), LOD_HIGHEST);
        assert_eq!(LOD_DISTANCE.parse("highest").unwrap(), LOD_DISTANCE.default);
        assert_eq!(LOD_DISTANCE.to_toml(LOD_DISTANCE.default).as_str(), Some("highest"));
        for (name, scale_at_4x) in [
            ("0.5", 2.0), ("1", 4.0), ("1.0", 4.0), ("1.5", 6.0),
            ("2", 8.0), ("3", 12.0), ("4", 16.0), ("retail", 1.0),
        ] {
            let v = LOD_DISTANCE.parse(name).unwrap();
            store.set(LOD_DISTANCE.key, v).unwrap();
            assert_eq!(lod_pixel_multiplier(1920, setting.get()), scale_at_4x);
            assert_eq!(with_env(&data, &[]).0.get(LOD_DISTANCE.key), Some(v));
        }
        let (locked, problems) = with_env(&data, &[("H2_LOD_DISTANCE", "1.5")]);
        assert!(problems.is_empty());
        assert_eq!(locked.get(LOD_DISTANCE.key), Some(Value::Int(3)));
        assert!(locked.state(LOD_DISTANCE.key).unwrap().locked);
        assert!(LOD_DISTANCE.parse("5").is_err());
        assert!(LOD_DISTANCE.parse("NaN").is_err());
        assert_eq!(RESOLUTION.scope, Scope::Host);
        assert_eq!(LOD_DISTANCE.scope, Scope::View);
    }

    /// Lighting is not a setting: the native renderer draws the game's
    /// own lighting, so no row, file key or variable of ours selects it.
    #[test]
    fn no_setting_selects_the_lighting() {
        for d in ALL {
            assert_ne!(d.key, "graphics.lighting");
            assert_ne!(d.env, Some("H2_LIGHTING"), "{}", d.key);
            assert_ne!(d.label, "Lighting", "{}", d.key);
        }
    }
}
