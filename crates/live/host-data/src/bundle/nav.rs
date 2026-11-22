//! The bots' data for every bundled multiplayer map: each structure BSP's
//! nav mesh (`<map>.<bsp>.bots`) and visibility grid (`<map>.<bsp>.vis`),
//! as `bots/<file>` in the archive. A dedicated host places them in its
//! data root ([`crate::install_bot_data`]), so neither a sim child (which
//! builds no meshes) nor a light host builds them at boot.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use bots::{
    autonav::{self, MapHeader, Region, Source as NavSource},
    cache::{self, MapData, Stamp},
    vis::{self, Vis},
};

use super::{HEADER, Payload, Source};
use crate::{BOTS_DIR, Error};

/// Where the bots' files come from, for [`super::BundleSpec::bots`].
#[derive(Clone, Debug)]
pub struct BotData {
    /// The bots directory read and built into: a data root's `bots`
    /// (`bots::cache::dir`), where `h2 bots nav` and the game write.
    pub dir: PathBuf,
    /// Build missing or stale maps first with the nav worker, as
    /// `h2 bots nav all` does (restoring from the shared cache of
    /// completed builds when it can). Off: every map's files must
    /// already be current.
    pub build: bool,
}

/// One multiplayer map going into the bundle.
pub(super) struct NavMap {
    /// The disc file's stem (`lockout`).
    pub key: String,
    pub header: [u8; HEADER],
    pub size: u64,
    pub source: Source,
}

/// Build (when asked) and check every map's files, copy them into
/// `staging`, and add them to `payload`. Returns how many maps carry
/// bot data: every one of `maps`, or an error naming the first that
/// can't.
pub(super) fn stage(
    bots: &BotData,
    maps: &[NavMap],
    staging: &Path,
    payload: &mut Vec<Payload>,
) -> Result<usize, Error> {
    if maps.is_empty() {
        return Ok(0);
    }
    if bots.build {
        build(&bots.dir, maps)?;
    }
    let out = staging.join(BOTS_DIR);
    crate::private_dir(&out, false)?;
    for map in maps {
        stage_map(&bots.dir, map, &out, payload)?;
    }
    payload.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(maps.len())
}

/// The nav worker over the bundled maps, as read from the disc.
fn build(dir: &Path, maps: &[NavMap]) -> Result<(), Error> {
    fs::create_dir_all(dir)?;
    let mut sources: Option<Vec<Box<dyn NavSource>>> = Some(
        maps.iter()
            .map(|map| region(map.key.clone(), &map.source, map.size))
            .collect(),
    );
    let (worker, _) = autonav::Worker::new(
        dir.to_path_buf(),
        Box::new(move || sources.take().unwrap_or_default()),
    );
    let summary = worker
        .shared_cache(autonav::shared_cache_dir())
        .log(|line| eprintln!("host-data: {line}"))
        .run();
    if !summary.failed.is_empty() {
        return Err(Error::Bundle(format!(
            "bot nav data failed to build for {}",
            summary.failed.join(", ")
        )));
    }
    Ok(())
}

fn region(key: String, source: &Source, size: u64) -> Box<dyn NavSource> {
    let reader = |path: PathBuf| -> io::Result<Box<dyn autonav::ReadSeek>> {
        Ok(Box::new(io::BufReader::new(fs::File::open(path)?)))
    };
    match source.clone() {
        Source::IsoRegion(iso, offset, len) => Box::new(Region::new(
            key,
            offset,
            len,
            Box::new(move || reader(iso.clone())),
        )),
        Source::File(path) => Box::new(Region::new(
            key,
            0,
            size,
            Box::new(move || reader(path.clone())),
        )),
        Source::Bytes(bytes) => Box::new(Region::new(
            key,
            0,
            size,
            Box::new(move || Ok(Box::new(io::Cursor::new(bytes.clone())))),
        )),
    }
}

/// Every BSP's `.bots` and `.vis` of one map, each checked against the
/// map's header: this generator, this map, a grid built from that mesh's
/// walls. A staged copy is what is checked and what is archived.
fn stage_map(
    dir: &Path,
    map: &NavMap,
    out: &Path,
    payload: &mut Vec<Payload>,
) -> Result<(), Error> {
    let bad = |why: String| Error::Bundle(format!("maps/{}.map: {why}", map.key));
    let header = MapHeader::parse(&map.header).map_err(bad)?;
    let hint = "`h2 bots nav all --disc <disc> --data <data>` builds it";
    let status = autonav::status(dir, &header.name, header.fingerprint);
    if status.stale() {
        return Err(bad(format!(
            "no current bot nav mesh in {} ({}); {hint}",
            dir.display(),
            status.why()
        )));
    }
    let stamp = Stamp::now(header.fingerprint);
    for bsp in 0.. {
        let nav_name = cache::file_name(&header.name, bsp);
        let Some(nav) = copy_in(&dir.join(&nav_name), &out.join(&nav_name))? else {
            if bsp == 0 {
                return Err(bad(format!("{nav_name} is missing; {hint}")));
            }
            break;
        };
        let data = match MapData::from_bytes(&read(&nav)?) {
            Some((data, built)) if built == stamp => data,
            // A higher BSP left from an older build of a map with more of
            // them: not this build's, so the map ends before it.
            _ if bsp > 0 => {
                fs::remove_file(&nav)?;
                break;
            }
            _ => return Err(bad(format!("{nav_name} is damaged; {hint}"))),
        };
        push(payload, &nav_name, nav)?;

        let vis_name = vis::file_name(&header.name, bsp);
        let Some(grid) = copy_in(&dir.join(&vis_name), &out.join(&vis_name))? else {
            return Err(bad(format!("{vis_name} is missing; {hint}")));
        };
        match Vis::from_bytes(&read(&grid)?) {
            Ok((vis, built)) if built == stamp && vis.fits(&data.walls) => {}
            Ok(_) => {
                return Err(bad(format!(
                    "{vis_name} was built for another nav build; {hint}"
                )));
            }
            Err(status) => return Err(bad(format!("{vis_name}: {}; {hint}", status.why()))),
        }
        push(payload, &vis_name, grid)?;
    }
    Ok(())
}

/// Copy `from` to `to`; `None` when there is no `from`.
fn copy_in(from: &Path, to: &Path) -> Result<Option<PathBuf>, Error> {
    match fs::symlink_metadata(from) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(Error::Bundle(format!(
                "{} must be a regular file",
                from.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut out = crate::new_private_file(to)?;
    io::copy(&mut fs::File::open(from)?, &mut out)?;
    Ok(Some(to.to_path_buf()))
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn push(payload: &mut Vec<Payload>, name: &str, staged: PathBuf) -> Result<(), Error> {
    let path = format!("{BOTS_DIR}/{name}");
    crate::validate_payload_path(&path)?;
    let size = fs::metadata(&staged)?.len();
    payload.push(Payload {
        path,
        size,
        source: Source::File(staged),
        sha256: String::new(),
    });
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::testkit::{self, TempDir};

    /// Current `.bots` and `.vis` files for the synthetic disc map `name`,
    /// as the nav worker writes them, in the data root `data`.
    pub(crate) fn write_current(data: &Path, disc: &Path, name: &str) {
        let map = testkit::read_all(&disc.join(format!("maps/{name}.map")));
        let header = MapHeader::parse(&map[..HEADER]).unwrap();
        autonav::write_map(
            &cache::dir(data),
            &header.name,
            header.fingerprint,
            &[MapData::default()],
        )
        .unwrap();
        let walls = MapData::default().walls;
        let grid = vis::build(&Default::default(), &walls);
        let stamp = Stamp::now(header.fingerprint);
        fs::write(
            cache::dir(data).join(vis::file_name(&header.name, 0)),
            grid.to_bytes(stamp),
        )
        .unwrap();
    }

    fn spec(tmp: &TempDir, data: &Path) -> crate::BundleSpec {
        let disc = tmp.join("disc");
        if !disc.exists() {
            testkit::write_disc_dir(&disc);
        }
        crate::BundleSpec {
            disc,
            out: tmp.join("bundle"),
            stripped: None,
            sim_stores: None,
            bots: Some(BotData {
                dir: cache::dir(data),
                build: false,
            }),
            force: true,
            zstd_level: 1,
        }
    }

    #[test]
    fn every_multiplayer_map_carries_its_nav_mesh_and_grid() {
        let tmp = TempDir::new("nav-bundle");
        let data = tmp.join("data");
        let spec = spec(&tmp, &data);
        for map in ["lockout", "zanzibar"] {
            write_current(&data, &spec.disc, map);
        }
        // Another map's files, and a leftover BSP from an older build of
        // lockout, stay out.
        fs::write(cache::dir(&data).join("a30_deltahalo.0.bots"), b"campaign").unwrap();
        fs::write(
            cache::dir(&data).join(cache::file_name("lockout", 1)),
            MapData::default().to_bytes(Stamp::now(1)),
        )
        .unwrap();
        let built = crate::build(&spec).unwrap();
        assert_eq!(built.nav_maps, 2);
        let paths: Vec<&str> = built
            .manifest
            .files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| p.starts_with("bots/"))
            .collect();
        assert_eq!(
            paths,
            [
                "bots/lockout.0.bots",
                "bots/lockout.0.vis",
                "bots/zanzibar.0.bots",
                "bots/zanzibar.0.vis"
            ]
        );
        let lockout = built
            .manifest
            .files
            .iter()
            .find(|f| f.path == "bots/lockout.0.bots")
            .unwrap();
        let original = testkit::read_all(&cache::dir(&data).join("lockout.0.bots"));
        assert_eq!(lockout.sha256, testkit::sha256_hex(&original));
    }

    #[test]
    fn a_stale_or_missing_map_is_refused_by_name() {
        let tmp = TempDir::new("nav-stale");
        let data = tmp.join("data");
        let spec = spec(&tmp, &data);
        write_current(&data, &spec.disc, "lockout");
        let err = crate::build(&spec).unwrap_err().to_string();
        assert!(
            err.contains("maps/zanzibar.map") && err.contains("none yet"),
            "{err}"
        );

        // Built from another map file of the same name.
        autonav::write_map(&cache::dir(&data), "zanzibar", 7, &[MapData::default()]).unwrap();
        let err = crate::build(&spec).unwrap_err().to_string();
        assert!(err.contains("the map changed"), "{err}");

        // A current mesh without its grid.
        write_current(&data, &spec.disc, "zanzibar");
        fs::remove_file(cache::dir(&data).join(vis::file_name("zanzibar", 0))).unwrap();
        let err = crate::build(&spec).unwrap_err().to_string();
        assert!(err.contains("zanzibar.0.vis is missing"), "{err}");
    }

    #[test]
    fn a_grid_from_another_build_is_refused() {
        let tmp = TempDir::new("nav-vis");
        let data = tmp.join("data");
        let spec = spec(&tmp, &data);
        for map in ["lockout", "zanzibar"] {
            write_current(&data, &spec.disc, map);
        }
        let grid = vis::build(&Default::default(), &MapData::default().walls);
        fs::write(
            cache::dir(&data).join(vis::file_name("lockout", 0)),
            grid.to_bytes(Stamp::now(3)),
        )
        .unwrap();
        let err = crate::build(&spec).unwrap_err().to_string();
        assert!(err.contains("lockout.0.vis"), "{err}");
    }
}
