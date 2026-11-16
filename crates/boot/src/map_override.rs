//! Edited maps in place of the disc's: `H2_MAP_DIR` names a host directory
//! whose `<name>.map` files the game reads as `D:\maps\<name>.map`
//! (`vfs::MapOverrideBackend`, docs/storage.md "Map overrides",
//! docs/scenario-editor.md "How patched maps reach the game").
//! Unset or empty, the DVD is mounted exactly as before.
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

/// Storage plus the DVD at `disc`, with the font rewrite if any, and the
/// disc's maps overridden from `maps` if given. Without `maps` this is
/// exactly `Vfs::with_disc` / `Vfs::with_disc_rewrite`.
pub fn mount<R: vfs::Rewrite + 'static>(
    cfg: vfs::Config,
    disc: &Path,
    fonts: Option<R>,
    maps: Option<&Path>,
) -> vfs::Result<vfs::Vfs> {
    let Some(maps) = maps else {
        return match fonts {
            Some(r) => vfs::Vfs::with_disc_rewrite(cfg, disc, r),
            None => vfs::Vfs::with_disc(cfg, disc),
        };
    };
    eprintln!(
        "map override: maps from {} replace the disc's",
        maps.display()
    );
    let disc = vfs::MapOverrideBackend::new(vfs::DiscBackend::open(disc)?, maps);
    match fonts {
        // Fonts live under `fonts\`, maps under `maps\`: the layers never meet.
        Some(r) => vfs::Vfs::with_disc_backend(cfg, vfs::RewriteBackend::new(disc, r)),
        None => vfs::Vfs::with_disc_backend(cfg, disc),
    }
}

#[cfg(test)]
mod tests {
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
        let maps = dirent(0, 37, 8, false, "level.map");
        image[35 * 2048..][..maps.len()].copy_from_slice(&maps);
        image[36 * 2048..][..6].copy_from_slice(b"glyphs");
        image[37 * 2048..][..8].copy_from_slice(b"disc map");
        let path = t.0.join("disc.iso");
        std::fs::write(&path, image).unwrap();
        path
    }
    /// Upper-cases every file under `fonts\`.
    struct Upper;
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
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = mount(cfg, &iso(t), fonts.then_some(Upper), maps).unwrap();
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

    #[test]
    fn a_missing_directory_boots_from_the_disc() {
        let t = Temp::new();
        let mut v = mounted(&t, false, Some(&t.0.join("absent")));
        assert_eq!(read(&mut v, "D:\\maps\\level.map"), b"disc map");
    }
}
