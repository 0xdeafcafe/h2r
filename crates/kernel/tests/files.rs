mod common;
use common::*;
use kernel::status;
use runtime::Event;
use std::collections::BTreeMap;
use vfs::{Backend, BasicInfo, DirectoryEntry, Error, Metadata, Timestamp, VolumeInfo};

struct MemoryDisk {
    capacity: usize,
    entries: BTreeMap<String, Vec<u8>>,
    basic: BTreeMap<String, BasicInfo>,
}
impl MemoryDisk {
    fn info(data: &[u8]) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: vfs::ATTR_ARCHIVE,
                creation_time: Timestamp(100),
                last_access_time: Timestamp(200),
                last_write_time: Timestamp(300),
                change_time: Timestamp(400),
            },
            end_of_file: data.len() as u64,
            allocation_size: 16384,
        }
    }
}
impl Backend for MemoryDisk {
    fn read_only(&self) -> bool {
        false
    }
    fn stat(&mut self, path: &str) -> vfs::Result<Metadata> {
        if path.is_empty() {
            let mut info = Self::info(&[]);
            info.basic.attributes = vfs::ATTR_DIRECTORY;
            return Ok(info);
        }
        let result = self
            .entries
            .get(&path.to_ascii_lowercase())
            .map(|data| Self::info(data))
            .ok_or(Error::ObjectNameNotFound);
        result.map(|mut info| {
            if let Some(basic) = self.basic.get(&path.to_ascii_lowercase()) {
                info.basic = *basic;
            }
            info
        })
    }
    fn read_directory(&mut self, path: &str) -> vfs::Result<Vec<DirectoryEntry>> {
        if !path.is_empty() {
            return Err(Error::NotADirectory);
        }
        Ok(self
            .entries
            .iter()
            .map(|(name, data)| DirectoryEntry {
                name: name.clone(),
                metadata: Self::info(data),
            })
            .collect())
    }
    fn read_at(&mut self, path: &str, offset: u64, out: &mut [u8]) -> vfs::Result<usize> {
        let data = self
            .entries
            .get(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?;
        let start = (offset as usize).min(data.len());
        let n = out.len().min(data.len() - start);
        out[..n].copy_from_slice(&data[start..start + n]);
        Ok(n)
    }
    fn write_at(&mut self, path: &str, offset: u64, bytes: &[u8]) -> vfs::Result<usize> {
        let data = self
            .entries
            .get_mut(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?;
        let end = offset as usize + bytes.len();
        if end > self.capacity {
            return Err(Error::DiskFull);
        }
        data.resize(end.max(data.len()), 0);
        data[offset as usize..end].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    fn volume_info(&mut self) -> vfs::Result<VolumeInfo> {
        Ok(VolumeInfo {
            label: "test".into(),
            filesystem: "FATX",
            serial: 7,
            read_only: false,
            bytes_per_sector: 512,
            sectors_per_allocation_unit: 32,
            total_allocation_units: 100,
            available_allocation_units: 90,
            maximum_component_name_length: 42,
        })
    }
    fn create(&mut self, path: &str, directory: bool, _: u32) -> vfs::Result<()> {
        if directory {
            return Err(Error::NotSupported);
        }
        self.entries.insert(path.to_ascii_lowercase(), Vec::new());
        Ok(())
    }
    fn set_len(&mut self, path: &str, size: u64) -> vfs::Result<()> {
        if size > self.capacity as u64 {
            return Err(Error::DiskFull);
        }
        self.entries
            .get_mut(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?
            .resize(size as usize, 0);
        Ok(())
    }
    fn set_basic_info(&mut self, path: &str, basic: BasicInfo) -> vfs::Result<()> {
        if !self.entries.contains_key(&path.to_ascii_lowercase()) {
            return Err(Error::ObjectNameNotFound);
        }
        self.basic.insert(path.to_ascii_lowercase(), basic);
        Ok(())
    }
    fn remove(&mut self, path: &str) -> vfs::Result<()> {
        self.entries
            .remove(&path.to_ascii_lowercase())
            .map(|_| ())
            .ok_or(Error::ObjectNameNotFound)
    }
}
const IOSB: u32 = DATA + 0x10;
const ATTR: u32 = DATA + 0x100;
const NAME: u32 = DATA + 0x80;
const BUFFER: u32 = DATA + 0x800;
const INFO: u32 = DATA + 0x500;
fn string(h: &Harness, descriptor: u32, buffer: u32, text: &str) {
    h.bytes(buffer, text.as_bytes());
    h.set(
        descriptor,
        (text.len() as u32) | ((text.len() as u32) << 16),
    );
    h.set(descriptor + 4, buffer);
}
fn path(h: &Harness, text: &str, root: u32) {
    string(h, NAME, DATA + 0x200, text);
    h.set(ATTR, root);
    h.set(ATTR + 4, NAME);
    h.set(ATTR + 8, 0x40);
}
fn setup() -> Harness {
    setup_with(4096, &[])
}
fn setup_with(capacity: usize, extra: &[(&str, Vec<u8>)]) -> Harness {
    let mut h = Harness::new();
    let mut storage = vfs::Vfs::new();
    let mut entries: BTreeMap<String, Vec<u8>> = [
        ("a.txt".into(), b"hello".to_vec()),
        ("b.bin".into(), b"other".to_vec()),
    ]
    .into();
    entries.extend(extra.iter().map(|(n, d)| (n.to_string(), d.clone())));
    storage
        .mount(
            "\\Device\\TestDisk",
            MemoryDisk {
                capacity,
                entries,
                basic: BTreeMap::new(),
            },
        )
        .unwrap();
    h.kernel.set_vfs(storage).unwrap();
    string(&h, NAME, DATA + 0x200, "\\??\\T:");
    string(&h, NAME + 8, DATA + 0x240, "\\Device\\TestDisk");
    assert_eq!(h.success(67, &[NAME, NAME + 8]), 0);
    h
}
fn open(h: &mut Harness, name: &str, root: u32, create: bool, directory: bool) -> u32 {
    path(h, name, root);
    let options = 0x20 | if directory { 1 } else { 0x40 };
    let result = if create {
        h.success(
            190,
            &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 0, 0, 7, 3, options],
        )
    } else {
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, options])
    };
    assert_eq!(result, 0);
    h.get(DATA)
}
fn position(h: &mut Harness, handle: u32, pos: u64) {
    h.runtime.memory().set_u64(INFO, pos).unwrap();
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 8, 14]), 0);
}

#[test]
fn create_initial_allocation_may_alias_iosb_and_failure_preserves_old_data() {
    let root = std::path::PathBuf::from(
        std::env::var_os("TMPDIR").unwrap_or_else(|| std::env::temp_dir().into_os_string()),
    )
    .join(format!("kernel-initial-allocation-{}", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let mut h = Harness::new();
    let mut fs = vfs::Vfs::new();
    fs.mount(
        "\\Device\\TestDisk",
        vfs::HostBackend::new(root.join("data"), root.join("meta"))
            .unwrap()
            .with_volume("test", 1, vfs::FATX_CLUSTER_SIZE),
    )
    .unwrap();
    h.kernel.set_vfs(fs).unwrap();
    path(&h, "\\Device\\TestDisk\\SaveMeta.xbx", 0);
    // XCreateSaveGame at 0x330f74 reuses the LARGE_INTEGER as its IOSB.
    h.runtime.memory().set_u64(IOSB, 1).unwrap();
    assert_eq!(
        h.success(
            190,
            &[DATA, vfs::Access::ALL.0, ATTR, IOSB, IOSB, 0, 7, 5, 0x60]
        ),
        0
    );
    let handle = h.get(DATA);
    assert_eq!(h.get(IOSB + 4), 2);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 24, 5]), 0);
    assert_eq!(
        h.runtime.memory().u64(INFO).unwrap(),
        vfs::FATX_CLUSTER_SIZE
    );
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 0);
    h.bytes(BUFFER, b"Name=synthetic");
    assert_eq!(h.success(236, &[handle, 0, 0, 0, IOSB, BUFFER, 14, 0]), 0);
    assert_eq!(h.success(187, &[handle]), 0);
    h.runtime
        .memory()
        .set_u64(IOSB, vfs::FATX_CLUSTER_SIZE + 1)
        .unwrap();
    assert_eq!(
        h.success(
            190,
            &[DATA, vfs::Access::ALL.0, ATTR, IOSB, IOSB, 0, 7, 5, 0x60]
        ),
        vfs::Error::DiskFull.ntstatus()
    );
    let handle = open(&mut h, "\\Device\\TestDisk\\SaveMeta.xbx", 0, false, false);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 14, 0]), 0);
    assert_eq!(
        h.runtime.memory().read::<14>(BUFFER).unwrap(),
        *b"Name=synthetic"
    );
    assert_eq!(h.success(187, &[handle]), 0);
}

#[test]
fn vfs_file_open_io_shared_position_duplication_information_and_close() {
    let mut h = setup();
    let handle = open(&mut h, "T:\\new.bin", 0, true, false);
    assert_eq!(h.get(IOSB + 4), 2);
    h.bytes(BUFFER, b"abc");
    assert_eq!(h.success(236, &[handle, 0, 0, 0, IOSB, BUFFER, 3, 0]), 0);
    assert_eq!(h.get(IOSB + 4), 3);
    position(&mut h, handle, 0);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 2, 0]), 0);
    assert_eq!(h.runtime.memory().read::<2>(BUFFER).unwrap(), *b"ab");
    assert_eq!(h.success(197, &[handle, DATA + 4, 0]), 0);
    let duplicate = h.get(DATA + 4);
    assert_eq!(h.success(219, &[duplicate, 0, 0, 0, IOSB, BUFFER, 1, 0]), 0);
    assert_eq!(h.runtime.memory().u8(BUFFER).unwrap(), b'c');
    assert_eq!(h.success(187, &[handle]), 0);
    assert_eq!(
        h.success(219, &[duplicate, 0, 0, 0, IOSB, BUFFER, 1, 0]),
        Error::EndOfFile.ntstatus()
    );
    assert_eq!(h.get(IOSB), Error::EndOfFile.ntstatus());
    assert_eq!(h.get(IOSB + 4), 0);
    assert_eq!(h.success(198, &[duplicate, IOSB]), 0);
    for (class, length) in [(4, 40), (5, 24), (8, 4), (14, 8), (16, 4)] {
        assert_eq!(h.success(211, &[duplicate, IOSB, INFO, length, class]), 0);
        assert_eq!(h.get(IOSB + 4), length);
    }
    assert_eq!(h.get(INFO), 0x20);
    assert_eq!(h.success(211, &[duplicate, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 3);
    assert_eq!(h.get(INFO + 16), 1);
    h.runtime.memory().set_u64(INFO, 5).unwrap();
    assert_eq!(h.success(226, &[duplicate, IOSB, INFO, 8, 20]), 0);
    assert_eq!(h.success(211, &[duplicate, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 5);
    for (class, length) in [(3, 24), (7, 32)] {
        assert_eq!(h.success(218, &[duplicate, IOSB, INFO, length, class]), 0);
        assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 100);
        assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 90);
    }
    assert_eq!(h.success(187, &[duplicate]), 0);
    assert_eq!(
        h.success(219, &[duplicate, 0, 0, 0, IOSB, BUFFER, 1, 0]),
        Error::InvalidHandle.ntstatus()
    );
    let reopened = open(&mut h, "T:\\new.bin", 0, false, false);
    h.success(187, &[reopened]);
    string(&h, NAME, DATA + 0x200, "\\??\\T:");
    assert_eq!(h.success(69, &[NAME]), 0);
}

#[test]
fn vfs_relative_opens_directory_packing_and_retry_preserve_cursor() {
    let mut h = setup();
    let root = open(&mut h, "T:\\", 0, false, true);
    let file = open(&mut h, "a.txt", root, false, false);
    h.success(187, &[file]);
    let args = [root, 0, 0, 0, IOSB, BUFFER, 4, 1, 0, 0];
    h.stopped(207, &args);
    assert_eq!(
        h.success(207, &[root, 0, 0, 0, IOSB, BUFFER, 256, 1, 0, 0]),
        0
    );
    assert_eq!(h.get(BUFFER), 0);
    assert_eq!(h.get(BUFFER + 60), 5);
    assert_eq!(
        h.runtime.memory().read::<5>(BUFFER + 64).unwrap(),
        *b"a.txt"
    );
    assert_eq!(
        h.success(207, &[root, 0, 0, 0, IOSB, BUFFER, 256, 12, 0, 0]),
        0
    );
    assert_eq!(h.get(BUFFER + 8), 5);
    assert_eq!(
        h.runtime.memory().read::<5>(BUFFER + 12).unwrap(),
        *b"b.bin"
    );
    assert_eq!(
        h.success(207, &[root, 0, 0, 0, IOSB, BUFFER, 256, 1, 0, 0]),
        Error::NoMoreFiles.ntstatus()
    );
    assert_eq!(
        h.success(207, &[root, 0, 0, 0, IOSB, BUFFER, 256, 1, 0, 1]),
        0
    );
    h.success(187, &[root]);
}

#[test]
fn vfs_guest_faults_do_not_create_files_or_advance_io_and_unsupported_work_stops() {
    let mut h = setup();
    path(&h, "T:\\absent.bin", 0);
    let t = h.call(190, &[0, vfs::Access::ALL.0, ATTR, IOSB, 0, 0, 7, 3, 0x60]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        Error::ObjectNameNotFound.ntstatus()
    );
    let handle = open(&mut h, "T:\\a.txt", 0, false, false);
    let t = h.call(219, &[handle, 0, 0, 0, IOSB, 0xdead0000, 5, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 8, 14]), 0);
    assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 0);
    h.stopped(219, &[handle, 0, 1, 0, IOSB, BUFFER, 5, 0]);
    h.stopped(211, &[handle, IOSB, INFO, 8, 9]);
    h.stopped(218, &[handle, IOSB, INFO, 64, 1]);
    h.stopped(211, &[handle, IOSB, INFO, 1, 5]);
    // Neither synchronous bit: an asynchronous file object (no stop).
    path(&h, "T:\\async.bin", 0);
    assert_eq!(
        h.success(
            190,
            &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 0, 0, 7, 3, 0x40],
        ),
        0
    );
    let asynchronous = h.get(DATA);
    assert_eq!(h.success(187, &[asynchronous]), 0);
    h.runtime.memory().set_u64(INFO, (-1i64) as u64).unwrap();
    assert_eq!(
        h.success(226, &[handle, IOSB, INFO, 8, 14]),
        Error::InvalidParameter.ntstatus()
    );
    h.success(187, &[handle]);
}

#[test]
fn unconfigured_file_exports_stop_without_fake_success() {
    let mut h = Harness::new();
    for &ordinal in &[67, 69, 190, 198, 202, 207, 211, 218, 219, 226, 236] {
        let export = kernel::EXPORTS
            .iter()
            .find(|e| e.ordinal == ordinal)
            .unwrap();
        h.stopped(ordinal, &vec![0; export.words]);
    }
}

#[test]
fn vfs_basic_time_sentinels_delete_pending_and_query_access_are_enforced() {
    let mut h = setup();
    let handle = open(&mut h, "T:\\new.bin", 0, true, false);
    h.bytes(INFO, &[0; 40]);
    h.runtime.memory().set_u64(INFO, 999).unwrap();
    h.runtime.memory().set_u64(INFO + 16, u64::MAX).unwrap();
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 40, 4]), 0);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 40, 4]), 0);
    assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 999);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 200);
    assert_eq!(h.runtime.memory().u64(INFO + 16).unwrap(), 300);
    h.bytes(INFO, &[1]);
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 1, 13]), 0);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u8(INFO + 20).unwrap(), 1);
    h.success(187, &[handle]);
    path(&h, "T:\\new.bin", 0);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        Error::ObjectNameNotFound.ntstatus()
    );
    path(&h, "T:\\a.txt", 0);
    assert_eq!(h.success(202, &[DATA, 0x100000, ATTR, IOSB, 7, 0x60]), 0);
    let limited = h.get(DATA);
    // Retail answers information queries whatever access the handle holds.
    assert_eq!(h.success(211, &[limited, IOSB, INFO, 40, 4]), 0);
    assert_eq!(h.success(211, &[limited, IOSB, INFO, 8, 14]), 0);
    assert_eq!(h.success(211, &[limited, IOSB, INFO, 24, 5]), 0);
    h.success(187, &[limited]);
}

#[test]
fn file_calls_reject_a_valid_handle_of_the_wrong_object_type() {
    let mut h = setup();
    assert_eq!(h.success(189, &[DATA, 0, 0, 0]), 0);
    let event = h.get(DATA);
    assert_eq!(
        h.success(219, &[event, 0, 0, 0, IOSB, BUFFER, 1, 0]),
        kernel::status::OBJECT_TYPE_MISMATCH
    );
    path(&h, "a.txt", event);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        kernel::status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.success(187, &[event]), 0);
}

#[test]
fn pre_io_failures_leave_the_io_status_block_untouched() {
    let mut h = setup();
    let marker = [0xa5; 8];
    h.bytes(IOSB, &marker);
    assert_eq!(
        h.success(219, &[0xbad, 0, 0, 0, IOSB, BUFFER, 1, 0]),
        Error::InvalidHandle.ntstatus()
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    h.success(189, &[DATA, 0, 0, 0]);
    let event = h.get(DATA);
    h.bytes(IOSB, &marker);
    assert_eq!(
        h.success(219, &[event, 0, 0, 0, IOSB, BUFFER, 1, 0]),
        kernel::status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    path(&h, "a.txt", event);
    h.bytes(IOSB, &marker);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        kernel::status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    path(&h, "a.txt", 0xbad);
    h.bytes(IOSB, &marker);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        Error::InvalidHandle.ntstatus()
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    path(&h, "T:\\a.txt", 0);
    h.bytes(IOSB, &marker);
    assert_eq!(
        h.success(202, &[DATA, 0x0040_0000, ATTR, IOSB, 7, 0x60]),
        Error::InvalidParameter.ntstatus()
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    let handle = open(&mut h, "T:\\a.txt", 0, false, false);
    h.bytes(IOSB, &marker);
    h.runtime.memory().set_u64(INFO, (-3i64) as u64).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 1, INFO]),
        Error::InvalidParameter.ntstatus()
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    // Invalid handles are rejected before even probing the caller's IOSB.
    assert_eq!(
        h.success(219, &[0xbad, 0, 0, 0, 0, BUFFER, 1, 0]),
        Error::InvalidHandle.ntstatus()
    );
}

#[test]
fn a_full_handle_table_fails_an_overwriting_open_before_the_file_is_truncated() {
    let mut h = setup();
    let spare = h.fill_handles();
    let marker = [0xa5; 8];
    h.bytes(IOSB, &marker);
    path(&h, "T:\\a.txt", 0);
    // NtCreateFile, FILE_OVERWRITE (4): VFS would truncate during the open.
    assert_eq!(
        h.success(
            190,
            &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 0, 0, 7, 4, 0x60]
        ),
        status::INSUFFICIENT_RESOURCES
    );
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    assert_eq!(h.success(187, &[spare]), 0);
    let handle = open(&mut h, "T:\\a.txt", 0, false, false);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 5);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 5, 0]), 0);
    assert_eq!(h.runtime.memory().read::<5>(BUFFER).unwrap(), *b"hello");
}

#[test]
fn other_hle_users_share_the_kernel_vfs_namespace_and_share_checks() {
    let mut h = setup();
    let shared = h.kernel.vfs().unwrap();
    let native = {
        let mut storage = shared.lock().unwrap();
        storage
            .namespace
            .create_symbolic_link("\\??\\U:", "\\Device\\TestDisk")
            .unwrap();
        storage
            .open(
                "U:\\a.txt",
                vfs::OpenOptions::from_nt(0x100001, 0, 1, 0x60, 0).unwrap(),
            )
            .unwrap()
            .handle
    };
    path(&h, "U:\\a.txt", 0);
    assert_eq!(
        h.success(202, &[DATA, 0x100001, ATTR, IOSB, 7, 0x60]),
        vfs::Error::SharingViolation.ntstatus()
    );
    shared.lock().unwrap().close(native).unwrap();
    let handle = open(&mut h, "U:\\a.txt", 0, false, false);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 5, 0]), 0);
    assert_eq!(h.runtime.memory().read::<5>(BUFFER).unwrap(), *b"hello");
}

#[test]
fn retail_disk_space_wrapper_opens_for_free_space_query_then_reads_volume_size() {
    let mut h = setup();
    path(&h, "T:\\", 0);
    // Exact retail arguments at 0x2d1620: SYNCHRONIZE | FILE_LIST_DIRECTORY,
    // FILE_SHARE_READ | FILE_SHARE_WRITE, FILE_OPEN_FOR_FREE_SPACE_QUERY |
    // FILE_SYNCHRONOUS_IO_NONALERT | FILE_DIRECTORY_FILE.
    assert_eq!(
        h.success(202, &[DATA, 0x0010_0001, ATTR, IOSB, 3, 0x0080_0021]),
        0
    );
    let handle = h.get(DATA);
    // Then 0x2d163b: FileFsSizeInformation (3) into a 24-byte buffer.
    assert_eq!(h.success(218, &[handle, IOSB, INFO, 24, 3]), 0);
    assert_eq!(h.get(IOSB + 4), 24);
    let memory = h.runtime.memory();
    assert_eq!(memory.u64(INFO).unwrap(), 100);
    assert_eq!(memory.u64(INFO + 8).unwrap(), 90);
    assert_eq!(memory.u32(INFO + 16).unwrap(), 32);
    assert_eq!(memory.u32(INFO + 20).unwrap(), 512);
    drop(memory);
    // The query flag is consumed by the open, not reported as a file mode.
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 4, 16]), 0);
    assert_eq!(h.get(INFO), 0x20);
    assert_eq!(h.success(187, &[handle]), 0);
}

#[test]
fn free_space_query_with_create_or_write_semantics_stops() {
    let mut h = setup();
    path(&h, "T:\\", 0);
    h.stopped(202, &[DATA, 0x0010_0003, ATTR, IOSB, 3, 0x0080_0021]);
    h.stopped(
        190,
        &[DATA, 0x0010_0001, ATTR, IOSB, 0, 0, 3, 3, 0x0080_0021],
    );
}

#[test]
fn valid_but_unimplemented_open_options_stop_instead_of_reporting_not_supported() {
    let mut h = setup();
    path(&h, "T:\\a.txt", 0);
    let marker = [0xa5; 8];
    h.bytes(IOSB, &marker);
    // FILE_COMPLETE_IF_OPLOCKED is a real option the VFS does not implement.
    h.stopped(202, &[DATA, 0x0010_0001, ATTR, IOSB, 7, 0x160]);
    assert_eq!(h.runtime.memory().read::<8>(IOSB).unwrap(), marker);
    assert!(
        h.kernel
            .stop_reasons()
            .iter()
            .any(|(o, r)| *o == 202 && r.contains("option unsupported"))
    );
}

const BIG: u32 = 0x2000_0000;
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn large_reads_and_writes_transfer_in_bounded_chunks() {
    let data = pattern(150_000);
    let mut h = setup_with(1 << 20, &[("big.bin", data.clone())]);
    h.runtime
        .memory()
        .map_zeroed(BIG, 0x40000, runtime::guest::Permissions::RW)
        .unwrap();
    let handle = open(&mut h, "T:\\big.bin", 0, false, false);
    // Explicit offset, spanning three host chunks.
    h.runtime.memory().set_u64(INFO, 10).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BIG, 149_990, INFO]),
        0
    );
    assert_eq!(h.get(IOSB + 4), 149_990);
    let mut out = vec![0; 149_990];
    h.runtime.memory().read_bytes(BIG, &mut out, 1).unwrap();
    assert_eq!(out, data[10..]);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 8, 14]), 0);
    assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 150_000);
    // At end of file the first chunk fails; that is the status reported.
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BIG, 1000, 0]),
        Error::EndOfFile.ntstatus()
    );
    // A read longer than the file is a successful short read.
    position(&mut h, handle, 0);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BIG, 0x40000, 0]), 0);
    assert_eq!(h.get(IOSB + 4), 150_000);

    // Write 200000 bytes through the chunked path, appending at end of file.
    let more = pattern(200_000);
    h.runtime.memory().write_bytes(BIG, &more, 1).unwrap();
    h.runtime.memory().set_u64(INFO, u64::MAX).unwrap();
    assert_eq!(
        h.success(236, &[handle, 0, 0, 0, IOSB, BIG, 200_000, INFO]),
        0
    );
    assert_eq!(h.get(IOSB + 4), 200_000);
    position(&mut h, handle, 150_000);
    h.runtime
        .memory()
        .write_bytes(BIG, &vec![0; 200_000], 1)
        .unwrap();
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BIG, 200_000, 0]), 0);
    let mut out = vec![0; 200_000];
    h.runtime.memory().read_bytes(BIG, &mut out, 1).unwrap();
    assert_eq!(out, more);
    // Zero-length transfers still complete.
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BIG, 0, 0]), 0);
    assert_eq!(h.get(IOSB + 4), 0);
}

/// Records the largest single host allocation made by the current thread while
/// armed. Test-only: the library itself forbids unsafe code.
struct LargestAllocation;
thread_local! {
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LARGEST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe impl std::alloc::GlobalAlloc for LargestAllocation {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            LARGEST.with(|l| l.set(l.get().max(layout.size())));
        }
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: LargestAllocation = LargestAllocation;

#[test]
fn alias_backed_guest_read_never_allocates_its_full_length_on_the_host() {
    let mut h = setup();
    let handle = open(&mut h, "T:\\a.txt", 0, false, false);
    // Eight 64 MiB virtual aliases of the same RAM: a valid 512 MiB buffer.
    for i in 0..8 {
        h.runtime
            .memory()
            .map(
                0x1000_0000 + i * 0x400_0000,
                0x400_0000,
                0,
                runtime::guest::Permissions::RW,
            )
            .unwrap();
    }
    ARMED.with(|a| a.set(true));
    let result = h.success(219, &[handle, 0, 0, 0, IOSB, 0x1000_0000, 0x2000_0000, 0]);
    ARMED.with(|a| a.set(false));
    assert_eq!(result, 0);
    assert_eq!(h.get(IOSB + 4), 5);
    assert_eq!(
        h.runtime.memory().read::<5>(0x1000_0000).unwrap(),
        *b"hello"
    );
    let largest = LARGEST.with(|l| l.get());
    assert!(
        largest <= 1 << 20,
        "host allocation of {largest} bytes for a guest read"
    );
}

#[test]
fn xapi_generic_write_handle_answers_basic_and_network_open_information() {
    let mut h = setup();
    // XAPI's TitleMeta.xbx open: GENERIC_WRITE | SYNCHRONIZE, with
    // FILE_SYNCHRONOUS_IO_ALERT rather than _NONALERT.
    path(&h, "T:\\a.txt", 0);
    assert_eq!(h.success(202, &[DATA, 0x4010_0000, ATTR, IOSB, 0, 0x50]), 0);
    let handle = h.get(DATA);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 4, 16]), 0);
    assert_eq!(h.get(INFO), 0x10);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 40, 4]), 0);
    assert_eq!(h.get(INFO + 32), vfs::ATTR_ARCHIVE);
    h.bytes(INFO, &[0xcc; 64]);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 56, 34]), 0);
    assert_eq!(h.get(IOSB + 4), 56);
    let memory = h.runtime.memory();
    for (i, time) in [100u64, 200, 300, 400].into_iter().enumerate() {
        assert_eq!(memory.u64(INFO + i as u32 * 8).unwrap(), time);
    }
    assert_eq!(memory.u64(INFO + 32).unwrap(), 16384);
    assert_eq!(memory.u64(INFO + 40).unwrap(), 5);
    assert_eq!(memory.u32(INFO + 48).unwrap(), vfs::ATTR_ARCHIVE);
    assert_eq!(memory.u32(INFO + 52).unwrap(), 0);
    assert_eq!(memory.u8(INFO + 56).unwrap(), 0xcc);
    drop(memory);
    // Writes still need write access; the handle has it.
    h.bytes(BUFFER, b"HE");
    assert_eq!(h.success(236, &[handle, 0, 0, 0, IOSB, BUFFER, 2, 0]), 0);
    // A short network-open buffer keeps the unverified short-buffer stop.
    h.stopped(211, &[handle, IOSB, INFO, 52, 34]);
    // Synchronous-alert and creation both admitted on NtCreateFile too.
    path(&h, "T:\\alert.bin", 0);
    assert_eq!(
        h.success(190, &[DATA, 0x4010_0000, ATTR, IOSB, 0, 0, 0, 2, 0x50]),
        0
    );
    // Neither synchronous bit: an asynchronous file object.
    assert_eq!(h.success(187, &[handle]), 0);
    path(&h, "T:\\a.txt", 0);
    assert_eq!(h.success(202, &[DATA, 0x0010_0001, ATTR, IOSB, 7, 0x40]), 0);
    assert_eq!(h.success(187, &[h.get(DATA)]), 0);
}

#[test]
fn symbolic_link_objects_open_query_and_close() {
    let mut h = setup();
    let target = DATA + 0x600;
    let out = DATA + 0x608;
    // XAPI 0x2d0e12: open \??\D: (here T:), then query into a 0x208 buffer.
    path(&h, "\\??\\T:", 0);
    assert_eq!(h.success(203, &[DATA, ATTR]), 0);
    let handle = h.get(DATA);
    h.set(target, 0x0208 << 16);
    h.set(target + 4, BUFFER);
    h.bytes(BUFFER, &[0xcc; 32]);
    assert_eq!(h.success(215, &[handle, target, out]), 0);
    assert_eq!(h.runtime.memory().u16(target).unwrap(), 16);
    assert_eq!(h.get(out), 17);
    assert_eq!(
        h.runtime.memory().read::<17>(BUFFER).unwrap(),
        *b"\\Device\\TestDisk\0"
    );
    // No room for the terminator: nothing is copied.
    h.set(target, 16 << 16);
    h.bytes(BUFFER, &[0xcc; 32]);
    assert_eq!(
        h.success(215, &[handle, target, 0]),
        status::BUFFER_TOO_SMALL
    );
    assert_eq!(h.runtime.memory().u8(BUFFER).unwrap(), 0xcc);
    // The target was captured at open.
    string(&h, NAME, DATA + 0x200, "\\??\\T:");
    assert_eq!(h.success(69, &[NAME]), 0);
    h.set(target, 0x0208 << 16);
    assert_eq!(h.success(215, &[handle, target, out]), 0);
    // Link handles are not events or files.
    assert_eq!(h.success(225, &[handle, 0]), status::OBJECT_TYPE_MISMATCH);
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 4, 0]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.success(187, &[handle]), 0);
    assert_eq!(
        h.success(215, &[handle, target, out]),
        status::INVALID_HANDLE
    );
    // A missing link fails cleanly, so XAPI falls back to \Device\CdRom0.
    path(&h, "\\??\\D:", 0);
    assert_eq!(
        h.success(203, &[DATA, ATTR]),
        Error::ObjectNameNotFound.ntstatus()
    );
}

fn raw_setup(title_id: Option<u32>) -> Harness {
    let mut h = Harness::config(kernel::Config {
        title_id,
        utility_partition: 4,
        ..kernel::Config::default()
    });
    h.kernel.set_vfs(vfs::Vfs::new()).unwrap();
    // XapiSelectCachePartition 0x2d592c: \Device\Harddisk0\partition0,
    // GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE, share 3, SYNCHRONOUS_IO_ALERT.
    string(&h, NAME, DATA + 0x200, "\\Device\\Harddisk0\\partition0");
    h.set(ATTR, 0);
    h.set(ATTR + 4, NAME);
    h.set(ATTR + 8, 0x40);
    assert_eq!(h.success(202, &[DATA, 0xc010_0000, ATTR, IOSB, 3, 0x10]), 0);
    h
}

#[test]
fn raw_partition0_holds_a_cache_database_assigning_the_utility_partition() {
    let mut h = raw_setup(Some(0x4d53_0064));
    let handle = h.get(DATA);
    h.runtime.memory().set_u64(INFO, 0x800).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 0x200, INFO]),
        0
    );
    assert_eq!(h.get(IOSB + 4), 0x200);
    let sector = h.runtime.memory().read::<512>(BUFFER).unwrap();
    let word = |at: usize| u32::from_le_bytes(sector[at..at + 4].try_into().unwrap());
    // The checks at 0x2d59bd..0x2d59d7.
    assert_eq!(word(0), 0x9731_5286);
    assert_eq!(word(4), 2);
    assert_eq!(word(0x1f8), 0xaa55_0000);
    // Entry 0: our title -> Partition4 (index 1), valid.
    assert_eq!([word(8), word(12), word(16)], [0x4d53_0064, 1, 1]);
    assert!(sector[20..0x1f8].iter().all(|b| *b == 0));
    // XAPI rewrites the database at the same offset; it reads back.
    h.bytes(BUFFER + 16, &0u32.to_le_bytes());
    h.runtime.memory().set_u64(INFO, 0x800).unwrap();
    assert_eq!(
        h.success(236, &[handle, 0, 0, 0, IOSB, BUFFER, 0x200, INFO]),
        0
    );
    h.bytes(BUFFER, &[0; 0x200]);
    h.runtime.memory().set_u64(INFO, 0x800).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 0x200, INFO]),
        0
    );
    assert_eq!(h.get(BUFFER + 16), 0);
    assert_eq!(h.get(BUFFER), 0x9731_5286);
    // Other sectors are zero.
    h.runtime.memory().set_u64(INFO, 0x600).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 0x200, INFO]),
        0
    );
    assert!(
        h.runtime
            .memory()
            .read::<512>(BUFFER)
            .unwrap()
            .iter()
            .all(|b| *b == 0)
    );
}

#[test]
fn raw_partition0_without_a_title_has_an_empty_valid_database() {
    let mut h = raw_setup(None);
    let handle = h.get(DATA);
    h.runtime.memory().set_u64(INFO, 0x800).unwrap();
    assert_eq!(
        h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 0x200, INFO]),
        0
    );
    let sector = h.runtime.memory().read::<512>(BUFFER).unwrap();
    assert_eq!(sector[..8], [0x86, 0x52, 0x31, 0x97, 2, 0, 0, 0]);
    assert!(sector[8..0x1f8].iter().all(|b| *b == 0));
    // An existing Partition0 in the VFS is left alone.
    let mut other = vfs::Vfs::new();
    other
        .mount(
            kernel::rawdisk::RAW_DISK_DEVICE,
            kernel::rawdisk::RawDisk::new(0x10_0000, &[]).unwrap(),
        )
        .unwrap();
    let h2 = Harness::new();
    h2.kernel.set_vfs(other).unwrap();
}

#[test]
fn raw_partition0_retains_a_bounded_number_of_written_sectors() {
    use kernel::limits::MAX_RAW_DISK_SECTORS;
    use kernel::rawdisk::{DISK_SIZE, RawDisk, SECTOR_SIZE};
    let mut disk = RawDisk::new(DISK_SIZE, &[]).unwrap();
    // The cache database already holds one sector of the budget.
    let first = 0x10_0000 / SECTOR_SIZE;
    let room = MAX_RAW_DISK_SECTORS as u64 - 1;
    for sector in first..first + room {
        assert_eq!(disk.write_at("", sector * SECTOR_SIZE, &[0xa5]).unwrap(), 1);
    }
    // Rewriting retained sectors, the database included, still succeeds.
    assert_eq!(
        disk.write_at("", first * SECTOR_SIZE + 1, &[0x5a]).unwrap(),
        1
    );
    assert_eq!(disk.write_at("", 0x800, &[0; 512]).unwrap(), 512);
    // A sparse write to a new sector exceeds the budget.
    let past = (first + room) * SECTOR_SIZE;
    assert_eq!(disk.write_at("", past, &[0xa5]), Err(Error::DiskFull));
    // A write spanning a retained sector and a new one changes neither.
    let straddle = past - 2;
    assert_eq!(
        disk.write_at("", straddle, &[0xff; 4]),
        Err(Error::DiskFull)
    );
    let mut bytes = [0u8; 4];
    assert_eq!(disk.read_at("", straddle, &mut bytes).unwrap(), 4);
    assert_eq!(bytes, [0; 4]);
    let mut sector = [0u8; 512];
    disk.read_at("", (first + room - 1) * SECTOR_SIZE, &mut sector)
        .unwrap();
    assert_eq!(sector[0], 0xa5);
    assert!(sector[1..].iter().all(|b| *b == 0));
    disk.read_at("", first * SECTOR_SIZE, &mut sector).unwrap();
    assert_eq!(sector[..2], [0xa5, 0x5a]);
}

const DOS_DEVICES: u32 = 0xffff_fffd;
const EVENT_OUT: u32 = DATA + 0x700;
const OFFSET: u32 = DATA + 0x710;

#[test]
fn dos_devices_root_names_resolve_relative_to_the_dos_devices_directory() {
    let mut h = setup();
    // XAPI CreateFile/DeleteFile: RootDirectory (HANDLE)-3 with "t:\a.txt".
    let handle = open(&mut h, "t:\\a.txt", DOS_DEVICES, false, false);
    assert_eq!(h.success(219, &[handle, 0, 0, 0, IOSB, BUFFER, 5, 0]), 0);
    assert_eq!(h.runtime.memory().read::<5>(BUFFER).unwrap(), *b"hello");
    h.success(187, &[handle]);
    let created = open(&mut h, "t:\\made.bin", DOS_DEVICES, true, false);
    assert_eq!(h.get(IOSB + 4), 2); // FILE_CREATED
    h.success(187, &[created]);
    // A missing name reports the VFS status, as an absolute name would.
    path(&h, "t:\\missing.bin", DOS_DEVICES);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]),
        Error::ObjectNameNotFound.ntstatus()
    );
    // Symbolic-link objects resolve under the same root.
    path(&h, "T:", DOS_DEVICES);
    assert_eq!(h.success(203, &[DATA, ATTR]), 0);
    h.success(187, &[h.get(DATA)]);
    // A rooted or empty name relative to \?? is not a verified form.
    path(&h, "\\t:\\a.txt", DOS_DEVICES);
    h.stopped(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 7, 0x60]);
    path(&h, "", DOS_DEVICES);
    h.stopped(210, &[ATTR, INFO]);
}

#[test]
fn query_full_attributes_reads_path_metadata_without_an_open_handle() {
    let mut h = setup();
    // An exclusive open with no sharing: a real open would collide, the
    // attribute query does not.
    path(&h, "T:\\a.txt", 0);
    assert_eq!(
        h.success(202, &[DATA, vfs::Access::ALL.0, ATTR, IOSB, 0, 0x60]),
        0
    );
    let exclusive = h.get(DATA);
    h.bytes(INFO, &[0xcc; 64]);
    path(&h, "t:\\a.txt", DOS_DEVICES);
    assert_eq!(h.success(210, &[ATTR, INFO]), 0);
    let m = h.runtime.memory();
    for (i, time) in [100u64, 200, 300, 400].into_iter().enumerate() {
        assert_eq!(m.u64(INFO + i as u32 * 8).unwrap(), time);
    }
    assert_eq!(m.u64(INFO + 32).unwrap(), 16384);
    assert_eq!(m.u64(INFO + 40).unwrap(), 5);
    assert_eq!(m.u32(INFO + 48).unwrap(), vfs::ATTR_ARCHIVE);
    assert_eq!(m.u32(INFO + 52).unwrap(), 0);
    assert_eq!(m.u8(INFO + 56).unwrap(), 0xcc);
    drop(m);
    h.success(187, &[exclusive]);
    // Missing names, and a trailing backslash on a file.
    path(&h, "t:\\fonts\\", DOS_DEVICES);
    assert_eq!(
        h.success(210, &[ATTR, INFO]),
        Error::ObjectNameNotFound.ntstatus()
    );
    path(&h, "t:\\a.txt\\", DOS_DEVICES);
    assert_eq!(
        h.success(210, &[ATTR, INFO]),
        Error::NotADirectory.ntstatus()
    );
    // A volume root is a directory.
    path(&h, "t:\\", DOS_DEVICES);
    assert_eq!(h.success(210, &[ATTR, INFO]), 0);
    assert_eq!(h.get(INFO + 48), vfs::ATTR_DIRECTORY);
    // Relative to a directory handle; a non-file handle is a type mismatch.
    let root = open(&mut h, "T:\\", 0, false, true);
    path(&h, "b.bin", root);
    assert_eq!(h.success(210, &[ATTR, INFO]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 40).unwrap(), 5);
    assert_eq!(h.success(189, &[EVENT_OUT, 0, 0, 0]), 0);
    path(&h, "b.bin", h.get(EVENT_OUT));
    assert_eq!(h.success(210, &[ATTR, INFO]), status::OBJECT_TYPE_MISMATCH);
    // Output is checked before anything else.
    path(&h, "t:\\a.txt", DOS_DEVICES);
    let t = h.call(210, &[ATTR, 0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

#[test]
fn query_full_attributes_stops_without_a_vfs() {
    let mut h = Harness::new();
    h.stopped(210, &[0, 0]);
}

#[test]
fn allocation_information_truncates_below_end_of_file_and_is_otherwise_a_reservation() {
    let mut h = setup();
    let handle = open(&mut h, "T:\\a.txt", 0, false, false);
    // XAPI SetEndOfFile: class 20 then class 19 with the same size.
    h.runtime.memory().set_u64(INFO, 3).unwrap();
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 8, 20]), 0);
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 8, 19]), 0);
    assert_eq!((h.get(IOSB), h.get(IOSB + 4)), (0, 0));
    // Larger: a cluster reservation only; end of file is unchanged.
    h.runtime.memory().set_u64(INFO, 0x10000).unwrap();
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 8, 19]), 0);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 3);
    // Smaller: truncates.
    h.runtime.memory().set_u64(INFO, 1).unwrap();
    assert_eq!(h.success(226, &[handle, IOSB, INFO, 8, 19]), 0);
    assert_eq!(h.success(211, &[handle, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 1);
    // Short buffer, negative size, missing write access.
    assert_eq!(
        h.success(226, &[handle, IOSB, INFO, 4, 19]),
        Error::InvalidParameter.ntstatus()
    );
    h.runtime.memory().set_u64(INFO, u64::MAX).unwrap();
    assert_eq!(
        h.success(226, &[handle, IOSB, INFO, 8, 19]),
        Error::InvalidParameter.ntstatus()
    );
    h.success(187, &[handle]);
    path(&h, "T:\\b.bin", 0);
    assert_eq!(h.success(202, &[DATA, 0x0010_0001, ATTR, IOSB, 7, 0x60]), 0);
    let read_only = h.get(DATA);
    h.runtime.memory().set_u64(INFO, 0).unwrap();
    assert_eq!(
        h.success(226, &[read_only, IOSB, INFO, 8, 19]),
        Error::AccessDenied.ntstatus()
    );
    assert_eq!(h.success(211, &[read_only, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 5);
}

/// NtWaitForSingleObjectEx with a zero timeout: 0 if signalled, else TIMEOUT.
fn signalled(h: &mut Harness, handle: u32) -> bool {
    h.runtime.memory().set_u64(OFFSET + 8, 0).unwrap();
    match h.success(234, &[handle, 0, 0, OFFSET + 8]) {
        0 => true,
        status::TIMEOUT => false,
        other => panic!("wait status {other:#x}"),
    }
}

#[test]
fn asynchronous_file_objects_complete_synchronously_with_event_and_iosb() {
    let mut h = setup();
    // XAPI DeleteFile: DELETE access only (no SYNCHRONIZE), options 0x4040.
    path(&h, "t:\\b.bin", DOS_DEVICES);
    assert_eq!(h.success(202, &[DATA, 0x10000, ATTR, IOSB, 7, 0x4040]), 0);
    let deleting = h.get(DATA);
    h.bytes(INFO, &[1]);
    assert_eq!(h.success(226, &[deleting, IOSB, INFO, 1, 13]), 0);
    assert_eq!(h.success(187, &[deleting]), 0);
    path(&h, "t:\\b.bin", DOS_DEVICES);
    assert_eq!(
        h.success(210, &[ATTR, INFO]),
        Error::ObjectNameNotFound.ntstatus()
    );

    // An overlapped reader: GENERIC_READ | GENERIC_WRITE, no synchronous bit.
    path(&h, "T:\\a.txt", 0);
    assert_eq!(h.success(202, &[DATA, 0xc000_0000, ATTR, IOSB, 7, 0x40]), 0);
    let file = h.get(DATA);
    assert_eq!(h.success(211, &[file, IOSB, INFO, 4, 16]), 0);
    assert_eq!(h.get(INFO), 0, "FileModeInformation: no synchronous bit");
    // No ByteOffset: invalid for an asynchronous object; IOSB untouched.
    h.set(IOSB, 0x103);
    h.set(IOSB + 4, 0xaaaa);
    assert_eq!(
        h.success(219, &[file, 0, 0, 0, IOSB, BUFFER, 5, 0]),
        status::INVALID_PARAMETER
    );
    assert_eq!((h.get(IOSB), h.get(IOSB + 4)), (0x103, 0xaaaa));
    // FILE_USE_FILE_POINTER_POSITION has no verified meaning here.
    h.runtime.memory().set_u64(OFFSET, (-2i64) as u64).unwrap();
    h.stopped(219, &[file, 0, 0, 0, IOSB, BUFFER, 5, OFFSET]);
    // XAPI ReadFile with an OVERLAPPED: event, no APC routine, the OVERLAPPED
    // as ApcContext (a completion key, ignored), IOSB = the OVERLAPPED.
    assert_eq!(h.success(189, &[EVENT_OUT, 0, 0, 1]), 0); // signalled
    let event = h.get(EVENT_OUT);
    h.runtime.memory().set_u64(OFFSET, 1).unwrap();
    h.bytes(BUFFER, &[0; 8]);
    assert_eq!(
        h.success(219, &[file, event, 0, IOSB, IOSB, BUFFER, 8, OFFSET]),
        0
    );
    assert_eq!((h.get(IOSB), h.get(IOSB + 4)), (0, 4));
    assert_eq!(h.runtime.memory().read::<4>(BUFFER).unwrap(), *b"ello");
    assert!(signalled(&mut h, event));
    // The position of an asynchronous object is not moved by transfers.
    assert_eq!(h.success(211, &[file, IOSB, INFO, 8, 14]), 0);
    assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 0);
    // A failed transfer clears the event and does not signal it.
    h.runtime.memory().set_u64(OFFSET, 100).unwrap();
    assert_eq!(
        h.success(219, &[file, event, 0, 0, IOSB, BUFFER, 8, OFFSET]),
        Error::EndOfFile.ntstatus()
    );
    assert!(!signalled(&mut h, event));
    // Writes at end of file (-1) append.
    h.bytes(BUFFER, b"!!");
    h.runtime.memory().set_u64(OFFSET, u64::MAX).unwrap();
    assert_eq!(
        h.success(236, &[file, event, 0, 0, IOSB, BUFFER, 2, OFFSET]),
        0
    );
    assert!(signalled(&mut h, event));
    assert_eq!(h.success(211, &[file, IOSB, INFO, 24, 5]), 0);
    assert_eq!(h.runtime.memory().u64(INFO + 8).unwrap(), 7);
    // An APC routine needs APC delivery; a non-event handle is a mismatch.
    h.stopped(219, &[file, 0, CODE, 0, IOSB, BUFFER, 1, OFFSET]);
    h.runtime.memory().set_u64(OFFSET, 0).unwrap();
    assert_eq!(
        h.success(219, &[file, file, 0, 0, IOSB, BUFFER, 1, OFFSET]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(
        h.success(219, &[file, 0x7ff0, 0, 0, IOSB, BUFFER, 1, OFFSET]),
        status::INVALID_HANDLE
    );
    // The NT low-bit convention (no completion-port post) is unverified on
    // Xbox: an explicit stop, whatever the ApcContext, rather than a silent
    // invalid-handle result.
    h.stopped(219, &[file, event | 1, 0, IOSB, IOSB, BUFFER, 1, OFFSET]);
    h.success(187, &[file]);
    // Synchronous objects take an event too, and move their position.
    let sync = open(&mut h, "T:\\a.txt", 0, false, false);
    assert_eq!(h.success(189, &[EVENT_OUT, 0, 0, 0]), 0); // not signalled
    let event = h.get(EVENT_OUT);
    assert!(!signalled(&mut h, event));
    assert_eq!(h.success(219, &[sync, event, 0, 0, IOSB, BUFFER, 3, 0]), 0);
    assert!(signalled(&mut h, event));
    assert_eq!(h.success(211, &[sync, IOSB, INFO, 8, 14]), 0);
    assert_eq!(h.runtime.memory().u64(INFO).unwrap(), 3);
    h.success(187, &[sync]);
}
