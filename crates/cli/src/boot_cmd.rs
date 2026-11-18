//! `h2 build`, `h2 run` and `h2 serve`: the boot tracer (crates/boot,
//! docs/runtime/BOOT.md); `serve` is `run` with the agent API on and no
//! building (docs/runtime/agent-api.md).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage:
  h2 build <disc.iso|default.xbe> [--out DIR] [--jobs N] [--check]
  h2 run   <disc.iso> [--out DIR] [--data DIR] [--fresh-data] [--slices N] [--transfers N] [--no-stopgaps] [--no-d3d] [--no-bink] [--no-dsound] [--no-xapi] [--no-mappack] [--no-halo1]
           [--movies DIR | --no-movies]
  h2 serve <disc.iso> [--socket PATH] [run's options]

--out defaults to $TMPDIR/h2boot (generated C, objects, libh2game.dylib).
--data defaults to $TMPDIR/h2boot-data (host root of the hard-disk partitions).
--fresh-data empties it first: the cache partitions, profiles and saves a
  run leaves behind change what the next run does, so runs compared frame
  for frame need it (and must not share a data directory while running).
  It refuses to when the disc, image or --out lies inside what it would empty.
`run` installs a mappack.iso found next to the disc into --data (once; a warning
never stops the run). --no-mappack or H2_MAPPACK=0 skips it (docs/game/map-pack.md).
`run` and `serve` convert a halo1.iso found next to the disc into --data when
it is new or the converter changed (a warning never stops the run);
--no-halo1 or H2_HALO1=0 skips it (docs/game/campaigns.md).
The intro, attract and credits movies play when `h2 movies` has installed
them for this disc (in --movies DIR, by default the app data directory);
otherwise, or with --no-movies, the game skips them. The run logs which
(`movies: playback (...)` or `movies: skip (...)`; docs/audio/bink-hle.md).
`run` builds first when the dylib is missing or was built from another
image or generator (its .stamp). `build --check` builds nothing: it exits 0
when --out's dylib is current for this h2 and the image, 3 otherwise.
`serve` runs headless with the agent API listening on --socket (default
$H2_AGENT, else the cache directory's h2recomp/agent.sock) until the game
exits, for `h2 agent` and other clients (docs/runtime/agent-api.md). It never
builds: when --out's dylib isn't current it stops and says so. Use the same
H2_OPT the image was built with.";

pub fn run(cmd: &str, args: Vec<String>) -> Result<()> {
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = content::store::default_data_root();
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let serve = cmd == "serve";
    // A server runs until the game exits.
    let mut slices = if serve { u64::MAX } else { 100_000u64 };
    let mut socket = None;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut stopgaps = true;
    let mut d3d = true;
    let mut bink = true;
    let mut movies = boot::movies_glue::default_dir();
    let mut dsound = true;
    let mut xapi = true;
    let mut fresh = false;
    let mut check = false;
    let mut no_mappack = false;
    let mut no_halo1 = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = PathBuf::from(val()?),
            "--data" => data = PathBuf::from(val()?),
            "--jobs" => jobs = val()?.parse()?,
            "--fresh-data" => fresh = true,
            "--check" if cmd == "build" => check = true,
            "--socket" if serve => socket = Some(PathBuf::from(val()?)),
            "--no-stopgaps" => stopgaps = false,
            "--no-d3d" => d3d = false,
            "--no-bink" => bink = false,
            "--movies" => movies = Some(PathBuf::from(val()?)),
            "--no-movies" => movies = None,
            "--no-dsound" => dsound = false,
            "--no-xapi" => xapi = false,
            "--no-mappack" => no_mappack = true,
            "--no-halo1" => no_halo1 = true,
            "--slices" => slices = val()?.parse()?,
            "--transfers" => transfers = val()?.parse()?,
            "-h" | "--help" => bail!("{USAGE}"),
            _ if disc.is_none() => disc = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let disc = disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .context(USAGE)?;
    let xbe = boot::read_xbe(&disc)?;
    let lib = out.join(boot::DYLIB);
    if check {
        let current = boot::dylib_current(&xbe, &lib);
        eprintln!(
            "h2 build --check: {} is {}",
            lib.display(),
            if current { "current" } else { "out of date" }
        );
        std::process::exit(if current { 0 } else { 3 });
    }
    if serve {
        if !boot::dylib_current(&xbe, &lib) {
            bail!(
                "h2 serve: {} isn't built for this image and this h2; build it with `h2 build` first (serve never builds)",
                lib.display()
            );
        }
        let path = socket.unwrap_or_else(agent_proto::default_socket);
        // SAFETY: set before the run starts any thread; boot reads it once.
        unsafe { std::env::set_var(agent_proto::ENV, &path) };
        #[cfg(unix)]
        stop_on_signal(path)?;
    }
    if cmd == "build" || !boot::dylib_current(&xbe, &lib) {
        if cmd != "build" && lib.exists() {
            eprintln!(
                "h2 run: {} was built from another image or generator; rebuilding",
                lib.display()
            );
        }
        boot::build(&xbe, &out, jobs)?;
    }
    if cmd == "build" {
        return Ok(());
    }
    if fresh {
        for d in fresh_data_dirs(&data, &[&disc, &out])? {
            std::fs::remove_dir_all(&d).with_context(|| format!("emptying {}", d.display()))?;
        }
    }
    if crate::mappack_cmd::auto_enabled(no_mappack) {
        crate::mappack_cmd::auto_install(&disc, &data);
    }
    if crate::h1_import::auto_enabled(no_halo1) {
        crate::h1_import::auto_import(&disc, &data);
    }
    let r = boot::run(
        &xbe,
        &lib,
        &boot::RunOpts {
            disc,
            data_root: data,
            max_slices: slices,
            transfers_per_slice: transfers,
            stopgaps,
            trace: Some(out.join("kernel-trace.tsv")),
            d3d,
            bink,
            dsound,
            xapi,
            movies,
        },
    );
    // The kernel's storage may outlive the run (or never drop): store its
    // pending write metadata now. Signals and other abnormal exits fall
    // under the bounded loss in docs/runtime/storage.md "Write metadata".
    vfs::flush_pending_metadata();
    let r = r?;
    for l in &r.kernel_log {
        println!("DbgPrint: {}", l.trim_end());
    }
    println!(
        "outcome: {}",
        r.outcome.chars().take(400).collect::<String>()
    );
    println!("stop: {}", r.stop);
    println!("last eip: {:#010x}", r.eip);
    Ok(())
}

/// The directories `--fresh-data` empties (`<data>/disk`, `<data>/metadata`,
/// those that exist), or an error, deleting nothing, when one of them is or
/// contains a path in `keep` (the disc or image, the build output).
fn fresh_data_dirs(data: &Path, keep: &[&Path]) -> Result<Vec<PathBuf>> {
    // Both spellings: as given (made absolute) and with links resolved.
    let forms = |p: &Path| -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Ok(a) = std::path::absolute(p) {
            v.push(a);
        }
        if let Ok(c) = p.canonicalize() {
            v.push(c);
        }
        v
    };
    let mut out = Vec::new();
    for sub in ["disk", "metadata"] {
        let d = data.join(sub);
        if !d.exists() {
            continue;
        }
        for k in keep {
            if forms(&d)
                .iter()
                .any(|d| forms(k).iter().any(|k| k.starts_with(d)))
            {
                bail!(
                    "--fresh-data would delete {} with {} in it; use another --data",
                    d.display(),
                    k.display()
                );
            }
        }
        out.push(d);
    }
    Ok(out)
}

/// `h2 serve` on Ctrl-C or SIGTERM: remove the socket file, store pending
/// file metadata, and exit (128 + the signal), instead of dying with the
/// socket left behind. SIGINT and SIGTERM are blocked here, before the run
/// starts any thread, so every thread inherits the mask and only this one
/// takes them.
#[cfg(unix)]
fn stop_on_signal(socket: PathBuf) -> Result<()> {
    // SAFETY: plain sigset operations on a local, and a mask change on
    // this thread before any other thread exists.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        let r = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if r != 0 {
            bail!(
                "h2 serve: blocking SIGINT/SIGTERM: {}",
                std::io::Error::from_raw_os_error(r)
            );
        }
        set
    };
    std::thread::Builder::new()
        .name("serve-signals".into())
        .spawn(move || {
            let mut sig = 0;
            // SAFETY: waits on the signals blocked above.
            if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                return;
            }
            let _ = std::fs::remove_file(&socket);
            vfs::flush_pending_metadata();
            eprintln!("h2 serve: signal {sig}: stopping");
            std::process::exit(128 + sig);
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-boot-cmd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fresh_data_does_not_delete_its_input() {
        let root = scratch("input");
        let data = root.join("data");
        let image = data.join("disk/Partition1/synthetic.xbe");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        std::fs::write(&image, b"XBEH").unwrap();
        std::fs::create_dir_all(data.join("metadata")).unwrap();
        let out = root.join("out");
        assert!(fresh_data_dirs(&data, &[&image, &out]).is_err());
        // The image inside metadata, or the data dir's partitions as --out.
        assert!(
            fresh_data_dirs(&data, &[&data.join("metadata/x.iso")]).is_err(),
            "inside metadata, by its spelling"
        );
        assert!(
            fresh_data_dirs(&data, &[&data.join("disk")]).is_err(),
            "equal"
        );
        // Spelled with `..` it is still inside.
        assert!(
            fresh_data_dirs(
                &data,
                &[&data.join("metadata/../disk/Partition1/synthetic.xbe")]
            )
            .is_err()
        );
        assert!(image.exists(), "nothing was deleted");
        let elsewhere = root.join("halo2.iso");
        std::fs::write(&elsewhere, b"").unwrap();
        let dirs = fresh_data_dirs(&data, &[&elsewhere, &out]).unwrap();
        assert_eq!(dirs, [data.join("disk"), data.join("metadata")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fresh_data_skips_what_is_not_there() {
        let root = scratch("empty");
        assert!(
            fresh_data_dirs(&root.join("data"), &[&root.join("halo2.iso")])
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
