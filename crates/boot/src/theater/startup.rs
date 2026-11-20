//! The host-side decisions that select the guest's initial conditions. Paths
//! stay local: level names resolve against the supplied disc and restored
//! content, and a sim's bot tuning is already resolved into the descriptor.
use super::{Mode, Session};
use crate::{Host, RunOpts, level_boot, setting_defs, sim_glue};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BOOT: film::Tag = *b"BOOT";
const MOVI: film::Tag = *b"MOVI";

#[derive(Serialize, Deserialize)]
pub(crate) struct Startup {
    dedicated: bool,
    theater_menu: bool,
    keyboard: bool,
    render_thread: bool,
    // Source name and whether the original Auto policy saw a physical pad.
    icons: Option<(String, bool)>,
    execution: Execution,
    #[serde(skip)]
    movies: Values,
    level: Option<StartLevel>,
    sim: Option<sim::config::MatchConfig>,
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
    boot_bots: Option<usize>,
    bot_skill: String,
    old_glue: bool,
    trampoline: bool,
    pipeline_render: bool,
}

impl Execution {
    fn capture() -> Self {
        let bots = std::env::var(bots::ENV).ok().filter(|s| !s.is_empty());
        let skill = bots
            .as_ref()
            .and_then(|_| std::env::var(bots::SKILL_ENV).ok())
            .and_then(|s| bots::mind::Skill::parse(&s))
            .unwrap_or_default();
        Self {
            coop: crate::coop_live::film::Options::capture(),
            boot_bots: bots.map(|s| bots::from_env_value(Some(&s)).unwrap_or(0)),
            bot_skill: format!("{skill:?}").to_ascii_lowercase(),
            old_glue: std::env::var_os("H2_BOOT_OLD_GLUE").is_some(),
            trampoline: std::env::var("H2_BOOT_TRAMPOLINE").as_deref() == Ok("1"),
            pipeline_render: std::env::var(feature::pipeline_render::ENV).as_deref() == Ok("1"),
        }
    }
}

/// Set the fixed, validated execution switches before starting a new replay
/// process. No film-supplied environment keys, paths or command lines.
pub fn configure_replay_child<R: Read + Seek>(
    film: &mut film::Reader<R>,
    child: &mut std::process::Command,
) -> Result<()> {
    let switches = feature::Switches::parse(&film.header.features).map_err(anyhow::Error::msg)?;
    child.env("H2_FEATURES", switches.spec());
    let Some(start) = Startup::read(film)? else {
        return Ok(());
    };
    let e = start.execution;
    child.env_remove(bots::ENV).env_remove(bots::SKILL_ENV);
    if let Some(count) = e.boot_bots {
        ensure!(
            count <= bots::local::MAX_LOCAL && bots::mind::Skill::parse(&e.bot_skill).is_some(),
            "invalid startup bots in BOOT"
        );
        child
            .env(bots::ENV, count.to_string())
            .env(bots::SKILL_ENV, e.bot_skill);
    }
    for (name, enabled) in [
        ("H2_BOOT_OLD_GLUE", e.old_glue),
        ("H2_BOOT_TRAMPOLINE", e.trampoline),
        (feature::pipeline_render::ENV, e.pipeline_render),
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

    // Only keys and value types from this build's schema may reach the private
    // TOML file. Preserve editable rows and per-profile skull choices.
    fn install(&self, path: &Path, mount_settings: bool) -> Result<settings::Store> {
        let defs = setting_defs::defs();
        let table_of = |values: &Values| -> Result<toml::Table> {
            let mut table = toml::Table::new();
            for (key, value) in values {
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
    pub(crate) fn read<R: Read + Seek>(reader: &mut film::Reader<R>) -> Result<Option<Self>> {
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
    pub(crate) fn restore(
        self,
        opts: &mut RunOpts,
        host: &mut Host,
    ) -> Result<Option<sim_glue::Sim>> {
        ensure!(
            self.execution == Execution::capture(),
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
        host.input = self.xapi
            .then(|| input::InputManager::new(Default::default()))
            .transpose()?;
        #[cfg(feature = "desktop-runtime")]
        {
            host.keyboard = self.keyboard
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
                    sim::config::resolve(config, &|_| Err("film bot tuning must be inline".into()))
                        .map_err(anyhow::Error::msg)?;
                Ok(sim_glue::Sim::new(PathBuf::from("film"), matched, None))
            })
            .transpose()
    }
}

impl Session {
    pub(crate) fn record_movies(&self, opts: &RunOpts, mode: bink_hle::Mode) {
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

    pub(crate) fn record_startup(
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
                    execution: Execution::capture(),
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
