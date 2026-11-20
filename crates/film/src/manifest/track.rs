//! The recording's guest storage, relative to its immutable DATA manifest.
//! A VFS write journal alone would miss host-side save edits, renames and
//! write-behind sidecars. The scheduler drains metadata before reconciliation.
use super::{Item, LIMIT, file_digest, keyframe::Delta};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, Metadata},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Stamp {
    len: u64,
    directory: bool,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
    #[cfg(windows)]
    identity: Option<(u64, [u8; 16], i64)>,
}

impl From<&Metadata> for Stamp {
    fn from(m: &Metadata) -> Self {
        Self {
            len: m.len(),
            directory: m.is_dir(),
            modified: m.modified().ok(),
            #[cfg(windows)]
            identity: None,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (m.dev(), m.ino(), m.ctime(), m.ctime_nsec())
            },
        }
    }
}

impl Stamp {
    pub(super) fn path(_path: &Path, _meta: &Metadata) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            let handle = File::options()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(_path)?;
            Self::file(&handle)
        }
        #[cfg(not(windows))]
        {
            Ok(Self::from(_meta))
        }
    }

    fn file(file: &File) -> Result<Self> {
        let meta = file.metadata()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            ensure!(
                meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "reparse point in guest disk capture"
            );
            let mut stamp = Self::from(&meta);
            stamp.identity = windows_identity(file)?;
            Ok(stamp)
        }
        #[cfg(not(windows))]
        {
            Ok(Self::from(&meta))
        }
    }

    fn reusable(&self) -> bool {
        #[cfg(windows)]
        {
            self.identity.is_some()
        }
        #[cfg(not(windows))]
        {
            cfg!(unix)
        }
    }
}

#[cfg(windows)]
fn windows_identity(file: &File) -> std::io::Result<Option<(u64, [u8; 16], i64)>> {
    use std::{
        io,
        mem::{MaybeUninit, size_of},
        os::windows::io::AsRawHandle,
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
            ERROR_NOT_SUPPORTED,
        },
        Storage::FileSystem::{
            FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx,
        },
    };
    let mut basic = MaybeUninit::<FILE_BASIC_INFO>::uninit();
    let mut id = MaybeUninit::<FILE_ID_INFO>::uninit();
    let handle = file.as_raw_handle();
    // SAFETY: the borrowed file keeps the handle live. Each information class
    // receives the corresponding writable structure and its exact byte count.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            basic.as_mut_ptr().cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        ) != 0
            && GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                id.as_mut_ptr().cast(),
                size_of::<FILE_ID_INFO>() as u32,
            ) != 0
    };
    if !ok {
        let error = io::Error::last_os_error();
        // FAT and some network shares expose only a subset of these classes.
        // Keep recording and verify their file contents at each boundary.
        return match error.raw_os_error().map(|e| e as u32) {
            Some(
                ERROR_INVALID_FUNCTION
                | ERROR_NOT_SUPPORTED
                | ERROR_INVALID_PARAMETER
                | ERROR_CALL_NOT_IMPLEMENTED,
            ) => Ok(None),
            _ => Err(error),
        };
    }
    // SAFETY: both successful calls above initialise their entire output.
    let (basic, id) = unsafe { (basic.assume_init(), id.assume_init()) };
    Ok(
        (basic.ChangeTime != 0 && id.FileId.Identifier != [0; 16]).then_some((
            id.VolumeSerialNumber,
            id.FileId.Identifier,
            basic.ChangeTime,
        )),
    )
}

pub(super) fn children(full: &Path) -> Result<Vec<OsString>> {
    let mut names = Vec::new();
    for child in fs::read_dir(full)? {
        ensure!(names.len() < 65536, "too many guest directory entries");
        names.push(child?.file_name());
    }
    names.sort_unstable();
    Ok(names)
}

struct Node {
    stamp: Stamp,
    children: Option<Vec<OsString>>,
    /// Only changes relative to DATA carry a payload.
    change: Option<Item>,
}

pub struct DiskTracker {
    root: PathBuf,
    initial: BTreeMap<PathBuf, String>,
    nodes: BTreeMap<PathBuf, Arc<Node>>,
}

impl DiskTracker {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            initial: BTreeMap::new(),
            nodes: BTreeMap::new(),
        }
    }

    pub(super) fn set_initial(&mut self, entries: &[Item]) {
        self.initial = entries
            .iter()
            .map(|e| (e.path.clone(), e.sha256.clone()))
            .collect();
    }

    pub(super) fn seed(
        &mut self,
        path: &Path,
        stamp: Stamp,
        children: Option<Vec<OsString>>,
    ) -> Result<()> {
        self.verify_path(path, &stamp)?;
        self.nodes.insert(
            path.into(),
            Arc::new(Node {
                stamp,
                children,
                change: None,
            }),
        );
        Ok(())
    }

    pub(super) fn seed_file(&mut self, path: &Path, stamp: Stamp, file: &File) -> Result<()> {
        ensure!(
            stamp == Stamp::file(file)?,
            "guest file changed during DATA capture"
        );
        self.seed(path, stamp, None)
    }

    fn verify_path(&self, path: &Path, stamp: &Stamp) -> Result<()> {
        let full = self.root.join(path);
        let meta = fs::symlink_metadata(&full)?;
        ensure!(
            (meta.is_file() || meta.is_dir()) && Stamp::path(&full, &meta)? == *stamp,
            "guest path changed during keyframe capture: {}",
            path.display()
        );
        Ok(())
    }

    /// Check identities at a stopped scheduler boundary. Unchanged directories
    /// keep their child list; unchanged files keep their digest and any edits
    /// already saved relative to DATA. No installed asset is revisited.
    pub fn capture(&mut self) -> Result<Vec<u8>> {
        let mut current = BTreeMap::new();
        let mut budget = LIMIT;
        for root in ["disk", "metadata"] {
            let path = Path::new(root);
            match fs::symlink_metadata(self.root.join(path)) {
                Ok(meta) => self.visit(path, meta, &mut current, &mut budget)?,
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let changed: Vec<_> = current.values().filter_map(|n| n.change.as_ref()).collect();
        let removed: Vec<_> = self
            .initial
            .keys()
            .filter(|p| !current.contains_key(*p))
            .collect();
        let delta = if changed.is_empty() && removed.is_empty() {
            Vec::new()
        } else {
            serde_json::to_vec(&Delta { changed, removed })?
        };
        self.nodes = current;
        Ok(delta)
    }

    fn visit(
        &self,
        path: &Path,
        meta: Metadata,
        current: &mut BTreeMap<PathBuf, Arc<Node>>,
        budget: &mut usize,
    ) -> Result<()> {
        ensure!(
            current.len() < 65536 && path.components().count() < 64,
            "guest disk index too large"
        );
        ensure!(
            meta.is_dir() || meta.is_file(),
            "unsupported guest path {}",
            path.display()
        );
        let full = self.root.join(path);
        let stamp = Stamp::path(&full, &meta)?;
        let old = self.nodes.get(path);
        // An mtime/byte-count pair can survive replacement or an in-place edit.
        // Without an inode and status-change timestamp, read the path again.
        let node = if let Some(node) = old.filter(|n| stamp.reusable() && n.stamp == stamp) {
            Arc::clone(node)
        } else {
            let initial = self.initial.get(path).map(String::as_str);
            let (children, change) = if meta.is_dir() {
                (
                    Some(children(&full)?),
                    (initial != Some("")).then(|| Item {
                        path: path.into(),
                        sha256: String::new(),
                        data: None,
                    }),
                )
            } else {
                let mut f = File::open(&full)?;
                ensure!(
                    Stamp::file(&f)? == stamp,
                    "guest file changed while opening"
                );
                let change = if meta.len() > LIMIT as u64 {
                    // A timestamp-only change of a large original file can be
                    // verified by streaming the digest without embedding it.
                    ensure!(
                        initial.is_some_and(|s| !s.is_empty())
                            && initial == Some(file_digest(&mut f)?.as_str()),
                        "changed guest file exceeds keyframe limit: {}",
                        path.display()
                    );
                    None
                } else {
                    let mut data = Vec::with_capacity(meta.len() as usize);
                    (&mut f).take(LIMIT as u64 + 1).read_to_end(&mut data)?;
                    ensure!(
                        data.len() as u64 == meta.len(),
                        "guest file changed during capture"
                    );
                    let sha256 = crate::xbe_digest(&data);
                    (Some(sha256.as_str()) != initial).then_some(Item {
                        path: path.into(),
                        sha256,
                        data: Some(data),
                    })
                };
                ensure!(
                    Stamp::file(&f)? == stamp,
                    "guest file changed during capture"
                );
                (None, change)
            };
            self.verify_path(path, &stamp)?;
            Arc::new(Node {
                stamp,
                children,
                change,
            })
        };
        if let Some(data) = node.change.as_ref().and_then(|e| e.data.as_ref()) {
            *budget = budget
                .checked_sub(data.len())
                .context("changed guest files exceed keyframe limit")?;
        }
        current.insert(path.into(), Arc::clone(&node));
        if let Some(children) = &node.children {
            for name in children {
                let child = path.join(name);
                let meta = fs::symlink_metadata(self.root.join(&child))?;
                self.visit(&child, meta, current, budget)?;
            }
            // A host rename or deletion during enumeration invalidates the
            // attempt. Keep the previous good index for the next boundary.
            self.verify_path(path, &node.stamp)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("film-disk-{}", live_proto::sign::random_id()));
            fs::create_dir_all(root.join("disk/nested")).unwrap();
            Self(root)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn successive_deltas_are_relative_to_data_and_include_external_edits() {
        let root = Tree::new();
        let file = root.0.join("disk/nested/save");
        fs::write(&file, b"abcd").unwrap();
        let original_mtime = fs::metadata(&file).unwrap().modified().unwrap();
        let (manifest, mut disk) = Manifest::capture_for_recording(&root.0).unwrap();
        assert!(disk.capture().unwrap().is_empty());

        // A host atomic replacement can preserve both size and mtime.
        let replacement = root.0.join("replacement");
        fs::write(&replacement, b"wxyz").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();
        fs::rename(&replacement, &file).unwrap();
        fs::create_dir(root.0.join("metadata")).unwrap();
        fs::write(root.0.join("metadata/clock"), b"new").unwrap();
        let first = disk.capture().unwrap();
        assert!(!first.is_empty());
        let cached = Arc::clone(&disk.nodes[Path::new("disk/nested/save")]);
        assert_eq!(first, disk.capture().unwrap());
        if cached.stamp.reusable() {
            assert!(Arc::ptr_eq(
                &cached,
                &disk.nodes[Path::new("disk/nested/save")]
            ));
        }

        fs::write(&file, b"abcd").unwrap();
        fs::remove_dir_all(root.0.join("metadata")).unwrap();
        assert!(disk.capture().unwrap().is_empty());
        let restored = root.0.join("restored");
        manifest.restore_keyframe(&restored, None, &first).unwrap();
        assert_eq!(
            fs::read(restored.join("disk/nested/save")).unwrap(),
            b"wxyz"
        );
        assert_eq!(fs::read(restored.join("metadata/clock")).unwrap(), b"new");
    }

    #[cfg(windows)]
    #[test]
    fn same_size_windows_edit_with_restored_last_write_time_changes_the_delta() {
        let root = Tree::new();
        let path = root.0.join("disk/nested/save");
        let fixed =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let write = |bytes: &[u8]| {
            use std::io::Write;
            let mut f = File::create(&path).unwrap();
            f.write_all(bytes).unwrap();
            f.set_modified(fixed).unwrap();
        };
        write(b"abcd");
        let (manifest, mut tracker) = Manifest::capture_for_recording(&root.0).unwrap();
        // A subsequent modify/close can land in the same Windows clock update.
        // Cross a clock update so the fixture actually exercises ChangeTime.
        std::thread::sleep(std::time::Duration::from_millis(32));
        write(b"wxyz");
        let delta = tracker.capture().unwrap();
        let replay = root.0.join("replay");
        manifest.restore_keyframe(&replay, None, &delta).unwrap();
        assert_eq!(fs::read(replay.join("disk/nested/save")).unwrap(), b"wxyz");
    }

    #[test]
    fn failed_capture_preserves_the_last_good_index() {
        let root = Tree::new();
        let (_, mut disk) = Manifest::capture_for_recording(&root.0).unwrap();
        let before = Arc::clone(&disk.nodes[Path::new("disk")]);
        let huge = root.0.join("disk/new");
        File::create(&huge)
            .unwrap()
            .set_len(LIMIT as u64 + 1)
            .unwrap();
        assert!(disk.capture().is_err());
        assert!(Arc::ptr_eq(&before, &disk.nodes[Path::new("disk")]));
        fs::remove_file(huge).unwrap();
        assert!(disk.capture().unwrap().is_empty());
    }
}
