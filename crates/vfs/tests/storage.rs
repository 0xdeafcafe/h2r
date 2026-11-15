use std::{
    fs,
    io::{Cursor, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use vfs::*;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-vfs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn storage(&self) -> Vfs {
        linked(Config::new(&self.0, 0x4D530064))
    }
}
/// Storage plus the host-tool links, and extra letters the tests use for the partitions.
fn linked(config: Config) -> Vfs {
    let mut vfs = Vfs::with_storage(config.clone()).unwrap();
    vfs.create_standard_links(&config).unwrap();
    for (drive, n) in [('E', 1), ('C', 2), ('X', 3), ('Y', 4)] {
        vfs.namespace
            .create_symbolic_link(
                &format!("\\??\\{drive}:"),
                &format!("\\Device\\Harddisk0\\Partition{n}"),
            )
            .unwrap();
    }
    vfs
}

#[test]
fn with_storage_creates_no_links() {
    let temp = TempDir::new();
    let mut vfs = Vfs::with_storage(Config::new(&temp.0, 0x4D530064)).unwrap();
    for drive in ['C', 'D', 'E', 'T', 'U', 'X', 'Y', 'Z'] {
        assert!(
            vfs.namespace
                .query_symbolic_link(&format!("\\??\\{drive}:"))
                .is_err(),
            "{drive}: must not exist"
        );
    }
    assert!(
        vfs.query_path("\\Device\\Harddisk0\\Partition1\\UDATA")
            .is_ok()
    );
}

#[test]
fn create_standard_links_rejects_bad_utility_partition() {
    let temp = TempDir::new();
    let mut vfs = Vfs::with_storage(Config::new(&temp.0, 0x4D530064)).unwrap();
    for partition in [0, 1, 2, 6] {
        let mut config = Config::new(&temp.0, 0x4D530064);
        config.utility_partition = partition;
        assert_eq!(
            vfs.create_standard_links(&config),
            Err(Error::InvalidParameter)
        );
    }
    assert!(vfs.namespace.query_symbolic_link("\\??\\D:").is_err());
}

#[test]
fn create_standard_links_creates_exactly_four() {
    let temp = TempDir::new();
    let config = Config::new(&temp.0, 0x4D530064);
    let mut vfs = Vfs::with_storage(config.clone()).unwrap();
    vfs.create_standard_links(&config).unwrap();
    let target = |d: char| vfs.namespace.query_symbolic_link(&format!("\\??\\{d}:"));
    assert_eq!(target('D'), Ok("\\Device\\CdRom0"));
    assert_eq!(
        target('T'),
        Ok("\\Device\\Harddisk0\\Partition1\\TDATA\\4D530064")
    );
    assert_eq!(
        target('U'),
        Ok("\\Device\\Harddisk0\\Partition1\\UDATA\\4D530064")
    );
    assert_eq!(target('Z'), Ok("\\Device\\Harddisk0\\Partition5"));
    for drive in ['C', 'E', 'X', 'Y'] {
        assert!(target(drive).is_err(), "{drive}: must not exist");
    }
    assert_eq!(
        vfs.create_standard_links(&config),
        Err(Error::ObjectNameCollision)
    );
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn options(disposition: CreateDisposition) -> OpenOptions {
    OpenOptions {
        access: Access::ALL,
        disposition,
        ..Default::default()
    }
}
fn create(fs: &mut Vfs, name: &str) -> Handle {
    fs.open(name, options(CreateDisposition::Create))
        .unwrap()
        .handle
}
fn directory(fs: &mut Vfs, name: &str) -> Handle {
    fs.open(
        name,
        OpenOptions {
            directory: Some(true),
            ..options(CreateDisposition::Create)
        },
    )
    .unwrap()
    .handle
}
fn read_all(fs: &mut Vfs, handle: Handle) -> Vec<u8> {
    let mut buf = vec![0; fs.query_information(handle).unwrap().metadata.end_of_file as usize];
    fs.read(handle, Offset::Absolute(0), &mut buf).unwrap();
    buf
}

#[test]
fn independent_closes_cannot_release_another_handles_reservation() {
    let temp = TempDir::new();
    let mut fs = Vfs::new();
    fs.mount(
        "\\Device\\Test",
        HostBackend::new(temp.0.join("data"), temp.0.join("meta"))
            .unwrap()
            .with_volume("small", 1, 2 * FATX_CLUSTER_SIZE),
    )
    .unwrap();
    let saved = "\\Device\\Test\\saved";
    let writer = fs
        .open_allocated(
            saved,
            options(CreateDisposition::Create),
            2 * FATX_CLUSTER_SIZE,
        )
        .unwrap()
        .handle;
    let duplicate = fs.duplicate(writer).unwrap();
    let reader = fs
        .open(
            saved,
            OpenOptions {
                access: Access::READ,
                ..options(CreateDisposition::Open)
            },
        )
        .unwrap()
        .handle;
    fs.close(reader).unwrap();
    let independent_writer = fs
        .open(saved, options(CreateDisposition::Open))
        .unwrap()
        .handle;
    fs.close(independent_writer).unwrap();
    assert_eq!(
        fs.query_information(writer)
            .unwrap()
            .metadata
            .allocation_size,
        2 * FATX_CLUSTER_SIZE
    );
    let other = "\\Device\\Test\\other";
    assert_eq!(
        fs.open_allocated(
            other,
            options(CreateDisposition::Create),
            2 * FATX_CLUSTER_SIZE
        ),
        Err(Error::DiskFull)
    );
    assert_eq!(fs.write(writer, Offset::Absolute(0), b"x"), Ok(1));
    fs.close(writer).unwrap();
    assert_eq!(
        fs.query_information(duplicate)
            .unwrap()
            .metadata
            .allocation_size,
        2 * FATX_CLUSTER_SIZE
    );
    fs.close(duplicate).unwrap();
    assert_eq!(
        fs.query_path(saved).unwrap().allocation_size,
        FATX_CLUSTER_SIZE
    );
    let other = fs
        .open_allocated(other, options(CreateDisposition::Create), FATX_CLUSTER_SIZE)
        .unwrap()
        .handle;
    fs.close(other).unwrap();
}

#[test]
fn overwritten_reservation_belongs_to_the_new_file_object() {
    let temp = TempDir::new();
    let mut fs = Vfs::new();
    fs.mount(
        "\\Device\\Test",
        HostBackend::new(temp.0.join("data"), temp.0.join("meta"))
            .unwrap()
            .with_volume("small", 1, 2 * FATX_CLUSTER_SIZE),
    )
    .unwrap();
    let saved = "\\Device\\Test\\saved";
    let old = fs
        .open_allocated(
            saved,
            options(CreateDisposition::Create),
            2 * FATX_CLUSTER_SIZE,
        )
        .unwrap()
        .handle;
    let new = fs
        .open_allocated(
            saved,
            options(CreateDisposition::Overwrite),
            2 * FATX_CLUSTER_SIZE,
        )
        .unwrap()
        .handle;
    fs.close(old).unwrap();
    assert_eq!(
        fs.query_information(new).unwrap().metadata.allocation_size,
        2 * FATX_CLUSTER_SIZE
    );
    assert_eq!(
        fs.open_allocated(
            "\\Device\\Test\\other",
            options(CreateDisposition::Create),
            1
        ),
        Err(Error::DiskFull)
    );
    fs.close(new).unwrap();
    assert_eq!(fs.query_path(saved).unwrap().allocation_size, 0);
    let other = fs
        .open_allocated(
            "\\Device\\Test\\other",
            options(CreateDisposition::Create),
            2 * FATX_CLUSTER_SIZE,
        )
        .unwrap()
        .handle;
    fs.close(other).unwrap();
}

#[test]
fn symbolic_links_paths_and_cycles() {
    let mut ns = Namespace::new();
    ns.create_symbolic_link("\\??\\D:", CDROM_DEVICE).unwrap();
    ns.create_symbolic_link("\\??\\Q:", "d:\\maps").unwrap();
    assert_eq!(
        ns.resolve("q:\\Test.map").unwrap(),
        "\\Device\\CdRom0\\maps\\Test.map"
    );
    assert_eq!(
        ns.resolve("\\device\\cdrom01\\file").unwrap(),
        "\\device\\cdrom01\\file"
    );
    assert_eq!(
        ns.create_symbolic_link("d:", CDROM_DEVICE),
        Err(Error::ObjectNameCollision)
    );
    ns.create_symbolic_link("\\Device\\CdRom0\\maps", "\\Device\\Other")
        .unwrap();
    assert_eq!(ns.resolve("Q:\\a").unwrap(), "\\Device\\Other\\a");
    ns.delete_symbolic_link("\\DEVICE\\CDROM0\\MAPS").unwrap();
    ns.create_symbolic_link("r:", "s:").unwrap();
    ns.create_symbolic_link("s:", "r:").unwrap();
    assert_eq!(ns.resolve("R:\\x"), Err(Error::ReparsePointNotResolved));
    for name in [
        "D:relative",
        "D:\\..\\x",
        "D:\\a/escape",
        "D:\\a\\\\b",
        "D:\\nul\0",
        "D:\\é",
    ] {
        assert!(ns.resolve(name).is_err(), "{name:?}");
    }
}

#[test]
fn fatx_names_are_bytes_and_spaces_are_preserved() {
    assert!(validate_fatx_name(&"x".repeat(42)).is_ok());
    for name in [
        "", ".", "..", "a:b", "a/b", "a\\b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "é",
        "a\u{7f}",
    ] {
        assert_eq!(
            validate_fatx_name(name),
            Err(Error::ObjectNameInvalid),
            "{name:?}"
        );
    }
    assert!(validate_fatx_name(&"x".repeat(43)).is_err());
    assert!(validate_fatx_name("Profile 1. ").is_ok());
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let file = create(&mut vfs, "U:\\Profile ");
    vfs.close(file).unwrap();
    assert!(vfs.query_path("U:\\profile ").is_ok());
    assert_eq!(
        vfs.query_path("U:\\profile"),
        Err(Error::ObjectNameNotFound)
    );
    assert_eq!(
        vfs.query_path(&format!("U:\\{}", "a".repeat(43))),
        Err(Error::ObjectNameInvalid)
    );
    let path = vec!["a".repeat(42); 6].join("\\");
    assert_eq!(validate_fatx_path(&path), Err(Error::ObjectNameInvalid));
}

#[test]
fn title_roots_alias_identity_and_separate_titles() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let handle = create(&mut vfs, "U:\\Profile");
    vfs.write(handle, Offset::Current, b"save").unwrap();
    for alias in [
        "u:\\profile",
        "E:\\UDATA\\4d530064\\PROFILE",
        "\\Device\\Harddisk0\\Partition1\\udata\\4D530064\\profile",
    ] {
        let other = vfs.open(alias, OpenOptions::default()).unwrap().handle;
        assert_eq!(read_all(&mut vfs, other), b"save");
        vfs.close(other).unwrap();
    }
    assert_eq!(
        fs::read(temp.0.join("disk/Partition1/UDATA/4D530064/Profile")).unwrap(),
        b"save"
    );
    let t = create(&mut vfs, "T:\\settings");
    let z = create(&mut vfs, "Z:\\cache");
    vfs.close(t).unwrap();
    vfs.close(z).unwrap();
    vfs.close(handle).unwrap();
    let mut other = linked(Config::new(&temp.0, 0x11111111));
    assert_eq!(
        other.query_path("U:\\Profile"),
        Err(Error::ObjectNameNotFound)
    );
    assert!(other.query_path("E:\\UDATA\\4D530064\\Profile").is_ok());
    assert!(
        vfs.query_path("\\Device\\Harddisk0\\Partition5\\cache")
            .is_ok()
    );
    assert!(vfs.query_path("C:\\").unwrap().is_directory());
    assert!(vfs.query_path("X:\\").unwrap().is_directory());
    assert!(vfs.query_path("Y:\\").unwrap().is_directory());
}

#[test]
fn every_create_disposition_existing_and_missing() {
    for disposition in [
        CreateDisposition::Open,
        CreateDisposition::Create,
        CreateDisposition::OpenIf,
        CreateDisposition::Overwrite,
        CreateDisposition::OverwriteIf,
        CreateDisposition::Supersede,
    ] {
        for exists in [false, true] {
            let temp = TempDir::new();
            let mut vfs = temp.storage();
            if exists {
                let h = create(&mut vfs, "U:\\file");
                vfs.write(h, Offset::Current, b"original").unwrap();
                vfs.close(h).unwrap();
            }
            let result = vfs.open("U:\\file", options(disposition));
            let expected = match (disposition, exists) {
                (CreateDisposition::Open | CreateDisposition::Overwrite, false) => {
                    Err(Error::ObjectNameNotFound)
                }
                (CreateDisposition::Create, true) => Err(Error::ObjectNameCollision),
                (_, false) => Ok(CreateInformation::Created),
                (CreateDisposition::Open | CreateDisposition::OpenIf, true) => {
                    Ok(CreateInformation::Opened)
                }
                (CreateDisposition::Supersede, true) => Ok(CreateInformation::Superseded),
                (_, true) => Ok(CreateInformation::Overwritten),
            };
            assert_eq!(
                result.map(|r| r.information),
                expected,
                "{disposition:?}, exists={exists}"
            );
            if let Ok(result) = result {
                let preserved = result.information == CreateInformation::Opened;
                assert_eq!(
                    read_all(&mut vfs, result.handle),
                    if preserved {
                        b"original".as_slice()
                    } else {
                        b""
                    }
                );
            }
        }
    }
}

#[test]
fn sharing_is_bidirectional_and_precedes_truncation() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = create(&mut vfs, "U:\\file");
    vfs.write(h, Offset::Current, b"keep").unwrap();
    vfs.close(h).unwrap();
    let reader = vfs
        .open(
            "U:\\file",
            OpenOptions {
                share: ShareAccess::READ,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert_eq!(
        vfs.open(
            "E:\\udata\\4D530064\\FILE",
            options(CreateDisposition::Overwrite)
        ),
        Err(Error::SharingViolation)
    );
    assert_eq!(read_all(&mut vfs, reader), b"keep");
    vfs.close(reader).unwrap();
    let writer = vfs
        .open(
            "U:\\file",
            OpenOptions {
                access: Access::WRITE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert_eq!(
        vfs.open(
            "U:\\file",
            OpenOptions {
                share: ShareAccess::READ,
                ..Default::default()
            }
        ),
        Err(Error::SharingViolation)
    );
    vfs.close(writer).unwrap();
    // Metadata-only handles don't acquire read/write share categories.
    let meta = vfs
        .open(
            "U:\\file",
            OpenOptions {
                access: Access::READ_ATTRIBUTES,
                share: ShareAccess::NONE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert!(
        vfs.open(
            "U:\\file",
            OpenOptions {
                access: Access::READ_ATTRIBUTES,
                share: ShareAccess::NONE,
                ..Default::default()
            }
        )
        .is_ok()
    );
    assert_eq!(
        vfs.read(meta, Offset::Current, &mut [0]),
        Err(Error::AccessDenied)
    );
}

#[test]
fn file_positions_eof_append_and_duplicate() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = create(&mut vfs, "T:\\file");
    vfs.write(h, Offset::Current, b"abc").unwrap();
    let duplicate = vfs.duplicate(h).unwrap();
    vfs.set_position(duplicate, 1).unwrap();
    let mut bytes = [0; 8];
    assert_eq!(vfs.read(h, Offset::Current, &mut bytes).unwrap(), 2);
    assert_eq!(&bytes[..2], b"bc");
    assert_eq!(vfs.query_information(duplicate).unwrap().position, 3);
    assert_eq!(
        vfs.read(duplicate, Offset::Current, &mut bytes),
        Err(Error::EndOfFile)
    );
    assert_eq!(vfs.read(h, Offset::Current, &mut []), Ok(0));
    vfs.close(h).unwrap();
    assert_eq!(vfs.close(h), Err(Error::InvalidHandle));
    vfs.set_end_of_file(duplicate, 5).unwrap();
    assert_eq!(read_all(&mut vfs, duplicate), b"abc\0\0");
    vfs.set_end_of_file(duplicate, 1).unwrap();
    assert_eq!(vfs.query_information(duplicate).unwrap().position, 5);
    let append = vfs
        .open(
            "T:\\file",
            OpenOptions {
                access: Access::APPEND,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    vfs.write(append, Offset::Absolute(0), b"z").unwrap();
    assert_eq!(read_all(&mut vfs, duplicate), b"az");
    assert_eq!(vfs.set_end_of_file(append, 0), Err(Error::AccessDenied));
    assert_eq!(
        vfs.write(duplicate, Offset::Absolute(u32::MAX as u64), b"x"),
        Err(Error::FileTooLarge)
    );
    assert_eq!(
        vfs.set_end_of_file(duplicate, u32::MAX as u64 + 1),
        Err(Error::FileTooLarge)
    );
    assert_eq!(
        vfs.read(duplicate, Offset::EndOfFile, &mut [0]),
        Err(Error::InvalidParameter)
    );
    vfs.close(append).unwrap();
    vfs.close(duplicate).unwrap();
}

#[test]
fn directories_relative_open_and_error_paths() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    assert_eq!(
        vfs.open("U:\\missing\\child", options(CreateDisposition::Create)),
        Err(Error::ObjectPathNotFound)
    );
    let dir = directory(&mut vfs, "U:\\Folder");
    let child = vfs
        .open_relative(dir, "Child", options(CreateDisposition::Create))
        .unwrap()
        .handle;
    assert!(vfs.query_path("u:\\folder\\CHILD").is_ok());
    assert_eq!(
        vfs.open_relative(dir, "..\\escape", options(CreateDisposition::Create)),
        Err(Error::ObjectNameInvalid)
    );
    assert_eq!(
        vfs.open_relative(child, "x", OpenOptions::default()),
        Err(Error::NotADirectory)
    );
    assert_eq!(
        vfs.open(
            "U:\\Folder",
            OpenOptions {
                directory: Some(false),
                ..Default::default()
            }
        ),
        Err(Error::FileIsADirectory)
    );
    assert_eq!(
        vfs.open(
            "U:\\Folder\\child",
            OpenOptions {
                directory: Some(true),
                ..Default::default()
            }
        ),
        Err(Error::NotADirectory)
    );
    assert_eq!(
        vfs.query_directory(child, None, false),
        Err(Error::InvalidParameter)
    );
    assert_eq!(
        vfs.read(dir, Offset::Current, &mut [0]),
        Err(Error::FileIsADirectory)
    );
    assert_eq!(
        vfs.set_delete_pending(dir, true),
        Err(Error::DirectoryNotEmpty)
    );
    vfs.set_delete_pending(child, true).unwrap();
    vfs.close(child).unwrap();
    vfs.set_delete_pending(dir, true).unwrap();
    vfs.close(dir).unwrap();
    assert_eq!(vfs.query_path("U:\\Folder"), Err(Error::ObjectNameNotFound));
}

#[test]
fn enumeration_patterns_restart_snapshots_and_buffer_retry() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    for name in ["beta.map", "Alpha.map", "noextension", "other.bin"] {
        let h = create(&mut vfs, &format!("U:\\{name}"));
        vfs.close(h).unwrap();
    }
    let dir = vfs.open("U:\\", OpenOptions::default()).unwrap().handle;
    assert_eq!(
        vfs.peek_directory(dir, Some("*.MAP"), false).unwrap().name,
        "Alpha.map"
    );
    assert_eq!(
        vfs.peek_directory(dir, None, false).unwrap().name,
        "Alpha.map"
    );
    assert_eq!(vfs.consume_directory(dir).unwrap().name, "Alpha.map");
    assert_eq!(
        vfs.query_directory(dir, Some("*.bin"), false).unwrap().name,
        "beta.map"
    );
    assert_eq!(
        vfs.query_directory(dir, None, false),
        Err(Error::NoMoreFiles)
    );
    let h = create(&mut vfs, "U:\\aardvark.map");
    vfs.close(h).unwrap();
    assert_eq!(
        vfs.query_directory(dir, None, false),
        Err(Error::NoMoreFiles)
    );
    assert_eq!(
        vfs.query_directory(dir, None, true).unwrap().name,
        "aardvark.map"
    );
    let second = vfs.open("U:\\", OpenOptions::default()).unwrap().handle;
    assert_eq!(
        vfs.query_directory(second, Some("absent*"), false),
        Err(Error::NoSuchFile)
    );
    assert_eq!(
        vfs.query_directory(second, None, false),
        Err(Error::NoMoreFiles)
    );
    assert!(wildcard_matches("*.*", "noextension"));
    assert!(wildcard_matches("a??ha.*", "ALPHA.map"));
    assert!(!wildcard_matches("a??ha", "Alpha.map"));
    assert!(wildcard_matches("*a*b*", "aaaab"));
}

#[test]
fn deletion_waits_for_final_object_and_duplicate_close() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = create(&mut vfs, "U:\\delete");
    let second = vfs
        .open("U:\\delete", OpenOptions::default())
        .unwrap()
        .handle;
    let duplicate = vfs.duplicate(h).unwrap();
    vfs.set_delete_pending(h, true).unwrap();
    assert!(vfs.query_information(second).unwrap().delete_pending);
    assert_eq!(
        vfs.open("U:\\delete", OpenOptions::default()),
        Err(Error::DeletePending)
    );
    vfs.close(h).unwrap();
    vfs.close(duplicate).unwrap();
    assert!(vfs.query_path("U:\\delete").is_ok());
    vfs.close(second).unwrap();
    assert_eq!(vfs.query_path("U:\\delete"), Err(Error::ObjectNameNotFound));
    let h = vfs
        .open(
            "U:\\auto",
            OpenOptions {
                delete_on_close: true,
                ..options(CreateDisposition::Create)
            },
        )
        .unwrap()
        .handle;
    vfs.close(h).unwrap();
    assert_eq!(vfs.query_path("U:\\auto"), Err(Error::ObjectNameNotFound));
    let h = create(&mut vfs, "U:\\cancel");
    vfs.set_delete_pending(h, true).unwrap();
    vfs.set_delete_pending(h, false).unwrap();
    vfs.close(h).unwrap();
    assert!(vfs.query_path("U:\\cancel").is_ok());
}

#[test]
fn logical_attributes_timestamps_persist_and_enforce_readonly() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = create(&mut vfs, "U:\\meta");
    vfs.write(h, Offset::Current, b"x").unwrap();
    let mut basic = vfs.query_information(h).unwrap().metadata.basic;
    basic.attributes = ATTR_READONLY | ATTR_HIDDEN | ATTR_SYSTEM;
    basic.creation_time = Timestamp(125_911_584_031_234_567);
    basic.last_write_time = Timestamp(125_911_584_051_234_567);
    vfs.set_basic_information(h, basic).unwrap();
    let saved = vfs.query_information(h).unwrap().metadata.basic;
    assert_eq!(saved.creation_time, Timestamp(125_911_584_020_000_000));
    assert_eq!(saved.last_write_time, Timestamp(125_911_584_040_000_000));
    assert_eq!(vfs.write(h, Offset::Current, b"x"), Ok(1));
    vfs.set_basic_information(h, basic).unwrap();
    vfs.close(h).unwrap();
    drop(vfs);
    let mut vfs = temp.storage();
    assert_eq!(vfs.query_path("U:\\META").unwrap().basic, saved);
    assert_eq!(
        vfs.open("U:\\meta", options(CreateDisposition::Overwrite)),
        Err(Error::AccessDenied)
    );
    let h = vfs
        .open(
            "U:\\meta",
            OpenOptions {
                access: Access::WRITE_ATTRIBUTES,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    basic.attributes = ATTR_ARCHIVE;
    vfs.set_basic_information(h, basic).unwrap();
    vfs.close(h).unwrap();
    let h = vfs
        .open("U:\\meta", options(CreateDisposition::Open))
        .unwrap()
        .handle;
    assert_eq!(vfs.write(h, Offset::Absolute(0), b"y"), Ok(1));
    assert_eq!(Timestamp(0).fatx(), Timestamp(125_911_584_000_000_000));
    assert_eq!(
        Timestamp(u64::MAX).fatx(),
        Timestamp(166_304_447_980_000_000)
    );
}

#[test]
fn volume_quota_and_sparse_growth_report_fatx_allocation() {
    let temp = TempDir::new();
    let mut backend = HostBackend::new(temp.0.join("root"), temp.0.join("state"))
        .unwrap()
        .with_volume("Tiny", 42, FATX_CLUSTER_SIZE * 2);
    backend.create("one", false, 0).unwrap();
    backend.write_at("ONE", 0, b"a").unwrap();
    assert_eq!(
        backend.stat("one").unwrap().allocation_size,
        FATX_CLUSTER_SIZE
    );
    backend.set_len("one", FATX_CLUSTER_SIZE * 2).unwrap();
    assert_eq!(backend.volume_info().unwrap().available_allocation_units, 0);
    backend.create("two", false, 0).unwrap();
    assert_eq!(backend.write_at("two", 0, b"x"), Err(Error::DiskFull));
    assert_eq!(backend.create("folder", true, 0), Err(Error::DiskFull));
    backend.set_len("one", 0).unwrap();
    assert_eq!(backend.volume_info().unwrap().available_allocation_units, 2);
}

#[test]
fn host_backend_scans_case_and_rejects_ambiguity() {
    let temp = TempDir::new();
    fs::create_dir_all(temp.0.join("root/Mixed")).unwrap();
    fs::write(temp.0.join("root/Mixed/Existing"), b"data").unwrap();
    let mut backend = HostBackend::new(temp.0.join("root"), temp.0.join("state")).unwrap();
    assert_eq!(backend.stat("mixed\\EXISTING").unwrap().end_of_file, 4);
    assert_eq!(
        backend.create("MIXED\\existing", false, 0),
        Err(Error::ObjectNameCollision)
    );
    // This detects actual host case behavior: create_new fails on a case-insensitive volume.
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp.0.join("root/Mixed/existing"))
        .is_ok()
    {
        assert_eq!(backend.stat("Mixed\\Existing"), Err(Error::FileCorrupt));
        assert_eq!(backend.read_directory("Mixed"), Err(Error::FileCorrupt));
    }
    assert!(HostBackend::new(temp.0.join("bad"), temp.0.join("bad/state")).is_err());
}

#[cfg(unix)]
#[test]
fn host_symlinks_do_not_escape_the_mount() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    fs::write(temp.0.join("outside"), b"private").unwrap();
    let title = temp.0.join("disk/Partition1/UDATA/4D530064");
    std::os::unix::fs::symlink(temp.0.join("outside"), title.join("alias")).unwrap();
    assert_eq!(vfs.query_path("U:\\alias"), Err(Error::AccessDenied));
    assert_eq!(
        vfs.open("U:\\alias", options(CreateDisposition::Overwrite)),
        Err(Error::AccessDenied)
    );
    std::os::unix::fs::symlink(&temp.0, title.join("escape")).unwrap();
    assert_eq!(
        vfs.query_path("U:\\escape\\outside"),
        Err(Error::AccessDenied)
    );
    assert_eq!(fs::read(temp.0.join("outside")).unwrap(), b"private");
}

fn dirent(right: u16, sector: u32, size: u32, directory: bool, name: &str) -> Vec<u8> {
    let mut bytes = vec![0; 14];
    bytes[2..4].copy_from_slice(&right.to_le_bytes());
    bytes[4..8].copy_from_slice(&sector.to_le_bytes());
    bytes[8..12].copy_from_slice(&size.to_le_bytes());
    bytes[12] = if directory { 0x10 } else { 0 };
    bytes[13] = name.len() as u8;
    bytes.extend_from_slice(name.as_bytes());
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes
}
fn image() -> Vec<u8> {
    let mut image = vec![0; 37 * 2048];
    image[32 * 2048..32 * 2048 + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    image[32 * 2048 + 20..32 * 2048 + 24].copy_from_slice(&33u32.to_le_bytes());
    let first_size = dirent(0, 34, 6, false, "default.xbe").len();
    let root = [
        dirent((first_size / 4) as u16, 34, 6, false, "default.xbe"),
        dirent(0, 35, 64, true, "maps"),
    ]
    .concat();
    image[32 * 2048 + 24..32 * 2048 + 28].copy_from_slice(&(root.len() as u32).to_le_bytes());
    image[33 * 2048..33 * 2048 + root.len()].copy_from_slice(&root);
    image[34 * 2048..34 * 2048 + 6].copy_from_slice(b"header");
    let child = dirent(0, 36, 3, false, &format!("{}.map", "a".repeat(43)));
    assert_eq!(child.len(), 64);
    image[35 * 2048..35 * 2048 + child.len()].copy_from_slice(&child);
    image[36 * 2048..36 * 2048 + 3].copy_from_slice(b"map");
    image
}

#[test]
fn synthetic_xiso_reads_enumerates_and_enforces_write_protection() {
    let mut vfs = Vfs::new();
    vfs.mount(
        CDROM_DEVICE,
        DiscBackend::new(Cursor::new(image())).unwrap(),
    )
    .unwrap();
    vfs.namespace
        .create_symbolic_link("D:", CDROM_DEVICE)
        .unwrap();
    let h = vfs
        .open("\\??\\d:\\DEFAULT.XBE", OpenOptions::default())
        .unwrap()
        .handle;
    let mut buf = [0; 20];
    assert_eq!(vfs.read(h, Offset::Absolute(2), &mut buf).unwrap(), 4);
    assert_eq!(&buf[..4], b"ader");
    assert_eq!(
        vfs.read(h, Offset::Current, &mut buf),
        Err(Error::EndOfFile)
    );
    assert_eq!(
        vfs.query_volume_information(h).unwrap().filesystem,
        "XDVDFS"
    );
    assert_eq!(
        vfs.open("D:\\default.xbe", options(CreateDisposition::Overwrite)),
        Err(Error::MediaWriteProtected)
    );
    assert_eq!(
        vfs.open("D:\\new", options(CreateDisposition::Create)),
        Err(Error::MediaWriteProtected)
    );
    let root = vfs.open("D:\\", OpenOptions::default()).unwrap().handle;
    assert_eq!(
        vfs.query_directory(root, None, false).unwrap().name,
        "default.xbe"
    );
    assert_eq!(vfs.query_directory(root, None, false).unwrap().name, "maps");
    assert_eq!(
        vfs.query_directory(root, None, false),
        Err(Error::NoMoreFiles)
    );
    let long = format!("D:\\maps\\{}.map", "A".repeat(43));
    let h = vfs.open(&long, OpenOptions::default()).unwrap().handle;
    assert_eq!(read_all(&mut vfs, h), b"map");
    assert_eq!(
        vfs.query_path("D:\\maps\\missing"),
        Err(Error::ObjectNameNotFound)
    );
    assert_eq!(
        vfs.query_path("D:\\absent\\missing"),
        Err(Error::ObjectPathNotFound)
    );
}

/// Ensures a one-byte read from a large file does not buffer that file via Xiso::read.
#[test]
fn disc_read_is_bounded_by_requested_range() {
    struct Tracked {
        inner: Cursor<Vec<u8>>,
        bytes: Arc<AtomicU64>,
    }
    impl Read for Tracked {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(b)?;
            self.bytes.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }
    }
    impl Seek for Tracked {
        fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(p)
        }
    }
    let bytes = Arc::new(AtomicU64::new(0));
    let mut disc = DiscBackend::new(Tracked {
        inner: Cursor::new(image()),
        bytes: bytes.clone(),
    })
    .unwrap();
    bytes.store(0, Ordering::Relaxed);
    let mut one = [0];
    assert_eq!(disc.read_at("default.xbe", 1, &mut one), Ok(1));
    assert_eq!(bytes.load(Ordering::Relaxed), 1);
    assert_eq!(one, *b"e");
}

#[test]
fn ntstatus_and_nt_flags() {
    assert_eq!(Error::SharingViolation.ntstatus(), 0xC0000043);
    assert_eq!(Error::NoMoreFiles.ntstatus(), 0x80000006);
    assert_eq!(Error::EndOfFile.ntstatus(), 0xC0000011);
    assert!(Access::from_nt(0x80000000).unwrap().contains(Access::READ));
    assert!(Access::from_nt(0x40000000).unwrap().contains(Access::WRITE));
    assert_eq!(Access::from_nt(0x02000000), Err(Error::NotSupported));
    assert_eq!(CreateDisposition::try_from(6), Err(Error::InvalidParameter));
}

#[test]
#[ignore = "requires user-owned disc/halo2.iso; does not extract any data"]
fn real_disc_lists_d_root() {
    let disc = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso"));
    let mut vfs = Vfs::new();
    vfs.mount(CDROM_DEVICE, DiscBackend::open(disc).unwrap())
        .unwrap();
    vfs.namespace
        .create_symbolic_link("D:", CDROM_DEVICE)
        .unwrap();
    let dir = vfs.open("D:\\", OpenOptions::default()).unwrap().handle;
    let mut count = 0;
    loop {
        match vfs.query_directory(dir, None, false) {
            Ok(entry) => {
                println!(
                    "{}{}",
                    entry.name,
                    if entry.metadata.is_directory() {
                        "\\"
                    } else {
                        ""
                    }
                );
                count += 1;
            }
            Err(Error::NoMoreFiles) => break,
            other => panic!("unexpected directory result: {other:?}"),
        }
    }
    assert!(count > 0);
    vfs.close(dir).unwrap();
}

#[test]
fn nt_create_options_are_decoded_without_silent_feature_drops() {
    let opts = OpenOptions::from_nt(0xC0000000, 3, 3, 0x60, ATTR_ARCHIVE).unwrap();
    assert_eq!(opts.directory, Some(false));
    assert!(opts.access.contains(Access::WRITE));
    assert_eq!(opts.disposition, CreateDisposition::OpenIf);
    assert_eq!(
        OpenOptions::from_nt(1, 7, 1, 0x20, 0).unwrap_err(),
        Error::InvalidParameter
    );
    assert_eq!(
        OpenOptions::from_nt(Access::ALL.0, 7, 1, 0x41, 0).unwrap_err(),
        Error::InvalidParameter
    );
    assert_eq!(
        OpenOptions::from_nt(Access::ALL.0, 7, 1, 0x2000, 0).unwrap_err(),
        Error::NotSupported
    );
}

#[test]
fn delete_on_close_cleans_up_at_session_end_and_cannot_remove_volume_root() {
    let temp = TempDir::new();
    {
        let mut vfs = temp.storage();
        vfs.open(
            "U:\\temporary",
            OpenOptions {
                delete_on_close: true,
                ..options(CreateDisposition::Create)
            },
        )
        .unwrap();
        let root = vfs
            .open("E:\\", options(CreateDisposition::Open))
            .unwrap()
            .handle;
        assert_eq!(vfs.set_delete_pending(root, true), Err(Error::AccessDenied));
        assert_eq!(
            vfs.open(
                "E:\\",
                OpenOptions {
                    delete_on_close: true,
                    ..options(CreateDisposition::Open)
                }
            ),
            Err(Error::AccessDenied)
        );
    }
    assert!(
        !temp
            .0
            .join("disk/Partition1/UDATA/4D530064/temporary")
            .exists()
    );
}

#[test]
fn directory_item_limit_is_enforced() {
    let temp = TempDir::new();
    let root = temp.0.join("root");
    fs::create_dir_all(&root).unwrap();
    for i in 0..FATX_MAX_DIRECTORY_ENTRIES {
        fs::write(root.join(format!("f{i}")), b"").unwrap();
    }
    let mut backend = HostBackend::new(root, temp.0.join("state")).unwrap();
    assert_eq!(backend.create("overflow", false, 0), Err(Error::DiskFull));
    backend.remove("f0").unwrap();
    backend.create("replacement", false, 0).unwrap();
}

#[cfg(unix)]
#[test]
fn host_hardlinks_cannot_bypass_sharing_or_escape_mount() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    fs::write(temp.0.join("outside"), b"private").unwrap();
    fs::hard_link(
        temp.0.join("outside"),
        temp.0.join("disk/Partition1/UDATA/4D530064/linked"),
    )
    .unwrap();
    assert_eq!(
        vfs.open("U:\\linked", options(CreateDisposition::Overwrite)),
        Err(Error::AccessDenied)
    );
    assert_eq!(fs::read(temp.0.join("outside")).unwrap(), b"private");
}

#[test]
fn destructive_directory_dispositions_fail_before_creation() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    for disposition in [CreateDisposition::OverwriteIf, CreateDisposition::Supersede] {
        assert_eq!(
            vfs.open(
                "U:\\must-not-exist",
                OpenOptions {
                    directory: Some(true),
                    ..options(disposition)
                }
            ),
            Err(Error::InvalidParameter)
        );
        assert_eq!(
            vfs.query_path("U:\\must-not-exist"),
            Err(Error::ObjectNameNotFound)
        );
    }
    assert!(OpenOptions::from_nt(Access::ALL.0, 7, 1, 0x2, 0).is_ok());
}

#[test]
fn trailing_separators_require_directories_and_readonly_delete_create_is_rejected() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = create(&mut vfs, "U:\\file");
    vfs.close(h).unwrap();
    assert_eq!(
        vfs.open("U:\\file\\", OpenOptions::default()),
        Err(Error::NotADirectory)
    );
    assert_eq!(
        vfs.open(
            "U:\\readonly",
            OpenOptions {
                delete_on_close: true,
                attributes: ATTR_READONLY,
                ..options(CreateDisposition::Create)
            }
        ),
        Err(Error::AccessDenied)
    );
    assert_eq!(
        vfs.query_path("U:\\readonly"),
        Err(Error::ObjectNameNotFound)
    );
}

#[test]
fn readonly_is_checked_at_open_and_cannot_revoke_granted_write_or_resize() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = vfs
        .open(
            "U:\\readonly",
            OpenOptions {
                attributes: ATTR_READONLY,
                ..options(CreateDisposition::Create)
            },
        )
        .unwrap()
        .handle;
    assert_eq!(vfs.write(h, Offset::Current, b"data"), Ok(4));
    vfs.set_end_of_file(h, 7).unwrap();
    assert_eq!(
        vfs.open(
            "U:\\readonly",
            OpenOptions {
                access: Access::WRITE,
                ..Default::default()
            }
        ),
        Err(Error::AccessDenied)
    );
    let mut basic = vfs.query_information(h).unwrap().metadata.basic;
    basic.attributes = 0;
    vfs.set_basic_information(h, basic).unwrap();
    let writer = vfs
        .open(
            "U:\\readonly",
            OpenOptions {
                access: Access::WRITE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    basic.attributes = ATTR_READONLY;
    vfs.set_basic_information(h, basic).unwrap();
    vfs.write(writer, Offset::Absolute(0), b"kept").unwrap();
    vfs.set_end_of_file(writer, 4).unwrap();
    vfs.close(writer).unwrap();
    assert_eq!(read_all(&mut vfs, h), b"kept");
    vfs.close(h).unwrap();
}

#[test]
fn write_through_and_unbuffered_flags_are_accepted_as_hints() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    for (i, flags) in [0x2, 0x8, 0xA, 0x6A].into_iter().enumerate() {
        let opts = OpenOptions::from_nt(0xC0000000, 7, 2, flags, 0).unwrap();
        let h = vfs.open(&format!("U:\\hint{i}"), opts).unwrap().handle;
        vfs.write(h, Offset::Absolute(1), b"unaligned").unwrap();
        assert_eq!(read_all(&mut vfs, h), b"\0unaligned");
        vfs.close(h).unwrap();
    }
}

#[test]
fn bad_metadata_and_unrepresentable_neighbors_do_not_poison_listing_or_create() {
    let temp = TempDir::new();
    let root = temp.0.join("root");
    let state = temp.0.join("state");
    let mut backend =
        HostBackend::new(&root, &state)
            .unwrap()
            .with_volume("small", 1, FATX_CLUSTER_SIZE * 3);
    backend.create("good", false, 0).unwrap();
    backend.create("bad", false, 0).unwrap();
    drop(backend);
    fs::write(state.join("626164/record"), b"").unwrap();
    let big = fs::File::create(root.join("oversized")).unwrap();
    big.set_len(u32::MAX as u64 + 1).unwrap();
    #[cfg(unix)]
    {
        fs::write(temp.0.join("outside"), b"x").unwrap();
        fs::hard_link(temp.0.join("outside"), root.join("hardlink")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("fifo"))
                .status()
                .unwrap()
                .success()
        );
    }
    let mut backend =
        HostBackend::new(&root, &state)
            .unwrap()
            .with_volume("small", 1, FATX_CLUSTER_SIZE * 3);
    assert_eq!(
        backend
            .read_directory("")
            .unwrap()
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["bad", "good"]
    );
    assert_eq!(backend.stat("bad").unwrap().basic.attributes, ATTR_ARCHIVE);
    backend.create("new", false, 0).unwrap();
    backend.write_at("new", 0, b"x").unwrap();
    assert_eq!(backend.volume_info().unwrap().available_allocation_units, 2);
    fs::write(state.join("626164/record"), b"HVF1truncated").unwrap();
    assert!(backend.stat("bad").is_ok());
    backend
        .set_basic_info("bad", BasicInfo::new(ATTR_HIDDEN))
        .unwrap();
    assert_eq!(fs::read(state.join("626164/record")).unwrap().len(), 40);
}

#[test]
fn host_litter_is_hidden_and_excluded_from_quota_and_directory_limits() {
    let temp = TempDir::new();
    let root = temp.0.join("root");
    fs::create_dir_all(&root).unwrap();
    for name in [".DS_Store", "._resource"] {
        fs::File::create(root.join(name))
            .unwrap()
            .set_len(FATX_CLUSTER_SIZE * 20)
            .unwrap();
    }
    for name in [".Spotlight-V100", ".fseventsd"] {
        fs::create_dir(root.join(name)).unwrap();
        fs::File::create(root.join(name).join("index"))
            .unwrap()
            .set_len(FATX_CLUSTER_SIZE * 20)
            .unwrap();
    }
    let mut backend = HostBackend::new(&root, temp.0.join("state"))
        .unwrap()
        .with_volume("tiny", 1, FATX_CLUSTER_SIZE);
    assert!(backend.read_directory("").unwrap().is_empty());
    assert_eq!(backend.volume_info().unwrap().available_allocation_units, 1);
    assert_eq!(backend.stat(".DS_Store"), Err(Error::ObjectNameNotFound));
    drop(backend);
    for n in 0..FATX_MAX_DIRECTORY_ENTRIES - 1 {
        fs::write(root.join(format!("f{n}")), b"").unwrap();
    }
    let mut backend = HostBackend::new(root, temp.0.join("state"))
        .unwrap()
        .with_volume("tiny", 1, FATX_CLUSTER_SIZE);
    backend.create("last", false, 0).unwrap();
    assert_eq!(backend.create("overflow", false, 0), Err(Error::DiskFull));
}

#[test]
fn delete_on_close_waits_for_requesting_object_cleanup_and_pending_is_sticky() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let first = vfs
        .open(
            "U:\\later",
            OpenOptions {
                delete_on_close: true,
                ..options(CreateDisposition::Create)
            },
        )
        .unwrap()
        .handle;
    let duplicate = vfs.duplicate(first).unwrap();
    let second = vfs
        .open("U:\\later", options(CreateDisposition::Open))
        .unwrap()
        .handle;
    assert!(!vfs.query_information(second).unwrap().delete_pending);
    vfs.close(first).unwrap();
    let third = vfs
        .open("U:\\later", options(CreateDisposition::Open))
        .unwrap()
        .handle;
    vfs.close(duplicate).unwrap();
    assert!(vfs.query_information(second).unwrap().delete_pending);
    assert_eq!(
        vfs.open("U:\\later", OpenOptions::default()),
        Err(Error::DeletePending)
    );
    vfs.set_delete_pending(second, false).unwrap(); // Cannot cancel another object's completed cleanup.
    assert!(vfs.query_information(second).unwrap().delete_pending);
    vfs.close(second).unwrap();
    assert!(vfs.query_path("U:\\later").is_ok());
    vfs.close(third).unwrap();
    assert_eq!(vfs.query_path("U:\\later"), Err(Error::ObjectNameNotFound));
}

#[test]
fn supersede_checks_implied_delete_without_granting_it_to_the_handle() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let opts = OpenOptions {
        access: Access::WRITE,
        disposition: CreateDisposition::Supersede,
        ..Default::default()
    };
    let created = vfs.open("U:\\supersede", opts).unwrap();
    assert_eq!(created.information, CreateInformation::Created);
    vfs.write(created.handle, Offset::Current, b"old").unwrap();
    vfs.close(created.handle).unwrap();
    let blocker = vfs
        .open(
            "U:\\supersede",
            OpenOptions {
                share: ShareAccess::READ | ShareAccess::WRITE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert_eq!(
        vfs.open("U:\\supersede", opts),
        Err(Error::SharingViolation)
    );
    assert_eq!(read_all(&mut vfs, blocker), b"old");
    vfs.close(blocker).unwrap();
    let implied = vfs.open("U:\\supersede", opts).unwrap();
    assert_eq!(implied.information, CreateInformation::Superseded);
    assert_eq!(
        vfs.query_information(implied.handle).unwrap().access,
        Access::WRITE
    );
    assert_eq!(
        vfs.set_delete_pending(implied.handle, true),
        Err(Error::AccessDenied)
    );
    vfs.close(implied.handle).unwrap();
    let replacement = vfs
        .open(
            "U:\\supersede",
            OpenOptions {
                access: Access::DELETE,
                ..opts
            },
        )
        .unwrap();
    assert_eq!(replacement.information, CreateInformation::Superseded);
    assert_eq!(
        vfs.query_information(replacement.handle)
            .unwrap()
            .metadata
            .end_of_file,
        0
    );
    assert_eq!(
        vfs.write(replacement.handle, Offset::Current, b"x"),
        Err(Error::AccessDenied)
    );
    vfs.close(replacement.handle).unwrap();
}

#[test]
fn original_xbox_fatx_validator_matches_the_illegal_character_bitmap() {
    // nboxkrnl FatxIsNameValid: decode the bitmap, not its incomplete punctuation comment.
    let illegal = [0xffffffffu32, 0xfc009c04, 0x10000000, 0x10000000];
    for byte in 0x20u8..0x7f {
        let name = format!("a{}b", byte as char);
        assert_eq!(
            validate_fatx_name(&name).is_err(),
            illegal[(byte >> 5) as usize] & (1 << (byte & 31)) != 0,
            "{name}"
        );
    }
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    for name in ["plus+", "comma,", "semi;", "equal="] {
        assert_eq!(
            vfs.open(&format!("U:\\{name}"), options(CreateDisposition::Create)),
            Err(Error::ObjectNameInvalid)
        );
    }
}

#[test]
fn kernel_masks_keep_literal_question_mark_and_dot_semantics() {
    // Cxbx forwards masks to native NtQueryDirectoryFile, except its *.* compatibility rewrite.
    assert!(!wildcard_matches("a*.*", "abc"));
    assert!(!wildcard_matches("ab?", "ab"));
    assert!(wildcard_matches("ab?", "abc"));
    assert!(wildcard_matches("a*.*", "abc.map"));
    assert!(wildcard_matches("*.*", "abc"));
}

#[test]
fn invalid_disc_names_do_not_prevent_valid_siblings_mounting() {
    for bad in [
        "bad:name",
        "bad?name",
        ".",
        "..",
        "bad\0name",
        "é",
        "fake/child",
    ] {
        let mut bytes = image();
        let first = dirent(0, 34, 6, false, bad);
        let root = [
            dirent((first.len() / 4) as u16, 34, 6, false, bad),
            dirent(0, 34, 6, false, "default.xbe"),
        ]
        .concat();
        bytes[32 * 2048 + 24..32 * 2048 + 28].copy_from_slice(&(root.len() as u32).to_le_bytes());
        bytes[33 * 2048..33 * 2048 + root.len()].copy_from_slice(&root);
        let mut disc = DiscBackend::new(Cursor::new(bytes)).unwrap();
        assert_eq!(
            disc.read_directory("")
                .unwrap()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["default.xbe"]
        );
        let mut buf = [0; 6];
        assert_eq!(disc.read_at("default.xbe", 0, &mut buf), Ok(6));
        assert_eq!(&buf, b"header");
    }
}

#[test]
fn intermediate_files_directory_queries_and_empty_masks_have_expected_statuses() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let file = create(&mut vfs, "U:\\file");
    assert_eq!(
        vfs.query_path("U:\\file\\child"),
        Err(Error::ObjectPathNotFound)
    );
    assert_eq!(
        vfs.query_directory(file, None, false),
        Err(Error::InvalidParameter)
    );
    let dir = vfs.open("U:\\", OpenOptions::default()).unwrap().handle;
    assert_eq!(
        vfs.query_directory(dir, Some(""), false).unwrap().name,
        "file"
    );
    let mut disc = DiscBackend::new(Cursor::new(image())).unwrap();
    assert_eq!(
        disc.stat("default.xbe\\child"),
        Err(Error::ObjectPathNotFound)
    );
    assert_eq!(
        disc.read_directory("default.xbe"),
        Err(Error::InvalidParameter)
    );
    vfs.close(file).unwrap();
    let new = create(&mut vfs, "U:\\new");
    assert_ne!(file, new);
}

#[test]
fn missing_home_is_a_configuration_error() {
    if std::env::var_os("VFS_TEST_NO_HOME").is_some() {
        assert_eq!(Config::for_title(1).unwrap_err(), Error::InvalidParameter);
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "missing_home_is_a_configuration_error"])
        .env_remove("HOME")
        .env("VFS_TEST_NO_HOME", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn litter_only_directory_is_deleted_through_guest_handles() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let h = directory(&mut vfs, "U:\\folder");
    let host = temp.0.join("disk/Partition1/UDATA/4D530064/folder");
    for name in [".DS_Store", "._fork"] {
        fs::write(host.join(name), b"finder").unwrap();
    }
    for name in [".Spotlight-V100", ".fseventsd"] {
        fs::create_dir(host.join(name)).unwrap();
        fs::write(host.join(name).join("index"), b"metadata").unwrap();
    }
    assert_eq!(vfs.query_directory(h, None, false), Err(Error::NoSuchFile));
    vfs.set_delete_pending(h, true).unwrap();
    assert_eq!(vfs.close(h), Ok(()));
    assert!(!host.exists());
    assert_eq!(vfs.query_path("U:\\folder"), Err(Error::ObjectNameNotFound));
}

#[test]
fn close_sync_is_conditional_and_sync_errors_do_not_fail_a_closed_handle() {
    struct Probe {
        flushes: Arc<AtomicU64>,
    }
    impl Backend for Probe {
        fn read_only(&self) -> bool {
            false
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            Ok(Metadata {
                basic: BasicInfo::new(if path == "folder" {
                    ATTR_DIRECTORY
                } else {
                    ATTR_ARCHIVE
                }),
                end_of_file: 1,
                allocation_size: 1,
            })
        }
        fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(vec![])
        }
        fn read_at(&mut self, _: &str, _: u64, _: &mut [u8]) -> Result<usize> {
            Ok(0)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
        fn flush(&mut self, _: &str) -> Result<()> {
            self.flushes.fetch_add(1, Ordering::Relaxed);
            Err(Error::IoDeviceError)
        }
    }
    let flushes = Arc::new(AtomicU64::new(0));
    let mut vfs = Vfs::new();
    vfs.mount(
        "\\Device\\Probe",
        Probe {
            flushes: flushes.clone(),
        },
    )
    .unwrap();
    let file = vfs
        .open("\\Device\\Probe\\file", OpenOptions::default())
        .unwrap()
        .handle;
    vfs.close(file).unwrap();
    let dir = vfs
        .open("\\Device\\Probe\\folder", options(CreateDisposition::Open))
        .unwrap()
        .handle;
    vfs.close(dir).unwrap();
    assert_eq!(flushes.load(Ordering::Relaxed), 0);
    let writer = vfs
        .open(
            "\\Device\\Probe\\file",
            OpenOptions {
                access: Access::WRITE,
                share: ShareAccess::NONE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    let duplicate = vfs.duplicate(writer).unwrap();
    vfs.close(writer).unwrap();
    assert_eq!(flushes.load(Ordering::Relaxed), 0);
    assert_eq!(vfs.flush(duplicate), Err(Error::IoDeviceError));
    assert_eq!(vfs.close(duplicate), Ok(()));
    assert_eq!(flushes.load(Ordering::Relaxed), 2);
    assert_eq!(vfs.close(duplicate), Err(Error::InvalidHandle));
    // The failed durability attempt must not leave an exclusive share reservation behind.
    let next = vfs
        .open("\\Device\\Probe\\file", OpenOptions::default())
        .unwrap()
        .handle;
    vfs.close(next).unwrap();
    assert_eq!(flushes.load(Ordering::Relaxed), 2);
}

#[test]
fn format_device_empties_one_volume_only_when_nothing_is_open() {
    let temp = TempDir::new();
    let mut fs = temp.storage();
    // X: is Partition3, Y: Partition4, Z: Partition5 (the utility volume).
    let dir = directory(&mut fs, "\\??\\Y:\\dir");
    fs.close(dir).unwrap();
    let file = create(&mut fs, "\\??\\Y:\\dir\\cache002.map");
    fs.write(file, Offset::Absolute(0), b"stale").unwrap();
    let keep = create(&mut fs, "\\??\\Z:\\cache000.map");
    fs.write(keep, Offset::Absolute(0), b"keep").unwrap();
    fs.close(keep).unwrap();

    // An open object on the volume refuses the format and changes nothing.
    assert_eq!(
        fs.format_device("\\Device\\Harddisk0\\Partition4"),
        Err(Error::SharingViolation)
    );
    assert!(fs.query_path("\\??\\Y:\\dir\\cache002.map").is_ok());
    fs.close(file).unwrap();

    fs.format_device("\\device\\harddisk0\\partition4").unwrap();
    assert_eq!(
        fs.query_path("\\??\\Y:\\dir").err(),
        Some(Error::ObjectNameNotFound)
    );
    let root = fs
        .open(
            "\\??\\Y:\\",
            OpenOptions {
                directory: Some(true),
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert_eq!(
        fs.query_directory(root, None, true).err(),
        Some(Error::NoSuchFile)
    );
    fs.close(root).unwrap();
    // The volume is usable again, the link survives and Z: is untouched.
    let fresh = create(&mut fs, "\\??\\Y:\\cache002.map");
    fs.close(fresh).unwrap();
    let keep = fs
        .open("\\??\\Z:\\cache000.map", OpenOptions::default())
        .unwrap()
        .handle;
    assert_eq!(read_all(&mut fs, keep), b"keep");
    fs.close(keep).unwrap();

    assert_eq!(
        fs.format_device("\\Device\\Harddisk0\\Partition9"),
        Err(Error::ObjectPathNotFound)
    );
    // Not a device name: only exact mounted devices are formatted.
    assert_eq!(
        fs.format_device("\\Device\\Harddisk0\\Partition4\\dir"),
        Err(Error::ObjectPathNotFound)
    );
}

#[cfg(unix)]
#[test]
fn format_device_stays_inside_the_volume_after_root_or_ancestor_swaps() {
    use std::os::unix::fs::symlink;
    for swap_ancestor in [false, true] {
        let temp = TempDir::new();
        let mut fs = Vfs::with_storage(Config::new(&temp.0, 0x4D530064)).unwrap();
        let file = create(&mut fs, "\\Device\\Harddisk0\\Partition4\\cache002.map");
        fs.close(file).unwrap();
        // Outside trees that must survive, shaped like the swapped paths.
        let outside = temp.0.join("outside");
        for sub in ["disk", "metadata"] {
            fs::create_dir_all(outside.join(sub).join("Partition4")).unwrap();
            fs::write(outside.join(sub).join("Partition4").join("victim"), b"x").unwrap();
        }
        let escape = temp.0.join("escape");
        fs::create_dir_all(&escape).unwrap();
        fs::write(escape.join("victim"), b"x").unwrap();
        // A symlink inside the volume pointing out of it.
        symlink(&escape, temp.0.join("disk").join("Partition4").join("link")).unwrap();
        let mut moved = Vec::new();
        for sub in ["disk", "metadata"] {
            if swap_ancestor {
                let from = temp.0.join(sub);
                let to = temp.0.join(format!("{sub}.moved"));
                fs::rename(&from, &to).unwrap();
                symlink(outside.join(sub), &from).unwrap();
                moved.push(to.join("Partition4"));
            } else {
                let from = temp.0.join(sub).join("Partition4");
                let to = temp.0.join(sub).join("Partition4.moved");
                fs::rename(&from, &to).unwrap();
                symlink(outside.join(sub).join("Partition4"), &from).unwrap();
                moved.push(to);
            }
        }
        fs.format_device("\\Device\\Harddisk0\\Partition4").unwrap();
        for sub in ["disk", "metadata"] {
            assert!(
                outside.join(sub).join("Partition4").join("victim").exists(),
                "{sub} (ancestor {swap_ancestor}): followed a swapped path"
            );
        }
        assert!(escape.join("victim").exists(), "followed an inner symlink");
        // The real volume (wherever it was moved) is the one emptied.
        for dir in moved {
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "{}", dir.display());
        }
    }
}
