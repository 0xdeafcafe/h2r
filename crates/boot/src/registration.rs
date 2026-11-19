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
//! Host-only pieces (the Live HLE, the window's pipelined render, the input
//! and the agent's socket) come in through the inputs and are `None` in a
//! test unless a test supplies a stand-in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use runtime::Runtime;

use crate::{
    agent_glue, aim_assist, bot_glue, campaign_menu, cheats, clankers, coop_probe, custom_games,
    game_setup_help, h1_options, h1_options_row, health, hud, hud_swap, layout_glue, level_boot, lockstep_glue,
    map_copy, map_select, map_slots, precision, signin_screen, skulls, skulls_custom, skulls_menu,
    skulls_mp, snapshot_probe, telemetry, touch_glue, ui, ui_watch, variant_ext, variant_page, xapi_glue,
};

/// What [`register_core`] needs: the registrations up to the touch glue.
pub struct CoreInputs<'a> {
    pub rt: &'a Arc<Runtime>,
    /// The telemetry configuration, or why there is none.
    pub telemetry: Result<telemetry::TelemetryConfig, String>,
    /// The mounted VFS, when there is one (map slots, our skulls' map reads).
    pub vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    /// The Video screen's model (a settings store's, else none).
    pub model: Option<Arc<ui::Model>>,
    pub campaigns: Option<Arc<crate::campaigns::State>>,
    pub settings: Option<Arc<settings::Store>>,
    pub live: Option<Arc<live_hle::Hle>>,
    pub data_root: &'a Path,
    pub packages: &'a Path,
    pub level: Option<level_boot::Request>,
    /// `H2_PERF_FROM=level`: wait for the level-loaded mark.
    pub perf_level: bool,
    /// The XAPI layer is on (bots and the Clankers page need pads).
    pub xapi: bool,
}

/// What [`register_core`] decided, for the install lines after the plan.
pub struct Core {
    pub telemetry: Option<Arc<telemetry::Telemetry>>,
    pub skulls_on: bool,
    pub skulls_mp_on: bool,
    pub skulls_custom_on: bool,
    pub health_on: bool,
    /// The bots are installed (their nav meshes may be built at boot).
    pub bots_on: bool,
}

/// Registers everything from the telemetry hooks to the touch glue.
pub fn register_core(features: &mut feature::Registry, i: CoreInputs<'_>) -> anyhow::Result<Core> {
    let rt: &Runtime = i.rt;
    let telemetry = i
        .telemetry
        .and_then(|config| telemetry::install(features, &config))
        .unwrap_or_else(|error| {
            eprintln!("live telemetry disabled: {error}");
            None
        });
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
    layout_glue::install(rt, features, i.model).context("installing the button layouts")?;
    features
        .add(&crate::list_class::ScrollArrowLayout)
        .map_err(|e| anyhow::anyhow!("list scroll arrows: {e}"))?;
    campaign_menu::install(rt, features, i.campaigns).context("installing the campaigns")?;
    map_select::install(rt, features).context("installing the map select")?;
    // The Halo 1 / Halo 2 options (docs/halo1/multiplayer.md "Halo 1
    // options"): the switches follow the settings, the HUD style is picked
    // at each game start, and a custom game's Game Setup gets the Halo 1
    // Options row while its map is a converted Halo 1 map.
    if let Some(store) = i.settings.as_ref() {
        h1_options::install_switches(store);
        let screen = Arc::new(h1_options::OptionsScreen { store: Arc::clone(store) });
        let class = h1_options_row::class(screen);
        if let Err(e) = features.add(&class) {
            eprintln!("h1 options: {e}");
        }
        features
            .add(&h1_options_row::SetupRow { data_root: i.data_root.to_path_buf() })
            .map_err(|e| anyhow::anyhow!("h1 options row: {e}"))?;
        // The value picker A opens on a setting row, and our Game Setup
        // rows' picture and description (docs/game/ui-builder.md).
        crate::ui_kit::picker::install(features).context("installing the value picker")?;
        game_setup_help::register(
            h1_options_row::OPTIONS_ACTION,
            game_setup_help::image::QUICK_OPTIONS,
            Arc::new(|_: &mut dyn engine::memory::GuestMemory| Some(h1_options::ROW_HELP_ID)),
        );
        game_setup_help::install(features).context("installing the Game Setup help")?;
    }
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
    features.add(&hud::Hud).map_err(|e| anyhow::anyhow!("hud: {e}"))?;
    features
        .add(&crate::loaded_map::LoadedMap)
        .map_err(|e| anyhow::anyhow!("loaded map: {e}"))?;
    // The party privacy as the Live session's visibility, and the Live
    // menu's Custom Game Browser and Content Download rows (registered with the layouts above;
    // docs/live/live-custom-games.md).
    custom_games::install_privacy(rt, i.live.as_ref(), i.settings.as_ref())
        .context("installing the Live game visibility")?;
    // A Live sign-in that needs a code asks for it in game, on our own
    // screen opened from the logon's polling (docs/live/live.md, "In-game
    // sign-in").
    signin_screen::install(rt, features, i.live.as_ref()).context("installing the sign-in prompt")?;
    // A campaign level in place of the main menu (`h2 play --level`,
    // H2_LEVEL) and the level-loaded mark H2_PERF_FROM=level waits on
    // (docs/runtime/level-boot.md).
    level_boot::install(rt, features, i.level, i.perf_level).context("installing the level boot")?;
    // Campaign skulls kept per profile: a pickup is remembered and a
    // campaign start turns on the profile's chosen skulls (docs/game/skulls.md).
    // Headless runs (no settings store) leave skulls as retail. Hooks on
    // the feature plan; the line after the plan installs says where.
    let skulls_on = skulls::install(features, i.settings.clone()).context("installing the skulls")?;
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
    // Its damage, shield and health scales on each damage instance.
    features
        .add(&variant_ext::Vitality)
        .map_err(|e| anyhow::anyhow!("variant options vitality: {e}"))?;
    // Our own skulls (from 15): hidden in campaign levels, their effects
    // ours on the tick (docs/game/skulls.md "Our skulls"). The skull weapon's
    // tag comes from the loaded map's file, found through the VFS.
    let custom_maps: Option<skulls_custom::MapLocator> = i.vfs.map(|vfs| -> skulls_custom::MapLocator {
        Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let file = crate::map_override::map_file(&mut v, map)?;
            Some(telemetry::MapFile { path: file.path, offset: file.offset, len: file.len })
        })
    });
    let skulls_custom_on = skulls_custom::install(features, i.settings.clone(), telemetry.clone(), custom_maps)
        .context("installing our skulls")?;
    // Halo 1's health model on Halo 1 maps (docs/halo1/health.md): no body
    // regeneration, health packs, a health meter. Off (Halo 2's) until the
    // map's identity or the H2_HEALTH_MODEL debug override says Halo 1.
    let health_on = health::install(features).context("installing the health model")?;
    // Precision variants on converted Halo 1 maps in the Halo 2 sandbox
    // (docs/halo1/multiplayer.md): the battle rifle for the SMG at game
    // activation, from the swaps each package's manifest records.
    precision::install(features, i.packages).context("installing the precision swaps")?;
    // Halo 1's or Halo 2's HUD on converted Halo 1 maps (docs/halo1/hud.md):
    // the owners' HUD references at game activation, by `hud::style_for_map`.
    hud_swap::install(features, i.packages, i.data_root).context("installing the HUD swap")?;
    // Stronger aim assist (docs/game/aim-assist.md): the weapons' aim cone and
    // pull at each map load, from the Aim Assist setting (Auto: Strong
    // while the touch controls show). Headless runs keep retail.
    aim_assist::install(features, i.settings.clone()).context("installing the aim assist")?;
    // Local bots (docs/game/bots.md, stage 1): pads on controller ports,
    // signed in on the players screen, their views and listeners hidden.
    // `H2_BOTS` adds brains at boot; idle with none.
    let mut bots_on = false;
    if i.xapi {
        bot_glue::add_from_env();
        bots_on = bot_glue::install(features, i.data_root).context("installing the bots")?;
        // The Clankers page and its Game Setup row (stage 7).
        if let Some(store) = i.settings.as_ref() {
            clankers::install(features, store, i.live.clone(), telemetry.as_ref()).context("installing the Clankers page")?;
        }
    }
    // The iOS touch controls' game side (docs/runtime/ios.md): direct
    // look into player one's facing, and the layout from player one's
    // seat and weapon. Idle while no touch controls show.
    touch_glue::install(features).context("installing the touch glue")?;
    Ok(Core { telemetry, skulls_on, skulls_mp_on, skulls_custom_on, health_on, bots_on })
}

/// The agent API's way in.
pub enum AgentSource {
    /// No agent (H2_AGENT unset and no debug menu).
    None,
    /// An agent with no socket, for the debug menu.
    Local,
    /// H2_AGENT=<socket or host:port>.
    Socket(PathBuf),
}

/// What [`register_late`] needs: the registrations after the host's d3d,
/// audio and input are in.
pub struct LateInputs<'a> {
    pub rt: &'a Arc<Runtime>,
    pub lockstep: lockstep_glue::Config,
    pub agent: AgentSource,
    pub xa: Option<&'a xapi_glue::Shared>,
    /// From [`pipeline_for`].
    pub pipeline: feature::pipeline_render::PipelineRender,
    pub coop_probe: Option<coop_probe::Config>,
    pub live: Option<Arc<live_hle::Hle>>,
    pub settings: Option<Arc<settings::Store>>,
    /// The package gate, with Live on (the Mods screens download through it).
    pub map_gate: Option<Arc<live_hle::maps::MapGate>>,
    pub disc: &'a Path,
    pub data_root: &'a Path,
}

/// The pipelined render as the plan gets it: off for a lockstep run, which
/// compares serial runs.
pub fn pipeline_for(
    pipeline: feature::pipeline_render::PipelineRender, lockstep_on: bool,
) -> feature::pipeline_render::PipelineRender {
    if lockstep_on {
        pipeline.disabled("lockstep (H2_LOCKSTEP/H2_INPUT_RECORD/H2_INPUT_REPLAY) compares serial runs")
    } else {
        pipeline
    }
}

/// What [`register_late`] made, for the run.
pub struct Late {
    pub harness: Option<lockstep_glue::Lockstep>,
    pub agent: Option<agent_glue::AgentGlue>,
    pub snapshot_probe: Option<snapshot_probe::Probe>,
}

/// Registers everything from the lockstep harness to the snapshot probe.
pub fn register_late(features: &mut feature::Registry, i: LateInputs<'_>) -> anyhow::Result<Late> {
    let lockstep_on = i.lockstep.enabled();
    let input_replay = i.lockstep.replay.is_some();
    let xa = i.xa.map(|x| &**x);
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
    // H2_AGENT=<socket>: the agent API (agent_glue.rs, docs/runtime/agent-api.md).
    let agent = match i.agent {
        // Over TCP (the Xbox app) a refused port must not stop the game:
        // it runs on, undriven, and says why.
        AgentSource::Socket(path) if matches!(agent_proto::Endpoint::from(path.clone()), agent_proto::Endpoint::Tcp(_)) => {
            match agent_glue::AgentGlue::install(features, path, xa, input_replay) {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("agent: not listening: {e:#}");
                    None
                }
            }
        }
        AgentSource::Socket(path) => Some(agent_glue::AgentGlue::install(features, path, xa, input_replay)?),
        // The debug menu talks to an agent: a local one, with no socket.
        AgentSource::Local => Some(agent_glue::AgentGlue::local()),
        AgentSource::None => None,
    };
    // Our cheats (cheats.rs): refused in co-op and lockstep runs, and on
    // Live but for the allowed gamertags.
    if let Some(a) = &agent {
        cheats::install(features, &a.agent, i.rt, lockstep_on, i.live.clone())?;
    }
    // The Rust datum, tag and locale functions, when H2_FEATURES has
    // impl.engine=on (off by default).
    features
        .add(&feature::leaves::EngineLeaves)
        .map_err(|e| anyhow::anyhow!("engine leaves: {e}"))?;
    // Render front-end time in the frame stats, when H2_FEATURES has
    // render-timing (off by default).
    features
        .add(&feature::render_timing::RenderTiming::new())
        .map_err(|e| anyhow::anyhow!("render timing: {e}"))?;
    // Which pages the render front-end writes and the sim reads, when
    // H2_FEATURES has render-census (off by default;
    // docs/graphics/pipelined-render.md).
    let census = feature::render_census::RenderCensus::new()
        .with_region(crate::d3d_glue::PUSH_BASE, crate::d3d_glue::PUSH_BASE + crate::d3d_glue::PUSH_LEN, "push_buffer")
        .with_region(crate::d3d_glue::SCRATCH_BASE, crate::d3d_glue::SCRATCH_BASE + 0x1000, "d3d_scratch");
    features.add(&census).map_err(|e| anyhow::anyhow!("render census: {e}"))?;
    // H2_PIPELINE_RENDER=1: the render front-end one frame behind the
    // sim, on its own host thread (docs/graphics/pipelined-render.md, "The
    // prototype"). Pipelined and serial runs are not lockstep-compatible.
    features.add(&i.pipeline).map_err(|e| anyhow::anyhow!("pipeline render: {e}"))?;
    // Stage 7a: the Rust render_views, when H2_FEATURES has
    // impl.render.views=on (off by default; docs/graphics/render-loop.md).
    features
        .add(&feature::render_loop::RenderLoop)
        .map_err(|e| anyhow::anyhow!("render loop: {e}"))?;
    // Occlusion culling of objects (H2_FEATURES=impl.render.occlusion=on,
    // off by default; docs/graphics/occlusion-culling.md).
    features
        .add(&feature::occlusion::OcclusionCull)
        .map_err(|e| anyhow::anyhow!("occlusion: {e}"))?;
    // The static world on the native GPU renderer
    // (on by default, H2_FEATURES=impl.render.native_world=off turns it
    // off; docs/graphics/native-renderer.md). Its layer-draw observer is
    // context-only, so the pipelined render runs it on its own thread. It
    // runs after render-timing's; the world's map file comes from the
    // locator boot sets (native_world_glue::set_locator).
    features
        .add(&feature::native_world::NativeWorld)
        .map_err(|e| anyhow::anyhow!("native world: {e}"))?;
    // Objects' render models on the native GPU renderer, skinned on the
    // GPU (H2_FEATURES=impl.render.native_objects=on, off by default;
    // docs/graphics/native-renderer.md, stage 2). Its observers on the
    // layer draw sit inside render-timing's and the world's; the models
    // load per map through native_objects_glue.
    features
        .add(&feature::native_objects::NativeObjects)
        .map_err(|e| anyhow::anyhow!("native objects: {e}"))?;
    // The sorted transparents' geometry on the native GPU renderer
    // (H2_FEATURES=impl.render.native_transparents=on, off by default;
    // docs/graphics/native-renderer.md, stage 3): a context-only skip on
    // the geometry entries' handler, in the game's sort order. Its host
    // (native_transparents_glue, installed with the world's) draws the
    // native world's transparent parts; everything else keeps its D3D draw.
    features
        .add(&feature::native_transparents::NativeTransparents)
        .map_err(|e| anyhow::anyhow!("native transparents: {e}"))?;
    // The dynamic lights on the native GPU renderer
    // (H2_FEATURES=impl.render.native_lights=on, off by default;
    // docs/graphics/native-renderer.md, stage 4): a context-only skip on
    // each light's passes, in the game's light order. Its host
    // (native_lights_glue) does the device side of a skipped light's tail
    // and reports the bound target, but takes no light yet, so every light
    // keeps its D3D passes.
    features
        .add(&feature::native_lights::NativeLights)
        .map_err(|e| anyhow::anyhow!("native lights: {e}"))?;
    // The view's post passes on the native GPU renderer
    // (H2_FEATURES=impl.render.native_post=on, off by default;
    // docs/graphics/native-renderer.md, stage 5): a context-only hook at
    // the colour pass's entry. Its host (native_post_glue, installed with
    // the world's) plans the glow, has the device drop exactly its quads
    // and draws the glow natively in place of the last; the game's code
    // runs and leaves every state. H2_POST_AB=<dir> places it with the
    // switch off too, observing, for the frame A/B's captures.
    feature::native_post::place_for_capture(crate::native_post_glue::ab_requested());
    features
        .add(&feature::native_post::NativePost)
        .map_err(|e| anyhow::anyhow!("native post: {e}"))?;
    // H2_UI_WATCH=1: the screen player one's input goes to, a line on each
    // change and `ui.screen` (`h2 coop test --drive`; ui_watch.rs).
    ui_watch::install(features)?;
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
    crate::clankermatch::install(features, i.live.clone()).context("installing Clankermatch")?;
    // Back to the Xbox Live menu, the cursor is on the browser or co-op
    // row the player chose, past the four rows retail restores.
    custom_games::install_cursor(features).context("installing the Live menu's cursor")?;
    // H2_SNAPSHOT_PROBE=1: count, per game tick, what would stop a machine
    // snapshot (snapshot_probe.rs, docs/runtime/snapshots.md).
    let snapshot_probe = snapshot_probe::Probe::from_env(features)?;
    Ok(Late { harness, agent, snapshot_probe })
}

#[cfg(test)]
pub(crate) mod tests;
