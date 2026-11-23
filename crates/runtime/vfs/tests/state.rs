//! Open files in a machine snapshot (`Vfs::state`, `Vfs::restore_state`):
//! a file open for writing is carried whole and written back at load
//! (docs/runtime/snapshots.md, "vfs and storage"). A larger one (a map
//! cache slot) is carried by reference and rewound through the journal.
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
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
            "h2-vfs-state-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn storage(&self) -> Vfs {
        let config = Config::new(&self.0, 0x4D530064);
        let mut vfs = Vfs::with_storage(config.clone()).unwrap();
        vfs.create_standard_links(&config).unwrap();
        vfs
    }
    /// The host file behind `z:\<name>`.
    fn utility(&self, name: &str) -> PathBuf {
        self.0.join("disk").join("Partition5").join(name)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const PREFERENCES: &str = "z:\\preferences.dat";
/// The title's own size for it.
const LEN: usize = 0x1fc;

/// The title's open of its preferences (`NtCreateFile` at boot):
/// GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE | FILE_READ_ATTRIBUTES,
/// FILE_OPEN_IF, kept open while it runs.
fn open_preferences(vfs: &mut Vfs) -> Handle {
    vfs.open(
        PREFERENCES,
        OpenOptions {
            access: Access(0xC010_0080),
            share: ShareAccess::READ,
            disposition: CreateDisposition::OpenIf,
            ..Default::default()
        },
    )
    .unwrap()
    .handle
}

fn fill(byte: u8) -> Vec<u8> {
    (0..LEN).map(|i| byte ^ i as u8).collect()
}

fn read_at(vfs: &mut Vfs, handle: Handle, at: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    let n = vfs.read(handle, Offset::Absolute(at), &mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn a_file_open_for_writing_is_carried_and_written_back() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let prefs = open_preferences(&mut vfs);
    let saved = fill(0x5a);
    assert_eq!(vfs.write(prefs, Offset::Absolute(0), &saved).unwrap(), LEN);
    vfs.set_position(prefs, 0x40).unwrap();

    let state = vfs.state().unwrap();
    let file = state
        .objects
        .iter()
        .find(|f| f.path == "preferences.dat")
        .expect("the open preferences are saved");
    assert_eq!(file.contents.as_deref(), Some(&saved[..]));
    assert_eq!(file.position, 0x40);
    assert!(file.changed);

    // The run goes on: the title saves new preferences, longer, through the
    // same handle.
    let later = [fill(0xa5), vec![7; 16]].concat();
    vfs.write(prefs, Offset::Absolute(0), &later).unwrap();
    vfs.flush(prefs).unwrap();
    assert_eq!(fs::read(temp.utility("preferences.dat")).unwrap(), later);

    vfs.restore_state(&state).unwrap();
    vfs.flush(prefs).unwrap();
    // The host file holds the saved bytes again, at the saved length.
    assert_eq!(fs::read(temp.utility("preferences.dat")).unwrap(), saved);
    // The same handle number reads them, from the saved position.
    let info = vfs.query_information(prefs).unwrap();
    assert_eq!(info.position, 0x40);
    assert_eq!(info.metadata.end_of_file, LEN as u64);
    let mut rest = vec![0; LEN];
    let n = vfs.read(prefs, Offset::Current, &mut rest).unwrap();
    assert_eq!(&rest[..n], &saved[0x40..]);
    // And writes through it as the saved run would.
    let next = fill(0x33);
    assert_eq!(vfs.write(prefs, Offset::Absolute(0), &next).unwrap(), LEN);
    assert_eq!(read_at(&mut vfs, prefs, 0, LEN), next);
    vfs.flush(prefs).unwrap();
    assert_eq!(fs::read(temp.utility("preferences.dat")).unwrap(), next);
    vfs.close(prefs).unwrap();
}

#[test]
fn a_restore_puts_back_the_same_state_twice() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let prefs = open_preferences(&mut vfs);
    vfs.write(prefs, Offset::Absolute(0), &fill(1)).unwrap();
    let state = vfs.state().unwrap();
    for byte in [2, 3] {
        vfs.write(prefs, Offset::Absolute(8), &fill(byte)).unwrap();
        vfs.restore_state(&state).unwrap();
        assert_eq!(vfs.state().unwrap(), state);
        assert_eq!(read_at(&mut vfs, prefs, 0, LEN + 8), fill(1));
    }
}

#[test]
fn a_fresh_storage_gets_the_carried_file() {
    let source = TempDir::new();
    let mut vfs = source.storage();
    let prefs = open_preferences(&mut vfs);
    vfs.write(prefs, Offset::Absolute(0), &fill(9)).unwrap();
    let state = vfs.state().unwrap();

    // Another data root with no preferences file at all.
    let target = TempDir::new();
    let mut fresh = target.storage();
    assert!(!target.utility("preferences.dat").exists());
    fresh.restore_state(&state).unwrap();
    fresh.flush(prefs).unwrap();
    assert_eq!(
        fs::read(target.utility("preferences.dat")).unwrap(),
        fill(9)
    );
    assert_eq!(read_at(&mut fresh, prefs, 0, LEN), fill(9));
    fresh.write(prefs, Offset::Absolute(0), &fill(4)).unwrap();
    fresh.flush(prefs).unwrap();
    assert_eq!(
        fs::read(target.utility("preferences.dat")).unwrap(),
        fill(4)
    );
}

#[test]
fn a_read_only_open_carries_no_contents() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let prefs = open_preferences(&mut vfs);
    vfs.write(prefs, Offset::Absolute(0), &fill(6)).unwrap();
    vfs.close(prefs).unwrap();
    let handle = vfs
        .open(
            PREFERENCES,
            OpenOptions {
                access: Access::READ,
                share: ShareAccess::READ,
                disposition: CreateDisposition::Open,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    let state = vfs.state().unwrap();
    assert_eq!(state.objects.len(), 1);
    assert_eq!(state.objects[0].contents, None);
    assert!(!state.objects[0].changed);
    vfs.restore_state(&state).unwrap();
    assert_eq!(read_at(&mut vfs, handle, 0, LEN), fill(6));
}

#[test]
fn a_file_past_the_carry_limit_is_referenced_and_one_open_for_deletion_refuses_the_save() {
    let temp = TempDir::new();
    let mut vfs = temp.storage();
    let slot = vfs
        .open(
            "z:\\cache000.map",
            OpenOptions {
                access: Access(0xC000_0000),
                disposition: CreateDisposition::Create,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    vfs.set_end_of_file(slot, MAX_CARRIED_BYTES + 1).unwrap();
    let state = vfs.state().unwrap();
    assert_eq!(state.objects[0].contents, None);
    let reference = state.objects[0].reference.expect("carried by reference");
    assert_eq!(reference.length, MAX_CARRIED_BYTES + 1);
    assert_eq!(reference.instance, vfs.journal_instance());
    vfs.write(slot, Offset::Absolute(3), b"map").unwrap();
    vfs.restore_state(&state).unwrap();
    vfs.flush(slot).unwrap();
    assert_eq!(
        fs::read(temp.utility("cache000.map")).unwrap(),
        vec![0; MAX_CARRIED_BYTES as usize + 1]
    );

    vfs.set_end_of_file(slot, MAX_CARRIED_BYTES).unwrap();
    let state = vfs.state().unwrap();
    assert_eq!(
        state.objects[0].contents.as_ref().map(Vec::len),
        Some(MAX_CARRIED_BYTES as usize),
        "at the limit it is carried"
    );
    assert_eq!(state.objects[0].reference, None);
    vfs.close(slot).unwrap();

    let doomed = vfs
        .open(
            "z:\\doomed.bin",
            OpenOptions {
                access: Access::ALL,
                disposition: CreateDisposition::Create,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    assert!(matches!(vfs.state(), Err(StateError::Deletable(_))));
    vfs.close(doomed).unwrap();
}

// Large files open for writing: the map cache slot the game keeps open for
// writing while it plays. An in-memory volume counts what the VFS reads and
// writes, so a capture and a restore can be shown to copy nothing.

#[derive(Default)]
struct Disk {
    files: HashMap<String, Vec<u8>>,
    /// Bytes read through `read_at`.
    read: u64,
    /// Calls that changed a file.
    writes: u64,
}

#[derive(Clone, Default)]
struct Mem(Arc<Mutex<Disk>>);

impl Mem {
    fn bytes(&self, name: &str) -> Vec<u8> {
        self.0.lock().unwrap().files[name].clone()
    }
    /// Bytes read and changes made since the last call.
    fn take_io(&self) -> (u64, u64) {
        let mut disk = self.0.lock().unwrap();
        (
            std::mem::take(&mut disk.read),
            std::mem::take(&mut disk.writes),
        )
    }
}

impl Backend for Mem {
    fn read_only(&self) -> bool {
        false
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let disk = self.0.lock().unwrap();
        let (attributes, len) = if path.is_empty() {
            (ATTR_DIRECTORY, 0)
        } else {
            let file = disk
                .files
                .get(&path.to_ascii_lowercase())
                .ok_or(Error::ObjectNameNotFound)?;
            (ATTR_ARCHIVE, file.len() as u64)
        };
        Ok(Metadata {
            basic: BasicInfo::new(attributes),
            end_of_file: len,
            allocation_size: len,
        })
    }
    fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
        Ok(Vec::new())
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let mut disk = self.0.lock().unwrap();
        let file = disk
            .files
            .get(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?;
        let from = (offset as usize).min(file.len());
        let n = buffer.len().min(file.len() - from);
        buffer[..n].copy_from_slice(&file[from..from + n]);
        disk.read += n as u64;
        Ok(n)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        Err(Error::NotSupported)
    }
    fn create(&mut self, path: &str, directory: bool, _: u32) -> Result<()> {
        if directory {
            return Err(Error::NotSupported);
        }
        let mut disk = self.0.lock().unwrap();
        disk.writes += 1;
        disk.files.insert(path.to_ascii_lowercase(), Vec::new());
        Ok(())
    }
    fn write_at(&mut self, path: &str, offset: u64, buffer: &[u8]) -> Result<usize> {
        let mut disk = self.0.lock().unwrap();
        disk.writes += 1;
        let file = disk
            .files
            .get_mut(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?;
        let end = offset as usize + buffer.len();
        if file.len() < end {
            file.resize(end, 0);
        }
        file[offset as usize..end].copy_from_slice(buffer);
        Ok(buffer.len())
    }
    fn set_len(&mut self, path: &str, size: u64) -> Result<()> {
        let mut disk = self.0.lock().unwrap();
        disk.writes += 1;
        disk.files
            .get_mut(&path.to_ascii_lowercase())
            .ok_or(Error::ObjectNameNotFound)?
            .resize(size as usize, 0);
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<()> {
        let mut disk = self.0.lock().unwrap();
        disk.writes += 1;
        disk.files
            .remove(&path.to_ascii_lowercase())
            .map(drop)
            .ok_or(Error::ObjectNameNotFound)
    }
}

const SLOT_DEVICE: &str = "\\Device\\Harddisk0\\Partition5";
const SLOT: &str = "\\Device\\Harddisk0\\Partition5\\cache000.map";
const SLOT_NAME: &str = "cache000.map";
/// Four times what a snapshot carries the bytes of.
const SLOT_LEN: usize = 4 << 20;

fn slot_bytes(seed: u8) -> Vec<u8> {
    (0..SLOT_LEN)
        .map(|i| seed ^ i as u8 ^ (i >> 9) as u8)
        .collect()
}

/// A VFS on an in-memory volume holding a 4 MiB cache slot, open for
/// reading and writing as the game keeps it.
fn open_slot(mem: &Mem) -> (Vfs, Handle) {
    let mut vfs = Vfs::new();
    vfs.mount(SLOT_DEVICE, mem.clone()).unwrap();
    let slot = vfs
        .open(
            SLOT,
            OpenOptions {
                access: Access(0xC000_0000),
                share: ShareAccess::READ,
                disposition: CreateDisposition::OpenIf,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    (vfs, slot)
}

fn slot_volume() -> Mem {
    let mem = Mem::default();
    mem.0
        .lock()
        .unwrap()
        .files
        .insert(SLOT_NAME.into(), slot_bytes(1));
    mem
}

#[test]
fn a_large_file_open_for_writing_is_captured_without_copying_it() {
    let mem = slot_volume();
    let (mut vfs, slot) = open_slot(&mem);
    vfs.set_position(slot, 0x800).unwrap();
    mem.take_io();

    let state = vfs.state().unwrap();
    let file = &state.objects[0];
    assert_eq!(file.contents, None, "no bytes in the snapshot");
    let reference = file.reference.expect("carried by reference");
    assert_eq!(reference.length, SLOT_LEN as u64);
    assert_eq!(file.position, 0x800);
    assert_eq!(vfs.journal_bytes(), 0, "and none in the journal");
    let (read, writes) = mem.take_io();
    assert!(
        read <= SLOT_LEN as u64,
        "one fingerprint pass at most: {read}"
    );
    assert_eq!(writes, 0);

    // The next capture of the unchanged file reads nothing at all.
    assert_eq!(vfs.state().unwrap(), state);
    assert_eq!(mem.take_io(), (0, 0));
}

#[test]
fn a_restore_after_no_write_costs_nothing() {
    let mem = slot_volume();
    let (mut vfs, slot) = open_slot(&mem);
    let state = vfs.state().unwrap();
    // Reads and seeks do not change the file.
    assert_eq!(
        read_at(&mut vfs, slot, 0x1000, 16),
        &slot_bytes(1)[0x1000..0x1010]
    );
    mem.take_io();
    vfs.restore_state(&state).unwrap();
    assert_eq!(mem.take_io(), (0, 0), "no read, no write");
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
    assert_eq!(vfs.state().unwrap(), state);
}

#[test]
fn a_write_after_the_capture_is_undone_by_the_restore() {
    let mem = slot_volume();
    let (mut vfs, slot) = open_slot(&mem);
    let state = vfs.state().unwrap();

    // The game copies part of a map in, and a write runs past the end.
    let chunk = vec![0xee; 0x10000];
    vfs.write(slot, Offset::Absolute(0x10_0000), &chunk)
        .unwrap();
    vfs.write(slot, Offset::Absolute(SLOT_LEN as u64 - 8), &[0x77; 40])
        .unwrap();
    assert_eq!(mem.bytes(SLOT_NAME).len(), SLOT_LEN + 32);
    // Only the overwritten bytes are journaled: 64 KiB and the last 8.
    assert_eq!(vfs.journal_bytes(), 0x10000 + 8);

    vfs.restore_state(&state).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1), "bytes and length");
    let info = vfs.query_information(slot).unwrap();
    assert_eq!(info.metadata.end_of_file, SLOT_LEN as u64);
    assert_eq!(info.position, 0);
    // The file is at the captured generation again: a second restore is free.
    assert_eq!(vfs.state().unwrap(), state);
    mem.take_io();
    vfs.restore_state(&state).unwrap();
    assert_eq!(mem.take_io(), (0, 0));

    // A truncation is undone as well.
    vfs.set_end_of_file(slot, 0x100).unwrap();
    vfs.restore_state(&state).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
}

#[test]
fn two_captures_restore_in_either_order() {
    let mem = slot_volume();
    let (mut vfs, slot) = open_slot(&mem);
    let first = vfs.state().unwrap();
    vfs.write(slot, Offset::Absolute(0x2000), &[0xaa; 0x3000])
        .unwrap();
    let middle = mem.bytes(SLOT_NAME);
    let second = vfs.state().unwrap();
    assert_ne!(second, first);
    vfs.write(slot, Offset::Absolute(0x4000), &[0xbb; 0x3000])
        .unwrap();
    vfs.write(slot, Offset::Absolute(SLOT_LEN as u64), &[0xcc; 0x100])
        .unwrap();

    vfs.restore_state(&second).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), middle);
    vfs.restore_state(&first).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
    // Back forward: the first restore's own changes were journaled.
    vfs.restore_state(&second).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), middle);
    assert_eq!(vfs.state().unwrap(), second);
    vfs.restore_state(&first).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
    assert_eq!(vfs.state().unwrap(), first);
}

#[test]
fn a_change_past_the_journal_limit_refuses_the_restore() {
    let mem = slot_volume();
    let (mut vfs, slot) = open_slot(&mem);
    let state = vfs.state().unwrap();
    // Copy map after map into the slot: more overwritten bytes than the
    // journal keeps.
    let copies = MAX_JOURNAL_BYTES / SLOT_LEN as u64 + 1;
    for copy in 0..copies {
        let map = slot_bytes(2 + copy as u8);
        assert_eq!(
            vfs.write(slot, Offset::Absolute(0), &map).unwrap(),
            SLOT_LEN
        );
        assert!(vfs.journal_bytes() <= MAX_JOURNAL_BYTES);
    }
    let last = slot_bytes(2 + (copies - 1) as u8);
    vfs.set_position(slot, 0x40).unwrap();
    match vfs.restore_state(&state) {
        Err(StateError::Unjournaled { path }) => {
            assert!(path.ends_with("cache000.map"), "{path}");
        }
        other => panic!("{other:?}"),
    }
    // Refused before anything changed.
    assert_eq!(mem.bytes(SLOT_NAME), last);
    assert_eq!(vfs.query_information(slot).unwrap().position, 0x40);

    // The slot holding the captured map again (the same map copied back)
    // matches the fingerprint, and the restore goes ahead.
    vfs.write(slot, Offset::Absolute(0), &slot_bytes(1))
        .unwrap();
    vfs.restore_state(&state).unwrap();
    assert_eq!(vfs.query_information(slot).unwrap().position, 0);
    assert_eq!(vfs.state().unwrap(), state);

    // From the restore on, changes are journaled again.
    vfs.write(slot, Offset::Absolute(0), &[9; 16]).unwrap();
    assert_eq!(vfs.journal_bytes(), 16);
    vfs.restore_state(&state).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
}

#[test]
fn another_vfs_checks_a_referenced_file_by_its_fingerprint() {
    let mem = slot_volume();
    let (mut vfs, _slot) = open_slot(&mem);
    let state = vfs.state().unwrap();
    drop(vfs);

    let mut other = Vfs::new();
    other.mount(SLOT_DEVICE, mem.clone()).unwrap();
    assert_ne!(
        other.journal_instance(),
        state.objects[0].reference.unwrap().instance
    );
    other.restore_state(&state).unwrap();
    assert_eq!(mem.bytes(SLOT_NAME), slot_bytes(1));
    drop(other);

    mem.0.lock().unwrap().files.get_mut(SLOT_NAME).unwrap()[5] ^= 1;
    let mut changed = Vfs::new();
    changed.mount(SLOT_DEVICE, mem.clone()).unwrap();
    assert!(matches!(
        changed.restore_state(&state),
        Err(StateError::Unjournaled { .. })
    ));
}
