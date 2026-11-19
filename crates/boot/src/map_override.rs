//! Edited maps in place of the disc's.
//!
//! - Map packages in `<data>/maps/` (docs/game/maps.md, `vfs::PackageBackend`):
//!   a base plus patches, served as `D:\maps\<name>.map`, or for a
//!   Multiplayer Map Pack base as the installed `T:\$c\<offer>\<name>.map`
//!   (`vfs::Config::map_packages`).
//! - `H2_MAP_DIR` names a host directory whose `<name>.map` files the game
//!   reads as `D:\maps\<name>.map` (`vfs::MapOverrideBackend`,
//!   docs/runtime/storage.md "Map overrides", docs/editor/scenario-editor.md "How patched
//!   maps reach the game"). It wins over a package for the same map.
//!
//! With neither (no packages directory, `H2_MAP_DIR` unset or empty), the
//! DVD is mounted exactly as before. The boot always passes the packages
//! directory, so its packages layer is there even while the folder is
//! empty, ready for one installed while the game runs; with no package it
//! serves the disc unchanged.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The environment variable naming the override directory.
pub const ENV: &str = "H2_MAP_DIR";

/// The override directory an `H2_MAP_DIR` value names; unset or empty is none.
pub fn dir_from(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The override directory from the environment.
pub fn dir() -> Option<PathBuf> {
    dir_from(std::env::var_os(ENV))
}

/// Which map exactly the game gets for `maps\<name>.map`: a package's
/// `MapId`, or a whole-file override's (`H2_MAP_DIR`). For a map pack
/// map (`content::map_pack`), the package served over its installed copy
/// under `data_root`. `None` means the retail map. For sessions and
/// telemetry: pass the loaded map's short name (the cache header's,
/// `0x00547898`, docs/live/live-gamestate.md).
pub fn map_id(vfs: &mut vfs::Vfs, data_root: &Path, name: &str) -> Option<content::MapId> {
    if let Some(id) = vfs.map_id(&format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE)) {
        return Some(id);
    }
    let path = map_pack_path(data_root, name)?;
    vfs.map_id(&path)
}

/// Where the host bytes of the loaded map named `name` are (its cache
/// header's name, `0x00547898`): `maps\<name>.map` when the disc has that
/// file, else the map in `maps` whose header carries that name. A
/// converted map is served under a disc map's file name with its own name
/// in the header (Halo 1's Blood Gulch is `h1_bloodgulch` in
/// `coagulation.map`, docs/halo1/multiplayer.md), so the file name
/// alone does not find it. Each candidate is read through the VFS's own
/// layers ([`vfs::Vfs::host_file`]), so an `H2_MAP_DIR` or package file is
/// the one found, as the game reads it. Tag names, telemetry and the
/// native world read the map through this.
pub fn map_file(vfs: &mut vfs::Vfs, name: &str) -> Option<vfs::HostFile> {
    let dir = format!("{}\\maps", vfs::CDROM_DEVICE);
    if let Some(file) = vfs.host_file(&format!("{dir}\\{name}.map")) {
        return Some(file);
    }
    let entries = vfs.list_directory(&dir).ok()?;
    entries
        .iter()
        .filter(|e| {
            let n = e.name.as_bytes();
            !e.metadata.is_directory()
                && n.len() > 4
                && n[n.len() - 4..].eq_ignore_ascii_case(b".map")
        })
        .filter_map(|e| vfs.host_file(&format!("{dir}\\{}", e.name)))
        .find(|file| header_name(file).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// The map name in a map file's header: 32 bytes at `+0x198`, as
/// `map::Map` reads it; `None` for a file too short or not text.
pub fn header_name(file: &vfs::HostFile) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const NAME: u64 = 0x198;
    const LEN: usize = 32;
    if file.len < NAME + LEN as u64 {
        return None;
    }
    let mut f = std::fs::File::open(&file.path).ok()?;
    f.seek(SeekFrom::Start(file.offset + NAME)).ok()?;
    let mut b = [0u8; LEN];
    f.read_exact(&mut b).ok()?;
    let end = b.iter().position(|c| *c == 0).unwrap_or(LEN);
    let name = std::str::from_utf8(&b[..end]).ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Where the VFS serves map pack map `name` installed under `data_root`:
/// `\Device\Harddisk0\Partition1\TDATA\<title>\$c\<offer>\<file>`.
pub fn map_pack_path(data_root: &Path, name: &str) -> Option<String> {
    let (offer, file, _) = crate::map_bases::map_pack_file(data_root, name)?;
    Some(format!(
        "{}\\TDATA\\{:08X}\\{}\\{offer}\\{file}",
        vfs::DATA_DEVICE,
        crate::new_maps::HALO2_TITLE,
        vfs::CONTENT_DIR
    ))
}

/// Where map packages live: `<data>/maps`.
pub fn packages_dir(data_root: &Path) -> PathBuf {
    data_root.join("maps")
}

/// Storage plus the DVD at `disc`, with the font rewrite if any, the
/// packages in `packages` (normally [`packages_dir`]), and the disc's maps
/// overridden from `maps` if given. With neither this is exactly
/// `Vfs::with_disc` / `Vfs::with_disc_rewrite`.
pub fn mount<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
    packages: Option<&Path>,
) -> vfs::Result<vfs::Vfs> {
    mount_with_hashes(
        cfg,
        disc,
        fonts,
        maps,
        packages,
        None,
        content::DiscHashes::default(),
        &[],
    )
}

/// [`mount`], loading `sandbox`'s variant of each converted Halo 1 map
/// (the `multiplayer.halo1_sandbox` setting, as `boot::new_maps` lists
/// them; `None` is the default, Halo 1's), with the package resolver recording each disc base it hashes
/// in `hashes` (shared with [`crate::map_bases::DiscBases`]), and
/// `campaign_maps` (map name, host file) served as `maps\<map>.map` where
/// the disc has no such map (docs/halo1/campaign-picker.md §3).
///
/// Given a packages directory, the packages layer is always mounted, even
/// with no package there yet, over the disc and (`vfs::Config::map_packages`)
/// over the map pack's installed maps on the data partition: a package installed while the game runs (a
/// Live join, the editor) is served at the map's next open
/// (`vfs::PackageBackend`, docs/game/maps.md §3). With an empty folder it serves
/// the disc's bytes unchanged.
#[allow(clippy::too_many_arguments)]
pub fn mount_with_hashes<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
    packages: Option<&Path>,
    sandbox: Option<&str>,
    hashes: content::DiscHashes,
    campaign_maps: &[(String, PathBuf)],
) -> vfs::Result<vfs::Vfs> {
    let mut cfg = cfg;
    if let Some(p) = packages {
        cfg.map_packages = Some(p.to_path_buf());
    }
    let library =
        packages.map(|p| content::Library::open_with_sandbox(p, sandbox).with_disc_hashes(hashes));
    if library.is_none() && maps.is_none() && campaign_maps.is_empty() {
        return match fonts {
            Some(r) => vfs::Vfs::with_disc_rewrite(cfg, disc, r),
            None => vfs::Vfs::with_disc(cfg, disc),
        };
    }
    // A level's Halo 2-assets map, when there is one, is served while the
    // pregame screen chooses Halo 2's assets (docs/halo1/campaign.md §5).
    let dvd = vfs::open_disc_with_mapping(disc, cfg.mmap_disc)?;
    let disc = vfs::CampaignMapsBackend::new(dvd, campaign_maps.iter().cloned())
        .with_assets(
            crate::h1_variants::campaign_choice(),
            crate::h1_variants::campaign_alternates(campaign_maps),
        );
    if !campaign_maps.is_empty() {
        eprintln!(
            "campaigns: {} map(s) served from <data>/campaigns",
            campaign_maps.len()
        );
    }
    if let Some(l) = &library {
        eprintln!(
            "map packages: {} package(s) in {}",
            l.packages().len(),
            l.root().display()
        );
    }
    if let Some(maps) = maps {
        eprintln!(
            "map override: maps from {} replace the disc's",
            maps.display()
        );
    }
    match (library, maps) {
        (Some(l), Some(m)) => finish(
            cfg,
            vfs::MapOverrideBackend::new(vfs::PackageBackend::new(disc, l), m),
            fonts,
        ),
        (Some(l), None) => finish(cfg, vfs::PackageBackend::new(disc, l), fonts),
        (None, Some(m)) => finish(cfg, vfs::MapOverrideBackend::new(disc, m), fonts),
        (None, None) => finish(cfg, disc, fonts),
    }
}

fn finish<B: vfs::Backend + 'static, R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: B,
    fonts: Option<R>,
) -> vfs::Result<vfs::Vfs> {
    match fonts {
        // Fonts live under `fonts\`, maps under `maps\`: the layers never meet.
        Some(r) => vfs::Vfs::with_disc_backend(cfg, vfs::RewriteBackend::new(disc, r)),
        None => vfs::Vfs::with_disc_backend(cfg, disc),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = base.join(format!(
                "h2-boot-map-override-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(p.join("maps")).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dirent(right: u16, sector: u32, size: u32, directory: bool, name: &str) -> Vec<u8> {
        let mut b = vec![0; 14];
        b[2..4].copy_from_slice(&right.to_le_bytes());
        b[4..8].copy_from_slice(&sector.to_le_bytes());
        b[8..12].copy_from_slice(&size.to_le_bytes());
        b[12] = if directory { 0x10 } else { 0 };
        b[13] = name.len() as u8;
        b.extend_from_slice(name.as_bytes());
        b.resize(b.len().next_multiple_of(4), 0);
        b
    }
    /// An XISO with `fonts\font` ("glyphs") and `maps\level.map` ("disc map").
    fn iso(t: &Temp) -> PathBuf {
        iso_at(&t.0, b"disc map")
    }
    /// That XISO in `dir`, with `level` as `maps\level.map` (at most one
    /// sector).
    pub(crate) fn iso_at(dir: &Path, level: &[u8]) -> PathBuf {
        let mut image = vec![0; 40 * 2048];
        image[32 * 2048..][..20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        image[32 * 2048 + 20..][..4].copy_from_slice(&33u32.to_le_bytes());
        let first = dirent(0, 34, 2048, true, "fonts");
        let root = [
            dirent((first.len() / 4) as u16, 34, 2048, true, "fonts"),
            dirent(0, 35, 2048, true, "maps"),
        ]
        .concat();
        image[32 * 2048 + 24..][..4].copy_from_slice(&(root.len() as u32).to_le_bytes());
        image[33 * 2048..][..root.len()].copy_from_slice(&root);
        let fonts = dirent(0, 36, 6, false, "font");
        image[34 * 2048..][..fonts.len()].copy_from_slice(&fonts);
        let maps = dirent(0, 37, level.len() as u32, false, "level.map");
        image[35 * 2048..][..maps.len()].copy_from_slice(&maps);
        image[36 * 2048..][..6].copy_from_slice(b"glyphs");
        image[37 * 2048..][..level.len()].copy_from_slice(level);
        let path = dir.join("disc.iso");
        std::fs::write(&path, image).unwrap();
        path
    }
    /// Upper-cases every file under `fonts\`.
    pub(crate) struct Upper;
    impl vfs::Rewrite for Upper {
        fn wants(&self, path: &str) -> bool {
            path.to_ascii_lowercase().starts_with("fonts\\")
        }
        fn rewrite(&mut self, _: &str, original: &[u8]) -> Option<Vec<u8>> {
            Some(original.to_ascii_uppercase())
        }
    }
    fn read(v: &mut vfs::Vfs, name: &str) -> Vec<u8> {
        let h = v.open(name, vfs::OpenOptions::default()).unwrap().handle;
        let mut buf = vec![0; v.query_information(h).unwrap().metadata.end_of_file as usize];
        assert_eq!(
            v.read(h, vfs::Offset::Absolute(0), &mut buf).unwrap(),
            buf.len()
        );
        v.close(h).unwrap();
        buf
    }
    fn mounted(t: &Temp, fonts: bool, maps: Option<&Path>) -> vfs::Vfs {
        mounted_with(t, fonts, maps, None)
    }
    fn mounted_with(
        t: &Temp,
        fonts: bool,
        maps: Option<&Path>,
        packages: Option<&Path>,
    ) -> vfs::Vfs {
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(t), fonts.then_some(Upper), maps, packages).unwrap();
        v.namespace
            .create_symbolic_link("\\??\\D:", vfs::CDROM_DEVICE)
            .unwrap();
        v
    }

    #[test]
    fn unset_or_empty_env_means_no_override() {
        assert_eq!(dir_from(None), None);
        assert_eq!(dir_from(Some(OsString::new())), None);
        assert_eq!(
            dir_from(Some("/x/maps".into())),
            Some(PathBuf::from("/x/maps"))
        );
    }

    #[test]
    fn without_a_directory_the_disc_and_fonts_are_served_as_before() {
        let t = Temp::new();
        std::fs::write(t.0.join("maps").join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted(&t, true, None);
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
        assert_eq!(read(&mut v, "D:\\fonts\\font"), b"GLYPHS");
        let mut v = mounted(&t, false, None);
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
        assert_eq!(read(&mut v, "D:\\fonts\\font"), b"glyphs");
    }

    #[test]
    fn with_a_directory_maps_are_overridden_and_fonts_still_rewritten() {
        let t = Temp::new();
        let dir = t.0.join("maps");
        std::fs::write(dir.join("LEVEL.MAP"), b"edited!!!").unwrap();
        for fonts in [true, false] {
            let mut v = mounted(&t, fonts, Some(&dir));
            assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"edited!!!");
            assert_eq!(
                v.host_file("\\Device\\CdRom0\\maps\\level.map")
                    .unwrap()
                    .len,
                9
            );
            let font: &[u8] = if fonts { b"GLYPHS" } else { b"glyphs" };
            assert_eq!(read(&mut v, "D:\\fonts\\font"), font);
        }
    }

    /// A map file whose header names it `name`.
    fn named_map(name: &str) -> Vec<u8> {
        let mut b = vec![0u8; 0x800];
        b[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
        b
    }

    #[test]
    fn a_map_is_found_by_its_file_name_or_by_the_name_in_its_header() {
        let t = Temp::new();
        let dir = t.0.join("maps");
        // A converted map served as the disc's level.map, named otherwise.
        std::fs::write(dir.join("level.map"), named_map("h1_bloodgulch")).unwrap();
        let mut v = mounted(&t, false, Some(&dir));
        let by_header = map_file(&mut v, "h1_bloodgulch").unwrap();
        assert_eq!(
            by_header.path,
            dir.join("level.map").canonicalize().unwrap()
        );
        assert_eq!((by_header.offset, by_header.len), (0, 0x800));
        assert_eq!(header_name(&by_header).as_deref(), Some("h1_bloodgulch"));
        // By its file name, as before.
        assert_eq!(map_file(&mut v, "level"), Some(by_header));
        assert_eq!(map_file(&mut v, "h1_sidewinder"), None);
        // Without the override the disc's level.map ("disc map", no
        // header) names nothing.
        let mut v = mounted(&t, false, None);
        assert!(map_file(&mut v, "level").is_some());
        assert_eq!(map_file(&mut v, "h1_bloodgulch"), None);
    }

    #[test]
    fn a_missing_directory_boots_from_the_disc() {
        let t = Temp::new();
        let mut v = mounted(&t, false, Some(&t.0.join("absent")));
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
    }

    /// A package in `<data>/maps` for `level`: an IPS patch over the disc's
    /// map ("disc map" becomes "DISC map").
    fn level_package(t: &Temp) -> PathBuf {
        let root = packages_dir(&t.0.join("data"));
        let dir = root.join("level-mod");
        std::fs::create_dir_all(&dir).unwrap();
        let patch = b"PATCH\0\0\0\0\x04DISCEOF";
        std::fs::write(dir.join("a.ips"), patch).unwrap();
        let mut m = content::Manifest::retail("level", content::Sha256::of(b"disc map"));
        m.patches.push(content::manifest::Layer {
            file: "a.ips".into(),
            sha256: content::Sha256::of(patch),
        });
        std::fs::write(dir.join("manifest.toml"), m.to_toml()).unwrap();
        root
    }

    /// Everything the guest sees of the disc, for comparing mounts.
    fn observe(v: &mut vfs::Vfs) -> Vec<String> {
        let mut seen = Vec::new();
        for name in ["D:\\maps\\level.map", "D:\\fonts\\font"] {
            seen.push(format!("{:?}", v.query_path(name)));
            seen.push(format!("{:?}", read(v, name)));
            seen.push(format!("{:?}", v.host_file(name)));
        }
        seen
    }

    #[test]
    fn without_packages_the_mount_is_unchanged() {
        let t = Temp::new();
        for fonts in [true, false] {
            let expected = observe(&mut mounted(&t, fonts, None));
            let root = packages_dir(&t.0.join("data"));
            // Missing, empty, holding only a cache or a broken package.
            assert_eq!(
                observe(&mut mounted_with(&t, fonts, None, Some(&root))),
                expected
            );
            std::fs::create_dir_all(root.join("cache")).unwrap();
            std::fs::create_dir_all(root.join("broken")).unwrap();
            std::fs::write(root.join("broken").join("manifest.toml"), "x").unwrap();
            assert_eq!(
                observe(&mut mounted_with(&t, fonts, None, Some(&root))),
                expected
            );
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn packages_are_served_and_fonts_still_rewritten() {
        let t = Temp::new();
        let root = level_package(&t);
        for fonts in [true, false] {
            let mut v = mounted_with(&t, fonts, None, Some(&root));
            assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"DISC map");
            let font: &[u8] = if fonts { b"GLYPHS" } else { b"glyphs" };
            assert_eq!(read(&mut v, "D:\\fonts\\font"), font);
        }
    }

    #[test]
    fn map_ids_name_packages_and_overrides_and_retail_is_none() {
        let t = Temp::new();
        // Retail: nothing to say.
        let mut v = mounted(&t, false, None);
        assert_eq!(map_id(&mut v, &t.0.join("data"), "level"), None);
        // A package: its manifest's id, through the font rewrite too.
        let root = level_package(&t);
        let mut ids = Vec::new();
        for fonts in [true, false] {
            let mut v = mounted_with(&t, fonts, None, Some(&root));
            ids.push(map_id(&mut v, &t.0.join("data"), "LEVEL").expect("a package names its map"));
            assert_eq!(map_id(&mut v, &t.0.join("data"), "other"), None);
        }
        assert_eq!(ids[0], ids[1]);
        // An override wins and has an id of its own, stable per contents.
        let dir = t.0.join("maps");
        std::fs::write(dir.join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        let o = map_id(&mut v, &t.0.join("data"), "level").unwrap();
        assert_ne!(o, ids[0]);
        assert_eq!(map_id(&mut v, &t.0.join("data"), "level"), Some(o));
        std::fs::write(dir.join("level.map"), b"edited again").unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        assert_ne!(map_id(&mut v, &t.0.join("data"), "level"), Some(o));
    }

    #[test]
    fn the_map_dir_wins_over_a_package() {
        let t = Temp::new();
        let root = level_package(&t);
        let dir = t.0.join("maps");
        std::fs::write(dir.join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"edited!!!");
        assert_eq!(read(&mut v, "D:\\fonts\\font"), b"GLYPHS");
        // Without the override file, the package shows through.
        std::fs::remove_file(dir.join("level.map")).unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"DISC map");
    }
}
