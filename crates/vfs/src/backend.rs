use crate::{BasicInfo, DirectoryEntry, Error, Metadata, Result, VolumeInfo};

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
