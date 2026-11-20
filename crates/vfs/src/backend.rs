use crate::{BasicInfo, DirectoryEntry, Error, FileSource, HostFile, Metadata, Result, VolumeInfo};

/// Backend paths are root-relative, backslash-separated, with no dot components.
/// Implementations must validate inputs even when used without Vfs. All calls are serialized by &mut Vfs.
/// Mutation defaults implement a read-only filesystem.
pub trait Backend: Send {
    /// Volume mutability is cheap to query; opening a file must not scan disk usage.
    fn read_only(&self) -> bool;
    fn stat(&mut self, path: &str) -> Result<Metadata>;
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>>;
    /// Return 0 at EOF. Vfs translates a nonempty read at EOF to STATUS_END_OF_FILE.
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize>;
    fn volume_info(&mut self) -> Result<VolumeInfo>;
    /// Where the file at `path` sits on the host, if its bytes are one
    /// contiguous region of a host file that a reader outside the VFS may
    /// open itself. `None` (the default) for anything else: directories,
    /// rewritten or in-memory files.
    fn host_file(&mut self, _path: &str) -> Option<HostFile> {
        None
    }
    /// Backing for readers outside the VFS, including immutable in-memory files.
    fn file_source(&mut self, path: &str) -> Option<FileSource> {
        self.host_file(path).map(FileSource::Host)
    }
    /// Pin the exact bytes served to a film observer so digest I/O can run
    /// outside the VFS lock. Composed streams without a detachable reader
    /// use the backend read path one block at a time.
    fn film_source(&mut self, _path: &str) -> Result<Option<crate::FilmMapSource>> {
        Ok(None)
    }
    /// Which map exactly is served at `path`, when that differs from the
    /// retail disc map: a map package's [`content::MapId`], or the identity
    /// of an overriding whole file. `None` (the default) means the
    /// backend serves `path` as it is (retail, for a disc map).
    fn map_id(&mut self, _path: &str) -> Option<content::MapId> {
        None
    }
    /// Readable ranges (offset and length) of a sparse sim-store map.
    /// A regular or overlaid map has its entire logical file available.
    fn map_ranges(&mut self, _path: &str) -> Option<Vec<(u64, u64)>> {
        None
    }
    /// A file object was opened on `path` (after a successful open, files
    /// only). Paired with [`closed`](Self::closed) when its last handle
    /// closes. Backends whose contents can change while the game runs
    /// (map packages) use the pair to keep an open file's contents fixed.
    fn opened(&mut self, _path: &str) {}
    /// The last handle of a file object opened on `path` closed.
    fn closed(&mut self, _path: &str) {}
    /// Look again at host-side contents that can change while the game
    /// runs (map packages installed or edited); what changed takes effect
    /// at the next open of each path. Wrappers forward it.
    fn rescan(&mut self) {}
    /// Whether the map at `path` would now be served from another package
    /// (or none, or one where there was none) than when it was last
    /// opened. False for a path never opened, open now, or not a map. The
    /// game's map cache reuses a slot by its path alone
    /// (docs/runtime/cache-slots.md, "Changed maps"), so the boot uses this to
    /// invalidate a slot holding an outdated copy.
    fn map_changed(&mut self, _path: &str) -> bool {
        false
    }
    fn create(&mut self, _path: &str, _directory: bool, _attributes: u32) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
    /// Initial allocation is a hint: it may reserve capacity but never extends
    /// EOF. A backend that cannot reserve ignores it (the default), as NT
    /// does for a file system that treats it as advisory; one that does
    /// reserve must fail a reservation it cannot honour (e.g. disk full)
    /// before creating or truncating data.
    /// `owner` is a unique VFS file-object identity, shared by duplicated handles.
    fn create_allocated(
        &mut self,
        path: &str,
        directory: bool,
        attributes: u32,
        allocation: u64,
        _owner: u64,
    ) -> Result<()> {
        let _ = allocation;
        self.create(path, directory, attributes)
    }
    fn overwrite_allocated(&mut self, path: &str, allocation: u64, _owner: u64) -> Result<()> {
        let _ = allocation;
        self.set_len(path, 0)
    }
    /// Release only this owner's unused allocation after its last handle closes.
    /// Independent opens still receive ordinary cleanup, but cannot trim it.
    fn release_allocation(&mut self, _path: &str, _owner: u64) -> Result<()> {
        Ok(())
    }
    fn write_at(&mut self, _path: &str, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(Error::MediaWriteProtected)
    }
    fn set_len(&mut self, _path: &str, _size: u64) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
    fn set_basic_info(&mut self, _path: &str, _basic: BasicInfo) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
    fn remove(&mut self, _path: &str) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
    fn flush(&mut self, _path: &str) -> Result<()> {
        Ok(())
    }
    /// Quick format: leave an empty volume. Only [`crate::Vfs::format_device`]
    /// calls this, and only while no file object is open on the volume.
    fn format(&mut self) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
    /// Final file-object cleanup. Read-only opens need no data sync by default;
    /// backends with deferred timestamps or dirty data can override this hook.
    fn cleanup(&mut self, path: &str, write_access: bool) -> Result<()> {
        if write_access {
            self.flush(path)
        } else {
            Ok(())
        }
    }
}
