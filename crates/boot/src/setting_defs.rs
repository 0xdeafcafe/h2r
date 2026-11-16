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
        valid: 1..=d3d::scaled::MAX_SCALE as i64,
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

/// Every setting, in the order `h2 settings list` shows them.
pub static ALL: &[&Def] = &[
    &FOV,
    &FPS,
    &RENDER_SCALE,
    &PAD_SPEAKER,
    &PAD_MUTE,
    &STEAM_SPEAKER,
    &STEAM_SENSITIVITY,
    &STEAM_LEFT_PAD,
    &LIVE_VISIBILITY,
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
];

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
        ALL,
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
            ],
        );
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
}
