//! Launch an isolated film player and reconstruct its guest disk on cold seeks.
mod verify;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(feature = "desktop")]
mod export;

const USAGE: &str = "h2 theater play FILM [--seek TICK] [--render-every N] [--speed 0.25..4] [--disc ISO] [--out IMAGE_DIR] [--data INITIAL_DATA] [--movies DIR] [--socket PATH]\n\
                    h2 theater prepare FILM [--disc ISO] [--out IMAGE_DIR] [--data INITIAL_DATA] [--movies DIR] [--socket PATH]";
#[derive(Clone)]
struct Options {
    film: PathBuf,
    prepare: bool,
    every: u32,
    speed: f64,
    disc: PathBuf,
    out: PathBuf,
    socket: PathBuf,
    data: Option<PathBuf>,
    movies: Option<PathBuf>,
    range: Option<[u64; 2]>,
    seek: Option<u64>,
}
impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let prepare = match args.next().as_deref() {
            Some("play") => false,
            Some("prepare") => true,
            _ => bail!("{USAGE}"),
        };
        let film = PathBuf::from(args.next().context(USAGE)?);
        let mut every = u32::from(!prepare && cfg!(feature = "desktop"));
        let mut data = None;
        let mut movies = None;
        let mut seek = None;
        let mut speed = 1.;
        let mut disc = std::env::var_os("H2_DISC").map(PathBuf::from);
        let mut out = std::env::var_os("H2_IMAGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("h2boot"));
        let mut socket = std::env::var_os("H2_AGENT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if prepare {
                    std::env::temp_dir().join(format!("h2t-{}.sock", live_proto::sign::random_id()))
                } else {
                    agent_proto::cache_dir().join("theater.sock")
                }
            });
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .with_context(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--render-every" if !prepare => every = value.parse()?,
                "--speed" if !prepare => speed = value.parse()?,
                "--seek" if !prepare => seek = Some(value.parse().context("invalid seek tick")?),
                "--data" => data = Some(PathBuf::from(value)),
                "--movies" => movies = Some(PathBuf::from(value)),
                "--disc" => disc = Some(value.into()),
                "--out" => out = value.into(),
                "--socket" => socket = value.into(),
                _ => bail!("unexpected {flag}: {USAGE}"),
            }
        }
        ensure!((0.25..=4.).contains(&speed), "speed must be 0.25 to 4");
        Ok(Self {
            film,
            prepare,
            every,
            speed,
            disc: disc.context("set H2_DISC or pass --disc")?,
            out,
            socket,
            data,
            movies,
            seek,
            range: if prepare {
                None
            } else {
                std::env::var("H2_THEATER_RANGE")
                    .ok()
                    .map(|value| parse_range(&value))
                    .transpose()?
            },
        })
    }
}

fn parse_range(value: &str) -> Result<[u64; 2]> {
    let (start, end) = value
        .split_once(':')
        .context("expected start:end for H2_THEATER_RANGE")?;
    let (start, end) = (start.parse::<u64>()?, end.parse::<u64>()?);
    ensure!(start < end, "invalid film clip range");
    Ok([start, end])
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("h2film-{}", live_proto::sign::random_id()));
        let mut dir = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dir.mode(0o700);
        }
        dir.create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    if args.first().is_some_and(|s| s == "open") {
        let [_, url] = args.as_slice() else {
            bail!("usage: h2 theater open URL");
        };
        let link: live_proto::theater_link::TheaterLink = url.parse()?;
        #[cfg(unix)]
        let socket = crate::agent_cmd::only_game().unwrap_or_else(agent_proto::default_socket);
        #[cfg(not(unix))]
        let socket = agent_proto::default_socket();
        match agent_proto::Client::connect(&socket) {
            Ok(mut client) => {
                client.set_timeout(Some(std::time::Duration::from_secs(60)))?;
                let screen =
                    client.call(agent_proto::theater::OPEN, serde_json::json!({"url":url}))?;
                println!("{screen}");
                return Ok(());
            }
            Err(error) if std::env::var_os(agent_proto::ENV).is_some() => return Err(error.into()),
            Err(_) => {}
        }
        #[cfg(feature = "desktop")]
        {
            boot::theater_ui::external_link(link);
            return crate::play_cmd::run(vec!["--live".into()]);
        }
        #[cfg(not(feature = "desktop"))]
        {
            let _ = link;
            bail!("Open the desktop game to receive this Theater link");
        }
    }
    let opts = Options::parse(args.clone().into_iter())?;
    #[cfg(feature = "desktop")]
    if let Some(job) = std::env::var_os("H2_THEATER_EXPORT_JOB") {
        return export::run(opts, Path::new(&job));
    }
    if let Some(work) = std::env::var_os("H2_THEATER_WORK") {
        return child(opts, Path::new(&work));
    }
    let viewer_data = opts.data.clone().unwrap_or_else(content::store::default_data_root);
    let ring_mib = boot::setting_defs::theater_ring_mib(Some(&viewer_data));
    let mut reader = film::Reader::open(&opts.film)?;
    let header = reader.header.clone();
    ensure!(
        !opts.prepare || header.version >= 2,
        "this film does not have an engine replay track"
    );
    let view = std::env::var("H2_THEATER_VIEW")
        .ok()
        .map(|v| serde_json::from_str::<agent_proto::theater::InitialView>(&v))
        .transpose()?;
    let target = opts
        .seek
        .or_else(|| view.and_then(|v| v.tick.map(u64::from)))
        .unwrap_or_else(|| {
            // A game cut from a session's tape opens at its game's start.
            let opening = || {
                if opts.prepare {
                    0
                } else {
                    boot::theater::autorecord::opening_tick(&header, reader.front.as_ref())
                }
            };
            opts.range.map_or_else(opening, |[start, _]| start)
        });
    if let Some([start, end]) = opts.range {
        ensure!((start..end).contains(&target), "seek outside film clip");
    }
    ensure!(target <= header.meta.length_ticks, "seek beyond film");
    let mut presentation = boot::theater::Presentation::default();
    if let Some(view) = view {
        presentation.start_view(view);
    }
    let mut launch = boot::theater::Restart {
        target,
        from_tick: (opts.seek.is_some() || target > 0).then_some(0),
        source: None,
        speed: opts.speed,
        paused: false,
        presentation,
    };
    let mut retries = 0;
    loop {
        let temp = Temp::new()?;
        let restart = temp.0.join("restart.json");
        serde_json::to_writer(fs::File::create(temp.0.join("launch.json"))?, &launch)?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("theater")
            .args(&args)
            .env("H2_AGENT", &opts.socket)
            .env("H2_THEATER_WORK", &temp.0)
            .env(boot::setting_defs::THEATER_RING_MIB_ENV, ring_mib.to_string())
            .env("H2_HOST_LOBBY", if header.lobby { "1" } else { "0" })
            .env(
                "H2_HOST_LOBBY_MACHINE_START",
                if header.lobby_machine_start { "1" } else { "0" },
            )
            .env("H2_TELEMETRY", "0");
        for key in [
            "H2_FILM_OUT",
            boot::agent_glue::HOST_TCP_ENV,
            "H2_HOST_TOKEN",
            "H2_HOST_LIVE_URL",
            "H2_TUNNEL_LISTEN",
            "H2_TUNNEL_URL",
            "H2_BOOT_GFX",
            "H2_SIM",
            "H2_LEVEL",
            "H2_BOOT_INPUT",
            "H2_INPUT_REPLAY",
            "H2_INPUT_RECORD",
        ] {
            cmd.env_remove(key);
        }
        boot::theater::configure_replay_child(&mut reader, &mut cmd)?;
        let status = cmd.status().context("starting film playback")?;
        ensure!(status.success(), "film playback exited with {status}");
        if retry_seek(&temp.0, &mut launch, &mut retries)? {
            continue;
        }
        retries = 0;
        if !restart.try_exists()? {
            return Ok(());
        }
        launch = serde_json::from_reader(fs::File::open(restart)?)?;
    }
}

fn retry_seek(work: &Path, launch: &mut boot::theater::Restart, retries: &mut u8) -> Result<bool> {
    let retry = work.join("retry-build.json");
    if !retry.try_exists()? {
        return Ok(false);
    }
    ensure!(*retries < 2, "could not prepare a consistent disk for this seek");
    launch.presentation.snapshot_build = Some(serde_json::from_reader(fs::File::open(retry)?)?);
    *retries += 1;
    Ok(true)
}

fn child(opts: Options, work: &Path) -> Result<()> {
    child_with_start(
        opts,
        work,
        serde_json::from_reader(fs::File::open(work.join("launch.json"))?)?,
        #[cfg(feature = "desktop")]
        None,
    )
}

fn child_with_start(
    opts: Options,
    work: &Path,
    launch: boot::theater::Restart,
    #[cfg(feature = "desktop")] capture: Option<std::sync::Arc<boot::theater::ClipCapture>>,
) -> Result<()> {
    #[cfg(feature = "desktop")]
    let media_options = opts.clone();
    let child_started = std::time::Instant::now();
    let mut film = film::Reader::open(&opts.film)?;
    let xbe = boot::read_xbe(&opts.disc)?;
    let xbe_sha256 = film::xbe_digest(&xbe);
    ensure!(
        film.header.xbe_sha256 == xbe_sha256,
        "film requires a different XBE"
    );
    let mut images = Vec::new();
    let mut diagnostics = Vec::new();
    for (path, stamp) in boot::theater::playback_images(&xbe, &opts.out) {
        let header = &film.header;
        match boot::theater::image_match(
            header.version,
            &header.meta.image_stamp,
            &header.runtime_stamp,
            header.guest_image.as_ref(),
            &stamp,
            &xbe_sha256,
        ) {
            Ok(mode) => images.push((path, stamp, mode)),
            Err(e) => diagnostics.push(format!("{}: {e}", path.display())),
        }
    }
    let (image, image_stamp, compatibility) = images
        .into_iter()
        .min_by_key(|(_, _, mode)| *mode != film::image::ImageMatch::Native)
        .with_context(|| {
            if diagnostics.is_empty() {
                "no current module installed for this XBE (check --disc, --out and H2_OPT)"
                    .to_owned()
            } else {
                format!("no compatible film image: {}", diagnostics.join(", "))
            }
        })?;
    if compatibility == film::image::ImageMatch::FromBoot {
        eprintln!("theater: compatible guest image; using this build's local cache");
    }
    ensure!(
        film.header.version >= 2 || film.header.meta.source == film::Source::Host,
        "v1 local films lack the controller input stream"
    );
    let entry = film.machine_section().unique(film::DATA)?.clone();
    let data = work.join("data");
    let viewer_data = opts.data.clone().unwrap_or_else(content::store::default_data_root);
    let films_dir = std::env::var_os("H2_FILM_DIR").map(PathBuf::from)
        .unwrap_or_else(|| viewer_data.join("films"));
    let (settings, problems) = boot::setting_defs::load(&viewer_data, &[]);
    for problem in problems {
        eprintln!("theater: {problem}");
    }
    let cap_bytes = boot::setting_defs::theater_cache_cap_bytes(Some(&settings));
    let delta = if launch.target > 0
        && film.header.version >= 2
        && launch.source != Some(agent_proto::theater::SeekRoute::Boot)
    {
        let mut track = film::blam::TrackReader::open(&opts.film)?;
        if compatibility == film::image::ImageMatch::FromBoot {
            track.disable_embedded_keyframes();
        }
        if cap_bytes > 0
            && let Some((stamp, switches)) = &launch.presentation.snapshot_build
            && stamp == &image_stamp
            && let Err(e) = track.attach_sidecar(&films_dir, stamp, switches)
        {
            eprintln!("theater: could not open sidecar for disk restore: {e:#}");
        }
        track.disk_delta_at(launch.target)?
    } else {
        Vec::new()
    };
    film.read::<film::manifest::Manifest>(&entry)?
        .restore_keyframe(&data, opts.data.as_deref(), &delta)?;
    let verification = Some(verify::mode(&opts, work)?);
    let boot = boot::RunOpts {
        disc: opts.disc,
        data_root: data,
        max_slices: u64::MAX,
        xapi: true,
        transfers_per_slice: film.header.transfers_per_slice,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        movies: opts.movies.map_or(
            boot::movies_glue::Movies::Off,
            boot::movies_glue::Movies::Dir,
        ),
    };
    #[cfg(feature = "desktop")]
    let film_parent = opts.film.parent().unwrap_or(Path::new(".")).to_owned();
    #[cfg(feature = "desktop")]
    let theater_disc = boot.disc.clone();
    let camera = boot::theater::FilmCamera::default();
    #[cfg(feature = "desktop")]
    let clip_marks = launch.presentation.clip_marks;
    #[cfg(feature = "desktop")]
    let starting_seek = (launch.target > 0).then_some(launch.target);
    let replay = boot::theater::Replay {
        verification,
        path: opts.film.clone(),
        cache: Some(boot::theater::ReplayCache {
            films_dir,
            cap_bytes,
            disk_digest: film::xbe_digest(&delta),
        }),
        unpaced: opts.prepare,
        render_every: opts.every,
        speed: launch.speed,
        seek_tick: launch.target,
        from_tick: launch.from_tick,
        seek_source: launch.source,
        paused_at_target: launch.paused,
        presentation: launch.presentation,
        restart: work.join("restart.json"),
        camera: camera.clone(),
        window_closed: None,
    };
    let retry = work.join("retry-build.json");
    let run = move |host, window_closed| -> Result<()> {
        let mut replay = replay;
        replay.window_closed = window_closed;
        let r = boot::run_theater(&xbe, &image, &boot, host, replay, child_started);
        vfs::flush_pending_metadata();
        if let Err(e) = &r
            && let Some(build) = e.downcast_ref::<boot::theater::RetrySeek>()
        {
            serde_json::to_writer(fs::File::create_new(retry)?, &build.0)?;
            return Ok(());
        }
        r.map(|_| ())
    };
    let host_bots = film.header.host_bots;
    let local_gpu = if film.header.meta.source == film::Source::Local {
        let hooks = film.header.render_hooks;
        let startup = boot::theater::Startup::read(&mut film)?.context("missing startup for local film")?;
        Some(startup.local_gpu_draws(hooks))
    } else {
        None
    };
    #[cfg(feature = "desktop")]
    if let Some(capture) = capture {
        let observer = film.header.version >= 2 && local_gpu != Some(true);
        return run(export::host(capture, camera, observer, host_bots)?, None);
    }
    if opts.every == 0 {
        let host = boot::Host { host_light: true, host_bots, ..Default::default() };
        if local_gpu == Some(true) {
            #[cfg(not(feature = "desktop"))]
            bail!("this local film requires an exact offscreen GPU reference; use a desktop build");
            #[cfg(feature = "desktop")]
            {
                let retail: boot::run_opts::NativeSize = std::sync::Arc::new(|| boot::setting_defs::GUEST_FRAME);
                // Keep the drawing hook plan, with the sole target and the lens fixed to retail.
                let host = boot::Host {
                    gfx: Some(boot::d3d_glue::offscreen_film_reference()?),
                    gfx_view: Some(boot::run_opts::DisplayView {
                        backend: None,
                        native_size: retail.clone(),
                        viewer_output: Some(retail),
                        display_size: std::sync::Arc::new(|size| size),
                    }),
                    ..host
                };
                return run(host, None);
            }
        }
        return run(host, None);
    }
    #[cfg(not(feature = "desktop"))]
    bail!("rendered film playback requires the desktop feature");
    #[cfg(feature = "desktop")]
    {
        use crate::play_cmd::theater::{film_entry, overlay};

        let dir = std::env::var_os("H2_FILM_DIR")
            .map(PathBuf::from)
            .or_else(|| opts.data.as_ref().map(|data| data.join("films")))
            .unwrap_or(film_parent);
        let publish_every = if film.header.version == 1 {
            1
        } else {
            opts.every
        };
        // When the recorded guest used a headless device, the independent
        // observer can present the match without changing that device's answers.
        let observer = film.header.version >= 2 && local_gpu != Some(true);
        let mut meta = film_entry(film::LibraryEntry::read(&opts.film)?);
        if let Err(error) = boot::theater_ui::stats::read_playback(&opts.film, &mut meta) {
            eprintln!("theater: reading stats for {}: {error:#}", meta.id);
        }
        let title = meta.map.clone();
        if let Ok(id) = std::env::var("H2_THEATER_FILM_ID") {
            meta.id = id;
        }
        let theater = boot::theater_ui::Theater::playback(
            meta,
            opts.range,
            clip_marks,
            starting_seek,
            opts.socket,
            dir.clone(),
            opts.film,
        )?;
        let (overlay, controller) = overlay(theater.clone(), dir.join("screenshots"), Some(&theater_disc));
        let config = app::AppConfig {
            title: format!("Theater — {}", title),
            audio: false,
            keyboard: false,
            gamepads: true,
            theater: Some(overlay),
            aspect: app::DisplayAspect::Source,
            mouse_capture: false,
            keep_open_on_guest_exit: false,
            ..Default::default()
        };
        let exit = app::run(config, move |mut window| {
            if let Some(input) = window.take_input() {
                controller.attach_input(input);
            }
            let native: boot::run_opts::NativeSize = std::sync::Arc::new(window.native_size());
            let raster = boot::d3d_glue::output_size(Some(&settings), native.clone())?;
            let film_media = export::media(
                media_options,
                dir,
                window.frame_sink(),
                raster.clone(),
            );
            let (gfx, gfx_view, film_view) = if observer {
                let mut sink = window.frame_sink();
                (
                    None,
                    None,
                    Some(boot::theater::HostView::new(
                        window.gfx_context(),
                        raster,
                        window.display_size(),
                        move |view, size| {
                            sink.submit_view(view, size, true)
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        },
                    )),
                )
            } else {
                camera.attach_guest_view(window.gfx_context().device.limits().max_texture_dimension_2d);
                let crop = camera.clone();
                let publish = camera.clone();
                let scale = camera.clone();
                let raster: boot::run_opts::NativeSize =
                    std::sync::Arc::new(move || scale.guest_raster(raster()));
                let present = window.frame_sink().into_filtered_guest_present_hook(
                    publish_every,
                    boot::setting_defs::GUEST_FRAME,
                    move || crop.present_crop(),
                    move |due, full_size| publish.publish_guest_present(due, full_size),
                    move |sink, result| {
                        camera.sample_guest_present(|| {
                            let seq = result.map_err(|e| anyhow::anyhow!("{e}"))?;
                            sink.read_published_rgba(seq).map_err(anyhow::Error::msg)
                        });
                    },
                );
                (
                    Some(d3d::GfxBackend::new(window.gfx_context())),
                    Some(boot::run_opts::DisplayView {
                        backend: Some(d3d::GfxBackend::new(window.gfx_context()).on_present(present)),
                        native_size: native,
                        viewer_output: Some(raster),
                        display_size: std::sync::Arc::new(window.display_size()),
                    }),
                    None,
                )
            };
            let life = window.lifecycle();
            run(
                boot::Host {
                    gfx,
                    gfx_view,
                    film_view,
                    film_media: Some(film_media),
                    theater_playback: Some(theater),
                    between_slices: Some(Box::new(move |_| !window.should_stop())),
                    host_light: true,
                    host_bots,
                    ..Default::default()
                },
                Some(std::sync::Arc::new(move || life.should_stop())),
            )
        })?;
        match exit.guest {
            app::GuestExit::Returned => Ok(()),
            app::GuestExit::Failed(e) | app::GuestExit::Panicked(e) => bail!("{e}"),
            other => bail!("film guest stopped: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theater_clip_range_requires_ordered_ticks() {
        assert_eq!(parse_range("0:30").unwrap(), [0, 30]);
        for invalid in ["0", "7:7", "9:8", "-1:30", "1:2:3"] {
            assert!(parse_range(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn theater_options_require_finite_speed_and_honor_render_zero() {
        let options = |speed: &str| {
            Options::parse(
                [
                    "play",
                    "game.h2film",
                    "--disc",
                    "disc.iso",
                    "--render-every",
                    "0",
                    "--speed",
                    speed,
                ]
                .map(String::from)
                .into_iter(),
            )
        };
        let o = options("0.25").unwrap();
        assert_eq!((o.every, o.speed), (0, 0.25));
        for v in ["NaN", "inf", "0", "4.01"] {
            assert!(options(v).is_err());
        }
    }

    #[test]
    fn prepare_takes_the_whole_film_and_runs_without_a_window() {
        let args = ["prepare", "synthetic.h2film", "--disc", "synthetic.iso"];
        let options = Options::parse(args.map(String::from).into_iter()).unwrap();
        assert!(options.prepare);
        assert_eq!(options.every, 0);
        assert_eq!(options.seek, None);
        assert_eq!(options.range, None);
        for flag in ["--seek", "--speed", "--render-every"] {
            assert!(Options::parse(args.into_iter().chain([flag, "1"]).map(String::from)).is_err());
        }
    }
}
