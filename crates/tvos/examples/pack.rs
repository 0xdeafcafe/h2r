//! Packs what the tvOS app bundles, on the Mac (platform/tvos/build.sh):
//!
//!     cargo run --release -p tvos --example pack -- <halo2.iso> <out-dir> [--gen DIR] [--no-module]
//!
//! * `<out>/disc/`: the disc files in `tvos::layout::DISC_FILES`, copied
//!   with their tree (files already the right size are kept);
//! * `<out>/h2game.framework/`: the generated module, lifted, emitted and
//!   compiled for tvOS (`H2_EMIT_TARGET=tvos`, `boot::build`) in `--gen`
//!   (default `$TMPDIR/h2tvos-gen`), with its stamp and an Info.plist;
//! * `<out>/bootcache/`: the image's analysis, so the first launch on the
//!   Apple TV need not analyse the XBE.
//!
//! The module build is the full-image emit and clang build (about 400 MB
//! of C): run it under the emit gate on a shared machine. `--no-module`
//! skips it.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const FRAMEWORK_PLIST: &str = include_str!("../../../platform/tvos/h2game-Info.plist");

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut pos = Vec::new();
    let mut gen_dir = std::env::temp_dir().join("h2tvos-gen");
    let mut module = true;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--gen" => gen_dir = PathBuf::from(args.next().context("--gen needs a directory")?),
            "--no-module" => module = false,
            _ => pos.push(PathBuf::from(a)),
        }
    }
    let [iso, out] = pos.as_slice() else {
        bail!("usage: pack <halo2.iso> <out-dir> [--gen DIR] [--no-module]");
    };
    std::fs::create_dir_all(out)?;
    disc(iso, &out.join("disc"))?;
    if module {
        build_module(iso, out, &gen_dir)?;
    }
    Ok(())
}

fn disc(iso: &Path, dest: &Path) -> Result<()> {
    let mut x = xiso::Xiso::open(BufReader::new(
        File::open(iso).with_context(|| format!("opening {}", iso.display()))?,
    ))?;
    let (mut files, mut bytes) = (0, 0u64);
    for e in x
        .entries()?
        .iter()
        .filter(|e| xiso::subset_matches(&e.path, e.is_dir, tvos::layout::DISC_FILES))
    {
        let to = dest.join(e.path.trim_start_matches('/'));
        if e.is_dir {
            std::fs::create_dir_all(&to)?;
            continue;
        }
        std::fs::create_dir_all(to.parent().unwrap())?;
        files += 1;
        bytes += e.size as u64;
        if std::fs::metadata(&to).is_ok_and(|m| m.len() == e.size as u64) {
            continue;
        }
        let tmp = to.with_extension("partial");
        x.copy_to(e, &mut File::create(&tmp)?)?;
        std::fs::rename(&tmp, &to)?;
    }
    eprintln!(
        "pack: disc: {files} files, {:.0} MB in {}",
        bytes as f64 / 1e6,
        dest.display()
    );
    Ok(())
}

fn build_module(iso: &Path, out: &Path, gen_dir: &Path) -> Result<()> {
    let cache = out.join("bootcache");
    std::fs::create_dir_all(&cache)?;
    // The generator's switches change the module and its stamp; the app on
    // the Apple TV computes the stamp with them unset, so they must be here.
    for k in [
        "H2_FASTMEM",
        "H2_PERF",
        "H2_X87_NATIVE",
        "H2_SSE_NATIVE",
        "H2_DIRECT_CALLS",
        "H2_ACCESS_COUNT",
        "H2_PASSES",
        "H2_OPT",
        "H2_DEBUG",
        "H2_REG_HOMES",
    ] {
        if std::env::var_os(k).is_some() {
            bail!("{k} is set; unset it (the app computes the module stamp with the defaults)");
        }
    }
    // SAFETY: set before any thread starts.
    unsafe {
        std::env::set_var("H2_EMIT_TARGET", "tvos");
        std::env::set_var("H2_BOOT_CACHE", &cache);
    }
    let xbe = boot::read_xbe(iso)?;
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let lib = boot::build(&xbe, gen_dir, jobs)?;
    let fw = out.join("h2game.framework");
    std::fs::create_dir_all(&fw)?;
    std::fs::copy(&lib, fw.join("h2game"))?;
    let mut stamp = lib.as_os_str().to_owned();
    stamp.push(".stamp");
    std::fs::copy(&stamp, fw.join("h2game.stamp"))?;
    std::fs::write(fw.join("Info.plist"), FRAMEWORK_PLIST)?;
    // Nothing else in the cache is for the app (the unread-target sets are
    // written on the device).
    for e in std::fs::read_dir(&cache)?.flatten() {
        if !e.file_name().to_string_lossy().starts_with("analysis-") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    eprintln!(
        "pack: module {:.0} MB in {} (the C and objects stay in {}; delete them when done)",
        std::fs::metadata(fw.join("h2game"))?.len() as f64 / 1e6,
        fw.display(),
        gen_dir.display()
    );
    Ok(())
}
