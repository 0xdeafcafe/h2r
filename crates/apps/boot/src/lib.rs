//! The composition root: the module `recomp` built (`boot::build`, the
//! re-exported pipeline), registered with the runtime, the kernel HLE
//! attached, every feature `game` holds planned onto the hooks
//! (`registration`), and the entry thread run until something stops it.
//! `game` is re-exported whole, so `boot::skulls` and `boot::RunOpts`
//! resolve. See docs/runtime/BOOT.md.
//!
//! Nothing derived from the disc is written outside the caller's output
//! directory, which should be under `$TMPDIR` or `target/`.

pub use game::*;
mod controller_lights;
mod idle_audio;
// The game's content packages and `packages.list` (packages.rs).
pub mod packages;
// `store.status`: the tag store and the maps it serves (store_status.rs).
pub mod store_status;
// maps.test.*: test maps from recipes (docs/game/test-maps.md).
pub mod registration;
#[cfg(feature = "test-maps")]
pub mod forge_maps;
#[cfg(feature = "test-maps")]
pub mod ground_builder;
// The AI ground a `map` item's ground file asks for (map_ground.rs).
#[cfg(feature = "test-maps")]
pub mod map_ground;
#[cfg(feature = "test-maps")]
pub mod nav_builder;
#[cfg(feature = "test-maps")]
pub mod test_maps_recipes;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use runtime::Event;

fn describe(ev: Option<&Event>, thread: u32) -> String {
    match ev {
        Some(Event::KernelCall(k)) => {
            let args: Vec<String> = k
                .stack_args
                .iter()
                .map(|a| a.as_ref().map_or("?".into(), |v| format!("{v:#x}")))
                .collect();
            format!(
                "kernel call #{} {} (ret {:x?}, args [{}], ecx {:#x}, edx {:#x})",
                k.ordinal,
                k.name,
                k.return_pc,
                args.join(", "),
                k.cpu.ecx,
                k.cpu.edx
            )
        }
        Some(Event::UnrecordedFault { pc }) => match shim::lookup(thread, *pc) {
            Some(s) => s,
            None => format!("unrecorded fault at {pc:#x}"),
        },
        Some(e) => format!("{e:x?}"),
        None => "no event".into(),
    }
}

/// Why `H2_PERF_FROM`, `H2_TIMELINE` or `H2_PROFILE` will do nothing in
/// this run, if they won't: the session needs an h2 built with
/// `--features perf`, and opens only at `H2_PERF_FROM` (a present number
/// or `level`), so `H2_TIMELINE`/`H2_PROFILE` alone write nothing.
pub fn perf_env_note(
    perf_build: bool,
    perf_from: bool,
    session_vars: bool,
) -> Option<&'static str> {
    match (perf_build, perf_from, session_vars) {
        (false, true, _) | (false, _, true) => Some(
            "perf: H2_PERF_FROM, H2_TIMELINE and H2_PROFILE need an h2 built with --features perf; no session, nothing written",
        ),
        (true, false, true) => Some(
            "perf: H2_TIMELINE/H2_PROFILE open their session at H2_PERF_FROM (a present or `level`), which is unset; nothing written",
        ),
        _ => None,
    }
}

/// Register the retail image's hooked interior sites (the render sites, the
/// script index's, `guest_layout::points::game_types`, the AI skulls'
/// campaign gates, `guest_layout::points::skull_gates`, and the AI's multiplayer
/// gate, `guest_layout::points::ai_gates`) with the image
/// builder (`recomp::register_retail_entries`), so discovery makes them
/// instruction entries in both tiers. Every binary that builds, stamps,
/// loads or analyses the image calls it first (`h2`'s `main`, the Apple
/// hosts); [`run`] and its variants call it too. Idempotent.
pub fn register_image_entries() {
    recomp::register_retail_entries(image_entries());
    // And what the lift may leave out (docs/lifter/lift-pruning.md).
    recomp::register_lift_replaced(registration::lift_replaced);
}

/// The list [`register_image_entries`] registers, in order.
pub fn image_entries() -> impl Iterator<Item = u32> {
    use guest_layout::points::{ai_gates, cache_wait, game_types, render_aspect, scene_effects, skull_gates};
    render_aspect::instruction_entries()
        .chain(scene_effects::instruction_entries())
        .chain(game_types::instruction_entries())
        .chain(skull_gates::instruction_entries())
        .chain(ai_gates::instruction_entries())
        .chain(cache_wait::instruction_entries())
}

/// Map the image, attach the kernel, register the generated module and run
/// the entry thread.
pub fn run(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts) -> Result<RunReport> {
    run_hosted(xbe_bytes, dylib, opts, Host::default())
}

/// [`run`] inside a host: its D3D backend, audio sink and input, and a
/// callback between slices that paces the run and can end it.
pub fn run_hosted(xbe_bytes: &[u8], dylib: &Path, opts: &RunOpts, host: Host) -> Result<RunReport> {
    run_hosted_with_live(xbe_bytes, dylib, opts, host, &LiveConfig::default())
}

/// [`run`] with the opt-in client configuration. Offline callers keep [`run`].
pub fn run_with_live(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    live: &LiveConfig,
) -> Result<RunReport> {
    run_hosted_with_live(xbe_bytes, dylib, opts, Host::default(), live)
}

/// [`run_hosted`] with explicit Live client settings. The host/CLI supplies
/// the account path and server base URL; no implicit environment settings.
pub fn run_hosted_with_live(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    host: Host,
    live: &LiveConfig,
) -> Result<RunReport> {
    run_hosted_with_publisher(xbe_bytes, dylib, opts, host, live, None)
}

/// A light host may publish retail sessions using its provider's lease.
pub fn run_hosted_with_publisher(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    host: Host,
    live: &LiveConfig,
    publisher: Option<HostPublisher>,
) -> Result<RunReport> {
    run_configured(xbe_bytes, dylib, opts, host, (live, publisher), None, None)
}

/// Runs a prepared headless match, optionally capturing it in Live.
pub fn run_sim(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    sim: sim_glue::Sim,
    upload: Option<telemetry::TelemetryConfig>,
) -> Result<RunReport> {
    run_configured(
        xbe_bytes,
        dylib,
        opts,
        Host::default(),
        (&LiveConfig::default(), None),
        Some((sim, upload)),
        None,
    )
}

/// Plays a film from its game start: no machine part is restored and no
/// startup decision is read. The data directory is empty and the game's
/// first map request is answered with the film's options. A film with no
/// game start is refused ([`theater::NO_GAME_START`]).
pub fn run_theater(
    xbe: &[u8],
    image: &Path,
    opts: &RunOpts,
    mut host: Host,
    replay: theater::Replay,
    child_started: std::time::Instant,
) -> Result<RunReport> {
    let start = film::file::game_start_of(&replay.path)?.context(theater::NO_GAME_START)?;
    anyhow::ensure!(host.level.is_none(), "a film plays the level its options name");
    host.level = Some(level_boot::Request::recorded(&start.options)?);
    let lan = LiveConfig::default();
    run_configured(xbe, image, opts, host, (&lan, None), None, Some((replay, child_started)))
}

/// A film played from its game start has the guest's storage writes go to a
/// private scratch (`vfs::Vfs::guard_writes`); the guard ends when the run
/// does, whatever way it ends.
struct WriteGuard(Arc<Mutex<vfs::Vfs>>);

impl WriteGuard {
    fn start(vfs: Arc<Mutex<vfs::Vfs>>, scratch: &Path) -> Result<Self> {
        vfs.lock()
            .unwrap_or_else(|e| e.into_inner())
            .guard_writes(scratch)
            .map_err(|e| anyhow::anyhow!("guarding the film's storage writes: {e:?}"))?;
        Ok(Self(vfs))
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).end_guard();
    }
}

/// Workers can still hold the VFS when the game exits. End this boot's
/// bindings on every exit, including a failed boot or guest fault.
struct EditorMount(vfs::MapRebind);

impl Drop for EditorMount {
    fn drop(&mut self) {
        self.0.clear_on_shutdown();
    }
}

fn run_configured(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    mut host: Host,
    (live, publisher): (&LiveConfig, Option<HostPublisher>),
    sim: Option<(sim_glue::Sim, Option<telemetry::TelemetryConfig>)>,
    replay: Option<(theater::Replay, std::time::Instant)>,
) -> Result<RunReport> {
    register_image_entries();
    let boot_started = std::time::Instant::now();
    let (replay, child_started) = match replay {
        Some((replay, start)) => (Some(replay), Some(start)),
        None => (None, None),
    };
    // H2_SIM and the CLI sim entry share the dedicated profile. The explicit
    // switch off supplies a stock headless control for the same roster.
    let (sim_mode, upload) = match sim {
        Some((sim, upload)) => (Some(sim), upload),
        None if replay.is_some() => (None, None),
        None => (sim_glue::from_env()?, None),
    };
    let switches = hooks::Switches::from_env().map_err(anyhow::Error::msg)?;
    let host_light =
        host.host_light || (sim_mode.is_some() && switches.feature("host-light", true));
    #[cfg(not(feature = "desktop-runtime"))]
    anyhow::ensure!(host_light, "the host binary requires the light host profile");
    let coalesce_host_ticks = host_light && switches.feature("host-light.tick-pacing", false);
    // `store-maps`: the disc's maps served from the tag store at
    // <data>/tags (docs/engine/asset-store.md, AS2); off until AS2 is
    // accepted. A disc with no maps of its own (`<data>/disc/`, the copy of
    // the image's other files) boots with the store's maps, or not at all
    // (store_status::serve_from_store).
    let store_maps = {
        let feature = switches.feature("store-maps", false);
        let has_maps = store_status::disc_has_maps(&opts.disc);
        let root = store_status::root(&opts.data_root);
        let stored = if has_maps {
            Vec::new()
        } else {
            vfs::store_map::status(&root).maps.into_iter().map(|m| m.map).collect()
        };
        let stored: Vec<&str> = stored.iter().map(String::as_str).collect();
        let serve =
            store_status::serve_from_store(feature, has_maps, &stored, &opts.disc, &root)?;
        if serve && !feature {
            log::note(&format!(
                "disc: {} has no maps; maps\\*.map are served from the store at {}",
                opts.disc.display(),
                root.display()
            ));
        }
        serve
    };
    let replaying = replay.is_some();
    // The game start a film plays from, if it has one (`theater::film_play`).
    let play_start = match &replay {
        Some(r) => film::file::game_start_of(&r.path)?,
        None => None,
    };
    let render_every = replay.as_ref().map_or(0, |r| r.render_every);
    if host_audit::module_from_env().is_some() {
        anyhow::ensure!(host_light, "H2_HOST_MODULE requires the light host profile");
    }
    if host.host_light && !replaying {
        anyhow::ensure!(live.enabled && sim_mode.is_none(),
            "--host-light serves the network lobby; unset H2_SIM");
        anyhow::ensure!(
            host.level.is_none() && std::env::var_os("H2_LEVEL").is_none_or(|v| v.is_empty()),
            "light host starts in the network lobby; unset H2_LEVEL"
        );
    }
    #[cfg(feature = "gpu")]
    let rendered_replay = replaying && render_every != 0 && sim_mode.is_none() && host.gfx.is_some();
    #[cfg(feature = "gpu")]
    let host_graphics = !rendered_replay && (host.gfx.is_some() || host.gfx_view.is_some());
    #[cfg(not(feature = "gpu"))]
    let host_graphics = false;
    if host_light || sim_mode.is_some() {
        anyhow::ensure!(
            !host_graphics,
            "light host and sim run with no renderer"
        );
        anyhow::ensure!(
            opts.d3d && opts.bink && opts.dsound && opts.xapi,
            "light host and sim require the guest D3D, Bink, DirectSound and XAPI shims"
        );
    }
    bots::vis::set_light_index_budget(host_light);
    let bots = opts.xapi && (!host_light || sim_mode.is_some() || host.host_bots);
    presents::keep_present_history(!host_light);
    let x = xbe::Xbe::parse(xbe_bytes)?;
    let slim_stamp = host_light.then(|| current_slim_stamp(xbe_bytes, dylib)).flatten();
    let tier = if slim_stamp.is_some() {
        anyhow::ensure!(std::env::var_os(host_audit::ENV).is_none_or(|p| p.is_empty()),
            "host audit requires a full generated module");
        Some(tier::Tier::Host)
    } else {
        tier::module_tier(xbe_bytes, dylib)
    };
    let Some(tier) = tier else {
        bail!(
            "{} was not built from this image by this generator (stamp {} wants {:?}, or the fast tier's {:?}); rebuild it with `h2 build`",
            dylib.display(),
            stamp_path(dylib).display(),
            build_stamp(xbe_bytes).trim(),
            tier::fast_stamp(xbe_bytes).trim()
        );
    };
    eprintln!(
        "tier: running the {} module {}",
        tier.name(),
        dylib.display()
    );
    let running_stamp = || match tier {
        tier::Tier::Host => slim_stamp.as_ref().expect("validated host stamp").clone(),
        tier::Tier::Clang => build_stamp(xbe_bytes),
        tier::Tier::Fast => tier::fast_stamp(xbe_bytes),
    };
    // Keep Halo's fixed arena, physical [0x61000, 0x3145000), free, with its
    // contiguous window at 0x80061000 and (boot addition) its write-combined
    // window at 0xf0061000, so MmAllocateContiguousMemoryEx can return
    // exactly what the game assumes. The write-combined window is a boot
    // stopgap: --no-stopgaps leaves it out.
    let mut options = runtime::loader::LoadOptions::halo2();
    options.map_guest_reads = host_light && switches.implementation("sim.mapped-reads", true);
    if opts.stopgaps {
        for r in &mut options.physical_reservations {
            r.aliases.push(render_glue::WC_BASE | r.physical.start);
        }
    }
    let mut loaded = runtime::load_xbe_with(xbe_bytes, &options).context("mapping the XBE")?;
    if opts.stopgaps {
        render_glue::map_reserved_write_combined(&loaded.runtime)?;
    }
    for n in &loaded.permission_notes {
        log::note(&format!("page {:#x} widened to {:?}", n.page, n.effective));
    }
    let t = Instant::now();
    // Shadow pages: trace 0xa0ff0000,
    // barrier 0xa0ff4000, live notice 0xa0ff6000. Hooks placed by the
    // feature plan (telemetry, lockstep, the level-loaded mark, our strings)
    // take theirs from the feature arena (`hooks::arena`).
    // SAFETY: the dylib is ours and stays loaded until the end of this fn.
    let module = unsafe { load_module(dylib, &loaded.runtime, xbe_bytes)? };
    anyhow::ensure!(module.slim == (tier == tier::Tier::Host), "module coverage table and build stamp disagree");
    // The shadow checks build the lifted original from this image on demand.
    sim_bridge::shadow::SHADOW.set_source(Box::new(fallback::shadow_original));
    // Interpreting past unsupported instructions is a stopgap too.
    fallback::set_recovery(opts.stopgaps);
    if opts.stopgaps {
        fallback::configure_analysis(&loaded.runtime, !host_light);
    }
    eprintln!(
        "registered {} entries, {} resumes in {:.1}s",
        module.entries,
        module.resumes,
        t.elapsed().as_secs_f64()
    );
    let k = kernel::Kernel::for_image(&mut loaded).context("attaching the kernel")?;
    // The disc's map hashes, shared by the package resolver and the Live
    // map gate's bases (docs/game/maps.md §5): each disc map is read at most
    // once a session, or not at all when the stamp under <data>/maps/cache
    // already has it.
    let disc_hashes = content::DiscHashes::default();
    let map_gate = live
        .enabled
        .then(|| map_bases::gate(&opts.disc, &opts.data_root, disc_hashes.clone()));
    let sim_network = sim_mode
        .as_ref()
        .filter(|_| !replaying)
        .map(|_| sim_glue::hosted::Network::reserve())
        .transpose()?;
    // A film plays from its game start: a replay installs no Live network.
    let _live = if replay.is_some() {
        None
    } else if let Some(net) = &sim_network {
        let lan = LiveConfig {
            enabled: true,
            ..Default::default()
        };
        live_glue::install_with_network(&loaded.runtime, &k, &lan, net.config.clone(), None)?
    } else {
        live_glue::install_configured(&loaded.runtime, &k, live, None, map_gate.clone(), publisher)?
    };
    let mut film_path = host.film_out.take().or_else(|| {
        std::env::var_os("H2_FILM_OUT").filter(|p| !p.is_empty()).map(PathBuf::from)
    });
    let film_dir = || {
        std::env::var_os("H2_FILM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| opts.data_root.join("films"))
    };
    let lockstep_config = lockstep_glue::Config::from_env();
    // A player session records into the film library by default
    // (theater::autorecord, docs/game/film-format.md "Recording every game").
    let autorecord = {
        use theater::autorecord::{self, Decision};
        let decision = autorecord::decide(&autorecord::Inputs {
            request: host.film_auto,
            feature_on: switches.feature(autorecord::FEATURE, true),
            explicit: film_path.is_some(),
            replaying,
            host_light,
            sim: sim_mode.is_some(),
            lockstep: lockstep_config.enabled(),
        });
        match decision {
            Decision::Record => {
                let dir = film_dir();
                let now = std::time::SystemTime::now();
                let ready = std::fs::create_dir_all(&dir)
                    .map_err(anyhow::Error::from)
                    .and_then(|()| autorecord::prune(&dir, autorecord::KEEP, now));
                match ready {
                    Ok(_) => {
                        // A default recording is given the library: each
                        // game's film opens there at the game's activation.
                        eprintln!("theater: recording this session; each game's film goes to {}", dir.display());
                        film_path = Some(dir);
                        true
                    }
                    Err(e) => {
                        eprintln!("theater: not recording this session: the film library {}: {e:#}", dir.display());
                        false
                    }
                }
            }
            Decision::Skip(why) => {
                eprintln!("theater: not recording this session: {why}");
                false
            }
            Decision::Quiet => false,
        }
    };
    let film = if replaying || film_path.is_some() {
        let stamp = running_stamp();
        let clock = k.clock();
        let session = match theater::Session::open(
            film_path,
            replay,
            theater::Facts {
                stamp,
                title_id: x.certificate.title_id,
                epoch: clock.system_time() - clock.counter(),
                light: host_light,
                bots,
                telemetry_hooks: telemetry::image_film_hooks(&loaded.runtime),
                render_hooks: run_plan::compiled_render_hooks(),
                live_service: live.server_url.is_some(),
                address: _live
                    .as_ref()
                    .map_or_else(String::new, |h| h.film_address().to_string()),
                player_features: autorecord,
            },
            xbe_bytes,
            opts,
        ) {
            Err(e) if !replaying => {
                eprintln!("theater: recording could not start: {e:#}");
                None
            }
            result => result?,
        };
        if let Some(session) = &session
            && std::env::var("H2_THEATER_MEASURE").as_deref() == Ok("1")
        {
            session.start_measurements(boot_started, child_started);
        }
        session
    } else {
        None
    };
    // Features declare their hooks here; the plan is built and installed
    // once, below, after the last of them (docs/engine/feature-engine.md).
    let audit = host_audit::Audit::from_env(running_stamp, host_light);
    anyhow::ensure!(
        audit.is_none() || host_audit::module_from_env().is_none(),
        "host audit requires a full generated module"
    );
    anyhow::ensure!(
        audit.is_none() || !switches.feature("render-census", false),
        "host audit and render-census both use the guest access trap"
    );
    anyhow::ensure!(
        audit.is_none() || !switches.implementation("sim.frame-guard", false),
        "host audit and impl.sim.frame-guard both use the guest access trap"
    );
    let lockstep_on = lockstep_config.enabled();
    let mut features = hooks::Registry::new(registration::profile_switches(switches, host_light));
    #[cfg(feature = "gpu")]
    let native_target: run_opts::NativeTarget = host.gfx_view.as_ref().map(|view| view.native_target.clone())
        .unwrap_or_else(|| Arc::new(|| settings::NativeTarget::windowed(setting_defs::GUEST_FRAME)));
    #[cfg(not(feature = "gpu"))]
    let native_target: run_opts::NativeTarget = Arc::new(|| settings::NativeTarget::windowed(setting_defs::GUEST_FRAME));
    #[cfg(feature = "gpu")]
    let output_size = if opts.d3d && ((!host_light && sim_mode.is_none()) || rendered_replay)
        && host.gfx_view.is_some()
    {
        match host.gfx_view.as_ref().and_then(|v| v.viewer_output.clone()) {
            Some(output) => Some(output),
            None => Some(render_glue::output_size(host.settings.as_deref(), native_target.clone())?),
        }
    } else {
        None
    };
    #[cfg(feature = "gpu")]
    let display_size = host.gfx_view.as_ref().map(|view| Arc::clone(&view.display_size));
    #[cfg(feature = "gpu")]
    let viewer_lod = host.gfx_view.as_ref().and_then(|view| view.viewer_lod.clone());
    #[cfg(feature = "gpu")]
    let mut active_size = output_size.as_ref().map(|output| {
        let initial = output();
        Arc::new(move || initial) as run_opts::NativeSize
    });
    std::fs::create_dir_all(&opts.data_root)?;
    let mut cfg = vfs::Config::new(&opts.data_root, x.certificate.title_id);
    cfg.mmap_disc = host_light;
    cfg.map_guest_reads = options.map_guest_reads;
    cfg.store_maps = store_maps.then(|| store_status::root(&opts.data_root));
    if host_light {
        cfg.server_maps = std::env::var_os("H2_SERVER_MAP_DIR")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        // Sim stores: the retail maps' bytes a recording saw the guest
        // read, served instead of the disc (crates/runtime/vfs/src/sim_store.rs).
        cfg.sim_store = std::env::var_os(vfs::sim_store::STORE_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        // A sim batch's warm N:: the template's slots, cloned in after the
        // boot formats N: (docs/runtime/cache-slots.md, "Sim batches: a warm N:").
        cfg.cache_seed = std::env::var_os(vfs::CACHE_SEED_ENV)
            .map(|paths| {
                std::env::split_paths(&paths)
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            })
            .unwrap_or_default();
    }
    // Converted Halo 1 maps follow the sandbox chosen per game (h1_variants).
    cfg.variants = h1_variants::mp_variants();
    cfg.content_maps = new_maps::mounted_packages();
    let uploading_sim = upload.is_some();
    let film_only_sim = upload.as_ref().filter(|config| config.film_only_host()).cloned();
    if film_only_sim.is_some() && film.is_none() {
        anyhow::bail!("a film-only simulation lease requires a film output");
    }
    #[cfg(not(feature = "desktop-runtime"))]
    if opts.xapi && host.input.is_none() {
        host.input = Some(input::InputManager::new(Default::default())?);
    }
    #[cfg(feature = "desktop-runtime")]
    if opts.xapi && host.input.is_none() {
        if sim_mode.is_some() || !cfg!(feature = "local-io") {
            // A simulated host starts playerless. Its bots are network peers.
            host.input = Some(input::InputManager::new(Default::default())?);
        } else {
            let (input, keyboard) = input::InputManager::with_keyboard_pad(Default::default())?;
            host.input = Some(input);
            host.keyboard = Some(keyboard);
        }
    }
    #[cfg(feature = "desktop-runtime")]
    let icon_options = match host.icons {
        Some(o) => Some(o),
        None => icons::choice_from_env()?.map(|choice| icons::IconOptions {
            choice,
            gamepads: false,
        }),
    };
    #[cfg(feature = "desktop-runtime")]
    let icons = icon_options
        .map(|o| Arc::new(icons::Icons::new(o, host.keyboard.as_ref())))
        .filter(|i| i.enabled());
    // The game's content packages: their per-map items through one rewrite,
    // after the others, cached under <data>/packages/cache
    // (docs/editor/studio-redesign.md, "How the game consumes packages").
    // The pogo stick is the `pogo` package (content/pogo), on while the
    // `pogo` feature is.
    // Read the set before preparing its maps. The mount gives the same set
    // the kits' inputs once the campaigns and fonts are ready.
    let content_set = {
        let switches = features.switches();
        packages::read_set(&opts.data_root, &|name| switches.feature(name, true))
    };
    // The maps those packages build (`map` and `map-patch` items) are written
    // as map packages under <data>/maps before the content library reads
    // that folder: a map built for the same inputs is left alone
    // (docs/editor/studio-redesign.md, "How the game consumes packages").
    let staged = packages::prepare_maps(&opts.data_root, &opts.disc, &content_set.loaded);
    // A headless match names its map up front: a package that did not
    // stage stops it here, rather than as a missing level later.
    if sim_mode.is_some() {
        packages::staged_or_named(&staged)?;
    }
    // Packages for maps the disc doesn't have become content packages
    // under T:\$c, which the main menu reads for its map list
    // (docs/game/maps.md §4); before the VFS lists that folder.
    let mods_not_shown = new_maps::install_at_boot(&opts.data_root, &opts.disc);
    // The map select by category (docs/game/map-select.md), from the list
    // the main menu will build: the disc's maps and those packages.
    let _ = map_select::prepare_at_boot(&opts.data_root, &opts.disc, mods_not_shown);
    // The Halo 1 maps' sandbox and the campaign's assets start as the
    // settings say; the pregame screen changes them per game, and the VFS
    // and the health model follow (h1_variants, docs/halo1/multiplayer.md).
    h1_variants::init_from_store(&setting_defs::load(&opts.data_root, &[]).0);
    // A second campaign (docs/halo1/campaign-picker.md): read before the
    // VFS mounts, which serves its converted maps.
    let campaigns = campaigns::boot_state(&opts.data_root, &opts.disc);
    let campaign_maps = campaigns
        .as_ref()
        .map(|s| campaigns::host_maps(&opts.data_root, s))
        .unwrap_or_default();
    // The discoverable socket can adopt this boot's DVD on a file reload.
    let agent_socket = agent_glue::socket_from_env().or_else(|| {
        host.discoverable_agent.then(agent_glue::discoverable_socket).flatten()
    });
    let editor_hooks_on = features.switches().feature(agent::EDIT_LIFECYCLE, true);
    // A default recording lets the editor write; the first write ends the
    // game in play's film at the tick before (`outside_change`, below)
    // instead of being refused.
    let recording_or_playback = agent_glue::reload_recording_refusal(
        replaying,
        film.as_ref().is_some_and(|f| !f.player_features()),
        &lockstep_config,
    );
    let reload_mount = if host_light {
        Err("the light host has no writable editor map mount")
    } else if let Some(reason) = recording_or_playback {
        Err(reason)
    } else if !editor_hooks_on {
        Err("editor lifecycle hooks are disabled")
    } else if !features.switches().feature(map_slots::FEATURE, true) {
        Err("the map-slots feature is disabled")
    } else if agent_socket.is_none() {
        Err("the editor agent socket is disabled")
    } else {
        Ok(())
    };
    let maps = map_override::dir();
    campaign_cache::configure(
        &mut features, &loaded.runtime, &mut cfg, &campaign_maps, maps.as_deref(),
    )?;
    let packages = workspace::converted::halo1_metadata(&store_status::root(&opts.data_root));
    // Our campaigns' level pictures, added to mainmenu as the VFS serves it
    // (docs/halo1/campaign-picker.md, "Pictures").
    let cards = campaigns.as_ref().and_then(|s| {
        let levels: Vec<(String, String)> = s
            .built
            .iter()
            .flat_map(|b| b.levels.iter().map(|l| (b.key.clone(), l.map.clone())))
            .collect();
        let mut cards = campaign_cards::read_cards(&opts.data_root, &levels);
        campaign_cards::picker::prepare(&opts.data_root, &opts.disc, &mut cards);
        campaign_cards::CardRewrite::new(cards, s.cards.clone())
            .map(|r| r.with_cache(&opts.data_root))
    });
    // The game's content packages: their per-map items and kits through the
    // one rewrite, cached under <data>/packages/cache
    // (docs/editor/studio-redesign.md, "How the game consumes packages").
    // The pogo stick is the `pogo` package (content/pogo), on while the
    // `pogo` feature is. The `campaign-cards` kit (content/halo1) adds our
    // campaigns' level pictures to mainmenu and the `glyphs` kit (content/core)
    // puts the button icon sets, then the bot mark's gear and the HUD's
    // marks, in every font (docs/game/bots.md, "Clanker appearance and Live
    // player options"); the superbounce world labels' font is read from
    // them as served (docs/game/superbounce.md, "Crouch spots in the world").
    #[cfg(feature = "desktop-runtime")]
    let font_icons: Option<Box<dyn vfs::Rewrite>> = icons
        .as_ref()
        .map(|i| Box::new(i.font_rewrite()) as Box<dyn vfs::Rewrite>);
    #[cfg(not(feature = "desktop-runtime"))]
    let font_icons: Option<Box<dyn vfs::Rewrite>> = None;
    let content_packages = packages::at_boot(
        &opts.data_root,
        content_set,
        workspace::KitInputs {
            cards,
            icons: font_icons,
        },
        cfg.store_maps.as_deref(),
    );
    // With `store-maps`, a level the set lays items over is served from its
    // image under <data>/store/images (docs/engine/asset-store.md,
    // "Runtime assembly").
    cfg.store_levels = content_packages.levels.clone();
    let rewrites = content_packages.rewrite;
    let mounted = map_override::mount_for_agent(
        cfg.clone(),
        &opts.disc,
        rewrites,
        maps.as_deref(),
        &campaign_maps,
        reload_mount.is_ok()
            && features.switches().feature("agent", true)
            && features.switches().feature(agent::sim::calls::FEATURE, true),
    );
    #[cfg(feature = "desktop-runtime")]
    if let Some(i) = &icons {
        icons::install(&loaded.runtime, Arc::clone(i))?;
    }
    let (v, map_rebind) = match mounted {
        Ok(mount) => mount,
        Err(e) => {
            // A raw XBE (or a file that is not an XISO) still boots, with no
            // DVD mounted: the first map open then fails in the guest.
            eprintln!(
                "warning: {} is not a mountable XISO ({e:?}); running with no DVD",
                opts.disc.display()
            );
            let storage = vfs::Vfs::with_storage(cfg).map_err(|e| anyhow::anyhow!("vfs: {e:?}"))?;
            (storage, None)
        }
    };
    let editor_mount = map_rebind.clone().map(EditorMount);
    k.set_vfs(v).context("attaching the VFS")?;
    let external_maps = map_rebind.clone().filter(|_| {
        !host.embedded_live && _live.as_ref().is_some_and(|hle| hle.config().server_url.is_some())
    });
    live_notice::install_for_maps(&loaded.runtime, live, external_maps)
        .context("installing the Live notice")?;
    if let (Some(ready), Some(vfs)) = (host.on_vfs.take(), k.vfs()) {
        ready(&vfs);
    }
    // The film writes nothing of the player's: its saves and profile are
    // read and written in a scratch that goes with the run.
    let _write_guard = match (&play_start, k.vfs()) {
        (Some(_), Some(vfs)) => Some(WriteGuard::start(vfs, &opts.data_root.join("film-scratch"))?),
        (Some(_), None) => bail!("a film played from its game start needs the VFS"),
        (None, _) => None,
    };
    // Live downloads publish their offers before the join is handed to the
    // game. The next main-menu tick refreshes the in-memory map list.
    if let (Some(vfs), Some(gate)) = (k.vfs(), &map_gate) {
        let disc = opts.disc.clone();
        let maps = new_maps::mounted_packages();
        let variants = h1_variants::mp_variants();
        // A member builds a map variant from its own disc, the AI ground
        // with it (docs/game/forge-firefight.md, "Playing it").
        gate.on_build(Box::new(game::map_variant::build_document));
        gate.on_prepare(Box::new(move |id| {
            // The package this game loads, for a replica's Firefight genesis.
            game::map_variant::set_loaded(Some(id));
            if maps.has_id(id) || variants.has_id(id) {
                Ok(())
            } else {
                Err(format!("map {id} has no playable slot on this console"))
            }
        }));
        gate.on_installed(Box::new(move |_| {
            let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_select::packages_changed(&disc);
            vfs.rescan_maps();
        }));
    }
    // Packages written while the game runs (the built-in Firefight kits)
    // reach the VFS through `map_select::publish_packages`.
    if let Some(vfs) = k.vfs() {
        map_select::on_vfs_rescan(Box::new(move || {
            vfs.lock().unwrap_or_else(|e| e.into_inner()).rescan_maps();
        }));
    }
    // The lightmap store reads the loaded map's geometry blocks and
    // lightmap pixels through the VFS, as the guest's caches would
    // (docs/engine/sim-render-split.md, "6. Render scratch and stamps").
    if let Some(vfs) = k.vfs() {
        hot_glue::install_lightmap_files(vfs);
    }
    let uses = opts
        .stopgaps
        .then(|| stopgaps::install(&k, &loaded.runtime));
    // The kernel, XAPI and DSound rounds retired the boot's wrappers and
    // game overrides (docs/runtime/BOOT.md "Retired"); only the
    // XMountUtilityDrive override stays, for --no-xapi.
    if let Some(u) = &uses
        && !opts.xapi
    {
        xapi_glue::install_no_xapi_mount(&loaded.runtime, &k, 5, u);
    }
    // Community variants and the dedicated-host matchmaking status text.
    community_variants::install_with_live(&loaded.runtime, _live.clone())
        .context("installing community variants and matchmaking strings")?;
    // Extra button layouts and the Controller Speaker row on the Controls
    // screen (docs/input/controller-layouts.md, docs/audio/playstation-audio.md).
    // The Video row on the Settings list opens our Video screen
    // (docs/game/settings-menu.md).
    let model = match host.settings.clone() {
        Some(store) => {
            // A film applies its own settings to the run.
            let menu_size = match &film {
                Some(session) => session.video_menu_size(native_target.clone(), &store)?,
                None => native_target.clone(),
            };
            Some(Arc::new(video_screen::model_with_native_target(
                store, video_screen::Switches::PLATFORM, menu_size,
            ).map_err(anyhow::Error::msg)?))
        }
        None => None,
    };
    // More than one campaign (docs/halo1/campaign-picker.md): campaigns
    // with a manifest and available levels get their level lists and the
    // picker ahead of Campaign Options. None installed: nothing is
    // hooked and the disc is not read for it.
    let campaigns = campaigns.map(|c| campaign_menu::prepare(c, host.settings.clone()));
    let level = match (host.level.take(), &sim_mode) {
        (Some(l), _) => Some(l),
        (None, Some(s)) => {
            s.select_map(&opts.disc, &opts.data_root)?;
            None
        }
        (None, None) => level_boot::from_env(&opts.disc, Some(&opts.data_root)).context("H2_LEVEL")?,
    };
    let perf_from_var = std::env::var("H2_PERF_FROM").ok();
    let perf_from = perf_from_var
        .as_deref()
        .and_then(level_boot::PerfFrom::parse);
    if let Some(v) = perf_from_var.as_deref().filter(|_| perf_from.is_none()) {
        eprintln!("perf: H2_PERF_FROM={v:?} is neither a present number nor `level`; ignored");
    }
    let level_name = level.as_ref().map(|l| l.name.clone());
    // Every feature the plan gets up to the touch glue is registered in
    // `registration` (and its tests build the plan from the same code).
    let film_out = {
        let parent = dylib.parent().unwrap_or(Path::new("."));
        if tier == tier::Tier::Fast { parent.parent().unwrap_or(parent) } else { parent }
    };
    let film_installation = || theater::Installation::new(
        xbe_bytes,
        theater::playback_images(xbe_bytes, film_out)
            .into_iter().map(|(_, stamp)| stamp).collect(),
        opts.data_root.clone(),
    );
    let theater = if let Some((theater, open)) = host.theater.take() {
        theater.install(
            &mut features,
            &film_dir(),
            film_installation(),
            live,
            _live.clone(),
            open,
        )?;
        // Resume queued uploads on sign-in, including a session which acquired
        // its account in the game after launch.
        let _ = theater.sync();
        Some(theater)
    } else {
        None
    };
    let core = registration::register_core_film(
        &mut features,
        registration::CoreInputs {
            rt: &loaded.runtime,
            telemetry: match upload {
                Some(config) => Ok(config),
                None if replaying => Ok(telemetry::TelemetryConfig::playback()),
                None => telemetry::TelemetryConfig::from_live(
                    live.enabled,
                    live.accounts_path.as_deref(),
                    live.server_url.as_deref(),
                ),
            },
            vfs: k.vfs(),
            theater: theater.clone(),
            model,
            campaigns,
            settings: host.settings.clone(),
            live: _live.clone(),
            data_root: &opts.data_root,
            packages: &packages,
            level,
            perf_level: perf_from == Some(level_boot::PerfFrom::Level),
            bots,
        },
        film.clone(),
        registration::non_player(film.as_deref(), replaying, host_light, sim_mode.is_some()),
        map_rebind.clone(),
    )?;
    let (skulls, skulls_mp_on, skulls_custom_on, health_on) = (
        core.skulls,
        core.skulls_mp_on,
        core.skulls_custom_on,
        core.health_on,
    );
    // The bots' nav meshes, built in the background where missing or stale
    // (docs/game/bots.md §10 stage 2); H2_BOTS_NAV_AUTO=0 turns it off.
    // Sim mode never builds them: a build on a thread is at the wall
    // clock's mercy (docs/runtime/sim.md §8).
    let _nav_worker = if let (true, Some(vfs), None) = (core.bots_on, k.vfs(), &sim_mode) {
        bots_nav_auto::start(&opts.data_root, vfs, level_name.clone())
    } else {
        None
    };
    if let Some(s) = &sim_mode {
        if !core.bots_on {
            anyhow::bail!("sim: the bots are off (H2_FEATURES or no XAPI); a match needs them");
        }
        // With no nav worker, the run's verified spots and ghost runs
        // still land in its data's bots directory, as a game's do
        // (docs/runtime/sim.md, "Bounce practice").
        bots::spots::set_verified_dir(bots::cache::dir(&opts.data_root));
        s.add_bots()?;
    }
    let _telemetry = core.telemetry;
    #[cfg(feature = "gpu")]
    let observer_window = host.film_view.is_some();
    #[cfg(not(feature = "gpu"))]
    let observer_window = false;
    if let Some(f) = &film {
        if let Some(capture) = &host.film_capture {
            f.attach_capture(capture.clone())?;
        }
        if let Some(media) = host.film_media.take() {
            f.attach_media(media, &loaded.runtime)?;
        }
        #[cfg(feature = "gpu")]
        if let Some(view) = host.film_view.take() {
            f.attach_host_view(view, k.vfs().context("Theater observer requires a mounted map")?)?;
        }
        f.set_match_sources(k.vfs(), _live.clone(), sim_mode.as_ref().map(|s| s.matched()));
    }
    if uploading_sim && let (Some(t), Some(sim)) = (&_telemetry, &sim_mode) {
        let sim = sim.clone();
        t.set_sim_combat(move || sim.live_combat());
    }
    if uploading_sim && _telemetry.is_none() {
        anyhow::bail!("sim upload requires the telemetry feature");
    }
    // Frame species and weapon names come from the loaded map's tag names,
    // which stay on disc: let telemetry find the map file through the VFS.
    // Which map exactly (docs/game/maps.md §5): a package's MapId, None for the
    // disc's own map. Games and advertisements carry it as `map_id`.
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        let binding = map_rebind.clone();
        t.set_map_id_locator(Box::new(move |guest| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_override::active_map_id(&mut v, guest, binding.as_ref())
        }));
    }
    // A matchmaking game's record names its playlist and Live session
    // (docs/live/live-matchmaking.md §8.3, §8.4): the server fills in the
    // hopper's title and finds the arbitration round by the session. A
    // clanker hopper's game is a Clankermatch (docs/game/bots.md §8.2).
    if let (Some(t), Some(hle)) = (&_telemetry, &_live) {
        let lobby = Arc::clone(hle);
        t.set_hosted_session_locator(Box::new(move || lobby.hosted_session()));
        let hle = Arc::clone(hle);
        t.set_playlist_locator(Box::new(move || {
            hle.matchmade().map(|(id, session)| telemetry::Playlist {
                name: None,
                id: id.to_string(),
                // dh-identity: latch the queue's decision for this game.
                frames_from_host: hle.frames_from_host(&session),
                session_id: Some(session),
                clankers: hle.clanker_hoppers().contains(id),
            })
        }));
    }
    if let (Some(hle), Some(vfs)) = (&_live, k.vfs()) {
        let binding = map_rebind.clone();
        hle.set_map_probe(Some(Arc::new(move |g: &runtime::Guest| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_override::lobby_map_id(&mut v, g, binding.as_ref())
        })));
    }
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        let binding = map_rebind.clone();
        t.set_map_locator(Box::new(move |guest| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_override::active_map_source(&mut v, guest, binding.as_ref())
        }));
    }
    // The frame rate for the frame owner's display frames between scenes
    // (docs/graphics/framescene.md, section 7).
    #[cfg(feature = "gpu")]
    let fps = render_glue::frame_rate(host.settings.as_ref());
    // No window: a headless run, which skips the movies by default.
    #[cfg(feature = "gpu")]
    let headless = host.presenter.is_none();
    #[cfg(not(feature = "gpu"))]
    let headless = true;
    let render_hooks = film.as_ref()
        .map_or_else(run_plan::compiled_render_hooks, |s| s.render_hooks());
    // A present-limited run consumes every sealed frame in order.
    let stop_at: Option<u64> = std::env::var("H2_BOOT_STOP_AT_PRESENT").ok().and_then(|v| v.parse().ok());
    // The guest's D3D entries and frame wait in every run; a renderer
    // only with a window (docs/graphics/native-renderer.md).
    #[cfg(feature = "gpu")]
    let mut d3d_calls = match (opts.d3d, host.gfx.take(), host.presenter.take()) {
        (false, _, _) => None,
        (true, Some(ctx), Some(presenter)) if !host_light => {
            let output = output_size.clone().expect("a window has an output size");
            let (width, height) = output();
            let (dw, dh) = native_target().drawable;
            eprintln!("render: {width}x{height}; drawable {dw}x{dh}");
            let locked = host.unpaced_display || stop_at.is_some() || host.film_capture.is_some();
            Some(render_glue::install_game(&loaded.runtime, ctx, presenter, output, fps, locked, render_hooks)?)
        }
        (true, _, _) => Some(render_glue::install_headless(&loaded.runtime)?),
    };
    #[cfg(not(feature = "gpu"))]
    let mut d3d_calls = opts.d3d.then(|| render_glue::install_headless(&loaded.runtime)).transpose()?;
    #[cfg(feature = "gpu")]
    if let (Some(active_size), Some(device)) = (active_size, &d3d_calls) {
        let projected_size: run_opts::NativeSize = Arc::new(move || {
            let size = active_size();
            display_size.as_ref().map_or(size, |display| display(size))
        });
        let hud: Arc<dyn Fn() -> u32 + Send + Sync> = match &host.settings {
            Some(store) => {
                let setting = store.handle::<u32>(setting_defs::HUD_POSITION.key);
                Arc::new(move || setting.get())
            }
            None => {
                let def = &setting_defs::HUD_POSITION;
                let value = match std::env::var("H2_HUD_POSITION") {
                    Ok(raw) => def.parse_env_text(&raw).map_err(anyhow::Error::msg)?,
                    Err(_) => def.default,
                };
                let position = value.as_int().expect("HUD Position is a choice") as u32;
                Arc::new(move || position)
            }
        };
        render_aspect::install(&mut features, projected_size, !lockstep_on, hud, device.ui_layout.clone())?;
    }
    // Every world frame renders on its own random stream, so what the view
    // draws (a wider guest camera, Theater's free camera) never chooses the
    // simulation's random numbers, in lockstep and input tapes too
    // (docs/graphics/high-resolution.md, "Determinism").
    // Its scratch in the arena (the render-state cache, object `+0xcc`,
    // the light render fields) is the render's own copy while it draws
    // (docs/engine/sim-render-split.md, "6. Render scratch and stamps").
    if opts.d3d && !host_light {
        render_rng::install(&mut features)?;
        render_scratch::install(&mut features)?;
        // The flare state is the render's: the simulation's bytes are
        // put back after every frame (docs/engine/sim-render-split.md,
        // "4. The flare visibility byte").
        flare_store::install(&mut features)?;
    }
    // Vblanks on the 60 Hz logical clock (docs/graphics/d3d.md "Vertical blank"),
    // delivered as DPCs; the frame wait above is the clocked one.
    if let Some(handler) = d3d_calls.as_mut().and_then(|i| i.vblank.take()) {
        k.add_periodic(kernel::periodic::Source {
            name: "vblank",
            rate: kernel::periodic::Rate::HZ_60,
            max_catch_up: 4,
            handler,
        })
        .map_err(|e| anyhow::anyhow!("vblank source: {e:?}"))?;
    }
    #[cfg(feature = "local-io")]
    let pad = host.pad_speaker.take();
    // Use the same audio path in a deterministic sim A/B without the LAN
    // lobby, the map mount changes, or a rendered control.
    #[cfg(feature = "local-io")]
    let clock_only_audio = host_light
        || (sim_mode.is_some() && features.switches().feature("audio.clock-only", false));
    // Without local I/O the host has no speakers: audio is always the clock.
    #[cfg(not(feature = "local-io"))]
    let clock_only_audio = true;
    #[cfg(not(feature = "local-io"))]
    let ds = opts.dsound
        .then(|| dsound_glue::install_clock_only(&loaded.runtime, &k, xbe_bytes))
        .transpose()?;
    #[cfg(feature = "local-io")]
    let mut ds = if opts.dsound && clock_only_audio {
        Some(dsound_glue::install_clock_only(&loaded.runtime, &k, xbe_bytes)?)
    } else {
        match (opts.dsound, host.audio_sink.take()) {
            (false, _) => None,
            (true, Some(open)) => Some(dsound_glue::install_with_sinks(
                &loaded.runtime,
                &k,
                xbe_bytes,
                open,
                pad,
            )?),
            (true, None) => Some(dsound_glue::install(&loaded.runtime, &k, xbe_bytes)?),
        }
    };
    // The output's PCM tap: `audio.meter` (mixer_meter.rs) and, when a
    // film records, its sound. A clock-only output renders no PCM.
    #[cfg(feature = "local-io")]
    if let Some(audio) = &mut ds {
        let film = host.film_capture.clone();
        audio.output.tap_samples(move |pcm| {
            game::mixer_meter::feed(pcm);
            if let Some(capture) = &film {
                capture.audio(pcm);
            }
        });
    }
    if let Some(audio) = &ds {
        let mixer = audio.mixer();
        if clock_only_audio {
            mixer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_packet_clock(
                    (host_light || sim_mode.is_some())
                        && features
                            .switches()
                            .implementation("audio.packet-clock", true),
                );
        } else {
            setup_audible_mixer(&mixer, features.switches());
            // The sound table's sounds play through this mixer
            // (game/src/sound_feed.rs, docs/engine/sound-instances.md,
            // "Mixer feed").
            #[cfg(feature = "gpu")]
            game::sound_feed::connect(Arc::clone(&mixer), audio.wma.as_ref().map(|w| w.tables()));
        }
    }
    // A default recording and its replay both plan Forge (registration::local_forge).
    let local_forge = registration::local_forge(
        features.switches(),
        film.as_deref(),
        replaying,
        host_light,
        lockstep_on,
    );
    if local_forge
        && let Some(input) = &mut host.input
    {
        input.add_gate(Box::new(forge_controls::gate()));
    }
    // While a film plays, the Theater owns the pads: A pauses, the d-pad
    // skips, the sticks fly the free camera, and the guest sees neutral pads
    // (`theater_ui::overlay::Gate`). `h2 theater` installs the same gate
    // itself, on the input it takes from the window.
    if let (Some(theater), Some(input)) = (&theater, &mut host.input) {
        input.add_gate(theater.pad_gate(None));
    }
    // `input.host` (`h2 agent hostpads`) reads the manager's report after
    // the manager moves into XAPI.
    let host_pads = host.input.as_ref().map(input::InputManager::host_report);
    #[cfg(feature = "desktop-runtime")]
    let (xa, pad) = match (opts.xapi, host.input.take()) {
        (false, _) => (None, None),
        (true, Some(input)) => (
            Some(xapi_glue::register_with(&loaded.runtime, &k, input)?),
            host.keyboard.take(),
        ),
        (true, None) => {
            let (x, pad) = xapi_glue::register(&loaded.runtime, &k)?;
            (Some(x), Some(pad))
        }
    };
    #[cfg(not(feature = "desktop-runtime"))]
    let xa = host.input.take()
        .map(|input| xapi_glue::register_with(&loaded.runtime, &k, input))
        .transpose()?;
    let mut controller_lights = xa.as_ref().filter(|_| !host_light).map(|x| {
        controller_lights::ControllerLights::new(x, host.settings.as_deref())
    });
    // Voice chat's communicator (docs/live/live-voice.md): off until a
    // microphone is chosen on the Voice screen.
    #[cfg(feature = "local-io")]
    let mut voice_link = xa.as_ref().filter(|_| !host_light).and_then(|x| {
        let game_mix = ds
            .as_ref()
            .filter(|_| !clock_only_audio)
            .map(dsound_glue::Audio::mixer);
        voice_glue::install(
            &loaded.runtime,
            x,
            host.settings.as_ref(),
            host.voice_io.take(),
            game_mix,
        )
    });
    #[cfg(feature = "desktop-runtime")]
    let script = xapi_glue::InputScript::from_env();
    // H2_LOCKSTEP / H2_INPUT_RECORD / H2_INPUT_REPLAY (lockstep_glue.rs).
    // H2_AGENT=<socket>: the agent API (agent_glue.rs, docs/runtime/agent-api.md);
    // the debug menu talks to a local one with no socket.
    // With none set, a game that asks for it opens `<pid>.sock` in the agents
    // directory for editors to find (`H2_AGENT=off` keeps it closed).
    let agent_source = match agent_socket {
        Some(path) => registration::AgentSource::Socket(path),
        None if film.is_some() => registration::AgentSource::Recording,
        None if host.debug_menu.is_some() || theater.is_some() => registration::AgentSource::Local,
        None => registration::AgentSource::None,
    };
    let watch_overrides = agent_glue::socket_from_env().is_some();
    let coop_probe = coop_probe::parse(|k| std::env::var(k).ok()).map_err(anyhow::Error::msg)?;
    #[cfg(feature = "gpu")]
    let native_sources = k.vfs().map(|vfs| native_map::mount(vfs, map_rebind.clone(), &opts.data_root));
    let editor_link = host.editor_link.take();
    // The live mirror's camera layer reads the sealed frame scene
    // (docs/editor/live-mirror.md, "Cameras"): why this run seals none.
    #[cfg(feature = "gpu")]
    let scene_camera_off = if render_hooks != film::RenderHooks::Gpu {
        Some("native rendering is off")
    } else if !features
        .switches()
        .feature(render_bridge::scene_step::ID, true)
    {
        Some("frame scene capture is off")
    } else {
        None
    };
    #[cfg(feature = "gpu")]
    let (world_line_present, debug_capture_unavailable) = {
        let sources = std::mem::take(&mut host.world_line_sources);
        let disabled = if render_hooks != film::RenderHooks::Gpu {
            Some((
                agent::editor::MarkAvailability::NativeOff,
                "native rendering is off",
            ))
        } else if !features
            .switches()
            .feature(render_bridge::scene_step::ID, true)
        {
            Some((
                agent::editor::MarkAvailability::SceneOff,
                "frame scene capture is off",
            ))
        } else {
            None
        };
        if let Some((reason, why)) = disabled {
            if let Some(link) = &editor_link {
                link.set_mark_capture_disabled(reason);
            }
            (None, Some(why))
        } else if let Some(device) = &d3d_calls
            && (editor_link.is_some() || !sources.is_empty())
        {
            let _ = device;
            let observer = world_lines::present_observer(
                editor_link.clone(),
                sources,
                Arc::new(|| render_glue::ui_state().projection_factor),
            );
            (Some(observer), None)
        } else {
            (None, Some("no world-line capture source"))
        }
    };
    #[cfg(not(feature = "gpu"))]
    let debug_capture_unavailable = Some("this host has no GPU renderer");
    #[cfg(feature = "gpu")]
    let world_line_capture_requested = world_line_present.is_some();
    // A default recording keeps the machine's outside inputs from boot,
    // so a client's game leaves a film (theater::machine,
    // docs/game/film-format.md "The machine tape"). Its pad recorder is
    // XAPI's innermost tap, under the assist's and the agent's; lockstep
    // owns the pad tap when it is on.
    if !replaying
        && !lockstep_on
        && let (Some(session), Some(hle), Some(x)) = (film.as_ref(), _live.as_ref(), xa.as_ref())
        && session.player_features()
    {
        match theater::machine::Machine::record(hle, &**x) {
            Ok(machine) => session.attach_machine(machine),
            Err(e) => eprintln!("theater: a client's game leaves no film this session: {e:#}"),
        }
    }
    // The lockstep harness, the agent, our cheats, the render features and
    // the probes (registration.rs, the rest of the plan's features).
    let late = registration::register_late_for_profile(
        &mut features,
        registration::LateInputs {
            #[cfg(feature = "gpu")]
            output: output_size.clone(),
            #[cfg(feature = "gpu")]
            viewer_lod,
            #[cfg(feature = "gpu")]
            world_line_present,
            rt: &loaded.runtime,
            lockstep: lockstep_config,
            agent: agent_source,
            allow_bot_add: !host_light || core.bots_on,
            xa: xa.as_ref(),
            coop_probe,
            live: _live.clone(),
            settings: host.settings.clone(),
            map_gate: map_gate.clone(),
            disc: &opts.disc,
            data_root: &opts.data_root,
        },
        render_hooks,
    )?;
    let mut harness = late.harness;
    let mut agent = late.agent;
    if local_forge {
        forge_glue::install(
            &mut features,
            agent.as_ref(),
            k.vfs(),
            &opts.data_root,
            host.settings.clone(),
            _live.clone(),
        )?;
        // Build AI ground and Test waves: the build over this disc's maps
        // (ground_builder.rs), whose map the game reloads onto.
        #[cfg(feature = "test-maps")]
        forge_glue::ground::install(Box::new(
            ground_builder::AppGround::disc(opts.disc.clone()).after(Box::new(|built| {
                forge_glue::reload::keep_ground_copy(built.map_bytes().to_vec());
                Ok(())
            })),
        ));
        // Rebuild map: the session's map built the same way.
        #[cfg(feature = "test-maps")]
        forge_glue::reload::install_builder(Box::new(ground_builder::AppCopy::disc(
            opts.disc.clone(),
        )));
        // Generate nav: the bots' nav over the same build (nav_builder.rs).
        #[cfg(feature = "test-maps")]
        forge_glue::nav::install(Box::new(nav_builder::AppNav::disc(opts.disc.clone())));
        // The lobby's map variant builds the same way (forge_maps.rs).
        #[cfg(feature = "test-maps")]
        game::map_variant::install(Box::new(forge_maps::DiscMaps::new(
            opts.disc.clone(),
            opts.data_root.clone(),
        )));
    }
    if let Some(a) = &mut agent {
        // A film's replay plans the dialog as its recording did, window or
        // none (theater::compatibility's hook digest), and takes no request.
        let takeover = registration::lab_takeover(registration::TakeoverInputs {
            window: !headless,
            d3d: opts.d3d,
            xapi: xa.is_some(),
            lockstep: lockstep_on,
            host_light,
            sim: sim_mode.is_some(),
            replaying,
        });
        a.configure_takeover(&mut features, !headless || observer_window, takeover.plan, takeover.accept)
            .context("installing lab takeover consent")?;
    }
    if let (Some(theater), Some(a)) = (&theater, &agent) {
        theater.connect(&a.agent);
    }
    #[cfg(feature = "local-io")]
    if let (Some(a), Some(audio)) = (&agent, &ds) {
        dsound_glue::add_agent_methods(&a.agent, audio.mixer());
    }
    // audio.meter: UNAVAILABLE where the mixer renders no samples.
    if let Some(a) = &agent {
        game::mixer_meter::add_agent_method(&a.agent);
    }
    let mut snapshot_probe = late.snapshot_probe;
    // edit.* and render.* on the agent (hot_glue.rs, docs/graphics/hot-iteration.md).
    if let Some(a) = &agent {
        let maps = || {
            k.vfs().map(|vfs| -> hot_glue::MapLocator {
                let binding = map_rebind.clone();
                Box::new(move |guest| {
                    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
                    map_override::active_map_file(&mut v, guest, binding.as_ref())
                })
            })
        };
        if !replaying
            && !host.embedded_live
            && _live.as_ref().is_some_and(|h| h.config().server_url.is_some())
        {
            a.agent.set_live_session_probe(agent_glue::remote_live_game);
            a.agent.set_live_edit_probe(agent_glue::live_edit_refusal);
        }
        #[cfg(feature = "desktop-runtime")]
        if let Some(vfs) = k.vfs() {
            agent::render::CAPTURE.set_map_source(move |name| {
                let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
                let path = vfs.content_map_path(name)?;
                Some((vfs.map_id(&path)?, vfs.content_base_name(name)?))
            });
        }
        let arena = hot_glue::install(
            &a.agent,
            &loaded.runtime,
            maps(),
            k.vfs(),
            watch_overrides,
            replaying,
        );
        ui_stack::install(&a.agent, core.ui_navigation);
        // profile.faults: the fault census (agent_glue.rs).
        agent_glue::add_runtime_methods(&a.agent, &loaded.runtime);
        // render.marks: where the window drew the world marks (world_lines.rs).
        #[cfg(feature = "gpu")]
        world_lines::add_marks_method(&a.agent);
        // render.markers: the Forge game type markers the last present drew
        // (forge_glue/markers_world.rs).
        forge_glue::markers_world::add_method(&a.agent);
        // sim.give and sim.spawn find tags as edit.* does (agent_glue.rs).
        if let Some(sim) = &a.sim {
            sim.set_tags(Box::new(hot_glue::map_tags(&loaded.runtime, maps())));
            // world.placement.edit grows blocks into edit.batch's arena.
            if let Some(calls) = sim.calls() {
                agent::sim::calls::lock(&calls).set_arena(arena);
                // edit.collision.commit's BSP activation runs on the same
                // guest calls (hot_glue.rs).
                hot_glue::install_collision_activation(&a.agent, calls);
            }
        }
        // world.* results name their tags the same way.
        a.agent.set_tag_names(Box::new(hot_glue::map_tags(&loaded.runtime, maps())));
        if let Some(audio) = &ds {
            dsound_glue::add_loop_agent_method(
                &mut features,
                &a.agent,
                audio,
                hot_glue::map_tags(&loaded.runtime, maps()),
            )?;
        }
    }
    // maps.test.*: test maps built from recipes and loaded through the
    // agent's guest calls (test_maps.rs, docs/game/test-maps.md).
    #[cfg(feature = "test-maps")]
    if let Some(a) = &agent {
        test_maps::install(
            &a.agent,
            Arc::new(test_maps_recipes::ScenarioRecipes),
            a.sim.as_ref().and_then(|s| s.calls()),
            &opts.disc,
            &opts.data_root,
            k.vfs(),
        );
    }
    // packages.list: the content set and each map read (packages.rs).
    if let Some(a) = &agent {
        packages::install(&a.agent, content_packages.summary, content_packages.served);
    }
    // store.status: the tag store and whether its maps are served
    // (store_status.rs).
    if let Some(a) = &agent {
        store_status::install(&a.agent, &opts.data_root, store_maps);
    }
    // game.menu: leave the game for the main menu through the agent's
    // guest calls (game_menu.rs, docs/runtime/agent-api.md).
    if let Some(a) = &agent {
        game::game_menu::install(&a.agent, a.sim.as_ref().and_then(|s| s.calls()));
        // game.load / game.levels: any disc level queued the same way
        // (game_load.rs).
        game::game_load::install(
            &a.agent,
            a.sim.as_ref().and_then(|s| s.calls()),
            &opts.disc,
            &opts.data_root,
        );
    }
    // Paired editor presentation and the debug menu's host methods.
    if let Some(a) = &agent {
        a.agent
            .collision_traces()
            .bind(&loaded.runtime, lockstep_on || film.is_some());
        agent::editor::install(&a.agent, editor_link.clone());
        agent::editor::install_scene(&a.agent, editor_link.clone());
        debug_menu_glue::install(
            host.debug_menu.as_ref(),
            host_pads,
            &a.agent,
            host.settings.clone(),
            host.perf_overlay.clone(),
            host.cheat_line.clone(),
        );
        if let Some(link) = &host.readout {
            game::debug_readout::install(&a.agent, link.clone());
        }
        if let Some(code) = &host.game_code {
            game::game_code::install(&a.agent, Arc::clone(code));
        }
        if let Some(lights) = &controller_lights {
            lights.install(&a.agent);
        }
    }
    // Every profile runs the Rust simulation: the played game, `h2 sim`
    // and the headless host (docs/engine/sim-rewrite.md).
    features
        .add(&sim_bridge::sim_native::SimNative::new(&loaded.runtime))
        .map_err(anyhow::Error::msg)?;
    // The simulation's lightmap sample (docs/engine/sim-render-split.md,
    // "6. Render scratch and stamps"): the tick's effect tint.
    features
        .add(&sim_bridge::sim_lightmap::LightmapStore)
        .map_err(anyhow::Error::msg)?;
    registration::register_viewer_skips(&mut features, host_light)?;
    if host_light {
        let frame = match &sim_mode {
            Some(sim) => sim.frame().clone(),
            // A playerless v2 film presents a separate viewer. Its recorded
            // guest skipped every world render, including the shared RNG draws.
            None => sim_bridge::sim_frame::SimFrame::new(
                if film.as_ref().is_some_and(|s| s.has_engine_track()) { 0 } else { render_every }
            ).on_skip(presents::note_skipped_present),
        };
        anyhow::ensure!(
            features.add(&frame).map_err(anyhow::Error::msg)?,
            "light host requires sim-frame (H2_FEATURES)"
        );
    }
    // The frame step (docs/graphics/retire-d3d-display.md, "The frame
    // step"): render_frame_and_present as Rust, with the native renderer
    // on. A light host's sim-frame owns the point.
    #[cfg(feature = "gpu")]
    if !host_light {
        features
            .add(&render_bridge::render_frame::RenderFrame::default())
            .map_err(anyhow::Error::msg)?;
    }
    if let Some(a) = &agent {
        host_metrics::install(&mut features, &a.agent, host_light)?;
        host_lobby::install(&mut features, &a.agent, host_light, opts, _live.clone())?;
    }
    #[cfg(feature = "gpu")]
    if opts.d3d
        && !headless
        && film.is_none()
        && !lockstep_on
        && let Some(screen) = host.return_to_menu.take()
    {
        features
            .add(&return_to_menu::ReturnToMenu(Arc::from(screen)))
            .map_err(anyhow::Error::msg)?;
    }
    if let Some(audit) = &audit {
        anyhow::ensure!(features.add(audit).map_err(anyhow::Error::msg)?,
            "H2_HOST_AUDIT needs the host-audit feature");
        if let Some(a) = &agent {
            audit.add_method(&a.agent, Arc::clone(&loaded.runtime));
        }
    }
    // Sim match metrics and match.* on the agent.
    if let Some(s) = &sim_mode {
        let playback = film.as_ref().filter(|f| f.has_engine_track()).map(|_| replaying);
        s.install_with_audio(&mut features, playback, ds.as_ref().map(|a| Arc::clone(&a.hle)))?;
        if let Some(a) = &agent {
            s.add_methods(&a.agent);
        }
    }
    if let Some(film) = &film {
        if let Some(x) = &xa {
            film.install_neutral_pads(x)?;
        }
        anyhow::ensure!(
            features
                .add(&theater::Observer(
                    film.clone(),
                    agent.as_ref().map(|a| a.agent.clone())
                ))
                .map_err(anyhow::Error::msg)?,
            "theater-film feature is disabled"
        );
        if let Some(a) = &agent {
            film.add_methods(&a.agent);
        }
    }
    if let (Some(theater), Some(a)) = (&host.theater_playback, &agent) {
        theater.add_playback_methods(&a.agent);
    }
    // A film player queues the film's game again for a seek behind the
    // tick shown (`theater::Session::requeue`).
    if replaying
        && let (Some(f), Some(a)) = (&film, &agent)
        && let Some(calls) = a.sim.as_ref().and_then(|s| s.calls())
    {
        f.attach_calls(calls);
    }
    if !replaying && let Some(a) = &agent {
        // A session that records its games can watch a film in itself:
        // it queues the film's game through the agent's guest calls and
        // opens the film page again when the viewer leaves.
        let watch = film.clone().filter(|f| f.can_watch());
        if let Some(f) = &watch {
            if let Some(calls) = a.sim.as_ref().and_then(|s| s.calls()) {
                f.attach_calls(calls);
            }
            if let Some(theater) = theater.clone() {
                f.on_return(move |id, why| theater.reopen_film(id, why));
            }
        }
        theater::library_methods(
            &a.agent,
            film_dir(),
            opts.disc.clone(),
            film_out.to_owned(),
            opts.data_root.clone(),
            film_installation(),
            watch,
        );
    }
    let bink = if opts.bink {
        // A lockstep run compares schedules and movie pacing follows the
        // wall clock, so it skips them; so, by default, do level boots and
        // headless runs.
        let why_off = if host_light {
            Some("light host".into())
        } else {
            movies_glue::why_off(&opts.movies, lockstep_on, level_name.as_deref(), headless)
        };
        // Film recording and playback use the recorded movie policy and hook plan.
        let allow_attract = why_off.is_none() && film.is_none();
        let attract_file = if allow_attract && opts.movies != movies_glue::Movies::Off {
            k.vfs().and_then(|vfs| {
                vfs.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .host_file(movies_glue::ATTRACT_VFS_PATH)
            })
        } else {
            None
        };
        let (mode, hle, attract) = movies_glue::install(
            &loaded.runtime,
            &opts.movies,
            &opts.disc,
            attract_file,
            allow_attract,
            why_off.as_deref(),
            #[cfg(feature = "local-io")]
            ds.as_ref()
                .filter(|_| !clock_only_audio)
                .map(dsound_glue::Audio::mixer),
            #[cfg(not(feature = "local-io"))]
            None,
        );
        if allow_attract && mode == bink_hle::Mode::Playback {
            attract.install(&mut features, agent.as_ref().map(|a| &a.agent))?;
        }
        Some(hle)
    } else {
        eprintln!("movies: off (--no-bink: the original player runs)");
        None
    };
    if let Some(camera) = film.as_ref().and_then(|film| film.camera_render_hooks()) {
        features.add(&camera).map_err(anyhow::Error::msg)?;
    }
    // Part of a machine snapshot's build key (snapshot_glue.rs).
    let switch_spec = features.switches().spec();
    let plan = features.build(&hooks::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        anyhow::anyhow!("feature plan refused: {}", c.join("; "))
    })?;
    // A pruned module's cut roots must still be replaced: refused here,
    // before any guest code runs (docs/lifter/lift-pruning.md).
    registration::check_pruned(&module.pruned, &plan)?;
    let verbose_plan = hooks::plan_verbose();
    let installed = plan
        .install(&loaded.runtime)
        .context("installing the feature plan")?;
    if let Some(why) = installed.off_reason(replication_admission::FEATURE) {
        bail!("replication admission needs its feature hooks: {why}");
    }
    if core.creature_probe
        && let Some(why) = installed.off_reason(actor_net::creature::FEATURE)
    {
        bail!("creature probe needs its feature hooks: {why}");
    }
    // After the plan: the keyboard bridge wraps whatever the plan put on
    // the UI pump (keyboard_glue::install).
    #[cfg(feature = "desktop-runtime")]
    if let Some(keyboard) = pad.as_ref() {
        keyboard_glue::install(&loaded.runtime, keyboard.clone())
            .context("installing host keyboard entry")?;
    }
    if let Some(session) = &film {
        session.check_simulation_hooks(&plan, &installed)?;
    }
    for line in plan.startup_text(&installed, verbose_plan).lines() {
        eprintln!("feature plan: {line}");
    }
    if let Some(audit) = &audit {
        if let Some(why) = installed.off_reason("host-audit") {
            anyhow::bail!("host audit needs its feature hooks: {why}");
        }
        audit.installed(&installed);
    }
    if film.is_some()
        && let Some(why) = installed.off_reason(theater::OBSERVER_FEATURE)
    {
        bail!("film observer unavailable: {why}");
    }
    if film.as_ref().is_some_and(|f| f.telemetry_plan().enabled())
        && let Some(why) = installed.off_reason("live-telemetry")
    {
        bail!("film event observer unavailable: {why}");
    }
    if host_light && let Some(why) = installed.off_reason(sim_bridge::sim_frame::ID) {
        anyhow::bail!("light host needs sim-frame: {why}");
    }
    if let Some(a) = &agent {
        #[cfg(feature = "gpu")]
        if let Some(why) = scene_camera_off.or_else(|| {
            installed
                .off_reason(render_bridge::scene_step::ID)
                .map(|_| "frame scene capture is off")
        }) {
            a.agent.set_camera_unavailable(why);
        }
        #[cfg(feature = "gpu")]
        let debug_capture_unavailable = debug_capture_unavailable.or_else(|| {
            installed
                .off_reason(render_bridge::scene_step::ID)
                .map(|_| "frame scene capture is off")
        });
        let unavailable = if headless {
            Some("this host has no window")
        } else if host.debug_views.is_none() {
            Some("this window has no debug-view source")
        } else {
            debug_capture_unavailable
        };
        debug_views::install(&a.agent, host.debug_views.take(), unavailable);
        a.set_plan(&plan, &installed);
        let rt = Arc::clone(&loaded.runtime);
        let reload_vfs = k.vfs();
        let extended_campaign = plan.is_on("h1-campaign-cache")
            && installed.off_reason("h1-campaign-cache").is_none();
        let external_live = _live.clone().filter(|hle| {
            !host.embedded_live && hle.config().server_url.is_some()
        });
        let rebind: agent::sim::MapRebind = Box::new(move |map, scenario, file, cancelled| {
            reload_mount.map_err(str::to_owned)?;
            let handle = map_rebind.clone().ok_or("no writable DVD map mount")?;
            let vfs = reload_vfs.clone().ok_or("the DVD is not mounted")?;
            if let Some(hle) = &external_live {
                hle.with_editor_reservation(|| Ok(()))?;
            }
            let (slot, index, source, limits) = {
                let mem = rt.memory();
                let index = mem.u32(map_slots::CURRENT).map_err(|e| e.to_string())?;
                let source = map_slots::current_source(&*mem)
                    .ok_or("the current cache slot has no known map source")?;
                let slot = map_override::reload_slot(&source, scenario)?;
                map_slots::can_reload(&*mem, &source)?;
                (slot, index, source, campaign_cache::reload_limits(&mem, map, extended_campaign)?)
            };
            let attached = handle.attach(&mut vfs.lock().unwrap_or_else(|e| e.into_inner()))?;
            if attached && let Some(hle) = &external_live {
                let mount = handle.clone();
                hle.set_logon_map_check(Box::new(move || {
                    if !mount.blocks_external_live() {
                        live_hle::status::OK
                    } else if mount.has_ever_bound() {
                        live_hle::status::CANNOT_ACCESS_SERVICE
                    } else {
                        live_hle::status::TASK_BUSY
                    }
                }));
            }
            let rt = Arc::clone(&rt);
            let external_live = external_live.clone();
            let (map, file) = (map.to_owned(), file.to_owned());
            map_override::prepare_on_worker(move || {
                handle.prepare_cancellable(&slot, &map, &file, limits.0, limits.1, cancelled).map(|prepared| {
                    agent::sim::PreparedMapReload::new(prepared.checksum(), move || {
                        let reserve = || {
                            let dvd = vfs.lock().unwrap_or_else(|e| e.into_inner());
                            prepared.reserve(&dvd)
                        };
                        let reserved = match external_live {
                            Some(hle) => hle.with_editor_reservation(reserve)?,
                            None => reserve()?,
                        };
                        let (mut keys, retired) = {
                            let mut mem = rt.memory();
                            if mem.u32(map_slots::CURRENT).ok() != Some(index)
                                || campaign_cache::reload_limits(&mem, &map, extended_campaign)? != limits
                            {
                                return Err("the active map cache slot or its capacity changed".into());
                            }
                            let mut keys = map_slots::reload_keys(&*mem, &source)?;
                            let retired = keys.retire(&mut *mem);
                            (keys, retired)
                        };
                        let runtime = Arc::downgrade(&rt);
                        Ok(agent::sim::ReservedMapReload::new(
                            move || reserved.commit(),
                            move || match runtime.upgrade() {
                                Some(rt) => keys.restore(&mut *rt.memory()),
                                None => Ok(()),
                            },
                        ).with_retirement(retired))
                    })
                })
            })
        });
        a.install_editor_methods(
            &plan, &installed, &loaded.runtime, editor_hooks_on, recording_or_playback,
            skulls.clone(), core.editor_sources.clone(), rebind,
            theater::autorecord::stopper(film.as_ref(), "an editor changed the game", true),
        );
        if let Some(why) = installed.off_reason("agent") {
            a.agent.set_no_tick_hook();
            eprintln!("agent: answering between slices only, no pausing: {why}");
        }
    }
    #[cfg(feature = "gpu")]
    if world_line_capture_requested && let Some(link) = &editor_link {
        note_editor_scene_install(link, &installed);
    }
    if verbose_plan {
        if let Some(store) = host.settings.as_ref().filter(|_| skulls.is_some()) {
            eprintln!("{}", skulls::report(&plan, &installed, store));
        }
        if skulls_mp_on {
            eprintln!("{}", skulls_mp::report(&plan, &installed));
        }
        if skulls_custom_on {
            eprintln!("{}", skulls_custom::report(&plan, &installed));
        }
        if health_on {
            eprintln!("{}", health::report(&plan, &installed));
        }
    }
    if let (Some(h), Some(why)) = (harness.as_mut(), installed.off_reason("lockstep")) {
        eprintln!("lockstep: hashing presents only: {why}");
        h.set_tick_note(why.to_owned());
    }
    let tr = if host_light {
        None
    } else {
        Some(trace::install_bounded(
            &loaded.runtime,
            opts.trace.as_deref(),
            host.trace_limit.unwrap_or(usize::MAX),
        )?)
    };
    let mut m = kernel::Machine::new(
        Arc::clone(&loaded.runtime),
        Arc::clone(&k),
        loaded.cpu,
        loaded.stack_bottom,
        loaded.stack_top,
    )
    .context("creating the entry thread")?;
    m.max_transfers_per_slice = opts.transfers_per_slice;
    // Shared profile. Training can run unpaced while a LAN host sleeps
    // on the normal logical deadlines.
    m.pacer = if let Some(sim) = &sim_mode {
        sim.pacer()
    } else if host_light || std::env::var_os("H2_BOOT_REALTIME").is_some() {
        Some(kernel::pacer::Pacer::real_time())
    } else {
        None
    };
    if coalesce_host_ticks
        && let Some(pacer) = m.pacer.as_mut()
        && pacer.ticks_per_second == kernel::clock::Clock::FREQUENCY
    {
        let phase = std::env::var_os("H2_HOST_TICK_PHASE_NS")
            .map(|value| value.to_string_lossy().parse::<u64>())
            .transpose()
            .context("H2_HOST_TICK_PHASE_NS must be an unsigned integer")?
            .unwrap_or(0);
        let live = _live.clone();
        pacer.coalesce_host_ticks(phase, move |timeout| {
            live.as_ref().is_some_and(|hle| hle.wait_for_network_input(timeout))
        });
    }
    eprintln!("entry {:#010x}, running", loaded.cpu.eip);
    let t = Instant::now();
    // One slice at a time so XAPI's input pump runs between slices
    // (docs/runtime/xapi.md "Input behavior and sources"); no guard is held here.
    let mut outcome = kernel::RunOutcome::SliceLimit;
    let mut host_stop = false;
    let mut input_errors = 0u64;
    let mut audio_errors: Vec<String> = Vec::new();
    // Logical time added for pending audio alone since a thread last ran.
    let mut audio_idle = 0u64;
    // H2_AUDIO_STATS=1: a line of audio health every few seconds.
    #[cfg(feature = "local-io")]
    let mut audio_report = ds
        .as_ref()
        .and_then(|_| dsound_glue::AudioReport::from_env());
    // Machine snapshots (snapshot_glue.rs, docs/runtime/snapshots.md):
    // what holds state outside guest RAM, for `state.*` and the probe.
    let snap_host = snapshot_glue::Host {
        editor_agent: agent.as_ref().map(|a| &a.agent),
        rt: &loaded.runtime,
        audio: ds.as_ref(),
        xapi: xa.as_ref(),
        bink: bink.as_deref(),
        live: _live.as_deref(),
    };
    let mut rebuilders = kernel::snapshot::Rebuilders::new();
    if let Some(tr) = &tr {
        rebuilders.add(trace::KERNEL_CALL_RETURN, trace::rebuilder(tr));
    }
    // The bounce search forks the machine in memory, and its sections
    // carry the build key (docs/runtime/bounce-search.md, "The fork").
    let bounce_fork = sim_mode
        .as_ref()
        .is_some_and(|s| s.matched().config.bounces.is_some());
    let shelf = if agent.is_some() || bounce_fork {
        let key = snapshot_glue::build_key(running_stamp(), &switch_spec, &plan, &snap_host.names());
        // The bounce search's forks need a build key without a standalone
        // snapshot directory.
        let shelf = snapshot_glue::Shelf::from_env(key.clone()).or_else(|| {
            bounce_fork.then(|| snapshot_glue::Shelf::new(&opts.data_root, key))
        });
        if let (Some(a), Some(shelf)) = (&agent, &shelf) {
            a.agent.set_snapshots(Box::new(shelf.clone()));
        }
        shelf
    } else {
        None
    };
    // H2_HOST_PERF=1 (the Xbox app's default): a line every 100 frames or
    // 5 s of where frame time goes on this host (host_perf.rs); it starts
    // frame stats quietly if nothing else has.
    #[cfg(feature = "gpu")]
    let threaded = d3d_calls.as_ref().is_some_and(render_glue::Installed::has_renderer);
    #[cfg(not(feature = "gpu"))]
    let threaded = false;
    let host_perf = host_perf::start_from_env(threaded);
    // H2_FRAME_STATS=1: frame times split by phase (docs/perf/notes/gameplay-budget.md).
    // `h2 play --frame-stats` may have started them already.
    let frame_stats = timeline::phase::enabled() || timeline::phase::start_from_env();
    let gpu = cfg!(feature = "gpu")
        && !headless;
    if frame_stats && !gpu {
        eprintln!("frame stats: no renderer in this run (headless), so the GPU counters stay 0");
    }
    // H2_PERF_FROM=N (cli built with `--features perf`): open
    // `perf::Session::from_env()` (H2_PROFILE, H2_TIMELINE) at present N;
    // H2_PERF_FROM=level at the first present after the level loaded
    // (`level_boot::loaded_at`). It is finished below, before the module is
    // dropped.
    if let Some(note) = perf_env_note(
        cfg!(feature = "perf"),
        perf_from.is_some(),
        ["H2_TIMELINE", "H2_PROFILE"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty())),
    ) {
        eprintln!("{note}");
    }
    #[cfg(feature = "perf")]
    let mut perf_from = perf_from;
    #[cfg(not(feature = "perf"))]
    let _ = &level_name;
    #[cfg(feature = "perf")]
    let mut perf_session: Option<perf::Session> = None;
    // The sampler's main target (agent glue sets it too).
    #[cfg(feature = "perf")]
    if sampling::seconds_from_env().is_some() {
        sampling::set_game_thread();
    }
    let measure_recording = !replaying && std::env::var("H2_THEATER_MEASURE").as_deref() == Ok("1");
    // Seal the game in play's film even when the loop below exits by error
    // or panic, skipping its teardown (`theater::Session::seal_on_drop`).
    let _film_seal = film.as_ref().and_then(|f| f.seal_on_drop());
    let first_slice = 0;
    let mut next_slice = 0;
    while next_slice < opts.max_slices {
        let slice = next_slice;
        next_slice += 1;
        if let Some(f) = &film {
            f.observe_pause(&loaded.runtime.memory());
            if measure_recording {
                f.measure_recording_pages(&loaded.runtime)?;
            }
            match f.before_slice(&mut m, agent.as_ref(), &loaded.runtime)? {
                theater::SliceAction::Run => {}
                theater::SliceAction::Stop => {
                    host_stop = true;
                    break;
                }
            }
        }
        if stop_at.is_some_and(|n| presents::presents() >= n) {
            break;
        }
        #[cfg(feature = "perf")]
        if let Some(from) = perf_from
            && from.due(presents::presents(), level_boot::loaded_at())
        {
            perf_from = None;
            let n = presents::presents();
            match (from, &level_name) {
                (level_boot::PerfFrom::Level, Some(l)) => {
                    eprintln!("perf: session from present {n}, the first after {l} loaded")
                }
                (level_boot::PerfFrom::Level, None) => {
                    eprintln!("perf: session from present {n}, the first after the level loaded")
                }
                _ => eprintln!("perf: session from present {n}"),
            }
            perf_session = perf::Session::from_env();
            // H2_SAMPLE_SECONDS: the in-process sampler's dump instead
            // (sampling.rs; for the Xbox app, which can't symbolise).
            if let Some(s) = sampling::seconds_from_env() {
                sampling::start_timed(s);
            }
        }
        #[cfg(feature = "perf")]
        sampling::tick();
        #[cfg(feature = "desktop-runtime")]
        if !replaying && let Some(pad) = &pad {
            script.step(slice, pad);
        }
        let ran_before = dsound_glue::guest_slices(&m);
        if slice == first_slice && let Some(f) = &film {
            f.measure_first_slice();
        }
        outcome = m.run(1).context("machine run")?;
        // Every thread waits and nothing is timed: pending audio is a wake
        // source the scheduler does not know about (dsound_glue::wake_idle).
        if matches!(outcome, kernel::RunOutcome::Deadlock { .. })
            && let Some(a) = &ds
        {
            match idle_audio::wake(
                a,
                &k,
                &loaded.runtime,
                &mut audio_idle,
                host.between_slices.as_mut(),
            ) {
                Ok(idle_audio::Next::Retry) => continue,
                Ok(idle_audio::Next::HostStop) => {
                    host_stop = true;
                    break;
                }
                Ok(idle_audio::Next::Deadlock) => {}
                Err(e) => audio_errors.push(format!("slice {slice}: {e}")),
            }
        }
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break;
        }
        // Only a guest thread running refills the audio-only budget, not a
        // slice that just delivered vblank.
        if dsound_glue::ran_guest_thread(&m, ran_before) {
            audio_idle = 0;
        }
        #[cfg(feature = "local-io")]
        if let Some(link) = voice_link.as_mut() {
            link.pump();
        }
        if let Some(lights) = &mut controller_lights {
            lights.sample(&loaded.runtime);
        }
        if let Some(x) = &xa
            && timeline::host::INPUT
                .time(|| {
                    xapi::service_input(
                        &loaded.runtime,
                        x,
                        &mut xapi_glue::KernelEvents(Arc::clone(&k)),
                    )
                })
                .is_err()
        {
            input_errors += 1;
        }
        if let Some(h) = &harness {
            h.after_slice(&loaded.runtime, presents::presents());
        }
        if let Some(a) = &agent {
            a.after_slice(&loaded.runtime, presents::presents());
        }
        // A `state.save` or `state.load` is due: run it here, between
        // slices (the agent's tick observer ended the slice for it).
        if let (Some(a), Some(shelf)) = (&agent, &shelf)
            && a.agent.snapshot_pending()
        {
            let mut machine =
                snap_host.machine(&mut m, &plan, &rebuilders, shelf, presents::presents());
            a.agent.run_snapshots(&mut machine);
        }
        // The bounce search asked for a fork at this tick and its tick
        // observer ended the slice after the tick: save the machine, or
        // put it back as a variant's capture left it.
        if let Some(shelf) = &shelf
            && sim_bounces::fork_pending()
        {
            let forked = {
                let mut machine =
                    snap_host.machine(&mut m, &plan, &rebuilders, shelf, presents::presents());
                sim_bounces::between_slices(&mut machine)
            };
            match forked {
                sim_bounces::Forked::Restored => {
                    plan.restore_presentation_pages(&loaded.runtime, &installed)?;
                    audio_idle = 0;
                }
                sim_bounces::Forked::Broken => {
                    host_stop = true;
                    break;
                }
                sim_bounces::Forked::Nothing | sim_bounces::Forked::Captured => {}
            }
        }
        if let Some(p) = &mut snapshot_probe {
            p.after_slice(&m, &plan, || snap_host.blockers());
        }
        if let Some(a) = &ds
            && let Err(e) = dsound_glue::advance(a, &k)
            && audio_errors.len() < 8
        {
            audio_errors.push(format!("slice {slice}: {e}"));
        }
        #[cfg(feature = "local-io")]
        if let (Some(a), Some(r)) = (&ds, &mut audio_report) {
            r.tick(a);
        }
        if let Some(f) = &mut host.between_slices
            && !f(&k)
        {
            host_stop = true;
            break;
        }
        if !uploading_sim && let Some(f) = &film {
            f.finish(false)?;
        }
        // Sim mode: the match is over (its end, its limits, match.stop).
        if let Some(s) = &sim_mode
            && !replaying && !s.keep_running()
        {
            host_stop = true;
            break;
        }
    }
    drop(controller_lights);
    if let Some(audit) = &audit {
        audit.finish(&loaded.runtime)?;
    }
    if !uploading_sim && let Some(f) = &film {
        if let Some(t) = &_telemetry {
            t.close_film();
        }
        f.finish(true)?;
    }
    // A shared copy-on-write guest RAM base (`state.load` with
    // "cow": true): census the pages the match wrote over it, so the
    // report says how much stayed shared.
    if let Some(s) = &sim_mode {
        let cow = loaded.runtime.memory().cow_census()?.map(|(w, t, runs)| {
            eprintln!(
                "sim: {w} of {t} guest pages written over the shared base ({} run(s))",
                runs.len()
            );
            (w, t)
        });
        let reads = k.vfs().map(|vfs| vfs.lock().unwrap_or_else(|e| e.into_inner()).guest_read_bytes())
            .unwrap_or_default();
        s.note_memory(cow, reads);
        let path_reads = k
            .vfs()
            .map(|vfs| vfs.lock().unwrap_or_else(|e| e.into_inner()).guest_path_reads())
            .unwrap_or_default();
        s.note_lean(&loaded.runtime.memory(), path_reads, &opts.data_root);
    }
    let sim_upload_result = match &sim_mode {
        Some(_) if replaying => Ok(()),
        Some(s) => match s.finish() {
            Ok(report) if uploading_sim => serde_json::to_value(report)
                .map_err(anyhow::Error::from)
                .and_then(|report| {
                    let telemetry = _telemetry.as_ref().expect("upload telemetry installed");
                    if let Some(config) = &film_only_sim {
                        // A tick-limited training job may have no retail GAME_END.
                        // Film ingestion must see the final scoreboard before the
                        // run's separate private performance report is accepted.
                        telemetry.close_film();
                        film.as_ref().context("missing simulation film")?
                            .finish_uploaded_sim(true)?;
                        config.publish_sim_report_after_film(report).map_err(anyhow::Error::msg)
                    } else {
                        telemetry.finish_sim(report).map_err(anyhow::Error::msg)
                    }
                }),
            Err(error) => Err(error),
            Ok(_) => Ok(()),
        },
        None => Ok(()),
    };
    if uploading_sim && (film_only_sim.is_none() || sim_upload_result.is_err()) && let Some(f) = &film {
        f.finish_uploaded_sim(sim_upload_result.is_ok())?;
    }
    if let Some(a) = &ds {
        a.output.halt();
    }
    if let Some(p) = &snapshot_probe {
        p.finish();
    }
    // H2_PGO=gen: the run is over (a present count, the slice limit, the
    // host); the training profile is complete.
    module.write_profile();
    // Fail what's queued and close the socket before the module goes.
    drop(agent);
    if let Some(h) = &harness {
        match h.finish() {
            Ok(line) => eprintln!("{line}"),
            Err(e) => eprintln!("lockstep: {e:#}"),
        }
    }
    let secs = t.elapsed().as_secs_f64();
    #[cfg(feature = "perf")]
    drop(perf_session.take());
    if host_perf {
        host_perf::stop();
    }
    if frame_stats && let Some(report) = timeline::phase::stop() {
        eprint!("{report}");
    }
    let (stop, eip) = match &outcome {
        kernel::RunOutcome::Stopped { thread, transfer } => {
            let th = m.thread(*thread).context("stopped thread")?;
            (
                format!(
                    "thread {thread}: {}",
                    describe(th.ctx.event(transfer.event_id), *thread)
                ),
                th.cpu.eip,
            )
        }
        o => (
            format!("{o:?}"),
            m.threads().next().map_or(0, |t| t.cpu.eip),
        ),
    };
    eprintln!(
        "ran {secs:.2}s, {} slices, {} preemptions",
        m.schedule().len(),
        m.preemptions()
    );
    let fps_from = std::env::var("H2_BOOT_FPS_FROM")
        .ok()
        .and_then(|v| v.parse().ok());
    if let Some(fps) = presents::frame_rate(t, 100, fps_from) {
        eprintln!("{fps}");
    }
    for th in m.threads() {
        let c = &th.cpu;
        eprintln!(
            "thread {}: eip {:#010x} eax {:#x} ecx {:#x} edx {:#x} ebx {:#x} esp {:#x} ebp {:#x} esi {:#x} edi {:#x}",
            th.id, c.eip, c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi
        );
        // EBP chain and raw stack words, for finding the caller path.
        {
            let mem = loaded.runtime.memory();
            let mut bp = c.ebp;
            let mut chain = Vec::new();
            for _ in 0..24 {
                let (Ok(next), Ok(ret)) = (mem.u32(bp), mem.u32(bp.wrapping_add(4))) else {
                    break;
                };
                chain.push(format!("{ret:#x}"));
                if next <= bp {
                    break;
                }
                bp = next;
            }
            eprintln!("  ebp chain (return addresses): {}", chain.join(" <- "));
            let words: Vec<String> = (0..24)
                .filter_map(|i| mem.u32(c.esp.wrapping_add(4 * i)).ok())
                .map(|v| format!("{v:x}"))
                .collect();
            eprintln!("  stack: {}", words.join(" "));
        }
        let tail: Vec<String> = th
            .ctx
            .events()
            .iter()
            .rev()
            .take(6)
            .map(|e| format!("{e:x?}").chars().take(300).collect())
            .collect();
        for e in tail.iter().rev() {
            eprintln!("  event: {e}");
        }
    }
    if let Some(u) = &uses {
        for (o, (name, n)) in u.lock().unwrap().iter() {
            eprintln!("stopgap used: #{o} {name} x{n}");
        }
    }
    if let Some(tr) = &tr {
        trace::flush(tr);
    }
    if let Some(h) = &ds {
        eprintln!("{}", dsound_glue::describe(h));
        if !audio_errors.is_empty() {
            eprintln!("dsound advance errors: {audio_errors:?}");
        }
    }
    if xa.is_some() {
        eprintln!("xapi: registered; input pump errors {input_errors}");
    }
    if let Some(i) = &d3d_calls {
        eprintln!("{}", render_glue::describe(i));
    }
    if let Some(s) = fallback::STATS.lock().unwrap().as_ref() {
        let first: Vec<String> = s.first.iter().map(|f| format!("{:#x}", f.0)).collect();
        fallback::log_cold_summary(s);
        eprintln!(
            "fallback: {} entries, {} instructions interpreted, {} x87 emulated; x87 mix {:?}; from [{}]",
            s.fallbacks,
            s.interpreted,
            s.x87,
            s.x87_mnemonics,
            first.join(" ")
        );
    }
    if let Some(tr) = &tr {
        let t = tr.lock().unwrap();
        eprintln!("{} kernel calls; last 40:", t.total);
        for c in t
            .calls
            .iter()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let args: Vec<String> = c.args.iter().map(|a| format!("{a:x}")).collect();
            eprintln!(
                "  #{:<5} t{} {:>3} {:<30} ret {:#08x} [{}] -> {} {}",
                c.seq,
                c.thread,
                c.ordinal,
                c.name,
                c.ret_pc,
                args.join(" "),
                c.result.map_or("(open)".into(), |r| format!("{r:#x}")),
                c.detail
            );
        }
    }
    let stops: BTreeMap<u32, &str> = k.stop_reasons().into_iter().collect();
    for (o, r) in &stops {
        eprintln!("kernel stop reason #{o}: {r}");
    }
    let outcome = if host_stop {
        "HostStop".to_string()
    } else {
        format!("{outcome:x?}")
    };
    let report = RunReport {
        outcome,
        stop,
        eip,
        kernel_log: k.log(),
    };
    #[cfg(feature = "gpu")]
    drop(native_sources);
    drop(editor_mount);
    drop(m);
    drop(loaded);
    drop(module);
    sim_upload_result.context("finishing the simulation")?;
    Ok(report)
}

/// Audible-audio setup on DirectSound's mixer: the audio switches, then the
/// game extensions that play into it (our medals' announcer lines,
/// medal_announcer.rs; the Pogo Stick's piston sounds, pogo_sound.rs) or
/// meter it (`audio.meter`, mixer_meter.rs).
///
/// The mixer guard never spans a `connect` call: one that locked the mixer
/// again on this thread would hang the boot for good (std mutexes are not
/// re-entrant; cd68951b6 hung every audible boot until the guard was dropped
/// first). `audible_mixer_setup_never_deadlocks` guards it.
fn setup_audible_mixer(mixer: &Arc<Mutex<audio::Mixer>>, switches: &hooks::Switches) {
    {
        let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
        m.set_hrtf(switches.audio_hrtf());
        m.set_distance_reverb(switches.distance_reverb());
    }
    game::medal_announcer::connect(Arc::clone(mixer));
    game::pogo_sound::connect(Arc::clone(mixer));
    game::mixer_meter::connect(Arc::clone(mixer));
}

#[cfg(feature = "gpu")]
fn note_editor_scene_install(link: &agent::editor::EditorLink, installed: &hooks::Installed) {
    if installed.off_reason(render_bridge::scene_step::ID).is_some() {
        link.set_mark_capture_disabled(agent::editor::MarkAvailability::SceneOff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cd68951b6: the audible path locked the mixer and then called
    /// `game::mixer_meter::connect`, which locked the same mutex again, and
    /// every audible boot hung forever. Run the real setup sequence with a
    /// real mixer on a thread with a timeout: a deadlock fails the test
    /// instead of hanging it.
    #[test]
    fn audible_mixer_setup_never_deadlocks() {
        let mixer = Arc::new(Mutex::new(audio::Mixer::new(48_000)));
        let switches = hooks::Switches::default();
        let (done, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            setup_audible_mixer(&mixer, &switches);
            done.send(()).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the audible mixer setup deadlocked");
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn an_install_refused_frame_scene_is_reported_by_editor_marks() {
        struct Game;

        impl agent::Reader for Game {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus::default()
            }

            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(vec![])
            }
        }

        let agent = agent::Agent::new();
        let link = agent::editor::EditorLink::default();
        agent::editor::install_scene(&agent, Some(link.clone()));
        let mut installed = hooks::Installed::default();
        installed.off.push((render_bridge::scene_step::ID, "missing required point".into()));
        note_editor_scene_install(&link, &installed);

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let connection = agent.connect(sender);
        agent.submit(connection, &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "editor.marks",
            "id": 1,
            "params": {"marks": [{"point": [1, 2, 3], "color": [1, 0, 0]}]}
        }).to_string());
        agent.on_tick(&mut Game);
        let reply: serde_json::Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert_eq!(reply["result"]["held"], 1);
        assert_eq!(reply["result"]["drawn"], false);
        assert_eq!(reply["result"]["why"], "frame scene capture is off");
    }

    #[test]
    fn perf_env_note_names_what_writes_nothing() {
        assert!(
            perf_env_note(false, false, true)
                .unwrap()
                .contains("--features perf")
        );
        assert!(
            perf_env_note(false, true, false)
                .unwrap()
                .contains("--features perf")
        );
        assert!(
            perf_env_note(true, false, true)
                .unwrap()
                .contains("H2_PERF_FROM")
        );
        assert_eq!(perf_env_note(true, true, true), None);
        assert_eq!(perf_env_note(false, false, false), None);
        assert_eq!(perf_env_note(true, false, false), None);
    }
}
