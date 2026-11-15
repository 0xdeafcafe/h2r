//! `h2 import-mappack`: install the Halo 2 Multiplayer Map Pack disc's
//! content into the emulated hard disk, and keep its title update aside.
//!
//! Nothing is read from the game here beyond the disc's directory tree and,
//! for the update, the XBE certificate. The layout and what the game does
//! with it are in docs/map-pack.md.
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::{Entry, Xiso};

use crate::safe_out::OutDir;

const HALO2_TITLE: u32 = 0x4D53_0064;
/// Where the disc keeps its content packages, one directory per package.
const DLC_DIR: &str = "/demos/dlc/";
/// Where the disc keeps one signed copy of the update per base disc version.
const UPDATE_DIR: &str = "/demos/update/Updates/";
const UPDATE_STEM: &str = "default.xbe.";
/// FATX allows 42 bytes per name (docs/storage.md).
const FATX_NAME_MAX: usize = 42;

const USAGE: &str = "usage: h2 import-mappack <mappack.iso> [--data DIR] [--disc <halo2.iso|default.xbe>]
                       [--no-update] [--force] [--dry-run]

Installs the Multiplayer Map Pack's content packages under the emulated hard
disk (<data>/disk/Partition1/TDATA/4D530064/$c/<offer id>/), and copies the
title update that matches your Halo 2 disc to <data>/updates/4D530064/<version>/.
The update is never placed where the game would launch it. --data defaults to
$TMPDIR/h2boot-data, as for `h2 play`. --disc defaults to $H2_DISC, then the
first .iso in disc/. See docs/map-pack.md.";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub iso: PathBuf,
    pub data: Option<PathBuf>,
    pub disc: Option<PathBuf>,
    pub update: bool,
    pub force: bool,
    pub dry_run: bool,
}

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut iso, mut data, mut disc) = (None, None, None);
    let (mut update, mut force, mut dry_run) = (true, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = Some(PathBuf::from(it.next().ok_or("--data needs a directory")?)),
            "--disc" => disc = Some(PathBuf::from(it.next().ok_or("--disc needs a path")?)),
            "--no-update" => update = false,
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if iso.is_none() => iso = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let iso = iso.ok_or("missing the map pack .iso")?;
    Ok(Args { iso, data, disc, update, force, dry_run })
}

/// One content package on the disc.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The disc directory's name: 8 hex digits.
    pub dir: String,
    pub offer: u64,
    pub files: Vec<Entry>,
}

/// What the disc holds, found from its directory tree alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub packages: Vec<Package>,
    /// `default.xbe.NN` copies, by NN (the base disc's certificate version).
    pub updates: Vec<(u32, Entry)>,
}

impl Plan {
    pub fn from_entries(entries: &[Entry]) -> Result<Self> {
        let mut packages: Vec<Package> = Vec::new();
        let mut updates = Vec::new();
        for e in entries {
            let lower = e.path.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix(DLC_DIR) {
                let parts: Vec<&str> = rest.split('/').collect();
                let dir = &e.path[DLC_DIR.len()..DLC_DIR.len() + parts[0].len()];
                let number = match u32::from_str_radix(dir, 16) {
                    Ok(n) if dir.len() == 8 => n,
                    // maps.png and the like sit beside the packages.
                    _ => continue,
                };
                match parts.as_slice() {
                    [_] if e.is_dir => packages.push(Package {
                        dir: dir.to_string(),
                        offer: vfs::offer_id(HALO2_TITLE, number),
                        files: Vec::new(),
                    }),
                    [_, name] if !e.is_dir => {
                        check_fatx_name(name, &e.path)?;
                        let pkg = packages
                            .iter_mut()
                            .find(|p| p.dir == dir)
                            .with_context(|| format!("{} is outside a listed package", e.path))?;
                        pkg.files.push(e.clone());
                    }
                    _ => bail!("unexpected entry {} in a content package", e.path),
                }
            } else if let Some(rest) = lower.strip_prefix(&UPDATE_DIR.to_ascii_lowercase())
                && let Some(n) = rest.strip_prefix(UPDATE_STEM)
                && let (Ok(n), false) = (n.parse::<u32>(), e.is_dir)
            {
                updates.push((n, e.clone()));
            }
        }
        packages.retain(|p| !p.files.is_empty());
        if packages.is_empty() {
            bail!("no content packages under {DLC_DIR}: is this the Multiplayer Map Pack disc?");
        }
        for p in &packages {
            let has_meta = p.files.iter().any(|f| f.path.to_ascii_lowercase().ends_with("/contentmeta.xbx"));
            if !has_meta {
                bail!("package {} has no ContentMeta.xbx", p.dir);
            }
        }
        updates.sort_by_key(|(n, _)| *n);
        Ok(Self { packages, updates })
    }

    /// The update copy signed for a base disc with this certificate version.
    pub fn update_for(&self, base_version: u32) -> Option<&Entry> {
        self.updates.iter().find(|(n, _)| *n == base_version & 0xff).map(|(_, e)| e)
    }
}

fn check_fatx_name(name: &str, path: &str) -> Result<()> {
    if name.is_empty() || name.len() > FATX_NAME_MAX || !name.is_ascii() {
        bail!("{path}: name is not a valid FATX name");
    }
    Ok(())
}

/// Checks a candidate update against the base image: same title, and the
/// low byte of its certificate version names the base disc's version.
/// Returns the update's certificate version.
pub fn check_update(update: &[u8], base: &xbe::Certificate) -> Result<u32> {
    let x = xbe::Xbe::parse(update).context("parsing the update XBE")?;
    let c = &x.certificate;
    if c.title_id != base.title_id {
        bail!("update title {:08X} is not the base's {:08X}", c.title_id, base.title_id);
    }
    if c.version & 0xff != base.version & 0xff || c.version >> 8 == 0 {
        bail!("update version {:#x} is not an update for base version {:#x}", c.version, base.version);
    }
    Ok(c.version)
}

/// Output path components, relative to the data root, for one package file.
/// Matches `vfs::Config::content_dir`.
fn package_components(p: &Package, file: &Entry) -> Vec<String> {
    let name = file.path.rsplit('/').next().unwrap_or_default();
    [
        "disk",
        "Partition1",
        "TDATA",
        &format!("{HALO2_TITLE:08X}"),
        vfs::CONTENT_DIR,
        &vfs::content_dir_name(p.offer),
        name,
    ]
    .map(str::to_string)
    .to_vec()
}

fn update_components(version: u32) -> Vec<String> {
    ["updates", &format!("{HALO2_TITLE:08X}"), &format!("{version:08x}"), "default.xbe"].map(str::to_string).to_vec()
}

/// Writes every package, and the update when `base` is given. Returns the
/// paths written (or, with `dry_run`, that would be).
pub fn install<R: Read + Seek>(
    iso: &mut Xiso<R>,
    plan: &Plan,
    out: &OutDir,
    data_root: &Path,
    base: Option<&xbe::Certificate>,
    dry_run: bool,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for p in &plan.packages {
        for f in &p.files {
            let comps = package_components(p, f);
            if dry_run {
                written.push(comps.iter().fold(data_root.to_path_buf(), |a, c| a.join(c)));
                continue;
            }
            let path = out
                .write_with(&comps, |w| iso.copy_to(f, w).map(drop).map_err(|e| std::io::Error::other(e.to_string())))
                .with_context(|| format!("writing {}", f.path))?;
            written.push(path);
        }
    }
    if let Some(base) = base {
        let Some(entry) = plan.update_for(base.version) else {
            let have: Vec<String> = plan.updates.iter().map(|(n, _)| format!("{n:02}")).collect();
            bail!(
                "no update on this disc for base version {:#x} (it has {}); rerun with --no-update",
                base.version,
                have.join(", ")
            );
        };
        let bytes = iso.read(entry)?;
        let version = check_update(&bytes, base).with_context(|| entry.path.clone())?;
        let comps = update_components(version);
        if dry_run {
            written.push(comps.iter().fold(data_root.to_path_buf(), |a, c| a.join(c)));
        } else {
            written.push(out.write(&comps, &bytes)?);
        }
    }
    Ok(written)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let a = match parse_args(&args) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(e) => bail!("{e}\n\n{USAGE}"),
    };
    let data = a.data.clone().unwrap_or_else(|| std::env::temp_dir().join("h2boot-data"));
    let base = if a.update {
        let disc = match a.disc.clone() {
            Some(d) => d,
            None => crate::default_disc().context("finding your Halo 2 disc for the update (or pass --no-update)")?,
        };
        let bytes = crate::load_xbe(&disc)?;
        let x = xbe::Xbe::parse(&bytes)?;
        if x.certificate.title_id != HALO2_TITLE {
            bail!("{} is not Halo 2 (title {:08X})", disc.display(), x.certificate.title_id);
        }
        Some(x.certificate.clone())
    } else {
        None
    };
    let mut iso = crate::open_iso(&a.iso)?;
    let plan = Plan::from_entries(&iso.entries()?)?;
    let out = OutDir::new(&data, a.force)?.protecting(&a.iso);
    let written = install(&mut iso, &plan, &out, &data, base.as_ref(), a.dry_run)?;
    let verb = if a.dry_run { "would write" } else { "wrote" };
    for p in &written {
        println!("{verb} {}", p.display());
    }
    println!(
        "{} package(s), {} file(s) into {}",
        plan.packages.len(),
        written.len() - usize::from(base.is_some()),
        data.display()
    );
    if base.is_some() {
        println!("the update is kept aside: nothing runs it yet (docs/title-updates.md)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    const SECTOR: usize = 2048;

    /// One directory table: entries chained down the right-hand side.
    fn table(items: &[(&str, u32, u32, bool)]) -> Vec<u8> {
        let mut recs: Vec<Vec<u8>> = Vec::new();
        for &(name, sector, size, dir) in items {
            let mut v = vec![0u8; 4];
            v.extend(sector.to_le_bytes());
            v.extend(size.to_le_bytes());
            v.push(if dir { 0x10 } else { 0 });
            v.push(name.len() as u8);
            v.extend(name.as_bytes());
            while !v.len().is_multiple_of(4) {
                v.push(0xFF);
            }
            recs.push(v);
        }
        let mut out = Vec::new();
        let mut offs = Vec::new();
        for r in &recs {
            offs.push(out.len());
            out.extend(r);
        }
        for i in 0..recs.len().saturating_sub(1) {
            let right = (offs[i + 1] / 4) as u16;
            out[offs[i] + 2..offs[i] + 4].copy_from_slice(&right.to_le_bytes());
        }
        out.resize(SECTOR, 0xFF);
        out
    }

    /// A synthetic XBE: just enough header for `xbe::Xbe::parse`.
    fn xbe(title: u32, version: u32) -> Vec<u8> {
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1000];
        d[0..4].copy_from_slice(b"XBEH");
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x104, base);
        put(&mut d, 0x108, 0x1000);
        put(&mut d, 0x10c, 0x1000);
        put(&mut d, 0x110, 0x178);
        put(&mut d, 0x118, base + 0x200); // certificate
        put(&mut d, 0x11c, 1); // one executable section over the headers
        put(&mut d, 0x120, base + 0x300);
        put(&mut d, 0x300, 0x04);
        put(&mut d, 0x304, base);
        put(&mut d, 0x308, 0x1000);
        put(&mut d, 0x314, base + 0x380); // its name, "x"
        d[0x380] = b'x';
        put(&mut d, 0x128, 0xA8FC_57AB ^ (base + 0x400)); // entry (retail key)
        put(&mut d, 0x158, 0x5B6D_40B6 ^ (base + 0x400)); // thunk (retail key)
        put(&mut d, 0x200, 0x1d0); // certificate size
        put(&mut d, 0x208, title);
        put(&mut d, 0x200 + 0xac, version);
        d
    }

    /// A map pack shaped disc: two packages, a stray file beside them,
    /// one empty directory written as a zeroed sector, and two updates.
    fn disc() -> Vec<u8> {
        let mut img = vec![0u8; 80 * SECTOR];
        let vd = 32 * SECTOR;
        img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
        img[vd + 24..vd + 28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        let s = SECTOR as u32;
        let dirs: [(usize, Vec<u8>); 9] = [
            (33, table(&[("default.xbe", 60, 4, false), ("demos", 34, s, true), ("videos", 45, s, true)])),
            (34, table(&[("dlc", 35, s, true), ("update", 38, s, true)])),
            (35, table(&[("00000001", 36, s, true), ("00000003", 37, s, true), ("maps.png", 61, 3, false)])),
            (
                36,
                table(&[
                    ("ContentMeta.xbx", 62, 5, false),
                    ("alpha.map", 63, 2000, false),
                    ("patch.lvl", 64, 1, false),
                ]),
            ),
            (37, table(&[("beta.map", 65, 7, false), ("ContentMeta.xbx", 66, 6, false)])),
            (38, table(&[("Updates", 39, s, true), ("updater.xbe", 61, 3, false)])),
            (
                39,
                table(&[
                    ("contentmeta.xbx.03", 61, 3, false),
                    ("default.xbe.03", 67, 0x1000, false),
                    ("default.xbe.04", 70, 0x1000, false),
                ]),
            ),
            (45, vec![0u8; SECTOR]),
            (46, vec![0u8; SECTOR]),
        ];
        for (sector, t) in dirs {
            img[sector * SECTOR..(sector + 1) * SECTOR].copy_from_slice(&t);
        }
        let mut fill =
            |sector: usize, bytes: &[u8]| img[sector * SECTOR..sector * SECTOR + bytes.len()].copy_from_slice(bytes);
        fill(62, b"meta1");
        fill(63, &[0xA5; 2000]);
        fill(64, b"p");
        fill(65, b"betamap");
        fill(66, b"meta03");
        fill(67, &xbe(HALO2_TITLE, 0x303));
        fill(70, &xbe(HALO2_TITLE, 0x304));
        img
    }

    fn base(version: u32) -> xbe::Certificate {
        xbe::Xbe::parse(&xbe(HALO2_TITLE, version)).unwrap().certificate
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-mappack-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plan_finds_packages_and_updates() {
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let dirs: Vec<_> = plan.packages.iter().map(|p| (p.dir.as_str(), p.offer, p.files.len())).collect();
        assert_eq!(dirs, [("00000001", 0x4D53_0064_0000_0001, 3), ("00000003", 0x4D53_0064_0000_0003, 2)]);
        let ups: Vec<u32> = plan.updates.iter().map(|(n, _)| *n).collect();
        assert_eq!(ups, [3, 4]);
        assert_eq!(plan.update_for(3).unwrap().path, "/demos/update/Updates/default.xbe.03");
        assert_eq!(plan.update_for(0x10a), None);
    }

    #[test]
    fn plan_refuses_a_disc_without_packages() {
        let mut img = disc();
        // Rename "dlc" so nothing matches.
        let at = 34 * SECTOR + 14;
        img[at..at + 3].copy_from_slice(b"xyz");
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        let err = Plan::from_entries(&iso.entries().unwrap()).unwrap_err();
        assert!(err.to_string().contains("no content packages"), "{err}");
    }

    #[test]
    fn plan_refuses_a_package_without_metadata() {
        let mut img = disc();
        let t = &img[37 * SECTOR..38 * SECTOR];
        let at = 37 * SECTOR + t.windows(15).position(|w| w == b"ContentMeta.xbx").unwrap();
        img[at..at + 15].copy_from_slice(b"ContentMeta.xyz");
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        let err = Plan::from_entries(&iso.entries().unwrap()).unwrap_err();
        assert!(err.to_string().contains("00000003 has no ContentMeta"), "{err}");
    }

    #[test]
    fn install_lays_packages_out_as_vfs_expects_and_keeps_the_update_aside() {
        let d = scratch("install");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let written = install(&mut iso, &plan, &out, &data, Some(&base(3)), false).unwrap();
        assert_eq!(written.len(), 6);

        let cfg = vfs::Config::new(&data, HALO2_TITLE);
        let one = cfg.content_dir(0x4D53_0064_0000_0001);
        assert_eq!(fs::read(one.join("ContentMeta.xbx")).unwrap(), b"meta1");
        assert_eq!(fs::read(one.join("alpha.map")).unwrap(), vec![0xA5; 2000]);
        assert_eq!(fs::read(one.join("patch.lvl")).unwrap(), b"p");
        let three = cfg.content_dir(0x4D53_0064_0000_0003);
        assert_eq!(fs::read(three.join("beta.map")).unwrap(), b"betamap");
        // The update matching base version 3, under its own version, not in TDATA.
        let up = data.join("updates/4D530064/00000303/default.xbe");
        assert_eq!(fs::read(&up).unwrap(), xbe(HALO2_TITLE, 0x303));
        assert!(!cfg.title_data_dir().join("$u").exists());

        // A second run refuses to overwrite without --force, and succeeds with it.
        assert!(install(&mut iso, &plan, &out, &data, None, false).is_err());
        let forced = OutDir::new(&data, true).unwrap();
        install(&mut iso, &plan, &forced, &data, Some(&base(3)), false).unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn dry_run_writes_nothing() {
        let d = scratch("dry");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let listed = install(&mut iso, &plan, &out, &data, Some(&base(4)), true).unwrap();
        assert_eq!(listed.len(), 6);
        assert_eq!(listed[5], data.join("updates/4D530064/00000304/default.xbe"));
        assert_eq!(fs::read_dir(&data).unwrap().count(), 0);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn install_refuses_a_base_version_with_no_update() {
        let d = scratch("noupdate");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let err = install(&mut iso, &plan, &out, &data, Some(&base(9)), true).unwrap_err();
        assert!(err.to_string().contains("it has 03, 04"), "{err}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn check_update_rejects_other_titles_and_versions() {
        assert_eq!(check_update(&xbe(HALO2_TITLE, 0x303), &base(3)).unwrap(), 0x303);
        assert!(check_update(&xbe(HALO2_TITLE, 0x304), &base(3)).is_err());
        assert!(check_update(&xbe(HALO2_TITLE, 0x3), &base(3)).is_err(), "the base itself is not an update");
        assert!(check_update(&xbe(0x4D53_80CD, 0x303), &base(3)).is_err());
        assert!(check_update(b"not an xbe", &base(3)).is_err());
    }

    #[test]
    fn parses_args() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let a = parse_args(&s(&["mp.iso", "--data", "/d", "--no-update", "--dry-run"])).unwrap();
        assert_eq!(
            a,
            Args {
                iso: "mp.iso".into(),
                data: Some("/d".into()),
                disc: None,
                update: false,
                force: false,
                dry_run: true
            }
        );
        assert!(parse_args(&s(&[])).is_err());
        assert!(parse_args(&s(&["a.iso", "b.iso"])).is_err());
        assert!(parse_args(&s(&["a.iso", "--bogus"])).is_err());
    }
}
