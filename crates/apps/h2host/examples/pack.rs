//! Packs what the tvOS, iOS or Xbox app bundles, on the Mac
//! (platform/tvos/build.sh, platform/ios/build.sh, platform/xbox/build.sh):
//!
//!     cargo run --release -p h2host --example pack -- <halo2.iso> <out-dir> \
//!         [--platform tvos|ios|xbox] [--gen DIR] [--no-module]
//!
//! * `<out>/disc/`: the disc files in `h2host::layout::DISC_FILES`, copied
//!   with their tree (files already the right size are kept);
//! * `<out>/h2game.framework/`: the generated module, lifted, emitted and
//!   compiled for the platform (`H2_EMIT_TARGET=tvos` or `ios`,
//!   `boot::build`) in `--gen` (default `$TMPDIR/h2<platform>-gen`), with
//!   its stamp and an Info.plist; for `xbox`, `<out>/h2game.dll` and
//!   `<out>/h2game.dll.stamp` instead (`H2_EMIT_TARGET=windows`, which
//!   needs `H2_XWIN`: platform/windows/env.sh);
//! * `<out>/bootcache/`: the image's analysis, so the first launch on the
//!   device need not analyse the XBE.
//!
//! `--platform` defaults to tvos.
//!
//! `--maps demo|all` (default demo) picks the disc files: `demo` is
//! `DISC_FILES`, `all` the whole disc (`h2host::maps`). Files an earlier
//! pack copied that this one does not want are removed from `<out>/disc/`,
//! so going back to `demo` shrinks the bundle again. The map pack's
//! hard-disk seed is `build.sh`'s (`h2 import-mappack`), not this.
//!
//! The module build is the full-image emit and clang build (about 400 MB
//! of C): run it under the emit gate on a shared machine. `--no-module`
//! skips it.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use h2host::layout::Platform;
use h2host::maps::Maps;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut pos = Vec::new();
    let mut gen_dir = None;
    let mut module = true;
    let mut platform = Platform::Tvos;
    let mut maps = Maps::Demo;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--gen" => gen_dir = Some(PathBuf::from(args.next().context("--gen needs a directory")?)),
            "--platform" => {
                let p = args.next().context("--platform needs tvos, ios or xbox")?;
                platform = Platform::parse(&p).with_context(|| format!("--platform {p}: expected tvos, ios or xbox"))?;
            }
            "--no-module" => module = false,
            "--maps" => {
                let m = args.next().context("--maps needs demo or all")?;
                maps = Maps::parse(&m).with_context(|| format!("--maps {m}: expected demo or all"))?;
            }
            _ => pos.push(PathBuf::from(a)),
        }
    }
    let [iso, out] = pos.as_slice() else {
        bail!("usage: pack <halo2.iso> <out-dir> [--platform tvos|ios|xbox] [--gen DIR] [--no-module]");
    };
    let gen_dir = gen_dir
        .unwrap_or_else(|| std::env::temp_dir().join(format!("h2{}-gen", platform.emit_target())));
    std::fs::create_dir_all(out)?;
    disc(iso, &out.join("disc"), maps)?;
    if module {
        build_module(iso, out, &gen_dir, platform)?;
    }
    Ok(())
}

fn disc(iso: &Path, dest: &Path, maps: Maps) -> Result<()> {
    let mut x = xiso::Xiso::open(BufReader::new(
        File::open(iso).with_context(|| format!("opening {}", iso.display()))?,
    ))?;
    let (mut files, mut bytes) = (0, 0u64);
    let mut keep = std::collections::BTreeSet::new();
    for e in x.entries()?.iter().filter(|e| maps.wants(&e.path, e.is_dir)) {
        let rel = PathBuf::from(e.path.trim_start_matches('/'));
        let to = dest.join(&rel);
        keep.insert(rel);
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
    let (gone, gone_bytes) = h2host::maps::prune(dest, &keep)?;
    if gone > 0 {
        eprintln!("pack: disc: removed {gone} files ({:.0} MB) an earlier pack left", gone_bytes as f64 / 1e6);
    }
    eprintln!(
        "pack: disc ({maps:?}): {files} files, {:.0} MB in {}",
        bytes as f64 / 1e6,
        dest.display()
    );
    Ok(())
}

fn build_module(iso: &Path, out: &Path, gen_dir: &Path, platform: Platform) -> Result<()> {
    let cache = out.join("bootcache");
    std::fs::create_dir_all(&cache)?;
    // The generator's switches change the module and its stamp; the app on
    // the device computes the stamp with them unset, so they must be here.
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
    ] {
        if std::env::var_os(k).is_some() {
            bail!("{k} is set; unset it (the app computes the module stamp with the defaults)");
        }
    }
    // SAFETY: set before any thread starts.
    unsafe {
        std::env::set_var("H2_EMIT_TARGET", platform.emit_target());
        std::env::set_var("H2_BOOT_CACHE", &cache);
    }
    let xbe = boot::read_xbe(iso)?;
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let lib = boot::build(&xbe, gen_dir, jobs)?;
    let mut stamp = lib.as_os_str().to_owned();
    stamp.push(".stamp");
    let module = match platform.framework_plist() {
        Some(plist) => {
            let fw = out.join("h2game.framework");
            std::fs::create_dir_all(&fw)?;
            std::fs::copy(&lib, fw.join("h2game"))?;
            std::fs::copy(&stamp, fw.join("h2game.stamp"))?;
            std::fs::write(fw.join("Info.plist"), plist)?;
            fw.join("h2game")
        }
        None => {
            let dll = out.join(h2host::layout::DLL);
            std::fs::copy(&lib, &dll)?;
            std::fs::copy(&stamp, out.join(format!("{}.stamp", h2host::layout::DLL)))?;
            dll
        }
    };
    // Only this build's analysis is for the app: the unread-target sets are
    // written on the device, and an older analysis (another analyzer or
    // Cargo.lock) would only be copied over and never match.
    let current = boot::analysis_cache(&xbe).and_then(|p| p.file_name().map(|n| n.to_owned()));
    for e in std::fs::read_dir(&cache)?.flatten() {
        if Some(e.file_name()) != current {
            let _ = std::fs::remove_file(e.path());
        }
    }
    eprintln!(
        "pack: module {:.0} MB at {} (the C and objects stay in {}; delete them when done)",
        std::fs::metadata(&module)?.len() as f64 / 1e6,
        module.display(),
        gen_dir.display()
    );
    Ok(())
}
