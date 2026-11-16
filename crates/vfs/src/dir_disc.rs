//! A host directory served as the DVD: the disc's files copied out with
//! their tree (`cargo run -p xiso --example subset`), read-only.
//!
//! The tvOS app bundles a trimmed copy of the disc this way, since a 7 GB
//! image does not fit (docs/tvos.md). Names match without regard to case,
//! as on the disc; the directory is indexed once, when opened, so files
//! added later are not seen.

use crate::{
    ATTR_DIRECTORY, ATTR_READONLY, BasicInfo, DirectoryEntry, Error, HostFile, Metadata, Result,
    Timestamp, VolumeInfo, backend::Backend, path::components,
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

struct Node {
    /// The name as the directory spells it.
    name: String,
    host: PathBuf,
    is_dir: bool,
    size: u64,
}

/// See the module docs.
pub struct DirDiscBackend {
    root: PathBuf,
    /// Lower-case backslash path from the root (no leading separator).
    entries: BTreeMap<String, Node>,
    volume_bytes: u64,
    /// The last file read, kept open: the game streams maps in many reads.
    open: Option<(String, File)>,
}

impl DirDiscBackend {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if !std::fs::metadata(&root)?.is_dir() {
            return Err(Error::ObjectPathNotFound);
        }
        let mut entries = BTreeMap::new();
        let mut volume_bytes = 0;
        let mut stack = vec![(root.clone(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            for e in std::fs::read_dir(&dir)? {
                let e = e?;
                let Ok(name) = e.file_name().into_string() else {
                    continue;
                };
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}\\{name}")
                };
                if components(&path).is_err() {
                    continue;
                }
                // Follow links, as the bundle copy may use them.
                let meta = std::fs::metadata(e.path())?;
                let is_dir = meta.is_dir();
                let size = if is_dir { 0 } else { meta.len() };
                volume_bytes += size;
                if is_dir {
                    stack.push((e.path(), path.clone()));
                }
                entries.insert(
                    path.to_ascii_lowercase(),
                    Node {
                        name,
                        host: e.path(),
                        is_dir,
                        size,
                    },
                );
            }
        }
        Ok(Self {
            root,
            entries,
            volume_bytes,
            open: None,
        })
    }

    /// The directory served.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn entry(&self, path: &str) -> Result<&Node> {
        components(path)?;
        if let Some(e) = self.entries.get(&path.to_ascii_lowercase()) {
            return Ok(e);
        }
        if let Some((parent, _)) = path.rsplit_once('\\') {
            match self.entries.get(&parent.to_ascii_lowercase()) {
                Some(e) if e.is_dir => {}
                _ => return Err(Error::ObjectPathNotFound),
            }
        }
        Err(Error::ObjectNameNotFound)
    }

    fn metadata(node: Option<&Node>) -> Metadata {
        let directory = node.is_none_or(|n| n.is_dir);
        let size = node.map_or(0, |n| n.size);
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if directory { ATTR_DIRECTORY } else { 0 },
                // As for the image: no timestamps on the disc.
                creation_time: Timestamp(0),
                last_access_time: Timestamp(0),
                last_write_time: Timestamp(0),
                change_time: Timestamp(0),
            },
            end_of_file: size,
            allocation_size: size.div_ceil(xiso::SECTOR) * xiso::SECTOR,
        }
    }
}

impl Backend for DirDiscBackend {
    fn read_only(&self) -> bool {
        true
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        let e = self.entry(path).ok().filter(|e| !e.is_dir)?;
        Some(HostFile {
            path: e.host.clone(),
            offset: 0,
            len: e.size,
        })
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        Ok(Self::metadata(if path.is_empty() {
            None
        } else {
            Some(self.entry(path)?)
        }))
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        if !self.stat(path)?.is_directory() {
            return Err(Error::InvalidParameter);
        }
        let base = if path.is_empty() {
            String::new()
        } else {
            format!("{}\\", path.to_ascii_lowercase())
        };
        Ok(self
            .entries
            .iter()
            .filter_map(|(key, node)| {
                let rest = key.strip_prefix(&base)?;
                if rest.is_empty() || rest.contains('\\') {
                    return None;
                }
                Some(DirectoryEntry {
                    name: node.name.clone(),
                    metadata: Self::metadata(Some(node)),
                })
            })
            .collect())
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if path.is_empty() {
            return Err(Error::FileIsADirectory);
        }
        let node = self.entry(path)?;
        if node.is_dir {
            return Err(Error::FileIsADirectory);
        }
        let len = node.size.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if len == 0 {
            return Ok(0);
        }
        let key = path.to_ascii_lowercase();
        let host = node.host.clone();
        let file = match &mut self.open {
            Some((k, f)) if *k == key => f,
            slot => &mut slot.insert((key, File::open(host)?)).1,
        };
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buffer[..len])?;
        Ok(len)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        Ok(VolumeInfo {
            label: "Xbox DVD".into(),
            filesystem: "XDVDFS",
            serial: 0,
            read_only: true,
            bytes_per_sector: 2048,
            sectors_per_allocation_unit: 1,
            total_allocation_units: self.volume_bytes.div_ceil(2048),
            available_allocation_units: 0,
            maximum_component_name_length: 255,
        })
    }
}

/// The DVD at `path`: a disc image ([`crate::DiscBackend`]) or a directory
/// holding a copy of the disc's files ([`DirDiscBackend`]).
pub fn open_disc(path: impl AsRef<Path>) -> Result<Box<dyn Backend>> {
    let path = path.as_ref();
    if std::fs::metadata(path)?.is_dir() {
        Ok(Box::new(DirDiscBackend::open(path)?))
    } else {
        Ok(Box::new(crate::DiscBackend::open(path)?))
    }
}

impl Backend for Box<dyn Backend> {
    fn read_only(&self) -> bool {
        (**self).read_only()
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        (**self).stat(path)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        (**self).read_directory(path)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        (**self).read_at(path, offset, buffer)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        (**self).volume_info()
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        (**self).host_file(path)
    }
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        (**self).map_id(path)
    }
    // The DVD is read-only: the mutation defaults stand for both kinds.
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
                "h2-vfs-dir-disc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree() -> Temp {
        let t = Temp::new();
        std::fs::create_dir_all(t.0.join("maps/fonts")).unwrap();
        std::fs::write(t.0.join("default.xbe"), b"XBEH....").unwrap();
        std::fs::write(
            t.0.join("maps/MainMenu.map"),
            (0u8..=255).collect::<Vec<_>>(),
        )
        .unwrap();
        std::fs::write(t.0.join("maps/fonts/font_table.txt"), b"x").unwrap();
        t
    }

    #[test]
    fn serves_a_directory_case_insensitively_and_read_only() {
        let t = tree();
        let mut d = DirDiscBackend::open(&t.0).unwrap();
        assert!(d.read_only());
        let m = d.stat("MAPS\\mainmenu.MAP").unwrap();
        assert_eq!(m.end_of_file, 256);
        assert!(!m.is_directory());
        assert!(d.stat("maps").unwrap().is_directory());
        assert!(d.stat("").unwrap().is_directory());
        let mut buf = [0u8; 8];
        assert_eq!(d.read_at("maps\\mainmenu.map", 250, &mut buf).unwrap(), 6);
        assert_eq!(&buf[..6], &[250, 251, 252, 253, 254, 255]);
        assert_eq!(d.read_at("maps\\mainmenu.map", 256, &mut buf).unwrap(), 0);
        assert_eq!(d.read_at("default.xbe", 0, &mut buf).unwrap(), 8);
        assert_eq!(&buf[..4], b"XBEH");
        assert!(matches!(
            d.read_at("maps", 0, &mut buf),
            Err(Error::FileIsADirectory)
        ));
        assert!(matches!(
            d.write_at("default.xbe", 0, b"x"),
            Err(Error::MediaWriteProtected)
        ));
        assert!(matches!(
            d.create("new.txt", false, 0),
            Err(Error::MediaWriteProtected)
        ));
    }

    #[test]
    fn missing_names_and_paths_are_told_apart() {
        let t = tree();
        let mut d = DirDiscBackend::open(&t.0).unwrap();
        assert!(matches!(
            d.stat("maps\\zanzibar.map"),
            Err(Error::ObjectNameNotFound)
        ));
        assert!(matches!(
            d.stat("bink\\intro_60.bik"),
            Err(Error::ObjectPathNotFound)
        ));
        assert!(matches!(
            d.stat("default.xbe\\x"),
            Err(Error::ObjectPathNotFound)
        ));
    }

    #[test]
    fn lists_one_level_with_the_names_as_spelled() {
        let t = tree();
        let mut d = DirDiscBackend::open(&t.0).unwrap();
        let mut names: Vec<String> = d
            .read_directory("maps")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        assert_eq!(names, ["MainMenu.map", "fonts"]);
        let mut root: Vec<String> = d
            .read_directory("")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        root.sort();
        assert_eq!(root, ["default.xbe", "maps"]);
        assert!(matches!(
            d.read_directory("default.xbe"),
            Err(Error::InvalidParameter)
        ));
    }

    #[test]
    fn host_files_are_whole_files() {
        let t = tree();
        let mut d = DirDiscBackend::open(&t.0).unwrap();
        let h = d.host_file("maps\\MAINMENU.map").unwrap();
        assert_eq!((h.offset, h.len), (0, 256));
        assert_eq!(std::fs::read(&h.path).unwrap().len(), 256);
        assert!(d.host_file("maps").is_none());
    }

    #[test]
    fn open_disc_picks_the_directory_backend_for_a_directory() {
        let t = tree();
        let mut b = open_disc(&t.0).unwrap();
        assert_eq!(b.stat("maps\\mainmenu.map").unwrap().end_of_file, 256);
        // A file that is not an image is refused, as before.
        assert!(open_disc(t.0.join("default.xbe")).is_err());
        assert!(open_disc(t.0.join("absent")).is_err());
    }
}
