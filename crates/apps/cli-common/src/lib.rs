//! What the `h2` and `h2-tools` binaries share: finding the disc, opening
//! it and its maps, and writing exporter output that never clobbers it
//! ([`safe_out`]). docs/runtime/cli-tools.md.

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use map::Map;
use xiso::Xiso;

pub mod safe_out;

/// Whether `p` is a file named `.iso` or `.xbe`.
pub fn is_disc(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    p.is_file() && matches!(ext.as_deref(), Some("iso" | "xbe"))
}

/// The checkout the binaries were built from.
pub fn repo_root() -> PathBuf {
    // crates/apps/cli-common -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The disc a command uses when none is given: `$H2_DISC`, else the
/// Halo 2 disc in the checkout's `disc/` folder ([`pick_default_disc`]).
pub fn default_disc() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("H2_DISC") {
        return Ok(p.into());
    }
    let dir = repo_root().join("disc");
    let isos: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_disc(p) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
        .collect();
    pick_default_disc(isos).with_context(|| {
        format!(
            "no disc given and none found in {} (put your .iso there or set H2_DISC)",
            dir.display()
        )
    })
}

/// The Halo 2 disc among `isos`: `halo2.iso` when present, else the first
/// by name that isn't another game's disc this repo also reads (`halo1.iso`,
/// the Map Pack's `mappack.iso`).
pub fn pick_default_disc(mut isos: Vec<PathBuf>) -> Option<PathBuf> {
    const OTHER_DISCS: [&str; 2] = ["halo1.iso", "mappack.iso"];
    let name = |p: &PathBuf| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    if let Some(p) = isos.iter().find(|p| name(p) == "halo2.iso") {
        return Some(p.clone());
    }
    isos.sort();
    isos.into_iter()
        .find(|p| !OTHER_DISCS.contains(&name(p).as_str()))
}

/// Opens a disc image, and [`safe_out::protect`]s it for the rest of the
/// process.
pub fn open_iso(path: &Path) -> Result<Xiso<BufReader<File>>> {
    safe_out::protect(path);
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(Xiso::open(BufReader::new(f))?)
}

/// Accepts a raw XBE or a disc image, returning the XBE bytes.
pub fn load_xbe(path: &Path) -> Result<Vec<u8>> {
    safe_out::protect(path);
    let mut magic = [0u8; 4];
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_exact(&mut magic)?;
    if &magic == b"XBEH" {
        return Ok(fs::read(path)?);
    }
    let mut iso = open_iso(path)?;
    let entry = iso.find("default.xbe")?;
    Ok(iso.read(&entry)?)
}

/// The map file `entry` of the disc image at `path`.
pub fn disc_map(path: &Path, partition: u64, entry: &xiso::Entry) -> Result<Map<BufReader<File>>> {
    let base = partition + u64::from(entry.sector) * xiso::SECTOR;
    Map::from_region(BufReader::new(File::open(path)?), base, entry.size.into())
        .with_context(|| format!("parsing {}", entry.path))
}

/// A `.map` file on the host, else the disc's map of that name
/// (`ascension`, `maps/ascension.map`, `/maps/ascension.map`).
pub fn load_map(name: &str, disc: Option<PathBuf>) -> Result<Map<BufReader<File>>> {
    if Path::new(name).is_file() {
        Map::from_reader(BufReader::new(File::open(name)?))
            .with_context(|| format!("parsing {name}"))
    } else {
        let disc = disc.map(Ok).unwrap_or_else(default_disc)?;
        let mut iso = open_iso(&disc)?;
        let mut path = name.trim_start_matches('/').to_owned();
        if !path.to_ascii_lowercase().ends_with(".map") {
            path.push_str(".map");
        }
        if !path.contains('/') {
            path = format!("maps/{path}");
        }
        let entry = iso.find(&path)?;
        disc_map(&disc, iso.partition_offset(), &entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_disc_prefers_halo2_and_skips_other_games() {
        let p = |n: &str| PathBuf::from("disc").join(n);
        let all = vec![p("halo1.iso"), p("halo2.iso"), p("mappack.iso")];
        assert_eq!(pick_default_disc(all), Some(p("halo2.iso")));
        let renamed = vec![p("halo1.iso"), p("my halo 2.iso"), p("mappack.iso")];
        assert_eq!(pick_default_disc(renamed), Some(p("my halo 2.iso")));
        assert_eq!(pick_default_disc(vec![p("halo1.iso")]), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-cli-common-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Opening a disc registers it, so any exporter's `OutDir` refuses it even
    /// under `--force` and even when the file has an innocent name.
    #[test]
    fn opened_discs_are_protected_from_every_exporter() {
        use std::io::{Seek, SeekFrom, Write};
        let d = scratch("protect");
        let disc = d.join("pretty.png");
        let mut f = File::create(&disc).unwrap();
        f.seek(SeekFrom::Start(0x10000)).unwrap();
        f.write_all(b"MICROSOFT*XBOX*MEDIA").unwrap();
        f.write_all(&[0u8; 8]).unwrap();
        drop(f);
        let before = fs::read(&disc).unwrap();
        open_iso(&disc).unwrap();
        let out = safe_out::OutDir::new(&d, true).unwrap();
        assert!(out.write(&["pretty.png".to_string()], b"png").is_err());
        assert_eq!(fs::read(&disc).unwrap(), before);
        fs::remove_dir_all(d).unwrap();
    }

    /// Registration must not depend on the disc-magic fallback: a file that is
    /// not a disc at all is still protected once a loader has seen it.
    #[test]
    fn loaders_register_their_input_even_without_disc_magic() {
        let d = scratch("register");
        let junk = d.join("junk.dat");
        fs::write(&junk, b"not a disc at all").unwrap();
        assert!(open_iso(&junk).is_err());
        let xbe = d.join("x.dat");
        fs::write(&xbe, b"XBEH synthetic").unwrap();
        load_xbe(&xbe).unwrap();
        let out = safe_out::OutDir::new(&d, true).unwrap();
        for n in ["junk.dat", "x.dat"] {
            assert!(
                out.write(&[n.to_string()], b"x").is_err(),
                "{n} was overwritten"
            );
        }
        assert_eq!(fs::read(&junk).unwrap(), b"not a disc at all");
        assert_eq!(fs::read(&xbe).unwrap(), b"XBEH synthetic");
        fs::remove_dir_all(d).unwrap();
    }
}
