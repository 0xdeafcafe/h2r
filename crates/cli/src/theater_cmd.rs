//! Each seek starts a new process so the game, HLE allocators and feature
//! statics all restart. The parent owns the restored disk and the seek reply.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const USAGE: &str = "h2 theater play FILM [--render-every N] [--speed 0.25..4] [--disc ISO] [--out IMAGE_DIR] [--data INITIAL_DATA] [--socket PATH]";
struct Options {
    film: PathBuf,
    every: u32,
    speed: f64,
    disc: PathBuf,
    out: PathBuf,
    socket: PathBuf,
    data: Option<PathBuf>,
    range: Option<[u64; 2]>,
}
impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        ensure!(args.next().as_deref() == Some("play"), "{USAGE}");
        let film = PathBuf::from(args.next().context(USAGE)?);
        let mut every = u32::from(cfg!(feature = "desktop"));
        let mut data = None;
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
                "--data" => data = Some(PathBuf::from(value)),
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
    if let Some(work) = std::env::var_os("H2_THEATER_WORK") {
        return child(opts, Path::new(&work));
    }
    let header = film::Reader::open(&opts.film)?.header;
    let mut target = opts.range.map_or(0, |[start, _]| start);
    let mut speed = opts.speed;
    let mut paused = false;
    loop {
        let temp = Temp::new()?;
        let restart = temp.0.join("restart.json");
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
            "H2_BOTS",
            "H2_INPUT_REPLAY",
            "H2_INPUT_RECORD",
        ] {
            cmd.env_remove(key);
        }
        let status = cmd.status().context("starting film playback")?;
        ensure!(status.success(), "film playback exited with {status}");
        if !restart.try_exists()? {
            return Ok(());
        }
        (target, speed, paused) = serde_json::from_reader(fs::File::open(restart)?)?;
    }
}

fn child(opts: Options, work: &Path) -> Result<()> {
    let mut film = film::Reader::open(&opts.film)?;
    let xbe = boot::read_xbe(&opts.disc)?;
    let image = [
        boot::tier::clang_dylib(&opts.out),
        boot::tier::fast_dylib(&opts.out),
    ]
    .into_iter()
    .find(|p| {
        boot::tier::module_tier(&xbe, p).is_some_and(|tier| {
            let stamp = match tier {
                boot::tier::Tier::Clang => boot::build_stamp(&xbe),
                boot::tier::Tier::Fast => boot::tier::fast_stamp(&xbe),
            };
            film.header.meta.playable_here(&stamp)
        })
    })
    .context("no installed image matches the film stamp (check --disc, --out and H2_OPT)")?;
    ensure!(
        film.header.meta.source == film::Source::Host,
        "exact playback requires a playerless host film"
    );
    let entry = film
        .entries
        .iter()
        .find(|e| e.tag == film::DATA)
        .context("film has no start manifest")?
        .clone();
    let data = work.join("data");
    film.read::<film::manifest::Manifest>(&entry)?
        .restore_with(&data, opts.data.as_deref())?;
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
        movies: boot::movies_glue::Movies::Off,
    };
    #[cfg(feature = "desktop")]
    let film_parent = opts.film.parent().unwrap_or(Path::new(".")).to_owned();
    let replay = boot::theater::Replay {
        path: opts.film,
        render_every: opts.every,
        speed: std::env::var("H2_THEATER_SPEED")?.parse()?,
        seek_tick: std::env::var("H2_THEATER_TARGET")?.parse()?,
        paused_at_target: std::env::var("H2_THEATER_PAUSED")? == "1",
        restart: work.join("restart.json"),
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
    if opts.every == 0 {
        return run(
            boot::Host {
                host_light: true,
                host_bots,
                ..Default::default()
            },
            None,
        );
    }
    #[cfg(not(feature = "desktop"))]
    bail!("rendered film playback requires the desktop feature");
    #[cfg(feature = "desktop")]
    {
        use crate::play_cmd::theater::{film_meta, overlay};

        let dir = std::env::var_os("H2_FILM_DIR")
            .map(PathBuf::from)
            .or_else(|| opts.data.as_ref().map(|data| data.join("films")))
            .unwrap_or(film_parent);
        let lease_id = film.header.lease_id;
        let meta = film.header.meta;
        let title = meta.map.clone();
        let stamp = meta.image_stamp.clone();
        let theater = boot::theater_ui::Theater::playback(
            film_meta(meta, false, &stamp, lease_id),
            opts.range,
            opts.socket,
            dir.clone(),
        )?;
        let (overlay, controller) = overlay(theater, dir.join("screenshots"));
        let config = app::AppConfig {
            title: format!("Theater — {}", title),
            audio: false,
            keyboard: false,
            gamepads: true,
            theater: Some(overlay),
            mouse_capture: false,
            keep_open_on_guest_exit: false,
            ..Default::default()
        };
        let exit = app::run(config, move |mut window| {
            if let Some(input) = window.take_input() {
                controller.attach_input(input);
            }
            let gfx = d3d::GfxBackend::new(window.gfx_context()).on_present(window.present_hook());
            let life = window.lifecycle();
            run(
                boot::Host {
                    gfx: Some(gfx),
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
