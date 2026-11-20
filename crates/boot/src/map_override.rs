//! Explicit editor overrides (`H2_MAP_DIR`) and map identity through
//! the VFS. Installed mods use their own content paths.
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

/// The package the game loaded, or an explicit editor override.
pub fn map_id(vfs: &mut vfs::Vfs, name: &str) -> Option<content::MapId> {
    if let Some(path) = vfs.content_map_path(name) {
        return vfs.map_id(&path);
    }
    vfs.map_id(&format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE))
}

/// The content package selected in the multiplayer lobby, or the map
/// currently loaded in a match. The selected list slot is authoritative
/// in the main menu, where the cache header still says `mainmenu`.
pub fn lobby_map_id(vfs: &mut vfs::Vfs, g: &runtime::Guest) -> Option<content::MapId> {
    if let Some(session) = crate::coop_probe::read_session(g).filter(|s| s.live()) {
        if !session.is_host() || session.is_campaign() {
            return None;
        }
        return vfs.map_id(&crate::map_select::selected_path(session.map)?);
    }
    let loaded = crate::telemetry::loaded_map_name(g).ok();
    if let Some(name) = loaded
        .as_deref()
        .filter(|n| !n.is_empty() && *n != "mainmenu")
    {
        return map_id(vfs, name);
    }

    use crate::layout_glue::Gm;
    let id = Gm::u32(g, crate::h1_options_row::LOBBY_MAP)?;
    vfs.map_id(&crate::map_select::selected_path(id)?)
}

/// The host file for a loaded map. A streamed overlay has no contiguous
/// host file, even though the VFS can read its bytes.
pub fn map_file(vfs: &mut vfs::Vfs, name: &str) -> Option<vfs::HostFile> {
    if let Some(path) = vfs.content_map_path(name) {
        return vfs.host_file(&path);
    }
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

/// Where map packages live: `<data>/maps`.
pub fn packages_dir(data_root: &Path) -> PathBuf {
    data_root.join("maps")
}

/// Mount storage and the DVD, with any explicit editor override and
/// campaign additions. The data partition serves registered mod paths.
pub fn mount<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
    campaign_maps: &[(String, PathBuf)],
) -> vfs::Result<vfs::Vfs> {
    if maps.is_none() && cfg.server_maps.is_none() && campaign_maps.is_empty() {
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
    if let Some(server_maps) = cfg.server_maps.clone() {
        let disc = vfs::MapOverrideBackend::server_maps(disc, server_maps);
        return match maps {
            Some(maps) => finish(cfg, vfs::MapOverrideBackend::new(disc, maps), fonts),
            None => finish(cfg, disc, fonts),
        };
    }
    match maps {
        Some(maps) => {
            eprintln!(
                "map override: maps from {} replace the disc's",
                maps.display()
            );
            finish(cfg, vfs::MapOverrideBackend::new(disc, maps), fonts)
        }
        None => finish(cfg, disc, fonts),
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
        _packages: Option<&Path>,
    ) -> vfs::Vfs {
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(t), fonts.then_some(Upper), maps, &[]).unwrap();
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
    fn legacy_packages_do_not_shadow_disc_or_fonts() {
        let t = Temp::new();
        let root = level_package(&t);
        for fonts in [true, false] {
            let mut v = mounted_with(&t, fonts, None, Some(&root));
            assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
            let font: &[u8] = if fonts { b"GLYPHS" } else { b"glyphs" };
            assert_eq!(read(&mut v, "D:\\fonts\\font"), font);
        }
    }

    #[test]
    fn retail_has_no_package_id_even_with_legacy_packages() {
        let t = Temp::new();
        let root = level_package(&t);
        let mut v = mounted_with(&t, false, None, Some(&root));
        assert_eq!(map_id(&mut v, "level"), None);
        let dir = t.0.join("maps");
        std::fs::write(dir.join("level.map"), b"edited!!!").unwrap();
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        let override_id = map_id(&mut v, "level").unwrap();
        // An editor publishes a replacement. The current mount holds its
        // original file open and caches its identity on first use.
        std::fs::write(dir.join("next.map"), b"edited again").unwrap();
        std::fs::rename(dir.join("next.map"), dir.join("level.map")).unwrap();
        assert_eq!(map_id(&mut v, "level"), Some(override_id));
        assert_eq!(read(&mut v, r"D:\maps\level.map"), b"edited!!!");
        drop(v);
        let mut v = mounted_with(&t, true, Some(&dir), Some(&root));
        assert_eq!(read(&mut v, r"D:\maps\level.map"), b"edited again");
        assert_ne!(map_id(&mut v, "level").unwrap(), override_id);
    }
}
