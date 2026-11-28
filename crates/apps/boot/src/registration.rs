//! Boot's feature registration, in one place a test can call.
//!
//! [`run_hosted_with_live`](crate::run_hosted_with_live) builds the feature
//! plan from a registry that many installs add to. Two features that hook
//! the same point with no order between them make the plan refuse, and the
//! game stops at boot (docs/engine/feature-engine.md, "Hook ordering").
//! Every `features.add` and `install` call lives in [`register_core`] and
//! [`register_late`], in the order boot always made them; boot calls both,
//! with its host-only work (the window, audio, input) between them, and the
//! tests below call both with test inputs and build the plan. A feature added
//! to either function is in the plan the tests build, so an unordered clash
//! fails `cargo test` instead of the game.
//!
//! Host-only pieces (the Live HLE, the input and the agent's socket) come in through the inputs and are `None` in a
//! test unless a test supplies a stand-in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use runtime::Runtime;

use crate::run_plan::{compiled_render_hooks, register_telemetry};

use crate::{
    agent_glue, aim_assist, bot_glue, campaign_menu, cheats, clankers, coop_probe, custom_games,
    game_setup_help, h1_options, h1_options_row, health, hud, hud_swap, layout_glue, level_boot, lockstep_glue,
    map_copy, map_select, map_slots, precision, signin_screen, skulls, skulls_custom, skulls_menu,
    skulls_mp, snapshot_probe, telemetry, touch_glue, ui, ui_back, ui_watch, variant_ext, variant_page, variant_weapons, xapi_glue,
};

/// The light host's switch defaults.
pub(crate) fn profile_switches(mut switches: hooks::Switches, light: bool) -> hooks::Switches {
    if light {
        // The lobby's background map copy, several steps a frame
        // (docs/runtime/cache-slots.md, "Copy speed on the loading screen");
        // `-map-copy.lobby` is its A/B arm.
        let lobby = switches.feature(map_copy::LOBBY, true);
        switches = switches.with(map_copy::LOBBY, lobby);
    }
    switches
}

/// The light profile requires a headless backend and the clock-only DirectSound shim.
pub(crate) fn register_viewer_skips(
    features: &mut hooks::Registry,
    host_light: bool,
) -> anyhow::Result<()> {
    if host_light {
        // The map status poll memo (docs/runtime/cache-slots.md, "The status poll").
        features
            .add(&sim_bridge::sim_status_poll::StatusPoll)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// What [`register_core`] needs: the registrations up to the touch glue.
pub struct CoreInputs<'a> {
    pub rt: &'a Arc<Runtime>,
    /// The telemetry configuration, or why there is none.
    pub telemetry: Result<telemetry::TelemetryConfig, String>,
    /// The mounted VFS, when there is one (map slots, our skulls' map reads).
    pub vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    /// The Video screen's model (a settings store's, else none).
    pub model: Option<Arc<ui::Model>>,
    pub theater: Option<crate::theater_ui::Theater>,
    pub campaigns: Option<Arc<crate::campaigns::State>>,
    pub settings: Option<Arc<settings::Store>>,
    pub live: Option<Arc<live_hle::Hle>>,
    pub data_root: &'a Path,
    pub packages: &'a Path,
    pub level: Option<level_boot::Request>,
    /// `H2_PERF_FROM=level`: wait for the level-loaded mark.
    pub perf_level: bool,
    /// Install bots and the Clankers page (requires XAPI pads).
    pub bots: bool,
}

/// What [`register_core`] decided, for the install lines after the plan.
pub struct Core {
    pub editor_sources: Option<Arc<crate::loaded_map::EditorSources>>,
    pub telemetry: Option<Arc<telemetry::Telemetry>>,
    pub ui_navigation: Arc<ui_back::Navigation>,
    pub skulls: Option<Arc<skulls::Tracker>>,
    pub skulls_mp_on: bool,
    pub skulls_custom_on: bool,
    pub health_on: bool,
    /// The bots are installed (their nav meshes may be built at boot).
    pub bots_on: bool,
    /// The slot-17 probe was selected for this plan.
    pub creature_probe: bool,
}

/// A boot without the player's own features (`register_core_film`'s
/// `non_player`: the Custom Game Browser's filters, a link joined with no
/// film): a light host, a sim, or a film that turns them off. With a film
/// the header decides, never whether this boot records or replays it, so a
/// recording and its replay plan the same hooks
/// (docs/game/film-format.md, "Recording every game").
pub(crate) fn non_player(
    film: Option<&crate::theater::Session>,
    replaying: bool,
    host_light: bool,
    sim: bool,
) -> bool {
    host_light
        || sim
        || match film {
            Some(film) => !film.player_features(),
            None => replaying,
        }
}

/// Local Forge's hooks and controls (`forge_glue::install`): on unless
/// `-forge`, a light host or lockstep. With a film the header decides as
/// in [`non_player`]: a default recording and its replay both plan Forge,
/// any other film neither.
pub(crate) fn local_forge(
    switches: &hooks::Switches,
    film: Option<&crate::theater::Session>,
    replaying: bool,
    host_light: bool,
    lockstep: bool,
) -> bool {
    switches.feature("forge", true)
        && !host_light
        && !lockstep
        && match film {
            Some(film) => film.player_features(),
            None => !replaying,
        }
}

/// What decides the lab takeover ([`lab_takeover`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TakeoverInputs {
    /// The boot has a window (or a Theater observer window).
    pub window: bool,
    /// The guest's own D3D draws.
    pub d3d: bool,
    /// XAPI pads are in.
    pub xapi: bool,
    pub lockstep: bool,
    pub host_light: bool,
    pub sim: bool,
    /// Theater's playback child.
    pub replaying: bool,
}

/// The lab takeover (`lab_takeover`, docs/runtime/lab.md, "Player consent
/// before a job"): whether a boot plans its dialog's hooks, and whether it
/// takes a lab's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Takeover {
    pub plan: bool,
    pub accept: bool,
}

/// A game drawn by its own D3D with XAPI pads, outside lockstep, the light
/// host and the sim, plans the dialog with or without a window and whether
/// it records or plays a film. A film's replay installs its recording's
/// simulation hooks (`theater::compatibility`'s hook digest): the
/// recording is a windowed game, and its replay is Theater's child, with a
/// window or none. Only a windowed game that is not a replay takes a
/// request, so the dialog never opens in a replay; its hooks act on a
/// request only.
pub(crate) fn lab_takeover(i: TakeoverInputs) -> Takeover {
    let plan = i.d3d && i.xapi && !i.lockstep && !i.host_light && !i.sim;
    Takeover {
        plan,
        accept: plan && i.window && !i.replaying,
    }
}

/// Registers everything from the telemetry hooks to the touch glue.
pub fn register_core(features: &mut hooks::Registry, i: CoreInputs<'_>) -> anyhow::Result<Core> {
    register_core_film(features, i, None, false, None)
}

pub(crate) fn register_core_film(
    features: &mut hooks::Registry,
    i: CoreInputs<'_>,
    film: Option<Arc<crate::theater::Session>>,
    non_player: bool,
    editor: Option<vfs::MapRebind>,
) -> anyhow::Result<Core> {
    let sources = editor.zip(i.vfs.as_ref()).map(|(handle, vfs)| {
        Arc::new(crate::loaded_map::EditorSources::new(vfs, handle))
    });
    let rt: &Runtime = i.rt;
    // A film turns the player features below off while it records and
    // when it plays back, so both run the same plan. A player session's
    // default recording, and its replay, keep them on
    // (theater::autorecord, docs/game/film-format.md "Recording every game").
    let features_off = film.as_ref().is_some_and(|f| !f.player_features());
    // The grown tag arena (docs/formats/map-format.md, "The tag arena"):
    // every boot, since maps built for it may need its room.
    features
        .add(&crate::tag_arena::TagArena)
        .map_err(|e| anyhow::anyhow!("tag-arena: {e}"))?;
    let creature_probe = features
        .add(&crate::actor_net::creature::CreatureProbe)
        .map_err(|e| anyhow::anyhow!("creature-probe: {e}"))?;
    let replication_version = live_proto::replication::local(creature_probe);
    if film.as_ref().is_none_or(|film| film.uses_replication_admission()) {
        let admission = features
            .add(&crate::replication_admission::ReplicationAdmission::new(replication_version))
            .map_err(|e| anyhow::anyhow!("replication admission: {e}"))?;
        anyhow::ensure!(admission, "replication admission must be enabled");
        if let Some(live) = &i.live {
            live.set_replication_version(replication_version);
        }
    } else {
        anyhow::ensure!(!creature_probe, "creature probe requires replication admission");
    }
    let config = i.telemetry.unwrap_or_else(|error| {
        eprintln!("live telemetry transport disabled: {error}");
        telemetry::TelemetryConfig::default()
    });
    let telemetry = register_telemetry(features, &config, film.as_ref())?;
    let link = Arc::new(crate::actor_net_link::HostLink::default());
    let reset = Arc::clone(&link);
    let live = i.live.clone();
    let inbox = i.live.clone();
    let capture = crate::actor_net::Capture::new(
        move |m, tick, host| {
            let result = link.post_tick(live.as_deref(), tick, |peer, budget, request_key| {
                // P3 leaves controls off. Tunnel peer IDs are connection IDs.
                match host.schedule(budget, None, request_key) {
                    Ok(packet) => packet,
                    Err(error) => {
                        eprintln!("actor-net: scheduling peer {peer} at tick {tick}: {error}");
                        None
                    }
                }
            });
            if let Err(error) = result {
                eprintln!("actor-net: sending host tick {tick}: {error}");
            }
            // The picks a client's players made, back up the channel into
            // the host's VM (docs/game/rules-world-api.md, "Pickers").
            #[cfg(feature = "rules")]
            for (bytes, seats) in crate::actor_net_link::take_picks(live.as_deref(), m) {
                crate::actor_net::pick::receive(&bytes, &seats);
            }
            // actors.net's view of each peer's budget.
            crate::actor_net::report::publish_host(tick, link.peers());
        },
        move || reset.reset(),
    )
    // A distributed client's half: drain the tunnel's actor inbox before
    // the tick and ask the host for a key when the replica needs one
    // (docs/game/ai-networking.md, "On a client, each tick").
    .with_client(move |client, tick| {
        crate::actor_net_link::pre_tick(inbox.as_deref(), |packet| {
            client.receive(&packet, tick);
        });
        if client.take_key_request() && !crate::actor_net_link::request_key(inbox.as_deref()) {
            eprintln!("actor-net: tick {tick}: no actor tunnel to ask the host for a key");
        }
        // This machine's players' picks, back up the channel
        // (docs/game/rules-world-api.md, "Pickers"), and whether the
        // tunnel carries them at all.
        #[cfg(feature = "rules")]
        crate::rules::world::pick::client_ready(crate::actor_net_link::pick_upstream(
            inbox.as_deref(),
        ));
        #[cfg(feature = "rules")]
        for bytes in crate::actor_net::pick::take_outbox() {
            if !crate::actor_net_link::send_pick(inbox.as_deref(), &bytes) {
                eprintln!("actor-net: tick {tick}: no actor tunnel carries a pick upstream");
            }
        }
    });
    features
        .add(&capture)
        .map_err(|e| anyhow::anyhow!("actor channel host capture: {e}"))?;
    if let Some(vfs) = i.vfs.clone() {
        features
            .add(&map_slots::MapSlots { vfs })
            .map_err(|e| anyhow::anyhow!("map slots: {e}"))?;
    }
    // Maps copy to their cache slots several steps a loading frame, not one
    // (docs/runtime/cache-slots.md, "Copy speed on the loading screen").
    features
        .add(&map_copy::MapCopy)
        .map_err(|e| anyhow::anyhow!("map copy: {e}"))?;
    features
        .add(&crate::h1_recordings::Playback)
        .map_err(|e| anyhow::anyhow!("recorded animation playback: {e}"))?;
    layout_glue::install(rt, features, i.model, i.theater.clone()).context("installing the button layouts")?;
    features
        .add(&crate::list_class::ScrollArrowLayout)
        .map_err(|e| anyhow::anyhow!("list scroll arrows: {e}"))?;
    campaign_menu::install(rt, features, i.campaigns).context("installing the campaigns")?;
    let ui_navigation = Arc::new(ui_back::Navigation::default());
    features
        .add(&ui_back::RetailBack(Arc::clone(&ui_navigation)))
        .map_err(|e| anyhow::anyhow!("ui-back: {e}"))?;
    map_select::install(features, i.live.clone(), Arc::clone(&ui_navigation))
        .context("installing the map select")?;
    // GAME MENU's Game Setup for the party leader: the map, the rules and
    // Quick Options for a restart, Clanker Options at once
    // (docs/game/settings-menu.md "Game Setup from the pause menu").
    crate::pause_change_map::install(features, i.settings.clone().filter(|_| i.bots))
        .context("installing the pause menu's Game Setup")?;
    // The Halo 1 / Halo 2 options (docs/halo1/multiplayer.md "Halo 1
    // options"): the switches follow the settings, the HUD style is picked
    // at each game start, and a custom game's Game Setup gets the Halo 1
    // Options row while its map is a converted Halo 1 map.
    if let Some(store) = i.settings.as_ref() {
        h1_options::install_switches(store);
        // The page (the shared settings dialog, its texts a feature of
        // their own) and the Game Setup row that opens it.
        let page = Arc::new(h1_options::page(Arc::clone(store)));
        if let Err(e) = features.add(&crate::ui_kit::screen::KitTexts(Arc::clone(&page))) {
            eprintln!("h1 options texts: {e}");
        }
        if let Err(e) = features.add(&page.class()) {
            eprintln!("h1 options: {e}");
        }
        features
            .add(&h1_options_row::SetupRow {
                data_root: i.data_root.to_path_buf(),
                page: Arc::clone(&page),
            })
            .map_err(|e| anyhow::anyhow!("h1 options row: {e}"))?;
        // The value picker A opens on a setting row, and our Game Setup
        // rows' picture and description (docs/game/ui-builder.md).
        crate::ui_kit::picker::install(features).context("installing the value picker")?;
        // Quick Options for a script variant: the dialog its marks fill
        // and the Game Setup row that opens it (docs/game/rules-variants.md).
        crate::variant_quick::page::install(features).context("installing Quick Options for script variants")?;
        // The row's description on Game Setup: the page's caller help,
        // from the page's own slot.
        game_setup_help::register(
            h1_options_row::OPTIONS_ACTION,
            game_setup_help::image::QUICK_OPTIONS,
            Arc::new(move |m: &mut dyn guest_layout::memory::GuestMemory| {
                use crate::list_class::Rows;
                let id = page.row_help_id()?;
                page.write_texts(m).ok()?;
                Some(id)
            }),
        );
        game_setup_help::install(features).context("installing the Game Setup help")?;
    }
    let live_browser = i.live.as_ref().filter(|_| i.settings.is_some() && !non_player);
    custom_games::browser_filters::install(features, live_browser, i.data_root)
        .context("installing the custom games browser filters")?;
    // The game variant editor's More Options row and page (our options in
    // the record's extension block; docs/game/game-options.md section 3).
    {
        let page = Arc::new(variant_page::Page::new());
        let class = variant_page::class(page, skulls_menu::DESCRIPTION_TEXT, skulls_menu::LIST_LAYOUT);
        if let Err(e) = features.add(&class) {
            eprintln!("variant options: {e}");
        }
        features
            .add(&variant_page::MoreRow)
            .map_err(|e| anyhow::anyhow!("variant options row: {e}"))?;
    }
    // The game type list's rows of ours after Territories (Party Games,
    // Firefight and Siege), and their variant lists
    // (docs/game/community-gametypes.md "Tweakers").
    features
        .add(&crate::tweakers::Tweakers)
        .map_err(|e| anyhow::anyhow!("tweakers: {e}"))?;
    features.add(&hud::Hud).map_err(|e| anyhow::anyhow!("hud: {e}"))?;
    features
        .add(&crate::loaded_map::LoadedMap)
        .map_err(|e| anyhow::anyhow!("loaded map: {e}"))?;
    // The party privacy as the Live session's visibility (the Live menu's
    // rows are registered with the layouts above;
    // docs/live/live-custom-games.md).
    custom_games::install_privacy(rt, i.live.as_ref(), i.settings.as_ref())
        .context("installing the Live game visibility")?;
    // A Live sign-in that needs a code asks for it in game, on our own
    // screen opened from the logon's polling (docs/live/live.md, "In-game
    // sign-in").
    signin_screen::install(rt, features, i.live.as_ref()).context("installing the sign-in prompt")?;
    let join_off = if film.is_some() { features_off } else { non_player };
    let stop_film = crate::theater::autorecord::stopper(film.as_ref(), "a Live session link was joined", false);
    crate::join_link::install(features, i.live.clone(), join_off, stop_film)
        .context("installing Live join links")?;
    // A campaign level in place of the main menu (`h2 play --level`,
    // H2_LEVEL) and the level-loaded mark H2_PERF_FROM=level waits on
    // (docs/runtime/level-boot.md).
    let mut level = i.level;
    let preview_script = level
        .as_mut()
        .and_then(|level| level.variant.as_mut())
        .and_then(|variant| variant.script.take());
    level_boot::install(rt, features, level, i.perf_level).context("installing the level boot")?;
    // Campaign skulls kept per profile: a pickup is remembered and a
    // campaign start turns on the profile's chosen skulls (docs/game/skulls.md).
    // Headless runs (no settings store) leave skulls as retail. Hooks on
    // the feature plan; the line after the plan installs says where.
    let skulls = skulls::install(features, i.settings.clone()).context("installing the skulls")?;
    // Skulls in multiplayer games: the host's set, advertised to guests on
    // our Live service and applied on every machine at game start
    // (docs/game/skulls.md "In multiplayer").
    let skulls_mp_on = skulls_mp::install(features, i.settings.clone(), i.live.clone(), telemetry.clone())
        .context("installing the multiplayer skulls")?;
    // The game variant's options sent to Live guests at each multiplayer
    // start (docs/game/game-options.md section 3).
    features
        .add(&variant_ext::Transport { hle: i.live.clone() })
        .map_err(|e| anyhow::anyhow!("variant options transport: {e}"))?;
    // The game-variant store: its variants in the variant list, their
    // options and script on the game at its start, the saved variants
    // ported at boot (docs/game/rules-engine.md, "Live game variants in the
    // game").
    crate::game_variants::install(
        features,
        crate::game_variants::Boot {
            data_root: i.data_root.to_path_buf(),
            live: i.live.clone(),
        },
    )
    .context("installing the game-variant store")?;
    if let Some(blob) = preview_script {
        crate::game_variants::store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add_script(blob)
            .map_err(anyhow::Error::msg)?;
    }
    if let Some(hle) = i.live.clone() {
        features
            .add(&crate::live_hopper::MapPackRotation(hle))
            .map_err(|e| anyhow::anyhow!("matchmaking map pack rotation: {e}"))?;
    }
    // Its damage, shield and health scales on each damage instance.
    features
        .add(&variant_ext::Vitality)
        .map_err(|e| anyhow::anyhow!("variant options vitality: {e}"))?;
    // Its movement speed, on the engine's per-player speed step.
    features
        .add(&variant_ext::Speed)
        .map_err(|e| anyhow::anyhow!("variant options speed: {e}"))?;
    // Our own skulls (from 15): hidden in campaign levels, their effects
    // ours on the tick (docs/game/skulls.md "Our skulls"). The skull weapon's
    // tag comes from the loaded map's file, found through the VFS.
    let custom_maps: Option<skulls_custom::MapLocator> = i.vfs.clone().map(|vfs| -> skulls_custom::MapLocator {
        Box::new(move |name, path| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            crate::map_override::map_file_at(&mut v, name, path)
        })
    });
    // The variant's other weapons as the starting and secondary weapon:
    // their tags are read from the same file.
    let weapon_maps: Option<skulls_custom::MapLocator> = i.vfs.clone().map(|vfs| -> skulls_custom::MapLocator {
        Box::new(move |name, path| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            crate::map_override::map_file_at(&mut v, name, path)
        })
    });
    features
        .add(&variant_weapons::Weapons::new(weapon_maps))
        .map_err(|e| anyhow::anyhow!("variant options weapons: {e}"))?;
    let skulls_custom_on = skulls_custom::install_for_maps(
        features, i.settings.clone(), telemetry.clone(), custom_maps, sources.as_deref(),
    )
        .context("installing our skulls")?;
    // Firefight on a kit map (docs/game/firefight.md, "The host runtime"):
    // on by default, `-firefight` turns it off. A film replays the host's
    // entries; a default recording and its replay run the director.
    // Its Game Setup row and options page come with it, where there is a
    // settings store to keep the choices.
    let firefight_options = i.settings.as_ref().map(crate::firefight::options::source);
    // The host's scores and result go to Live and the film through
    // telemetry, and the result to a sim run's report (`sim_glue`).
    let firefight_telemetry = telemetry.clone();
    let firefight_reports = Some(Box::new(move |report: crate::firefight::runtime::Report| {
        crate::sim_glue::note_firefight(&report);
        let Some(t) = &firefight_telemetry else {
            return;
        };
        match report {
            crate::firefight::runtime::Report::Score(score) => t.firefight_score(score),
            crate::firefight::runtime::Report::Ended(ended) => t.firefight_ended(ended),
        }
    }) as Box<crate::firefight::runtime::ReportSink>);
    // A Live host names the match in its session data, so its guests and
    // Live's Clanker roster know it (docs/game/firefight.md, "Live").
    let firefight_session = i.live.clone().map(|hle| {
        Box::new(move |f: Option<live_proto::session::Firefight>| {
            hle.set_session_firefight(f)
        }) as Box<crate::firefight::runtime::SessionSink>
    });
    let firefight_on = crate::firefight::feature::install_with(
        features,
        features_off,
        firefight_options,
        firefight_reports,
        firefight_session,
    )
        .context("installing Firefight")?;
    if firefight_on && let Some(store) = i.settings.as_ref() {
        crate::firefight::options::install(features, store)
            .context("installing the Firefight options page")?;
    }
    // The party lobby: its GAME MODE picker and Multiplayer, Forge and
    // Firefight as modes of the lobby (docs/game/party-lobby.md). It has
    // no switch. It is in every run a settings store is opened for, as
    // Game Setup's other rows of ours are.
    if let Some(store) = i.settings.as_ref() {
        let offer = crate::lobby_start::Offer {
            forge: crate::forge_glue::enabled(features.switches()),
            firefight: firefight_on,
        };
        // The FORGE and FIREFIGHT modes, with their features
        // (docs/game/party-lobby.md, "Where each mode runs"). CO-OP is
        // retail's co-op lobby and always present; co-op over Xbox Live
        // (`coop_live`, below) is read when the lobby is on Live.
        crate::lobby_tabs::set_present(|p| {
            p.forge = offer.forge;
            p.firefight = offer.firefight;
        });
        crate::lobby_mode::install(
            features,
            crate::lobby_mode::LobbyMode {
                theater: i.theater,
                offer,
                store: Some(Arc::clone(store)),
                data: i.data_root.to_path_buf(),
                vfs: i.vfs.clone(),
            },
        )
        .context("installing the party lobby")?;
    }
    // Rules scripts (docs/game/rules-engine.md, "The host runtime"): on
    // unless H2_FEATURES turns `rules` off. A Live host puts the script's hash
    // on its session. A film replays a game whose host passed the start
    // gate, so the gate does not run in Theater. A default recording runs it.
    #[cfg(feature = "rules")]
    crate::rules::feature::install(
        features,
        i.live.clone(),
        features_off || film.as_ref().is_some_and(|f| f.plays_back()),
        i.vfs.clone(),
    )
    .context("installing rules scripts")?;
    for effect in [
        &crate::h1_particle_physics::H1ParticlePhysics as &dyn hooks::Feature,
        &crate::h1_flag::H1Flag,
        &crate::h1_glow::H1Glow,
        &crate::h1_lightning::H1Lightning,
        &crate::h1_sound_promotion::H1SoundPromotion,
    ] {
        features
            .add(effect)
            .map_err(|e| anyhow::anyhow!("installing {}: {e}", effect.manifest().id))?;
    }
    features
        .add(&crate::h1_conversation::Conversations)
        .map_err(|e| {
            anyhow::anyhow!("installing converted conversation playback and casting: {e}")
        })?;
    features
        .add(&crate::h1_command_atoms::CommandAtoms)
        .map_err(|e| anyhow::anyhow!("Halo 1 command lists: {e}"))?;
    // Halo 1's health model on Halo 1 maps (docs/halo1/health.md): no body
    // regeneration, health packs, a health meter. Off (Halo 2's) until the
    // map's identity or the H2_HEALTH_MODEL debug override says Halo 1.
    features
        .add(&crate::h1_squad_storage::SquadStorage)
        .map_err(|e| anyhow::anyhow!("campaign squad pools: {e}"))?;
    let health_on = health::install(features).context("installing the health model")?;
    features
        .add(&crate::h1_ai_control::AiControl)
        .map_err(|e| anyhow::anyhow!("Halo 1 AI control: {e}"))?;
    features
        .add(&crate::h1_script_builtins::ScriptBuiltins::default())
        .map_err(|e| anyhow::anyhow!("Halo 1 script builtins: {e}"))?;
    // Precision variants on converted Halo 1 maps in the Halo 2 sandbox
    // (docs/halo1/multiplayer.md): the battle rifle for the SMG at game
    // activation, from the swaps each package's manifest records.
    features
        .add(&crate::data_salts::DataSalts)
        .map_err(|e| anyhow::anyhow!("data salts: {e}"))?;
    features
        .add(&crate::data_salts::ObjectPoolClear)
        .map_err(|e| anyhow::anyhow!("object pool clear: {e}"))?;
    precision::install(features, i.packages).context("installing the precision swaps")?;
    // A Forge spawner's spare clips (docs/game/forge-map-variants.md): each
    // weapon the spawner makes takes the session's clips in reserve.
    features
        .add(&game::forge_spare_clips::SpareClips)
        .map_err(|e| anyhow::anyhow!("forge spare clips: {e}"))?;
    // A Forge starting point's weight: its bias added to the spawn
    // chooser's weight for the location.
    features
        .add(&game::forge_spawn_weight::SpawnWeight)
        .map_err(|e| anyhow::anyhow!("forge spawn weight: {e}"))?;
    // A Forge spawner's place at start: a held-back item waits its respawn
    // time after each round reset.
    features
        .add(&game::forge_place_at_start::PlaceAtStart)
        .map_err(|e| anyhow::anyhow!("forge place at start: {e}"))?;
    // A built Forge map's record of the three, read at activation, so a
    // match on the variant has them with no session.
    if let Some(vfs) = i.vfs.clone() {
        features
            .add(&game::forge_record::Record { vfs })
            .map_err(|e| anyhow::anyhow!("forge record: {e}"))?;
    }
    // Halo 1's or Halo 2's HUD on converted Halo 1 maps (docs/halo1/hud.md):
    // the owners' HUD references at game activation, by `hud::style_for_map`.
    hud_swap::install(features, i.packages, i.data_root).context("installing the HUD swap")?;
    crate::h1_hud_text::install(features)
        .map_err(|e| anyhow::anyhow!("installing Halo 1 HUD text: {e}"))?;
    // Stronger aim assist (docs/game/aim-assist.md): the weapons' aim cone and
    // pull at each map load, from the Aim Assist setting (Auto: Strong
    // while the touch controls show). Headless runs keep retail.
    aim_assist::install(features, i.settings.clone()).context("installing the aim assist")?;
    // The superbounce game type's HUD plate and spot lines
    // (docs/game/superbounce.md): idle unless the game's script requires
    // `hud.superbounce`.
    crate::superbounce_mode::install(features, telemetry.clone()).context("installing the superbounce mode")?;
    // Superbounce's Pogo Stick (docs/game/pogo-stick.md): the secondary
    // weapon of a game whose script has `pogo` on, on a map with the kit.
    crate::pogo_mode::install(features).context("installing the pogo stick")?;
    // No black overlay over a private playerless view (Forge's monitor,
    // the agent camera, the Pogo chase camera, Theater's cameras).
    features
        .add(&game::spectator_view::PrivateViewOverlay)
        .map_err(|e| anyhow::anyhow!("private-view-overlay: {e}"))?;
    // Teabags in every game, for the incel medal (docs/game/teabag.md):
    // recorded to Live and the film on the console that records the game.
    crate::teabag::install(features, telemetry.clone()).context("installing the teabag watch")?;
    // HUD elements of ours drawn by the game's HUD (docs/game/hud-kit.md),
    // and the superbounce HUD on it, fed from the mode's state; its
    // tutorial's seen flag is a setting.
    crate::hud_kit::install(features).context("installing the HUD kit")?;
    crate::superbounce_hud::install(features, i.settings.clone())
        .context("installing the superbounce HUD")?;
    // The Theater's playback HUD on the kit, fed from the host's transport
    // (docs/game/theater.md, "The playback HUD"); hidden outside a film.
    crate::theater_ui::hud::register();
    // The Firefight HUD on the kit, fed from the mode blob
    // (docs/game/firefight.md, "The HUD").
    if firefight_on {
        crate::firefight::hud::install(features).context("installing the Firefight HUD")?;
    }
    // The base HUD (score and clock) of a game on the script index
    // (docs/game/game-types-without-carriers.md, "The display"); hidden
    // in every other game.
    #[cfg(feature = "rules")]
    crate::rules::base_hud::install(features).context("installing the base HUD")?;
    // The announcer speaks the HUD's medal lines (medal_announcer.rs,
    // docs/audio/audio.md, "Our medals' announcer lines"); boot gives it
    // the mixer once the audio is up.
    crate::medal_announcer::install(features).context("installing the medal announcer")?;
    // Hold X while falling to steer onto a bounce strip, opt in by the
    // superbounce.fall_assist setting
    // (docs/game/superbounce.md, "Hold X: the fall assist"). Its input tap
    // goes in with the XAPI input taps (register_late).
    crate::superbounce_assist::install(features, i.settings.clone())
        .context("installing the superbounce assist")?;
    // A Clanker brain on player.0's pad through that tap, by the
    // player.bot setting (player_bot.rs, docs/runtime/agent-api-sim.md,
    // "player.bot").
    if let Some(s) = &i.settings {
        crate::player_bot::follow_setting(s);
    }
    // Bots (docs/game/bots.md): phantom peers in a session this console
    // hosts. Idle with none.
    let mut bots_on = false;
    if i.bots {
        crate::clanker_card::install(features, i.live.clone())
            .context("installing Clanker player options")?;
        let recorded = film.filter(|f| f.has_engine_track());
        bots_on = bot_glue::install_for_maps(features, i.data_root, recorded, sources.as_deref())
            .context("installing the bots")?;
        // The Clankers page and its Game Setup row (stage 7).
        if let Some(store) = i.settings.as_ref() {
            clankers::install(features, store, i.live.clone(), telemetry.as_ref()).context("installing the Clankers page")?;
        }
    }
    if let Some(hle) = i.live.as_ref().filter(|_| !features_off) {
        crate::live_glue::install_browser_advertisement(
            features, hle, i.settings.as_ref(), bots_on,
        )
        .context("installing Live browser advertisements")?;
    }
    if let Some(store) = i.settings.as_ref() {
        features
            .add(&crate::lobby_options::LobbyOptions::new(
                Arc::clone(store),
                i.live.clone(),
                bots_on,
            ))
            .map_err(|e| anyhow::anyhow!("lobby options: {e}"))?;
    }
    // The iOS touch controls' game side (docs/runtime/ios.md): direct
    // look into player one's facing, and the layout from player one's
    // seat and weapon. Idle while no touch controls show.
    touch_glue::install(features).context("installing the touch glue")?;
    Ok(Core {
        editor_sources: sources,
        telemetry,
        ui_navigation,
        skulls,
        skulls_mp_on,
        skulls_custom_on,
        health_on,
        bots_on,
        creature_probe,
    })
}

/// The agent API's way in.
pub enum AgentSource {
    /// No agent (H2_AGENT unset and no debug menu).
    None,
    /// An agent with no socket, for the debug menu.
    Local,
    /// Full tick and input observers for film capture, without a listener.
    Recording,
    /// H2_AGENT=<socket or host:port>.
    Socket(PathBuf),
}

/// What [`register_late`] needs: the registrations after the host's
/// render, audio and input are in.
pub struct LateInputs<'a> {
    /// The renderer's output size now, when one draws this run.
    #[cfg(feature = "gpu")]
    pub output: Option<crate::run_opts::NativeSize>,
    #[cfg(feature = "gpu")]
    pub viewer_lod: Option<settings::Setting<u32>>,
    #[cfg(feature = "gpu")]
    pub world_line_present: Option<Arc<dyn render_bridge::scene_step::PresentObserver>>,
    pub rt: &'a Arc<Runtime>,
    pub lockstep: lockstep_glue::Config,
    pub agent: AgentSource,
    pub allow_bot_add: bool,
    pub xa: Option<&'a xapi_glue::Shared>,
    pub coop_probe: Option<coop_probe::Config>,
    pub live: Option<Arc<live_hle::Hle>>,
    pub settings: Option<Arc<settings::Store>>,
    /// The package gate, with Live on (the Mods screens download through it).
    pub map_gate: Option<Arc<live_hle::maps::MapGate>>,
    pub disc: &'a Path,
    pub data_root: &'a Path,
}

/// What [`register_late`] made, for the run.
pub struct Late {
    pub harness: Option<lockstep_glue::Lockstep>,
    pub agent: Option<agent_glue::AgentGlue>,
    pub snapshot_probe: Option<snapshot_probe::Probe>,
}

/// Registers everything from the lockstep harness to the snapshot probe.
pub fn register_late(features: &mut hooks::Registry, i: LateInputs<'_>) -> anyhow::Result<Late> {
    register_late_for_profile(features, i, compiled_render_hooks())
}

pub(crate) fn register_late_for_profile(
    features: &mut hooks::Registry,
    i: LateInputs<'_>,
    render_hooks: film::RenderHooks,
) -> anyhow::Result<Late> {
    anyhow::ensure!(
        compiled_render_hooks().supports(render_hooks),
        "unavailable render hook profile {render_hooks:?}"
    );
    let lockstep_on = i.lockstep.enabled();
    let input_replay = i.lockstep.replay.is_some();
    let xa = i.xa.map(AsRef::as_ref);
    // H2_LOCKSTEP / H2_INPUT_RECORD / H2_INPUT_REPLAY (lockstep_glue.rs).
    let harness = if lockstep_on {
        let h = lockstep_glue::Lockstep::install(features, i.lockstep, lockstep::TickHook::HALO2, xa)?;
        if let Some(n) = &h.tick_note {
            eprintln!("lockstep: hashing presents only: {n}");
        }
        Some(h)
    } else {
        None
    };
    // The superbounce fall assist's input tap (superbounce_assist.rs): under
    // the agent's tap, over a recording's or replay's, so a recording logs the
    // pad the guest got and a replay overwrites it.
    crate::superbounce_assist::install_tap(xa);
    // H2_AGENT=<socket>: the agent API (agent_glue.rs, docs/runtime/agent-api.md).
    let agent = match i.agent {
        // Over TCP (the Xbox app) a refused port must not stop the game:
        // it runs on, undriven, and says why.
        AgentSource::Socket(path) if matches!(agent_proto::Endpoint::from(path.clone()), agent_proto::Endpoint::Tcp(_)) => {
            match agent_glue::AgentGlue::install(features, path, xa, input_replay, i.allow_bot_add, i.data_root) {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("agent: not listening: {e:#}");
                    None
                }
            }
        }
        AgentSource::Socket(path) => Some(agent_glue::AgentGlue::install(
            features, path, xa, input_replay, i.allow_bot_add, i.data_root,
        )?),
        // The debug menu talks to an agent: a local one, with no socket.
        AgentSource::Recording => Some(agent_glue::AgentGlue::with_endpoint(
            features, None, xa, input_replay, i.allow_bot_add, i.data_root,
        )?),
        AgentSource::Local => Some(agent_glue::AgentGlue::local()),
        AgentSource::None => None,
    };
    // Our cheats (cheats.rs): refused in co-op and lockstep runs, and on
    // Live but for the allowed gamertags.
    if let Some(a) = &agent {
        a.install_matchmaking_trace(features)?;
        cheats::install(features, &a.agent, i.rt, lockstep_on, i.live.clone())?;
    }
    // The Rust datum, tag and locale functions (on unless H2_FEATURES has
    // impl.engine=off).
    features
        .add(&hooks::leaves::EngineLeaves)
        .map_err(|e| anyhow::anyhow!("engine leaves: {e}"))?;
    // The rasterizer's start-up and shutdown with no device
    // (docs/graphics/d3d-entries.md, "Start-up and shutdown"): every run.
    features
        .add(&render_bridge::rasterizer::Rasterizer)
        .map_err(|e| anyhow::anyhow!("rasterizer: {e}"))?;
    // The sound table (docs/engine/sound-instances.md): its bodies answer
    // every sound entry and the tick driver runs its phase. Every run.
    features.add(&sim_bridge::sim_sound::entries::SoundTable::process())
        .map_err(|e| anyhow::anyhow!("sound table: {e}"))?;
    // The World that holds the table's sounds: the snapshot's sim section
    // (docs/engine/sim-world.md). Every run.
    features.add(&sim_bridge::world::SimWorld::process())
        .map_err(|e| anyhow::anyhow!("sim world: {e}"))?;
    // The effects phase in Rust (docs/engine/effects-parts.md): the part arm
    // here, the effect walk in the tick driver below.
    features.add(&sim_bridge::sim_effects_parts::SimEffectsParts)
        .map_err(|e| anyhow::anyhow!("effect parts: {e}"))?;
    // The tick itself, from Rust; every hook on GAME_TICK stays around it.
    features
        .add(&sim_bridge::tick::TickDriver)
        .map_err(|e| anyhow::anyhow!("tick driver: {e}"))?;
    features.add(&sim_bridge::sim_physics::NativePhysics)
        .map_err(|e| anyhow::anyhow!("simulation physics: {e}"))?;
    // SimExt, the extension records saved with checkpoints and snapshots
    // (docs/engine/sim-rewrite.md section 2). Always on; with no records it
    // saves nothing and leaves the sim hash alone.
    features
        .add(&sim_bridge::sim_ext::SimExtFeature)
        .map_err(|e| anyhow::anyhow!("sim ext: {e}"))?;
    // Render front-end time in the frame stats, when H2_FEATURES has
    // render-timing (off by default).
    features
        .add(&render_bridge::render_timing::RenderTiming::new())
        .map_err(|e| anyhow::anyhow!("render timing: {e}"))?;
    // The view begin (brightness gamma ramp) and the vblank install, in Rust
    // (docs/graphics/d3d-device.md, "Brightness and gamma"). Every run.
    features
        .add(&render_bridge::display::Display)
        .map_err(|e| anyhow::anyhow!("display: {e}"))?;
    // The frame step leaves the game-state arena as it found it, under
    // impl.sim.frame-guard (off; docs/engine/sim-render-split.md, "The proof").
    features
        .add(&lockstep::frame_guard::FrameGuard::default())
        .map_err(|e| anyhow::anyhow!("frame guard: {e}"))?;
    #[cfg(feature = "gpu")]
    if render_hooks == film::RenderHooks::Gpu {
        use render_bridge::native_scene_lists::NativeSceneLists;

        let view_scale = if let Some(output) = i.output.clone() {
            let lod = crate::lod_distance::LodDistance::new(
                i.settings.as_deref(), Some(output), i.viewer_lod,
            )?;
            features.add(&lod).map_err(|e| anyhow::anyhow!("LOD distance: {e}"))?;
            lod.view_scale()
        } else {
            None
        };
        features
            .add(&render_bridge::native_map_work::NativeMapWork)
            .map_err(|e| anyhow::anyhow!("native map lifecycle: {e}"))?;
        // The frame step's FrameScene: camera, world and objects extracted
        // for the native stages (docs/graphics/framescene.md).
        features
            .add(&render_bridge::scene_step::FrameSceneStep {
                present: i.world_line_present,
            })
            .map_err(|e| anyhow::anyhow!("frame scene: {e}"))?;
        // The static world on the native GPU renderer
        // (on by default; docs/graphics/native-renderer.md). Its layer-draw observer
        // runs after render-timing's. The world reads its map and resources
        // through the VFS (native_map).
        features
            .add(&render_bridge::native_world::NativeWorld)
            .map_err(|e| anyhow::anyhow!("native world: {e}"))?;
        // Planar fog, layer 16, on the native GPU renderer
        // (on by default since 2026-11-20;
        // docs/graphics/native-water-fog.md, "Stage 4b"). Its layer-draw
        // observer sits inside render-timing's and the world's; the fog
        // materials load with the world (native_world_glue's worker,
        // native_fog_glue's load).
        features
            .add(&render_bridge::native_fog::NativeFog)
            .map_err(|e| anyhow::anyhow!("native fog: {e}"))?;
        // World layers 1, 2, 5, 7, 8, 17 and 20 drawn from the scene's plan
        // (frame-scene.layers, on with the native renderer;
        // docs/graphics/framescene.md, slice 4). The layer sets load with
        // the world (native_layers_glue).
        features
            .add(&render_bridge::native_layers::NativeLayers)
            .map_err(|e| anyhow::anyhow!("native layers: {e}"))?;
        // Water edge blend (layer 6) and the water, camouflage and hologram
        // census. The layer observer is always on.
        features
            .add(&render_bridge::native_water::NativeWater)
            .map_err(|e| anyhow::anyhow!("native water: {e}"))?;
        // Model decorators (layer 3), instanced on the native GPU renderer
        // (docs/graphics/native-decorators.md). Its layer-draw observer
        // follows the frame scene's; the passes load with the world
        // (native_world_glue's worker, native_decorators_glue's load).
        features
            .add(&render_bridge::native_decorators::NativeDecorators)
            .map_err(|e| anyhow::anyhow!("native decorators: {e}"))?;
        // Objects' render models on the native GPU renderer, skinned on the
        // GPU (on by default since 2026-11-20;
        // docs/graphics/native-objects.md, stage 2). Its observers on the
        // layer draw sit inside render-timing's and the world's; the models
        // load per map through native_objects_glue.
        features
            .add(&render_bridge::native_objects::NativeObjects::with_lod_scale(view_scale))
            .map_err(|e| anyhow::anyhow!("native objects: {e}"))?;
        // The first-person arms and weapon on the native GPU renderer
        // (on by default since 2026-11-20): both 0x0003fdf0 layer-3
        // draws, with the light and transparent calls
        // between and after them left in guest order.
        features
            .add(&render_bridge::native_first_person::NativeFirstPerson)
            .map_err(|e| anyhow::anyhow!("native first person: {e}"))?;
        features
            .add(&render_bridge::native_world_layers::NativeWorldLayers)
            .map_err(|e| anyhow::anyhow!("native world layers: {e}"))?;
        if let Some(scale) = view_scale {
            features.add(&NativeSceneLists::with_lod_scale(scale))
        } else {
            features.add(&NativeSceneLists)
        }.map_err(|e| anyhow::anyhow!("native scene lists: {e}"))?;
        // Mark the sorted geometry entries so their resolved indexed draws
        // can use a cached vertex binding in the game's order (off by default).
        features
            .add(&render_bridge::native_transparents::NativeTransparents)
            .map_err(|e| anyhow::anyhow!("native transparents: {e}"))?;
        // Dynamic decals in their seven observed pass positions
        // (on by default since 2026-11-20;
        // docs/graphics/native-decals-research.md). Only a fully resolved callback
        // is replaced; all others keep their D3D draws.
        features
            .add(&render_bridge::native_decals::NativeDecals)
            .map_err(|e| anyhow::anyhow!("native decals: {e}"))?;
        // Decal decorators at their sorted place, their draw cut once a
        // frame proves the stage (docs/graphics/retire-d3d-decal-decorators.md).
        features
            .add(&render_bridge::native_decal_decorators::NativeDecalDecorators)
            .map_err(|e| anyhow::anyhow!("native decal decorators: {e}"))?;
        features
            .add(&render_bridge::native_particles::NativeParticles)
            .map_err(|e| anyhow::anyhow!("native particles: {e}"))?;
        // The dynamic lights on the native GPU renderer
        // (on by default since 2026-11-20;
        // docs/graphics/native-lights.md, stage 4): a context-only skip on
        // each light's passes, in the game's light order. Its host
        // (native_lights_glue) does the device side of a skipped light's tail
        // and reports the bound target, but takes no light yet, so every light
        // keeps its D3D passes.
        features
            .add(&render_bridge::native_lights::NativeLights)
            .map_err(|e| anyhow::anyhow!("native lights: {e}"))?;
        features
            .add(&render_bridge::native_lights::walk::NativeLightWalk)
            .map_err(|e| anyhow::anyhow!("native light walk: {e}"))?;
        // Shader-driven gels: observe target-13 production before light consumption.
        features
            .add(&render_bridge::native_gels::NativeGels)
            .map_err(|e| anyhow::anyhow!("native gels: {e}"))?;
        features
            .add(&render_bridge::native_shadows::NativeShadows)
            .map_err(|e| anyhow::anyhow!("native shadows: {e}"))?;
    }
    features
        .add(&h1_runtime::h1_projectiles::Projectiles)
        .map_err(|e| anyhow::anyhow!("Halo 1 projectile flight: {e}"))?;
    features
        .add(&h1_runtime::h1_scope_zoom::ScopeZoom)
        .map_err(|e| anyhow::anyhow!("Halo 1 scope zoom: {e}"))?;
    #[cfg(feature = "gpu")]
    if render_hooks == film::RenderHooks::Gpu {
        // The view's post passes on the native GPU renderer
        // (on by default since 2026-11-20;
        // docs/graphics/native-post-hud.md, stage 5): a context-only hook at
        // the colour pass's entry. Its host (native_post_glue, installed with
        // the world's) plans the glow, has the device drop exactly its quads
        // and draws the glow natively in place of the last; the game's code
        // runs and leaves every state.
        features
            .add(&render_bridge::native_post::NativePost)
            .map_err(|e| anyhow::anyhow!("native post: {e}"))?;
        // The scene's target select 0x0001cd30 and clear 0x00014b60 made
        // on the device without their guest bodies, one call in 64 checked
        // against the body (native_targets_glue, installed with the device).
        features
            .add(&render_bridge::native_targets::NativeTargets)
            .map_err(|e| anyhow::anyhow!("native targets: {e}"))?;
        // The flares' visibility walker call 0x00020bb0 (from the query
        // pass 0x0002dd30) and reader 0x00020770 made in Rust: one count
        // pass on the device, the bytes carried from it two frames later;
        // one query in 64 checked against the bodies
        // (native_flare_query_glue, installed with the device).
        features
            .add(&render_bridge::native_flare_query::NativeFlareQuery)
            .map_err(|e| anyhow::anyhow!("native flare query: {e}"))?;
        // The flare draw pass's late glows 0x00046ea0 (the post's last
        // calls) drawn on the device, one in 64 checked against the body
        // (native_flare_draw_glue, installed with the device).
        features
            .add(&render_bridge::native_flare_draw::NativeFlareDraw)
            .map_err(|e| anyhow::anyhow!("native flare draw: {e}"))?;
        // The planar fog's screen quad 0x00025d50 drawn on the device, its
        // fan cut (native_fog_quad_glue, installed with the device).
        features
            .add(&render_bridge::native_fog_quad::NativeFogQuad)
            .map_err(|e| anyhow::anyhow!("native fog quad: {e}"))?;
        // The second fog's fans 0x00025dc0 drawn on the device, their
        // setups cut (native_fog_fans_glue, installed with the device).
        features
            .add(&render_bridge::native_fog_fans::NativeFogFans)
            .map_err(|e| anyhow::anyhow!("native fog fans: {e}"))?;
        // Native UI producers and the ordered menu, HUD and loading callbacks.
        features
            .add(&render_bridge::native_hud::NativeHud)
            .map_err(|e| anyhow::anyhow!("native hud: {e}"))?;
    }
    // H2_UI_WATCH=1: the screen player one's input goes to, a line on each
    // change and `ui.screen` (`h2 coop test --drive`; ui_watch.rs).
    ui_watch::install(features, i.disc)?;
    // H2_COOP_PROBE=1: the retail synchronous campaign path over System Link
    // (docs/live/live-coop.md section 9). Added last, so it can see every
    // override placed outside the plan and refuse instead of failing it.
    let probe_on = i.coop_probe.is_some();
    coop_probe::install(i.rt, features, i.coop_probe, i.disc, i.data_root)?;
    // Co-op over Live (docs/live/live-coop.md, "Co-op over Live (built)"):
    // the Xbox Live menu's Co-op Campaign row and screens, and the probe's
    // hooks armed only for a co-op session. With H2_COOP_PROBE on, the
    // probe holds those hooks and the screens only advertise and invite.
    if i.live.is_some() && std::env::var_os("H2_NO_COOP_ROW").is_none() {
        let arm = if probe_on {
            None
        } else {
            coop_probe::install_live(i.rt, features, i.data_root)?
        };
        if let Some(arm) = &arm {
            crate::coop_look::install(
                features,
                Arc::clone(arm),
                i.settings.as_deref(),
                agent.as_ref().map(|a| &a.agent),
            )?;
        }
        // coop.max_players: the host's co-op limit (docs/live/live-coop.md 12).
        if let (Some(arm), Some(store)) = (&arm, &i.settings) {
            let n = arm.set_max_players(i64::from(crate::setting_defs::coop_max_players(store)));
            if n != coop_probe::MAX_PLAYERS {
                eprintln!("coop-live: co-op takes up to {n} players (coop.max_players)");
            }
            // coop.join_in_progress: the host lets friends join a running
            // game (docs/live/live-coop.md 14).
            let jip = crate::setting_defs::coop_join_in_progress(store);
            crate::coop_jip::set_enabled(jip);
            if !jip {
                eprintln!("coop-jip: off (coop.join_in_progress)");
            }
        }
        let on = crate::coop_live::install(features, i.live.clone(), arm, i.settings.clone(), i.disc)?;
        custom_games::set_coop_row(on);
    }
    // The mod repository (docs/game/maps.md §5, "Mod downloads"): the Xbox
    // Live menu's last row opens our Mods screens in place of Content
    // Download's Map Pack dialog.
    let mods_on = crate::mods_screen::install(features, i.live.clone(), i.map_gate, i.data_root)?;
    custom_games::set_mods_row(mods_on);
    // Clankermatch (docs/game/bots.md §8): the Live menu's row splits the
    // OptiMatch playlists into the server's clanker hoppers and the rest.
    if i.allow_bot_add {
        crate::clankermatch::install(features, i.live.clone()).context("installing Clankermatch")?;
        // The lobby reads the server's Clanker marks to say whether
        // MULTIPLAYER's playlist is a Clanker one.
        crate::lobby_tabs::header::set_live(i.live.clone());
    }
    // Back to the Xbox Live menu, the cursor is on the browser or co-op
    // row the player chose, past the four rows retail restores.
    custom_games::install_cursor(features).context("installing the Live menu's cursor")?;
    // H2_SNAPSHOT_PROBE=1: count, per game tick, what would stop a machine
    // snapshot (snapshot_probe.rs, docs/runtime/snapshots.md).
    let snapshot_probe = snapshot_probe::Probe::from_env(features)?;
    Ok(Late { harness, agent, snapshot_probe })
}

mod profiles;
pub use profiles::{check_pruned, lift_replaced, replaced_entries};

#[cfg(test)]
pub(crate) mod tests;
