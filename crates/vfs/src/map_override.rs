//! A read-only backend that serves `maps\*.map` from a host directory.
//!
//! [`MapOverrideBackend`] wraps the disc. When the game names a map the disc
//! has (`maps\<name>.map`, exactly one level down) and the override directory
//! holds a file of the same name (ASCII case-insensitive, like the disc), that
//! file is served instead: its size in `stat` and in listings of `maps`, its
//! bytes in `read_at`, its path in `host_file`. Reads stream from the host
//! file; nothing is buffered, so a map of any size up to the 4 GiB XDVDFS
//! limit works. Everything else, and every map with no override, comes from
//! the inner backend unchanged.
//!
//! An override is chosen once per map, on first use, and kept for the
//! session: the open host file holds its bytes even if the file is replaced
//! later. The choice is reported once through the notice callback. A symlink
//! that leaves the directory, anything that is not a regular file, an
//! ambiguous name (two host names differing only in case), a file too large
//! for XDVDFS, or one that cannot be opened is refused with a warning, and
//! the disc's map is served. Nothing is ever written. See docs/runtime/storage.md
//! ("Map overrides") and docs/editor/scenario-editor.md.
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use crate::{Backend, DirectoryEntry, Error, HostFile, Metadata, Result, VolumeInfo};

/// Receives one line per decision: which file serves a map, or why an
/// override was refused.
pub type MapOverrideNotice = Box<dyn FnMut(&str) + Send>;

struct Override {
    file: File,
    path: PathBuf,
    len: u64,
    /// Its identity, hashed on first ask.
    id: Option<content::MapId>,
}

pub struct MapOverrideBackend<B> {
    inner: B,
    /// Canonical override directory; `None` if it could not be resolved, in
    /// which case every map comes from the inner backend.
    dir: Option<PathBuf>,
    /// By lowercase `maps\<name>.map`: the chosen override, or `None` for a
    /// map served from the inner backend.
    files: HashMap<String, Option<Override>>,
    notice: MapOverrideNotice,
}

/// Whether `path` names a map directly under `maps` (any case).
pub(crate) fn is_map_path(path: &str) -> Option<&str> {
    let (dir, name) = path.split_once('\\')?;
    if !dir.eq_ignore_ascii_case("maps") || name.contains('\\') {
        return None;
    }
    let ext = name.len().checked_sub(4).map(|i| &name.as_bytes()[i..])?;
    ext.eq_ignore_ascii_case(b".map").then_some(name)
}

impl<B: Backend> MapOverrideBackend<B> {
    /// Serve maps from `dir` over `inner`. A directory that does not exist or
    /// cannot be resolved is reported through the notice and ignored.
    pub fn new(inner: B, dir: impl AsRef<Path>) -> Self {
        Self::with_notice(
            inner,
            dir,
            Box::new(|line: &str| eprintln!("map override: {line}")),
        )
    }

    pub fn with_notice(inner: B, dir: impl AsRef<Path>, mut notice: MapOverrideNotice) -> Self {
        let dir = dir.as_ref();
        let dir = match dir.canonicalize() {
            Ok(d) if d.is_dir() => Some(d),
            Ok(_) => {
                notice(&format!(
                    "warning: {} is not a directory; using the disc's maps",
                    dir.display()
                ));
                None
            }
            Err(e) => {
                notice(&format!(
                    "warning: cannot use {} ({e}); using the disc's maps",
                    dir.display()
                ));
                None
            }
        };
        Self {
            inner,
            dir,
            files: HashMap::new(),
            notice,
        }
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    /// The override for `path`, choosing it on first use.
    fn chosen(&mut self, path: &str) -> Option<&mut Override> {
        let name = is_map_path(path)?;
        let dir = self.dir.as_ref()?;
        let key = path.to_ascii_lowercase();
        if !self.files.contains_key(&key) {
            // Only maps the disc has: a missing name stays missing.
            let on_disc = self.inner.stat(path).ok().filter(|m| !m.is_directory());
            let chosen = match on_disc {
                Some(disc) => match find(dir, name) {
                    Ok(None) => None,
                    Ok(Some(o)) => {
                        (self.notice)(&format!(
                            "maps\\{name} from {} ({} bytes; disc {} bytes)",
                            o.path.display(),
                            o.len,
                            disc.end_of_file
                        ));
                        Some(o)
                    }
                    Err(why) => {
                        (self.notice)(&format!(
                            "warning: maps\\{name}: {why}; using the disc's copy"
                        ));
                        None
                    }
                },
                None => None,
            };
            self.files.insert(key.clone(), chosen);
        }
        self.files.get_mut(&key)?.as_mut()
    }
}

/// `meta` for a file of `len` bytes: its size, sector-rounded allocation.
pub(crate) fn resize(meta: &mut Metadata, len: u64) {
    meta.end_of_file = len;
    meta.allocation_size = len.div_ceil(xiso::SECTOR) * xiso::SECTOR;
}

/// The host file in `dir` named `name` (ASCII case-insensitive), checked and
/// opened. `Ok(None)` when there is none; `Err` with a reason when there is
/// one that cannot be used.
fn find(dir: &Path, name: &str) -> std::result::Result<Option<Override>, String> {
    let mut found = None;
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        let host = entry.file_name();
        if host.to_str().is_some_and(|h| h.eq_ignore_ascii_case(name)) {
            if found.is_some() {
                return Err(format!(
                    "{} holds more than one file named {name} in different case",
                    dir.display()
                ));
            }
            found = Some(entry.path());
        }
    }
    let Some(path) = found else { return Ok(None) };
    let link = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let target = if link.file_type().is_symlink() {
        let target = path
            .canonicalize()
            .map_err(|e| format!("{}: broken link ({e})", path.display()))?;
        if !target.starts_with(dir) {
            return Err(format!(
                "{} links outside {} (to {})",
                path.display(),
                dir.display(),
                target.display()
            ));
        }
        target
    } else {
        path
    };
    let file = File::open(&target).map_err(|e| format!("cannot open {}: {e}", target.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{}: {e}", target.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", target.display()));
    }
    #[cfg(unix)]
    {
        // The object opened must be the one checked, not a later swap.
        use std::os::unix::fs::MetadataExt;
        let checked =
            std::fs::metadata(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        if (checked.dev(), checked.ino()) != (meta.dev(), meta.ino()) {
            return Err(format!("{} changed while it was opened", target.display()));
        }
    }
    if meta.len() > u64::from(u32::MAX) {
        return Err(format!(
            "{} is {} bytes, over the disc's 4 GiB file limit",
            target.display(),
            meta.len()
        ));
    }
    Ok(Some(Override {
        file,
        path: target,
        len: meta.len(),
        id: None,
    }))
}

/// The `MapId` of a whole-file override of map `map`.
pub(crate) fn override_id(map: &str, file: &Path) -> std::io::Result<content::MapId> {
    let mut f = File::open(file)?;
    let mut h = content::Hasher::default();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let manifest = content::Manifest {
        package: content::manifest::Package {
            format: content::manifest::FORMAT,
            name: map.to_owned(),
            author: None,
            description: None,
            image: None,
        },
        base: content::manifest::Base {
            map: map.to_owned(),
            sha256: None,
            derived: true,
        },
        patches: vec![content::manifest::Layer {
            file: format!("{map}.map"),
            sha256: h.finish(),
        }],
        packs: Vec::new(),
        result: None,
        list: None,
    };
    Ok(content::MapId::of(&manifest))
}

impl<B: Backend> Backend for MapOverrideBackend<B> {
    fn opened(&mut self, path: &str) {
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    /// An `H2_MAP_DIR` file is chosen once per session: never changed.
    fn map_changed(&mut self, path: &str) -> bool {
        self.chosen(path).is_none() && self.inner.map_changed(path)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let mut meta = self.inner.stat(path)?;
        if let Some(o) = self.chosen(path) {
            let len = o.len;
            resize(&mut meta, len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if self.dir.is_none() || !path.eq_ignore_ascii_case("maps") {
            return Ok(entries);
        }
        for e in &mut entries {
            if e.metadata.is_directory() {
                continue;
            }
            if let Some(o) = self.chosen(&format!("{path}\\{}", e.name)) {
                let len = o.len;
                resize(&mut e.metadata, len);
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let Some(o) = self.chosen(path) else {
            return self.inner.read_at(path, offset, buffer);
        };
        let n = o.len.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        o.file.seek(SeekFrom::Start(offset))?;
        match o.file.read_exact(&mut buffer[..n]) {
            Ok(()) => Ok(n),
            // Truncated under us: its stated size can no longer be served.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::FileCorrupt),
            Err(e) => Err(e.into()),
        }
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    /// An overridden map is a user map: the identity of a derived manifest
    /// whose one layer is the whole file, so a local edit is never taken
    /// for retail. Hashed once, on first ask.
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        let Some(name) = is_map_path(path) else {
            return self.inner.map_id(path);
        };
        let map = name[..name.len() - 4].to_ascii_lowercase();
        let Some(o) = self.chosen(path) else {
            return self.inner.map_id(path);
        };
        if o.id.is_none() {
            o.id = Some(override_id(&map, &o.path).ok()?);
        }
        o.id
    }
    /// An overridden map is all of its host file.
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        if let Some(o) = self.chosen(path) {
            return Some(HostFile {
                path: o.path.clone(),
                offset: 0,
                len: o.len,
            });
        }
        self.inner.host_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ATTR_DIRECTORY, ATTR_READONLY, BasicInfo};
    use std::sync::{Arc, Mutex};

    /// A disc with `maps\a.map`, `maps\b.map`, `maps\notes.txt` and `default.xbe`.
    struct Disc;
    fn meta(len: u64, dir: bool) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if dir { ATTR_DIRECTORY } else { 0 },
                ..Default::default()
            },
            end_of_file: len,
            allocation_size: len.div_ceil(2048) * 2048,
        }
    }
    const FILES: [(&str, &[u8]); 4] = [
        ("maps\\a.map", b"disc-a"),
        ("maps\\b.map", b"disc-bb"),
        ("maps\\notes.map.txt", b"t"),
        ("default.xbe", b"xbe"),
    ];
    impl Backend for Disc {
        fn read_only(&self) -> bool {
            true
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            if path.is_empty() || path.eq_ignore_ascii_case("maps") {
                return Ok(meta(0, true));
            }
            FILES
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .map(|f| meta(f.1.len() as u64, false))
                .ok_or(Error::ObjectNameNotFound)
        }
        fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(FILES
                .iter()
                .filter_map(|(p, d)| {
                    let (dir, name) = p.rsplit_once('\\').unwrap_or(("", p));
                    dir.eq_ignore_ascii_case(path).then(|| DirectoryEntry {
                        name: name.to_owned(),
                        metadata: meta(d.len() as u64, false),
                    })
                })
                .collect())
        }
        fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            let f = FILES
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .ok_or(Error::ObjectNameNotFound)?;
            let start = (offset as usize).min(f.1.len());
            let n = (f.1.len() - start).min(buffer.len());
            buffer[..n].copy_from_slice(&f.1[start..start + n]);
            Ok(n)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
        fn host_file(&mut self, path: &str) -> Option<HostFile> {
            Some(HostFile {
                path: PathBuf::from("/disc.iso"),
                offset: 7,
                len: self.stat(path).ok()?.end_of_file,
            })
        }
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = base.join(format!("h2-map-override-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("maps")).unwrap();
            Self(p)
        }
        fn maps(&self) -> PathBuf {
            self.0.join("maps")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn backend(dir: &Path) -> (MapOverrideBackend<Disc>, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&lines);
        let b = MapOverrideBackend::with_notice(
            Disc,
            dir,
            Box::new(move |l: &str| log.lock().unwrap().push(l.to_owned())),
        );
        (b, lines)
    }
    fn read_all(b: &mut impl Backend, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4];
        loop {
            match b.read_at(path, out.len() as u64, &mut buf).unwrap() {
                0 => return out,
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn an_override_has_a_derived_id_and_other_paths_have_none() {
        let t = Temp::new("id");
        std::fs::write(t.maps().join("a.map"), b"edited map bytes").unwrap();
        let (mut b, _) = backend(&t.maps());
        let id = b.map_id("maps\\A.MAP").unwrap();
        assert_eq!(b.map_id("maps\\a.map"), Some(id));
        assert_eq!(id, override_id("a", &t.maps().join("a.map")).unwrap());
        // Not overridden, or not a map: the disc's, which says nothing.
        assert_eq!(b.map_id("maps\\b.map"), None);
        assert_eq!(b.map_id("default.xbe"), None);
        // Derived, so never a retail id for the same bytes' hash.
        let retail = content::MapId::retail("a", content::Sha256::of(b"edited map bytes"));
        assert_ne!(id, retail);
    }

    #[test]
    fn map_paths_are_exactly_one_level_under_maps() {
        assert_eq!(is_map_path("maps\\a.map"), Some("a.map"));
        assert_eq!(is_map_path("MAPS\\A.MAP"), Some("A.MAP"));
        assert_eq!(is_map_path("maps\\.map"), Some(".map"));
        assert_eq!(is_map_path("maps\\sub\\a.map"), None);
        assert_eq!(is_map_path("other\\a.map"), None);
        assert_eq!(is_map_path("maps\\a.mapx"), None);
        assert_eq!(is_map_path("maps\\map"), None);
        assert_eq!(is_map_path("a.map"), None);
        assert_eq!(is_map_path("maps"), None);
    }

    #[test]
    fn override_is_served_case_insensitively_with_its_size_and_logged_once() {
        let t = Temp::new("serve");
        std::fs::write(t.maps().join("A.Map"), b"edited map bytes").unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(b.stat("MAPS\\a.MAP").unwrap().end_of_file, 16);
        assert_eq!(b.stat("maps\\a.map").unwrap().allocation_size, 2048);
        assert!(b.stat("maps\\a.map").unwrap().basic.attributes & ATTR_READONLY != 0);
        assert_eq!(read_all(&mut b, "maps\\A.MAP"), b"edited map bytes");
        let mut buf = [0u8; 8];
        assert_eq!(b.read_at("maps\\a.map", 12, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"ytes");
        assert_eq!(b.read_at("maps\\a.map", 16, &mut buf).unwrap(), 0);
        assert_eq!(b.read_at("maps\\a.map", u64::MAX, &mut buf).unwrap(), 0);
        let host = b.host_file("maps\\a.map").unwrap();
        assert_eq!(host.offset, 0);
        assert_eq!(host.len, 16);
        assert!(host.path.ends_with("A.Map"));
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("maps\\a.MAP from ")
                && lines[0].contains("16 bytes; disc 6 bytes"),
            "{lines:?}"
        );
    }

    #[test]
    fn other_files_and_maps_without_an_override_fall_through() {
        let t = Temp::new("through");
        std::fs::write(t.maps().join("a.map"), b"edited").unwrap();
        std::fs::write(t.maps().join("default.xbe"), b"not me").unwrap();
        std::fs::write(t.maps().join("notes.map.txt"), b"not me").unwrap();
        std::fs::write(t.maps().join("new.map"), b"not on disc").unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\b.map"), b"disc-bb");
        assert_eq!(read_all(&mut b, "default.xbe"), b"xbe");
        assert_eq!(read_all(&mut b, "maps\\notes.map.txt"), b"t");
        assert_eq!(b.host_file("maps\\b.map").unwrap().offset, 7);
        // A map the disc lacks stays missing.
        assert_eq!(b.stat("maps\\new.map"), Err(Error::ObjectNameNotFound));
        assert!(b.read_at("maps\\new.map", 0, &mut [0; 4]).is_err());
        assert!(lines.lock().unwrap().is_empty());
        assert!(b.read_only());
        assert_eq!(
            b.write_at("maps\\a.map", 0, b"x"),
            Err(Error::MediaWriteProtected)
        );
        assert_eq!(
            b.create("maps\\c.map", false, 0),
            Err(Error::MediaWriteProtected)
        );
        assert_eq!(b.set_len("maps\\a.map", 0), Err(Error::MediaWriteProtected));
        assert_eq!(b.remove("maps\\a.map"), Err(Error::MediaWriteProtected));
        assert_eq!(std::fs::read(t.maps().join("a.map")).unwrap(), b"edited");
    }

    #[test]
    fn listing_of_maps_shows_override_sizes() {
        let t = Temp::new("list");
        std::fs::write(t.maps().join("B.MAP"), vec![1u8; 5000]).unwrap();
        let (mut b, _) = backend(&t.maps());
        let sizes: Vec<_> = b
            .read_directory("MAPS")
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.metadata.end_of_file, e.metadata.allocation_size))
            .collect();
        assert_eq!(
            sizes,
            [
                ("a.map".to_owned(), 6, 2048),
                ("b.map".to_owned(), 5000, 6144),
                ("notes.map.txt".to_owned(), 1, 2048)
            ]
        );
    }

    #[test]
    fn missing_directory_and_unusable_files_fall_back_to_the_disc() {
        let t = Temp::new("fallback");
        let (mut b, lines) = backend(&t.0.join("absent"));
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert_eq!(lines.lock().unwrap().len(), 1);
        assert!(lines.lock().unwrap()[0].starts_with("warning: cannot use"));

        // A directory where the map should be, and names that differ only in case.
        std::fs::create_dir(t.maps().join("a.map")).unwrap();
        std::fs::write(t.maps().join("b.map"), b"one").unwrap();
        std::fs::write(t.maps().join("B.map"), b"two").unwrap();
        let case_sensitive = std::fs::read_dir(t.maps()).unwrap().count() == 3;
        let (mut b, lines) = backend(&t.maps());
        for _ in 0..2 {
            assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
            assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 6);
        }
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "warned once: {lines:?}");
        assert!(lines[0].contains("not a regular file"), "{lines:?}");
        drop(lines);
        if case_sensitive {
            assert_eq!(read_all(&mut b, "maps\\b.map"), b"disc-bb");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_are_followed_and_escaping_ones_refused() {
        let t = Temp::new("links");
        std::fs::write(t.0.join("outside.map"), b"outside").unwrap();
        std::fs::write(t.maps().join("real.bin"), b"inside").unwrap();
        std::os::unix::fs::symlink(t.0.join("outside.map"), t.maps().join("a.map")).unwrap();
        std::os::unix::fs::symlink("real.bin", t.maps().join("b.map")).unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 6);
        assert_eq!(read_all(&mut b, "maps\\b.map"), b"inside");
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("links outside")),
            "{lines:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_override_falls_back_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("perm");
        let p = t.maps().join("a.map");
        std::fs::write(&p, b"secret").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read it anyway; then there is nothing to test.
        if File::open(&p).is_ok() {
            return;
        }
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert!(lines.lock().unwrap()[0].contains("cannot open"));
    }

    #[test]
    fn override_streams_past_64_mib_from_a_sparse_file() {
        let t = Temp::new("sparse");
        let len = 300 * 1024 * 1024u64;
        let f = File::create(t.maps().join("a.map")).unwrap();
        f.set_len(len).unwrap();
        let mut f = f;
        use std::io::Write;
        for (at, mark) in [
            (0u64, b"head"),
            (100 * 1024 * 1024, b"mark"),
            (len - 4, b"foot"),
        ] {
            f.seek(SeekFrom::Start(at)).unwrap();
            f.write_all(mark).unwrap();
        }
        drop(f);
        let (mut b, _) = backend(&t.maps());
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, len);
        let mut buf = [9u8; 8];
        assert_eq!(
            b.read_at("maps\\a.map", 100 * 1024 * 1024 - 2, &mut buf)
                .unwrap(),
            8
        );
        assert_eq!(&buf, b"\0\0mark\0\0");
        assert_eq!(b.read_at("maps\\a.map", len - 6, &mut buf).unwrap(), 6);
        assert_eq!(&buf[..6], b"\0\0foot");
        assert_eq!(b.read_at("maps\\a.map", 0, &mut buf).unwrap(), 8);
        assert_eq!(&buf[..4], b"head");
    }

    #[test]
    fn truncated_override_is_an_error_not_short_data() {
        let t = Temp::new("trunc");
        let p = t.maps().join("a.map");
        std::fs::write(&p, vec![1u8; 4096]).unwrap();
        let (mut b, _) = backend(&t.maps());
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 4096);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(10)
            .unwrap();
        assert_eq!(
            b.read_at("maps\\a.map", 0, &mut [0; 64]),
            Err(Error::FileCorrupt)
        );
    }
}
