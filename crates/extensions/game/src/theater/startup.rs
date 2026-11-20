//! The host-side decisions that select the guest's initial conditions. Paths
//! stay local: level names resolve against the supplied disc and restored
//! content, and a sim's bot tuning is already resolved into the descriptor.
use super::{Mode, Session};
use crate::{Host, RunOpts, level_boot, setting_defs, sim_glue};
use anyhow::{Context, Result, ensure};
use render_bridge::pipeline_render::{self, HandbackOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BOOT: film::Tag = *b"BOOT";
const MOVI: film::Tag = *b"MOVI";

#[derive(Serialize, Deserialize)]
pub struct Startup {
    dedicated: bool,
    theater_menu: bool,
    keyboard: bool,
    render_thread: bool,
    /// Whether guest D3D read real GPU surfaces. A GPU-capable executable
    /// can also record a local game with the CPU headless backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gpu_draws: Option<bool>,
    // Source name and whether the original Auto policy saw a physical pad.
    icons: Option<(String, bool)>,
    execution: Execution,
    #[serde(skip)]
    movies: Values,
    level: Option<StartLevel>,
    sim: Option<sim_bench::config::MatchConfig>,
    xapi: bool,
    stopgaps: bool,
    d3d: bool,
    dsound: bool,
    bink: bool,
    /// The settings used for the package library and campaign mounts.
    data_settings: Settings,
    /// A desktop host also installs the settings UI and its gameplay hooks.
    host_settings: Option<Settings>,
}

#[derive(Serialize, Deserialize)]
struct StartLevel {
    name: String,
    difficulty: String,
    scenario: String,
    campaign: u32,
    map: u32,
}

/// Switches set by the host before boot which affect guest execution even
/// after H2_FEATURES has been matched. All children use the same named fields.
#[derive(Serialize, Deserialize, PartialEq)]
struct Execution {
    coop: crate::coop_live::film::Options,
    old_glue: bool,
    trampoline: bool,
    /// Old films: the retired H2_PIPELINE_RENDER alias. New films carry the
    /// pipelined frame in H2_FEATURES and always write false here.
    #[serde(default)]
    pipeline_render: bool,
    #[serde(default, with = "HandbackWire")]
    pipeline_handback: HandbackOptions,
}

/// A bounded list of raw guest addresses. Only the four declared options
/// are translated into child environment variables.
#[derive(Serialize, Deserialize)]
#[serde(remote = "HandbackOptions", deny_unknown_fields)]
struct HandbackWire {
    predict_frames: bool,
    replay_seeds: bool,
    defer_io: bool,
    #[serde(deserialize_with = "refusal_addresses")]
    refuse: Vec<u32>,
}

const MAX_PIPELINE_REFUSALS: usize = 1024;

fn refusal_addresses<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Vec<u32>, D::Error> {
    struct Addresses;

    impl<'de> serde::de::Visitor<'de> for Addresses {
        type Value = Vec<u32>;

        fn expecting(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                fmt,
                "up to {MAX_PIPELINE_REFUSALS} guest function addresses"
            )
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u32>, A::Error> {
            let mut addresses = Vec::new();
            while let Some(address) = seq.next_element()? {
                if addresses.len() == MAX_PIPELINE_REFUSALS {
                    return Err(serde::de::Error::custom(
                        "too many film pipeline refusal addresses",
                    ));
                }
                addresses.push(address);
            }
            Ok(addresses)
        }
    }

    reader.deserialize_seq(Addresses)
}

fn validate_handback(options: &HandbackOptions) -> Result<()> {
    ensure!(
        options.refuse.len() <= MAX_PIPELINE_REFUSALS,
        "too many film pipeline refusal addresses"
    );
    Ok(())
}

fn configure_handback(options: &HandbackOptions, child: &mut std::process::Command) -> Result<()> {
    validate_handback(options)?;
    for (name, enabled) in [
        (pipeline_render::PREDICT_FRAMES_ENV, options.predict_frames),
        (pipeline_render::SEED_REPLAY_ENV, options.replay_seeds),
        (pipeline_render::DEFER_IO_ENV, options.defer_io),
    ] {
        child.env(name, if enabled { "1" } else { "0" });
    }
    if options.refuse.is_empty() {
        child.env_remove(pipeline_render::REFUSE_ENV);
    } else {
        let addresses = options
            .refuse
            .iter()
            .map(|a| format!("{a:x}"))
            .collect::<Vec<_>>();
        child.env(pipeline_render::REFUSE_ENV, addresses.join(","));
    }
    Ok(())
}

impl Execution {
    fn capture() -> Result<Self> {
        let pipeline_handback = HandbackOptions::from_env();
        validate_handback(&pipeline_handback)?;
        Ok(Self {
            pipeline_handback,
            coop: crate::coop_live::film::Options::capture(),
            old_glue: std::env::var_os("H2_BOOT_OLD_GLUE").is_some(),
            trampoline: std::env::var("H2_BOOT_TRAMPOLINE").as_deref() == Ok("1"),
            pipeline_render: false,
        })
    }
}

/// Set the fixed, validated execution switches before starting a new replay
/// process. No film-supplied environment keys, paths or command lines.
pub fn configure_replay_child<R: Read + Seek>(
    film: &mut film::Reader<R>,
    child: &mut std::process::Command,
) -> Result<()> {
    let switches = hooks::Switches::parse(&film.header.features).map_err(anyhow::Error::msg)?;
    child.env("H2_FEATURES", switches.spec());
    let Some(start) = Startup::read(film)? else {
        return configure_handback(&HandbackOptions::default(), child);
    };
    let e = start.execution;
    if e.pipeline_render {
        child.env("H2_FEATURES", switches.with(pipeline_render::SWITCH, true).spec());
    }
    configure_handback(&e.pipeline_handback, child)?;
    for (name, enabled) in [
        ("H2_BOOT_OLD_GLUE", e.old_glue),
        ("H2_BOOT_TRAMPOLINE", e.trampoline),
    ] {
        if enabled {
            child.env(name, "1");
        } else {
            child.env_remove(name);
        }
    }
    for def in setting_defs::defs() {
        if def.scope != settings::Scope::Host
            && let Some(name) = def.env
        {
            child.env_remove(name);
        }
    }
    e.coop.configure(child)?;
    Ok(())
}

type Values = BTreeMap<String, String>;

#[derive(Serialize, Deserialize)]
struct Settings {
    values: Values,
    locked: Values,
    profiles: BTreeMap<String, Values>,
}

impl Settings {
    fn capture(store: &settings::Store) -> Self {
        let mut values = Values::new();
        let mut locked = Values::new();
        for def in store
            .defs()
            .into_iter()
            .filter(|d| d.scope != settings::Scope::Host)
        {
            let state = store.state(def.key).expect("registered setting");
            values.insert(def.key.into(), def.name(state.saved));
            if state.locked {
                locked.insert(def.key.into(), def.name(state.value));
            }
        }
        let profiles = store
            .profiles()
            .into_iter()
            .map(|profile| {
                let values = store
                    .profile_values(&profile)
                    .into_iter()
                    .filter_map(|(key, v)| {
                        let def = store.def(key)?;
                        (def.scope != settings::Scope::Host).then(|| (key.into(), def.name(v)))
                    })
                    .collect();
                (profile, values)
            })
            .collect();
        Self {
            values,
            locked,
            profiles,
        }
    }

    /// Earlier films used the same integral render scale for raster size
    /// and visual geometry selection. Recover its startup geometry only.
    fn legacy_lod_multiplier(&self) -> Option<f32> {
        use crate::setting_defs::{GUEST_FRAME, LEGACY_SCALE, LOD_DISTANCE, lod_pixel_multiplier};

        let effective = |key: &str| self.locked.get(key).or_else(|| self.values.get(key));
        let size = LEGACY_SCALE.parse(effective(LEGACY_SCALE.key)?).ok()?;
        let units = LOD_DISTANCE.parse(effective(LOD_DISTANCE.key)?).ok()?.as_int()?;
        Some(lod_pixel_multiplier(size.size(GUEST_FRAME).1, units as u32))
    }

    // Only keys and value types from this build's schema may reach the private
    // TOML file. Preserve editable rows and per-profile skull choices.
    fn install(&self, path: &Path, mount_settings: bool) -> Result<settings::Store> {
        let defs = setting_defs::defs();
        let table_of = |values: &Values| -> Result<toml::Table> {
            let mut table = toml::Table::new();
            for (key, value) in values {
                // Older films captured raster size for its effect on guest
                // LOD. The viewer owns its display even for these films.
                if key == setting_defs::LEGACY_SCALE.key {
                    setting_defs::LEGACY_SCALE.parse(value).map_err(anyhow::Error::msg)?;
                    continue;
                }
                let def = defs
                    .iter()
                    .find(|d| d.key == key.as_str() && d.scope != settings::Scope::Host)
                    .with_context(|| format!("unknown film setting {key}"))?;
                let parsed = def.parse(value).map_err(anyhow::Error::msg)?;
                let (section, field) = def.table_key();
                table
                    .entry(section)
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                    .as_table_mut()
                    .context("invalid setting section")?
                    .insert(field.into(), def.to_toml(parsed));
            }
            Ok(table)
        };
        let mut table = table_of(&self.values)?;
        let mut profiles = toml::Table::new();
        for (name, values) in &self.profiles {
            profiles.insert(name.clone(), toml::Value::Table(table_of(values)?));
        }
        table.insert(settings::PROFILES.into(), toml::Value::Table(profiles));
        // The package and campaign code reloads its own store from settings.toml.
        // Write effective overrides there; the separate desktop store retains
        // their lock and its saved values.
        let overrides = table_of(&self.locked)?;
        if mount_settings {
            for (section, group) in overrides {
                let target = table
                    .entry(section)
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                target
                    .as_table_mut()
                    .context("invalid setting section")?
                    .extend(group.as_table().context("invalid setting group")?.clone());
            }
        }
        std::fs::File::create_new(path)
            .with_context(|| format!("restoring film settings to {}", path.display()))?
            .write_all(toml::to_string(&table)?.as_bytes())?;
        let flags = self
            .locked
            .iter()
            .filter(|(key, _)| key.as_str() != setting_defs::LEGACY_SCALE.key)
            .map(|(key, value)| settings::Flag {
                key: key.clone(),
                value: value.clone(),
                flag: "film",
            })
            .collect::<Vec<_>>();
        let (store, errors) = settings::Store::load(&defs, Some(path), &|_| None, &flags);
        ensure!(errors.is_empty(), "invalid film settings: {errors:?}");
        Ok(store)
    }
}

impl Startup {
    /// Select the guest backend for a local film played without a window.
    /// Older desktop recordings retain the play window's settings store or
    /// render-thread flag, but no separate flag for offscreen GPU capture.
    pub fn local_gpu_draws(&self, hooks: Option<film::RenderHooks>) -> bool {
        self.d3d && self.gpu_draws.unwrap_or_else(|| {
            !matches!(hooks, Some(film::RenderHooks::Slim | film::RenderHooks::Desktop))
                && (self.render_thread || self.host_settings.is_some())
        })
    }

    /// Guest geometry from the startup of a film that recorded both the
    /// legacy render scale and LOD Distance. Viewer raster size is separate.
    pub fn legacy_lod_multiplier(&self) -> Option<f32> {
        self.host_settings.as_ref().unwrap_or(&self.data_settings).legacy_lod_multiplier()
    }

    pub fn read<R: Read + Seek>(reader: &mut film::Reader<R>) -> Result<Option<Self>> {
        if reader.header.version == 1 {
            return Ok(None);
        }
        let entry = reader.machine_section().unique(BOOT)?.clone();
        let mut start: Self = reader.read(&entry)?;
        if start.bink {
            let entry = reader.machine_section().unique(MOVI)?.clone();
            start.movies = reader.read(&entry)?;
        }
        Ok(Some(start))
    }

    /// Apply only to the fresh, private data directory prepared by Theater.
    pub fn restore(
        self,
        opts: &mut RunOpts,
        host: &mut Host,
    ) -> Result<Option<sim_glue::Sim>> {
        ensure!(
            self.execution == Execution::capture()?,
            "film startup switches differ"
        );
        host.host_light = self.dedicated;
        #[cfg(feature = "gpu")]
        {
            host.render_thread = self.render_thread && host.gfx.is_some();
        }
        #[cfg(feature = "desktop-runtime")]
        {
            host.icons = Some(match self.icons.as_ref() {
                Some((name, gamepads)) => crate::icons::IconOptions {
                    choice: name.parse().map_err(anyhow::Error::msg)?,
                    gamepads: *gamepads,
                },
                None => crate::icons::IconOptions {
                    choice: font::icons::IconChoice::Set(font::icons::IconSet::Xbox),
                    gamepads: false,
                },
            });
        }
        // Reproduce the text trampoline's presence before any guest code runs.
        // PADR/PDEV supply a separate input source for every XAPI port.
        ensure!(!self.keyboard || self.xapi, "film keyboard requires XAPI");
        host.input = self
            .xapi
            .then(|| input::InputManager::new(Default::default()))
            .transpose()?;
        #[cfg(feature = "desktop-runtime")]
        {
            host.keyboard = self
                .keyboard
                .then(|| input::backend::keyboard::KeyboardMouse::new(Default::default()));
        }
        #[cfg(not(feature = "desktop-runtime"))]
        ensure!(!self.keyboard, "film requires the desktop virtual keyboard");
        opts.xapi = self.xapi;
        opts.stopgaps = self.stopgaps;
        opts.d3d = self.d3d;
        opts.dsound = self.dsound;
        opts.bink = self.bink;
        opts.movies = if self.movies.is_empty() {
            crate::movies_glue::Movies::Off
        } else {
            let dir = opts
                .movies
                .dir()
                .or_else(crate::movies_glue::default_dir)
                .context("film requires the original transcoded movies (--movies DIR)")?;
            ensure!(
                movie_assets(&dir, &opts.disc)? == self.movies,
                "transcoded movies differ from film"
            );
            crate::movies_glue::Movies::Dir(dir)
        };
        if self.theater_menu {
            // Keep the recorded guest menu and its hook allocation order. The
            // independent playback overlay owns the viewer's actual library.
            host.theater = Some((crate::theater_ui::Theater::default(), |_| {
                Err("the guest film library is offline during playback".into())
            }));
        }
        self.data_settings
            .install(&settings::path_in(&opts.data_root), true)?;
        host.settings = self
            .host_settings
            .map(|settings| {
                settings
                    .install(&opts.data_root.join("film-host-settings.toml"), false)
                    .map(Arc::new)
            })
            .transpose()?;
        ensure!(
            !(self.dedicated && self.sim.is_some()),
            "sim cannot be a dedicated lease"
        );
        ensure!(
            self.sim.is_none() || self.level.is_none(),
            "two film start levels"
        );
        if let Some(level) = self.level {
            let req = level_boot::resolve_in(
                &opts.disc,
                Some(&opts.data_root),
                &level.name,
                level_boot::Difficulty::parse(&level.difficulty)?,
            )?;
            ensure!(
                req.scenario == level.scenario
                    && req.campaign == level.campaign
                    && req.map == level.map,
                "installed level differs from BOOT"
            );
            host.level = Some(req);
        }
        self.sim
            .map(|config| {
                let matched =
                    sim_bench::config::resolve(config, &|_| Err("film bot tuning must be inline".into()))
                        .map_err(anyhow::Error::msg)?;
                Ok(sim_glue::Sim::new(PathBuf::from("film"), matched, None))
            })
            .transpose()
    }
}

impl Session {
    pub fn record_movies(&self, opts: &RunOpts, mode: bink_hle::Mode) {
        let mut state = self.lock();
        let Mode::Record(rec) = &mut state.mode else {
            return;
        };
        let Some(out) = &mut rec.out else { return };
        let movies = match mode {
            bink_hle::Mode::Skip => Ok(Values::new()),
            bink_hle::Mode::Playback => opts
                .movies
                .dir()
                .context("missing movie directory")
                .and_then(|dir| movie_assets(&dir, &opts.disc)),
        };
        if let Err(e) = movies.and_then(|m| out.chunk(MOVI, &m)) {
            state.fail(format!("writing MOVI: {e:#}"));
        }
    }

    pub fn record_startup(
        &self,
        opts: &RunOpts,
        host: &Host,
        level: Option<&level_boot::Request>,
        sim: Option<&sim_glue::Sim>,
    ) {
        let mut state = self.lock();
        let Mode::Record(rec) = &mut state.mode else {
            return;
        };
        let Some(out) = &mut rec.out else { return };
        let mut write = || -> Result<()> {
            let sim = sim
                .map(|s| serde_json::from_value(s.matched().resolved_json()))
                .transpose()?;
            let level = level.filter(|_| sim.is_none()).map(|l| StartLevel {
                name: l.name.clone(),
                difficulty: l.difficulty.to_string(),
                scenario: l.scenario.clone(),
                campaign: l.campaign,
                map: l.map,
            });
            #[cfg(feature = "desktop-runtime")]
            let icons = host
                .icons
                .or(
                    crate::icons::choice_from_env()?.map(|choice| crate::icons::IconOptions {
                        choice,
                        gamepads: false,
                    }),
                )
                .map(|o| {
                    let name = match o.choice {
                        font::icons::IconChoice::Auto => "auto".into(),
                        font::icons::IconChoice::Set(set) => {
                            format!("{set:?}").to_ascii_lowercase()
                        }
                    };
                    (name, o.gamepads)
                });
            #[cfg(not(feature = "desktop-runtime"))]
            let icons = None;
            #[cfg(feature = "desktop-runtime")]
            let keyboard = opts.xapi && host.keyboard.is_some();
            #[cfg(not(feature = "desktop-runtime"))]
            let keyboard = false;
            #[cfg(feature = "gpu")]
            let render_thread = host.render_thread;
            #[cfg(not(feature = "gpu"))]
            let render_thread = false;
            out.chunk(
                BOOT,
                &Startup {
                    level,
                    sim,
                    dedicated: host.host_light,
                    theater_menu: host.theater.is_some(),
                    keyboard,
                    icons,
                    render_thread,
                    gpu_draws: Some({
                        #[cfg(feature = "gpu")]
                        { opts.d3d && (host.gfx.is_some() || std::env::var_os("H2_BOOT_GFX").is_some()) }
                        #[cfg(not(feature = "gpu"))]
                        { false }
                    }),
                    execution: Execution::capture()?,
                    movies: Values::new(),
                    xapi: opts.xapi,
                    stopgaps: opts.stopgaps,
                    d3d: opts.d3d,
                    dsound: opts.dsound,
                    bink: opts.bink,
                    data_settings: Settings::capture(&setting_defs::load(&opts.data_root, &[]).0),
                    host_settings: host.settings.as_deref().map(Settings::capture),
                },
            )
        };
        if let Err(e) = write() {
            state.fail(format!("writing BOOT: {e:#}"));
        }
    }
}

#[cfg(feature = "movie-playback")]
fn movie_assets(dir: &Path, disc: &Path) -> Result<Values> {
    let bink_hle::Selection::Playback { names, .. } =
        crate::movies_glue::choose(Some(dir), disc, None)
    else {
        anyhow::bail!("recorded transcoded movies unavailable (--movies DIR)");
    };
    names
        .into_iter()
        .map(|name| {
            let file = movies::locate(dir, &name).context("installed movie changed")?;
            Ok((
                name,
                film::manifest::file_digest(&mut std::fs::File::open(file)?)?,
            ))
        })
        .collect()
}

#[cfg(not(feature = "movie-playback"))]
fn movie_assets(_: &Path, _: &Path) -> Result<Values> {
    anyhow::bail!("film requires a build with movie playback")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_film_selects_its_recorded_gpu_readback_backend() {
        use film::RenderHooks::{Gpu, Slim};
        let data = serde_json::json!({ "values": {}, "locked": {}, "profiles": {} });
        let mut wire = serde_json::json!({
            "dedicated": false, "theater_menu": false, "keyboard": false, "render_thread": false,
            "execution": {
                "coop": crate::coop_live::film::Options::capture(), "old_glue": false, "trampoline": false,
            },
            "xapi": true, "stopgaps": true, "d3d": true, "dsound": true, "bink": false,
            "data_settings": data.clone(),
        });
        let gpu = |wire: &serde_json::Value, hooks| {
            serde_json::from_value::<Startup>(wire.clone()).unwrap().local_gpu_draws(hooks)
        };
        assert!(!gpu(&wire, Some(Gpu)), "legacy CPU run with GPU-capable hooks");
        wire["host_settings"] = data;
        assert!(gpu(&wire, Some(Gpu)), "legacy play-window recording");
        assert!(!gpu(&wire, Some(Slim)));
        wire["gpu_draws"] = false.into();
        assert!(!gpu(&wire, Some(Gpu)), "an explicit CPU backend takes priority");
        wire["host_settings"] = serde_json::Value::Null;
        wire["gpu_draws"] = true.into();
        assert!(gpu(&wire, Some(Gpu)), "offscreen GPU recording");
        wire["d3d"] = false.into();
        assert!(!gpu(&wire, Some(Gpu)));
    }

    #[test]
    fn pipeline_execution_options_restore_the_recorded_handback_and_legacy_defaults() {
        let mut wire = serde_json::json!({
            "coop": crate::coop_live::film::Options::capture(),
            "old_glue": false,
            "trampoline": false,
            "pipeline_render": true,
        });
        let legacy: Execution = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(legacy.pipeline_handback, HandbackOptions::default());
        wire["pipeline_handback"] = serde_json::json!({
            "predict_frames": false, "replay_seeds": true, "defer_io": false,
            "refuse": [0x00111222u32, 0x00333444u32]
        });
        let restored: Execution = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), wire);
        let mut cmd = std::process::Command::new("film-child");
        configure_handback(&restored.pipeline_handback, &mut cmd).unwrap();
        let vars: BTreeMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(
            vars,
            BTreeMap::from([
                (pipeline_render::PREDICT_FRAMES_ENV, "0"),
                (pipeline_render::SEED_REPLAY_ENV, "1"),
                (pipeline_render::DEFER_IO_ENV, "0"),
                (pipeline_render::REFUSE_ENV, "111222,333444"),
            ])
        );
        wire["pipeline_handback"]["refuse"] = serde_json::json!(vec![0; MAX_PIPELINE_REFUSALS + 1]);
        assert!(serde_json::from_value::<Execution>(wire).is_err());
    }

    #[test]
    fn legacy_film_lod_uses_the_recorded_scale_without_locking_the_viewer_raster() {
        let root = std::env::temp_dir().join(format!("film-lod-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&root).unwrap();
        let mut film = Settings {
            values: BTreeMap::from([("graphics.render_scale".into(), "2".into())]),
            locked: BTreeMap::from([("graphics.render_scale".into(), "3".into())]),
            profiles: BTreeMap::new(),
        };
        assert_eq!(film.legacy_lod_multiplier(), None, "predates LOD Distance");
        film.values.insert("graphics.lod_distance".into(), "1.5".into());
        assert_eq!(film.legacy_lod_multiplier(), Some(4.5));
        let file = root.join("game.toml");
        let restored = film.install(&file, true).unwrap();
        let resolution = restored.state(setting_defs::RESOLUTION.key).unwrap();
        assert!(!resolution.locked);
        assert_eq!(resolution.value, settings::Resolution::Native.value());
        let saved = std::fs::read_to_string(file).unwrap();
        assert!(!saved.contains("render_scale"));
        assert!(!saved.contains("resolution"));
        assert!(saved.contains("lod_distance"));
        film.locked.insert("graphics.render_scale".into(), "bad".into());
        assert!(film.install(&root.join("bad.toml"), true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replay_rebuilds_locked_overrides_and_editable_profile_settings() {
        let root =
            std::env::temp_dir().join(format!("film-settings-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&root).unwrap();
        let defs = setting_defs::defs();
        let (store, errors) = settings::Store::load(
            &defs,
            Some(&root.join("original.toml")),
            &|_| None,
            &[settings::Flag::parse_set("graphics.fov=95").unwrap()],
        );
        assert!(errors.is_empty());
        let key = setting_defs::HALO1_ASSETS.key;
        store
            .profile_set("Player 1", key, settings::Value::Int(2))
            .unwrap();
        let saved = Settings::capture(&store);
        let replay = saved.install(&root.join("player.toml"), false).unwrap();
        assert!(replay.state("graphics.fov").unwrap().locked);
        assert_eq!(replay.get("graphics.fov"), Some(settings::Value::Int(95)));
        assert_eq!(
            replay.profile_get("Player 1", key),
            Some(settings::Value::Int(2))
        );
        assert!(!replay.state(key).unwrap().locked);
        assert!(saved.install(&root.join("original.toml"), false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
