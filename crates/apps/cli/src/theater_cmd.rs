//! Launch an isolated film player. It plays the film from its game start
//! and seeks in its own process (docs/game/theater-design.md, "Playback").
mod verify;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(feature = "desktop")]
mod export;

const USAGE: &str = "h2 theater play FILM [--seek TICK] [--render-every N] [--speed 0.25..4] [--disc ISO] [--out IMAGE_DIR] [--data VIEWER_DATA] [--movies DIR] [--socket PATH]";
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
    let header = film::file::header_of(&opts.film)?;
    let view = std::env::var("H2_THEATER_VIEW")
        .ok()
        .map(|v| serde_json::from_str::<agent_proto::theater::InitialView>(&v))
        .transpose()?;
    let target = opts
        .seek
        .or_else(|| view.and_then(|v| v.tick.map(u64::from)))
        // A film opens at its game's first update, tick 1.
        .unwrap_or_else(|| opts.range.map_or(0, |[start, _]| start));
    if let Some([start, end]) = opts.range {
        ensure!((start..end).contains(&target), "seek outside film clip");
    }
    ensure!(target <= header.meta.length_ticks, "seek beyond film");
    let mut presentation = boot::theater::Presentation::default();
    if let Some(view) = view {
        presentation.start_view(view);
    }
    let launch = boot::theater::Launch {
        target,
        speed: opts.speed,
        paused: false,
        presentation,
    };
    let temp = Temp::new()?;
    serde_json::to_writer(fs::File::create(temp.0.join("launch.json"))?, &launch)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("theater")
        .args(&args)
        .env("H2_AGENT", &opts.socket)
        .env("H2_THEATER_WORK", &temp.0)
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
    boot::theater::configure_replay_child(&header, &mut cmd)?;
    let status = cmd.status().context("starting film playback")?;
    ensure!(status.success(), "film playback exited with {status}");
    Ok(())
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
    launch: boot::theater::Launch,
    #[cfg(feature = "desktop")] capture: Option<std::sync::Arc<boot::theater::ClipCapture>>,
) -> Result<()> {
    #[cfg(feature = "desktop")]
    let media_options = opts.clone();
    let child_started = std::time::Instant::now();
    let header = film::file::header_of(&opts.film)?;
    let xbe = boot::read_xbe(&opts.disc)?;
    let xbe_sha256 = film::xbe_digest(&xbe);
    ensure!(
        header.xbe_sha256 == xbe_sha256,
        "film requires a different XBE"
    );
    // Any image of this XBE plays a film with a game start: the film holds
    // the actions, and the simulation's digests check them.
    let (image, _image_stamp) = boot::theater::playback_images(&xbe, &opts.out)
        .into_iter()
        .next()
        .context("no current module installed for this XBE (check --disc, --out and H2_OPT)")?;
    // A film plays from its game start: the data directory is empty and
    // private, with nothing of the viewer's disk read for it
    // (docs/game/theater-design.md, "Playback").
    ensure!(
        film::file::game_start_of(&opts.film)?.is_some(),
        "{}",
        boot::theater::NO_GAME_START
    );
    let data = work.join("data");
    fs::create_dir_all(&data)?;
    // The viewer's display settings: the window's output size.
    #[cfg(feature = "desktop")]
    let settings = {
        let viewer_data = opts.data.clone().unwrap_or_else(content::store::default_data_root);
        let (settings, problems) = boot::setting_defs::load(&viewer_data, &[]);
        for problem in problems {
            eprintln!("theater: {problem}");
        }
        settings
    };
    let verification = Some(verify::mode(&opts, work)?);
    let boot = boot::RunOpts {
        disc: opts.disc,
        data_root: data,
        max_slices: u64::MAX,
        xapi: true,
        transfers_per_slice: header.transfers_per_slice,
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
    #[cfg(feature = "desktop")]
    let clip_marks = launch.presentation.clip_marks;
    #[cfg(feature = "desktop")]
    let starting_seek = (launch.target > 0).then_some(launch.target);
    let replay = boot::theater::Replay {
        verification,
        path: opts.film.clone(),
        render_every: opts.every,
        speed: launch.speed,
        seek_tick: launch.target,
        paused_at_target: launch.paused,
        presentation: launch.presentation,
        camera: boot::theater::FilmCamera::default(),
        window_closed: None,
    };
    let run = move |host, window_closed| -> Result<()> {
        let mut replay = replay;
        replay.window_closed = window_closed;
        let r = boot::run_theater(&xbe, &image, &boot, host, replay, child_started);
        vfs::flush_pending_metadata();
        r.map(|_| ())
    };
    let host_bots = header.host_bots;
    #[cfg(feature = "desktop")]
    if let Some(capture) = capture {
        return run(export::host(capture, host_bots)?, None);
    }
    if opts.every == 0 {
        return run(boot::Host { host_light: true, host_bots, ..Default::default() }, None);
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
            let native: boot::run_opts::NativeTarget = std::sync::Arc::new(window.native_target());
            let raster = boot::render_glue::output_size(Some(&settings), native)?;
            let film_media = export::media(
                media_options,
                dir,
                window.frame_sink(),
                raster.clone(),
            );
            // The film's view is drawn by the host, beside a guest that
            // draws nothing of its own (`theater::HostView`).
            let mut sink = window.frame_sink();
            let film_view = boot::theater::HostView::new(
                window.gfx_context(),
                raster,
                window.display_size(),
                move |view, size| {
                    sink.submit_view(view, size, true)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
            );
            let life = window.lifecycle();
            run(
                boot::Host {
                    film_view: Some(film_view),
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
    fn play_is_the_only_theater_command_that_takes_a_film() {
        let args = |command: &str| {
            [command, "synthetic.h2film", "--disc", "synthetic.iso"].map(String::from).into_iter()
        };
        assert!(Options::parse(args("play")).is_ok());
        assert!(Options::parse(args("prepare")).is_err());
    }
}
