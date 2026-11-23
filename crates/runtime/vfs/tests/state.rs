//! Open files in a machine snapshot (`Vfs::state`, `Vfs::restore_state`):
//! a file open for writing is carried whole and written back at load
//! (docs/runtime/snapshots.md, "vfs and storage").
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
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
fn a_file_too_large_to_carry_or_open_for_deletion_refuses_the_save() {
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
    match vfs.state() {
        Err(StateError::TooLarge { path, bytes }) => {
            assert!(path.ends_with("cache000.map"), "{path}");
            assert_eq!(bytes, MAX_CARRIED_BYTES + 1);
        }
        other => panic!("{other:?}"),
    }
    vfs.set_end_of_file(slot, MAX_CARRIED_BYTES).unwrap();
    assert!(vfs.state().is_ok(), "at the limit it is carried");
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
