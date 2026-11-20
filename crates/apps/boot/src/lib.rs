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
// maps.test.*: test maps from recipes (docs/game/test-maps.md).
pub mod registration;
#[cfg(feature = "test-maps")]
pub mod test_maps_recipes;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// Boots a host film with local HLE responses and an isolated restored disk.
pub fn run_theater(
    xbe: &[u8],
    image: &Path,
    opts: &RunOpts,
    mut host: Host,
    replay: theater::Replay,
) -> Result<RunReport> {
    let mut reader = film::Reader::open(&replay.path)?;
    let lan = LiveConfig {
        enabled: !reader.header.xnet_addr.is_empty(),
        ..LiveConfig::default()
    };
    let mut opts = opts.clone();
    let sim = match theater::Startup::read(&mut reader)? {
        Some(start) => start.restore(&mut opts, &mut host)?,
        None => {
            anyhow::ensure!(reader.header.meta.source == film::Source::Host,
                "v1 local films lack the controller input stream");
            None
        }
    };
    run_configured(xbe, image, &opts, host, (&lan, None), sim.map(|s| (s, None)), Some(replay))
}

fn run_configured(
    xbe_bytes: &[u8],
    dylib: &Path,
    opts: &RunOpts,
    mut host: Host,
    (live, publisher): (&LiveConfig, Option<HostPublisher>),
    sim: Option<(sim_glue::Sim, Option<telemetry::TelemetryConfig>)>,
    replay: Option<theater::Replay>,
) -> Result<RunReport> {
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
    let replaying = replay.is_some();
    let render_every = replay.as_ref().map_or(0, |r| r.render_every);
    if host_audit::module_from_env().is_some() {
        anyhow::ensure!(host_light, "H2_HOST_MODULE requires the light host profile");
    }
    if host.host_light {
        anyhow::ensure!(live.enabled && sim_mode.is_none(),
            "--host-light serves the network lobby; unset H2_SIM");
        anyhow::ensure!(
            host.level.is_none() && std::env::var_os("H2_LEVEL").is_none_or(|v| v.is_empty()),
            "light host starts in the network lobby; unset H2_LEVEL"
        );
    }
    #[cfg(feature = "gpu")]
    let host_graphics = (host.gfx.is_some() && !(replaying && render_every != 0))
        || host.gfx_view.is_some()
        || host.render_thread;
    #[cfg(not(feature = "gpu"))]
    let host_graphics = false;
    if host_light || sim_mode.is_some() {
        anyhow::ensure!(
            !host_graphics && std::env::var_os("H2_BOOT_GFX").is_none(),
            "light host and sim require a headless backend (unset H2_BOOT_GFX)"
        );
        anyhow::ensure!(
            opts.d3d && opts.bink && opts.dsound && opts.xapi,
            "light host and sim require the guest D3D, Bink, DirectSound and XAPI shims"
        );
    }
    bots::vis::set_light_index_budget(host_light);
    let bots = opts.xapi && (!host_light || sim_mode.is_some() || host.host_bots);
    d3d_diag::keep_present_history(!host_light);
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
            r.aliases.push(d3d_glue::WC_BASE | r.physical.start);
        }
    }
    let mut loaded = runtime::load_xbe_with(xbe_bytes, &options).context("mapping the XBE")?;
    if opts.stopgaps {
        d3d_glue::map_reserved_write_combined(&loaded.runtime)?;
    }
    for n in &loaded.permission_notes {
        log::note(&format!("page {:#x} widened to {:?}", n.page, n.effective));
    }
    let t = Instant::now();
    // Shadow pages: trace 0xa0ff0000, d3d 0xa0ff2000,
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
    let _live = if let Some(r) = &replay {
        let header = film::Reader::open(&r.path)?.header;
        if header.xnet_addr.is_empty() {
            None
        } else {
            let network = live_hle::NetworkConfig::new(header.xnet_addr.parse()?);
            live_glue::install_film(&loaded.runtime, &k, network, header.live_service)?
        }
    } else if let Some(net) = &sim_network {
        let lan = LiveConfig {
            enabled: true,
            ..Default::default()
        };
        live_glue::install_with_network(&loaded.runtime, &k, &lan, net.config.clone(), None)?
    } else {
        live_glue::install_configured(&loaded.runtime, &k, live, None, map_gate.clone(), publisher)?
    };
    let film_path = host.film_out.take().or_else(|| {
        std::env::var_os("H2_FILM_OUT").filter(|p| !p.is_empty()).map(PathBuf::from)
    });
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
        if let (Some(hle), Some(session)) = (&_live, &session) {
            hle.set_film_tap(session.clone());
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
    let mut features = hooks::Registry::new(registration::profile_switches(switches, host_light));
    // Our text on the cannot-connect dialog when Live is off or its server
    // is unreachable (docs/live/live.md, "Client HLE: Live not set up").
    live_notice::install(&loaded.runtime, live).context("installing the Live notice")?;
    std::fs::create_dir_all(&opts.data_root)?;
    let mut cfg = vfs::Config::new(&opts.data_root, x.certificate.title_id);
    cfg.mmap_disc = host_light;
    cfg.map_guest_reads = options.map_guest_reads;
    if host_light {
        cfg.server_maps = std::env::var_os("H2_SERVER_MAP_DIR")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        // Sim stores: the retail maps' bytes a recording saw the guest
        // read, served instead of the disc (crates/runtime/vfs/src/sim_store.rs).
        cfg.sim_store = std::env::var_os(vfs::sim_store::STORE_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
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
    // Explicit editor maps still use H2_MAP_DIR. Installed packages use
    // their independent content offers (docs/game/maps.md).
    let maps = map_override::dir();
    campaign_cache::configure(
        &mut features, &loaded.runtime, &mut cfg, &campaign_maps, maps.as_deref(),
    )?;
    let packages = map_override::packages_dir(&opts.data_root);
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
    // Every font gets the bot mark's gear (docs/game/bots.md, "Clanker appearance and Live player options"), after
    // the icon sets.
    #[cfg(feature = "desktop-runtime")]
    let fonts = vfs::Chain(
        icons.as_ref().map(|i| i.font_rewrite()),
        bot_mark::GearRewrite,
    );
    #[cfg(not(feature = "desktop-runtime"))]
    let fonts = bot_mark::GearRewrite;
    let rewrites = Some(vfs::Chain(fonts, cards));
    let mounted = map_override::mount(
        cfg.clone(),
        &opts.disc,
        rewrites,
        maps.as_deref(),
        &campaign_maps,
    );
    #[cfg(feature = "desktop-runtime")]
    if let Some(i) = &icons {
        icons::install(&loaded.runtime, Arc::clone(i))?;
    }
    let v = match mounted {
        Ok(v) => v,
        Err(e) => {
            // A raw XBE (or a file that is not an XISO) still boots, with no
            // DVD mounted: the first map open then fails in the guest.
            eprintln!(
                "warning: {} is not a mountable XISO ({e:?}); running with no DVD",
                opts.disc.display()
            );
            vfs::Vfs::with_storage(cfg).map_err(|e| anyhow::anyhow!("vfs: {e:?}"))?
        }
    };
    k.set_vfs(v).context("attaching the VFS")?;
    if let (Some(ready), Some(vfs)) = (host.on_vfs.take(), k.vfs()) {
        ready(&vfs);
    }
    // Live downloads publish their offers before the join is handed to the
    // game. The next main-menu tick refreshes the in-memory map list.
    if let (Some(vfs), Some(gate)) = (k.vfs(), &map_gate) {
        let disc = opts.disc.clone();
        let maps = new_maps::mounted_packages();
        let variants = h1_variants::mp_variants();
        gate.on_prepare(Box::new(move |id| {
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
        Some(store) => Some(Arc::new(
            video_screen::model(store, video_screen::Switches::PLATFORM)
                .map_err(anyhow::Error::msg)?,
        )),
        None => None,
    };
    // More than one campaign (docs/halo1/campaign-picker.md): campaigns
    // installed under <data>/campaigns get their level lists and the
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
    if let Some(session) = &film {
        session.record_startup(opts, &host, level.as_ref(), sim_mode.as_ref());
    }
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
    let film_dir = || {
        std::env::var_os("H2_FILM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| opts.data_root.join("films"))
    };
    let film_out = {
        let parent = dylib.parent().unwrap_or(Path::new("."));
        if tier == tier::Tier::Fast { parent.parent().unwrap_or(parent) } else { parent }
    };
    let film_stamps = || theater::playback_images(xbe_bytes, film_out)
        .into_iter().map(|(_, stamp)| stamp).collect::<Vec<_>>();
    let theater = if let Some((theater, open)) = host.theater.take() {
        theater.install(
            &mut features,
            &film_dir(),
            film_stamps(),
            live,
            _live.clone(),
            open,
        )?;
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
    )?;
    let (skulls_on, skulls_mp_on, skulls_custom_on, health_on) = (
        core.skulls_on,
        core.skulls_mp_on,
        core.skulls_custom_on,
        core.health_on,
    );
    // The bots' nav meshes, built in the background where missing or stale
    // (docs/game/bots.md §10 stage 2); H2_BOTS_NAV_AUTO=0 turns it off.
    // Sim mode never builds them: a build on a thread is at the wall
    // clock's mercy (docs/runtime/sim.md §8).
    if let (true, Some(vfs), None) = (core.bots_on, k.vfs(), &sim_mode) {
        bots_nav_auto::start(&opts.data_root, &opts.disc, vfs, level_name.clone(), host_light);
    }
    if let Some(s) = &sim_mode {
        if !core.bots_on {
            anyhow::bail!("sim: the bots are off (H2_FEATURES or no XAPI); a match needs them");
        }
        s.add_bots()?;
    }
    let _telemetry = core.telemetry;
    if let Some(f) = &film {
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
        t.set_map_id_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let id = map_override::map_id(&mut v, map)?;
            Some((id, v.content_base_name(map)))
        }));
    }
    // A matchmaking game's record names its playlist and Live session
    // (docs/live/live-matchmaking.md §8.3, §8.4): the server fills in the
    // hopper's title and finds the arbitration round by the session. A
    // clanker hopper's game is a Clankermatch (docs/game/bots.md §8.2).
    if let (Some(t), Some(hle)) = (&_telemetry, &_live) {
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
        hle.set_map_probe(Some(Arc::new(move |g: &runtime::Guest| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            map_override::lobby_map_id(&mut v, g)
        })));
    }
    if let (Some(t), Some(vfs)) = (&_telemetry, k.vfs()) {
        t.set_map_locator(Box::new(move |map| {
            let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
            let file = map_override::map_file(&mut v, map)?;
            Some(telemetry::MapFile {
                path: file.path,
                offset: file.offset,
                len: file.len,
            })
        }));
    }
    // Sample the render thread too when profiling.
    #[cfg(all(feature = "perf", feature = "gpu"))]
    let _ = d3d::RENDER_THREAD_START.set(|| perf::register_current_thread("render"));
    // In-between frames at the display rate (docs/game/high-framerate.md,
    // "Phase 2" and "E1"): `impl.render.interpolate` or
    // `impl.render.reproject`, render thread only; in a window, or on the
    // headless GPU backend (`H2_BOOT_GFX`, paced on the wall clock).
    #[cfg(feature = "gpu")]
    let interp = if opts.d3d {
        d3d_glue::interp_render_thread(
            host.gfx.is_some(),
            host.render_thread,
            std::env::var_os("H2_BOOT_GFX").is_some(),
            d3d_glue::render_thread_requested(),
        )
        .and_then(|render_thread| d3d_glue::interpolation(host.settings.as_ref(), render_thread))
    } else {
        None
    };
    // No window: a headless run, which skips the movies by default.
    #[cfg(feature = "gpu")]
    let headless = host.gfx.is_none();
    #[cfg(not(feature = "gpu"))]
    let headless = true;
    #[cfg(feature = "gpu")]
    if opts.d3d && host.gfx_view.is_none() && interp.is_none() && let Some(b) = &mut host.gfx {
        native_shadows_glue::configure_single(b);
    }
    let render_hooks = film.as_ref()
        .map_or_else(run_plan::compiled_render_hooks, |s| s.render_hooks());
    #[cfg(feature = "gpu")]
    let mut d3d_calls = match (opts.d3d, host.gfx.take()) {
        (false, _) => None,
        (true, Some(b)) => Some(match host.gfx_view.take() {
            Some((view, scale)) => {
                let single = d3d_glue::single_copy(host.settings.as_deref())?;
                let mut b = d3d::ScaledBackend::with_single_copy(b, view, scale, single).map_err(anyhow::Error::msg)?;
                eprintln!("d3d: rendering at {scale}x; single copy {single}");
                // The Video screen's Resolution row changes the scale at the
                // next frame boundary (docs/graphics/high-resolution.md).
                // The Lighting row (docs/graphics/e3-lighting-setting.md)
                // changes the view's shadows there too.
                if let Some(store) = &host.settings {
                    let wanted = store.handle::<u32>(setting_defs::RENDER_SCALE.key);
                    b.follow_scale(move || wanted.get());
                    let lighting = store.handle::<u32>(setting_defs::LIGHTING.key);
                    b.set_lighting(setting_defs::lighting_of(lighting.get()));
                    b.follow_lighting(move || setting_defs::lighting_of(lighting.get()));
                }
                let report = Some(d3d_glue::scale_report(&mut b));
                if host.render_thread {
                    d3d_glue::install_threaded_for_profile(&loaded.runtime, b, report, interp, render_hooks)?
                } else {
                    d3d_glue::install_for_profile(&loaded.runtime, b, report, render_hooks)?
                }
            }
            None if host.render_thread => {
                d3d_glue::install_threaded_for_profile(&loaded.runtime, b, None, interp, render_hooks)?
            }
            None => d3d_glue::install_for_profile(&loaded.runtime, b, None, render_hooks)?,
        }),
        (true, None) if host_light => Some(d3d_glue::install_light_for_profile(&loaded.runtime, render_hooks)?),
        (true, None) => Some(d3d_glue::install_headless_for_profile(
            &loaded.runtime, interp, render_hooks, host.settings.as_deref(),
        )?),
    };
    #[cfg(all(not(feature = "gpu"), feature = "desktop-runtime"))]
    let mut d3d_calls = opts.d3d.then(|| {
        if host_light {
            d3d_glue::install_light_for_profile(&loaded.runtime, render_hooks)
        } else {
            d3d_glue::install_headless_cpu_for_profile(&loaded.runtime, render_hooks)
        }
    }).transpose()?;
    #[cfg(not(feature = "desktop-runtime"))]
    let mut d3d_calls = Some(d3d_glue::install_light_for_profile(&loaded.runtime, render_hooks)?);
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
    if d3d_calls.is_some() && uses.is_some() {
        d3d_glue::install_back_surfaces(&loaded.runtime)?;
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
    let ds = if opts.dsound && clock_only_audio {
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
    if let Some(audio) = &ds {
        let mixer = audio.mixer();
        let mut mixer = mixer.lock().unwrap_or_else(|e| e.into_inner());
        if clock_only_audio {
            mixer.set_packet_clock(
                (host_light || sim_mode.is_some())
                    && features
                        .switches()
                        .implementation("audio.packet-clock", true),
            );
        } else {
            mixer.set_hrtf(features.switches().audio_hrtf());
            mixer.set_distance_reverb(features.switches().distance_reverb());
        }
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
    if let Some(keyboard) = pad.as_ref() {
        keyboard_glue::install_with_tape(&loaded.runtime, keyboard.clone(), keyboard_glue::PUMP,
            film.as_ref().and_then(|session| session.keyboard_tape()))
            .context("installing host keyboard entry")?;
    }
    #[cfg(feature = "desktop-runtime")]
    let script = xapi_glue::InputScript::from_env();
    // H2_LOCKSTEP / H2_INPUT_RECORD / H2_INPUT_REPLAY (lockstep_glue.rs).
    let lockstep_config = lockstep_glue::Config::from_env();
    let lockstep_on = lockstep_config.enabled();
    // H2_AGENT=<socket>: the agent API (agent_glue.rs, docs/runtime/agent-api.md);
    // the debug menu talks to a local one with no socket.
    // With none set, a game that asks for it opens `<pid>.sock` in the agents
    // directory for editors to find (`H2_AGENT=off` keeps it closed).
    let agent_source = match agent_glue::socket_from_env()
        .or_else(|| host.discoverable_agent.then(agent_glue::discoverable_socket).flatten())
    {
        Some(path) => registration::AgentSource::Socket(path),
        None if film.is_some() => registration::AgentSource::Recording,
        None if host.debug_menu.is_some() || theater.is_some() => registration::AgentSource::Local,
        None => registration::AgentSource::None,
    };
    let watch_overrides = agent_glue::socket_from_env().is_some();
    let pipeline = registration::pipeline_for(
        d3d_glue::pipeline_render(d3d_calls.as_ref()),
        lockstep_on,
        host_light,
    );
    #[cfg(feature = "desktop-runtime")]
    render_probe::set_pipeline(pipeline.clone());
    let coop_probe = coop_probe::parse(|k| std::env::var(k).ok()).map_err(anyhow::Error::msg)?;
    #[cfg(feature = "gpu")]
    native_map::set_vfs(k.vfs());
    // The lockstep harness, the agent, our cheats, the render features and
    // the probes (registration.rs, the rest of the plan's features).
    let late = registration::register_late_for_profile(
        &mut features,
        registration::LateInputs {
            rt: &loaded.runtime,
            lockstep: lockstep_config,
            agent: agent_source,
            allow_bot_add: !host_light || core.bots_on,
            xa: xa.as_ref(),
            pipeline,
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
    let agent = late.agent;
    if let (Some(theater), Some(a)) = (&theater, &agent) {
        theater.connect(&a.agent);
    }
    #[cfg(feature = "local-io")]
    if let (Some(a), Some(audio)) = (&agent, &ds) {
        dsound_glue::add_agent_methods(&a.agent, audio.mixer());
    }
    let mut snapshot_probe = late.snapshot_probe;
    // edit.* and render.* on the agent (hot_glue.rs, docs/graphics/hot-iteration.md).
    if let Some(a) = &agent {
        let maps = || {
            k.vfs().map(|vfs| -> hot_glue::MapLocator {
                Box::new(move |map| {
                    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
                    let file = map_override::map_file(&mut v, map)?;
                    Some(telemetry::MapFile {
                        path: file.path,
                        offset: file.offset,
                        len: file.len,
                    })
                })
            })
        };
        if !replaying
            && !host.embedded_live
            && _live.as_ref().is_some_and(|h| h.config().server_url.is_some())
        {
            a.agent.set_live_session_probe(agent_glue::remote_live_game);
        }
        hot_glue::install(&a.agent, &loaded.runtime, maps(), watch_overrides);
        ui_stack::install(&a.agent, core.ui_navigation);
        // profile.faults: the fault census (agent_glue.rs).
        agent_glue::add_runtime_methods(&a.agent, &loaded.runtime);
        // sim.give and sim.spawn find tags as edit.* does (agent_glue.rs).
        if let Some(sim) = &a.sim {
            sim.set_tags(Box::new(hot_glue::map_tags(&loaded.runtime, maps())));
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
    // The debug menu's host methods and connection (debug_menu_glue.rs).
    if let Some(a) = &agent {
        debug_menu_glue::install(
            host.debug_menu.as_ref(),
            host_pads,
            &a.agent,
            host.settings.clone(),
            host.perf_overlay.clone(),
        );
    }
    if sim_mode.is_some() || host_light {
        features
            .add(&sim_bridge::sim_native::SimNative::new(&loaded.runtime))
            .map_err(anyhow::Error::msg)?;
    }
    registration::register_viewer_skips(&mut features, host_light)?;
    if host_light {
        let frame = match &sim_mode {
            Some(sim) => sim.frame().clone(),
            // A playerless v2 film presents a separate viewer. Its recorded
            // guest skipped every world render, including the shared RNG draws.
            None => sim_bridge::sim_frame::SimFrame::new(
                if film.as_ref().is_some_and(|s| s.has_engine_track()) { 0 } else { render_every }
            ).on_skip(d3d_diag::note_skipped_present),
        };
        anyhow::ensure!(
            features.add(&frame).map_err(anyhow::Error::msg)?,
            "light host requires sim-frame (H2_FEATURES)"
        );
    }
    if let Some(a) = &agent {
        host_metrics::install(&mut features, &a.agent, host_light)?;
        host_lobby::install(&mut features, &a.agent, host_light, opts, _live.clone())?;
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
        s.install_with_film(&mut features, playback)?;
        if let Some(a) = &agent {
            s.add_methods(&a.agent);
        }
    }
    if let Some(film) = &film {
        if let Some(x) = &xa {
            film.install_local_pads(x)?;
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
    if !replaying && let Some(a) = &agent {
        theater::library_methods(
            &a.agent,
            film_dir(),
            opts.disc.clone(),
            film_out.to_owned(),
            opts.data_root.clone(),
            film_stamps(),
        );
    }
    // Part of a machine snapshot's build key (snapshot_glue.rs).
    let switch_spec = features.switches().spec();
    let plan = features.build(&hooks::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        anyhow::anyhow!("feature plan refused: {}", c.join("; "))
    })?;
    let verbose_plan = hooks::plan_verbose();
    let installed = plan
        .install(&loaded.runtime)
        .context("installing the feature plan")?;
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
        && let Some(why) = installed.off_reason("theater-film")
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
        a.set_plan(&plan, &installed);
        if let Some(why) = installed.off_reason("agent") {
            a.agent.set_no_tick_hook();
            eprintln!("agent: answering between slices only, no pausing: {why}");
        }
    }
    if verbose_plan {
        if let Some(store) = host.settings.as_ref().filter(|_| skulls_on) {
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
    let bink = if opts.bink {
        // A lockstep run compares schedules and movie pacing follows the
        // wall clock, so it skips them; so, by default, do level boots and
        // headless runs.
        let why_off = if host_light {
            Some("light host".into())
        } else {
            movies_glue::why_off(&opts.movies, lockstep_on, level_name.as_deref(), headless)
        };
        let (mode, hle) = movies_glue::install(
            &loaded.runtime,
            &opts.movies,
            &opts.disc,
            why_off.as_deref(),
            #[cfg(feature = "local-io")]
            ds.as_ref()
                .filter(|_| !clock_only_audio)
                .map(dsound_glue::Audio::mixer),
            #[cfg(not(feature = "local-io"))]
            None,
        );
        if let Some(session) = &film {
            session.record_movies(opts, mode);
        }
        Some(hle)
    } else {
        eprintln!("movies: off (--no-bink: the original player runs)");
        None
    };
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
        rt: &loaded.runtime,
        d3d: d3d_calls.as_ref().map(|i| &*i.snapshot),
        audio: ds.as_ref(),
        xapi: xa.as_ref(),
        bink: bink.as_deref(),
        live: _live.as_deref(),
    };
    let mut rebuilders = kernel::snapshot::Rebuilders::new();
    if let Some(tr) = &tr {
        rebuilders.add(trace::KERNEL_CALL_RETURN, trace::rebuilder(tr));
    }
    let shelf = if agent.is_some() || film.is_some() {
        let key = snapshot_glue::build_key(running_stamp(), &switch_spec, &plan, &snap_host.names());
        // A film embeds snapshots. Its build key is needed even when the
        // user has no standalone snapshot directory.
        let shelf = snapshot_glue::Shelf::from_env(key.clone()).or_else(|| {
            film.as_ref().map(|_| snapshot_glue::Shelf::new(&opts.data_root, key))
        });
        if let (Some(a), Some(shelf)) = (&agent, &shelf) {
            a.agent.set_snapshots(Box::new(shelf.clone()));
        }
        shelf
    } else {
        None
    };
    // H2_BOOT_STOP_AT_PRESENT=N: end the run once N presents are out.
    let stop_at: Option<u64> = std::env::var("H2_BOOT_STOP_AT_PRESENT")
        .ok()
        .and_then(|v| v.parse().ok());
    // H2_HOST_PERF=1 (the Xbox app's default): a line every 100 frames or
    // 5 s of where frame time goes on this host (host_perf.rs); it starts
    // frame stats quietly if nothing else has.
    #[cfg(feature = "gpu")]
    let threaded = host.render_thread || d3d_glue::render_thread_requested();
    #[cfg(not(feature = "gpu"))]
    let threaded = false;
    let host_perf = host_perf::start_from_env(threaded);
    // H2_FRAME_STATS=1: frame times split by phase (docs/perf/notes/gameplay-budget.md).
    // `h2 play --frame-stats` may have started them already.
    let frame_stats = timeline::phase::enabled() || timeline::phase::start_from_env();
    let gpu = cfg!(feature = "gpu")
        && (!headless || std::env::var_os("H2_BOOT_GFX").is_some());
    if let Some(note) = d3d_glue::frame_stats_gpu_note(frame_stats && d3d_calls.is_some(), gpu) {
        eprintln!("{note}");
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
    let mut film_slices = if let Some(f) = &film
        && let Some(frame) = f.take_keyframe()
    {
        let shelf = shelf.as_ref().context("film keyframe requires a snapshot build key")?;
        let mut machine = snap_host.machine(&mut m, &plan, &rebuilders, shelf, d3d_diag::presents());
        f.restore_keyframe(frame, &mut machine, agent.as_ref())?
    } else {
        0
    };
    for slice in film_slices..opts.max_slices {
        if let Some(f) = &film {
            f.observe_pause(&loaded.runtime.memory());
            if replaying {
                if let Some(shelf) = &shelf {
                    f.replay_device_barrier(slice, &snap_host, shelf)?;
                }
            } else {
                if let Some(shelf) = &shelf && f.keyframe_due() {
                    let lobby = agent.as_ref().map_or(0, |a| a.agent.film_lobby_boundary());
                    let mut machine = snap_host.machine(
                        &mut m, &plan, &rebuilders, shelf, d3d_diag::presents(),
                    );
                    f.try_keyframe(slice, lobby, &mut machine);
                }
            }
            if !f.before_slice(slice, &mut m, agent.as_ref(), &loaded.runtime)? {
                host_stop = true;
                break;
            }
            film_slices = slice + 1;
        }
        if stop_at.is_some_and(|n| d3d_diag::presents() >= n) {
            break;
        }
        #[cfg(feature = "perf")]
        if let Some(from) = perf_from
            && from.due(d3d_diag::presents(), level_boot::loaded_at())
        {
            perf_from = None;
            let n = d3d_diag::presents();
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
        outcome = m.run(1).context("machine run")?;
        // Every thread waits and nothing is timed: pending audio is a wake
        // source the scheduler does not know about (dsound_glue::wake_idle).
        if matches!(outcome, kernel::RunOutcome::Deadlock { .. })
            && let Some(a) = &ds
        {
            match dsound_glue::wake_idle(a, &k, &loaded.runtime, &mut audio_idle) {
                Ok(true) => continue,
                Ok(false) => {}
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
            h.after_slice(&loaded.runtime, d3d_diag::presents());
        }
        if let Some(a) = &agent {
            a.after_slice(&loaded.runtime, d3d_diag::presents());
        }
        // A `state.save` or `state.load` is due: run it here, between
        // slices (the agent's tick observer ended the slice for it).
        if let (Some(a), Some(shelf)) = (&agent, &shelf)
            && a.agent.snapshot_pending()
        {
            let mut machine =
                snap_host.machine(&mut m, &plan, &rebuilders, shelf, d3d_diag::presents());
            a.agent.run_snapshots(&mut machine);
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
            f.finish(film_slices, false)?;
        }
        // Sim mode: the match is over (its end, its limits, match.stop).
        if let Some(s) = &sim_mode
            && !replaying && !s.keep_running()
        {
            host_stop = true;
            break;
        }
    }
    if let Some(audit) = &audit {
        audit.finish(&loaded.runtime)?;
    }
    if !uploading_sim && let Some(f) = &film {
        if let Some(t) = &_telemetry {
            t.close_film();
        }
        f.finish(film_slices, true)?;
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
                            .finish_uploaded_sim(film_slices, true)?;
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
        f.finish_uploaded_sim(film_slices, sim_upload_result.is_ok())?;
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
    if let Some(fps) = d3d_diag::frame_rate(t, 100, fps_from) {
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
        eprintln!("{}", d3d_glue::describe(i));
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
    drop(m);
    drop(loaded);
    drop(module);
    sim_upload_result.context("finishing the simulation")?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

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
