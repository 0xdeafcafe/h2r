//! Each seek starts a new process so the game, HLE allocators and feature
//! statics all restart. The parent owns the restored disk and the seek reply.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(feature = "desktop")]
mod export;

const USAGE: &str = "h2 theater play FILM [--seek TICK] [--render-every N] [--speed 0.25..4] [--disc ISO] [--out IMAGE_DIR] [--data INITIAL_DATA] [--movies DIR] [--socket PATH]";
#[derive(Clone)]
struct Options {
    film: PathBuf,
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
        ensure!(args.next().as_deref() == Some("play"), "{USAGE}");
        let film = PathBuf::from(args.next().context(USAGE)?);
        let mut every = u32::from(cfg!(feature = "desktop"));
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
            .unwrap_or_else(|| agent_proto::cache_dir().join("theater.sock"));
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .with_context(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--render-every" => every = value.parse()?,
                "--speed" => speed = value.parse()?,
                "--seek" => seek = Some(value.parse().context("invalid seek tick")?),
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
            every,
            speed,
            disc: disc.context("set H2_DISC or pass --disc")?,
            out,
            socket,
            data,
            movies,
            seek,
            range: std::env::var("H2_THEATER_RANGE")
                .ok()
                .map(|value| parse_range(&value))
                .transpose()?,
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
    let opts = Options::parse(args.clone().into_iter())?;
    #[cfg(feature = "desktop")]
    if let Some(job) = std::env::var_os("H2_THEATER_EXPORT_JOB") {
        return export::run(opts, Path::new(&job));
    }
    if let Some(work) = std::env::var_os("H2_THEATER_WORK") {
        return child(opts, Path::new(&work));
    }
    let mut reader = film::Reader::open(&opts.film)?;
    let header = reader.header.clone();
    let mut target = opts
        .seek
        .unwrap_or_else(|| opts.range.map_or(0, |[start, _]| start));
    if let Some([start, end]) = opts.range {
        ensure!((start..end).contains(&target), "seek outside film clip");
    }
    ensure!(target <= header.meta.length_ticks, "seek beyond film");
    let mut speed = opts.speed;
    let mut paused = false;
    let mut presentation = boot::theater::Presentation::default();
    loop {
        let temp = Temp::new()?;
        let restart = temp.0.join("restart.json");
        serde_json::to_writer(fs::File::create(temp.0.join("view.json"))?, &presentation)?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("theater")
            .args(&args)
            .env("H2_AGENT", &opts.socket)
            .env("H2_THEATER_WORK", &temp.0)
            .env("H2_THEATER_TARGET", target.to_string())
            .env("H2_THEATER_SPEED", speed.to_string())
            .env("H2_THEATER_PAUSED", if paused { "1" } else { "0" })
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
        if !restart.try_exists()? {
            return Ok(());
        }
        (target, speed, paused, presentation) = serde_json::from_reader(fs::File::open(restart)?)?;
    }
}

struct Start {
    tick: u64,
    speed: f64,
    paused: bool,
    presentation: boot::theater::Presentation,
}

fn child(opts: Options, work: &Path) -> Result<()> {
    child_with_start(
        opts,
        work,
        Start {
            tick: std::env::var("H2_THEATER_TARGET")?.parse()?,
            speed: std::env::var("H2_THEATER_SPEED")?.parse()?,
            paused: std::env::var("H2_THEATER_PAUSED")? == "1",
            presentation: serde_json::from_reader(fs::File::open(work.join("view.json"))?)?,
        },
        #[cfg(feature = "desktop")]
        None,
    )
}

fn child_with_start(
    opts: Options,
    work: &Path,
    start: Start,
    #[cfg(feature = "desktop")] capture: Option<std::sync::Arc<boot::theater::ClipCapture>>,
) -> Result<()> {
    #[cfg(feature = "desktop")]
    let media_options = opts.clone();
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
    let (image, _stamp, compatibility) = images
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
        eprintln!("theater: compatible guest image; verifying from boot with recorded hashes");
    }
    ensure!(
        film.header.version >= 2 || film.header.meta.source == film::Source::Host,
        "v1 local films lack the controller input stream"
    );
    let entry = film.machine_section().unique(film::DATA)?.clone();
    let data = work.join("data");
    let target = start.tick;
    let delta = if target > 0
        && film.header.version >= 2
        && compatibility == film::image::ImageMatch::Native
    {
        film::blam::TrackReader::open(&opts.film)?.disk_delta_at(target)?
    } else {
        Vec::new()
    };
    film.read::<film::manifest::Manifest>(&entry)?
        .restore_keyframe(&data, opts.data.as_deref(), &delta)?;
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
    let camera = boot::theater::FilmCamera::default();
    let presentation = start.presentation;
    #[cfg(feature = "desktop")]
    let clip_marks = presentation.clip_marks;
    let replay = boot::theater::Replay {
        path: opts.film.clone(),
        render_every: opts.every,
        speed: start.speed,
        seek_tick: target,
        paused_at_target: start.paused,
        presentation,
        restart: work.join("restart.json"),
        camera: camera.clone(),
        window_closed: None,
    };
    let run = move |host, window_closed| -> Result<()> {
        let mut replay = replay;
        replay.window_closed = window_closed;
        let r = boot::run_theater(&xbe, &image, &boot, host, replay);
        vfs::flush_pending_metadata();
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
        let mut meta = film_entry(film::LibraryEntry::read(&opts.film)?, &_stamp);
        let title = meta.map.clone();
        if let Ok(id) = std::env::var("H2_THEATER_FILM_ID") {
            meta.id = id;
        }
        let theater = boot::theater_ui::Theater::playback(
            meta,
            opts.range,
            clip_marks,
            opts.socket,
            dir.clone(),
        )?;
        let (overlay, controller) = overlay(theater, dir.join("screenshots"));
        let data = opts.data.unwrap_or_else(content::store::default_data_root);
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
            let (store, problems) = boot::setting_defs::load(&data, &[]);
            for p in problems {
                eprintln!("theater: {p}");
            }
            let native: boot::run_opts::NativeSize = std::sync::Arc::new(window.native_size());
            let raster = boot::d3d_glue::output_size(Some(&store), native.clone())?;
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
                let present = window
                    .frame_sink()
                    .into_guest_crop_present_hook(publish_every, boot::setting_defs::GUEST_FRAME, move || camera.present_crop());
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
}
