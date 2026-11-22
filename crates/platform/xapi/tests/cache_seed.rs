//! N:'s seed after its format: a sim batch's warm cache slots
//! (docs/runtime/cache-slots.md, "Sim batches: a warm N:").
//!
//! A "producer" VFS writes slot files on Partition4 as the title's copier
//! would; their files and FATX records are kept the way the batch's template
//! keeps them (`disk/`, `metadata/<name in hex>/record`). A fresh data root
//! whose VFS names that directory in `vfs::Config::cache_seed` then mounts
//! N: as XAPI does: the format empties it, the seed fills it. Synthetic
//! bytes, no game data.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use vfs::{Access, CreateDisposition, Metadata, Offset, OpenOptions};
use xapi::{Storage, VfsStorage};

const TITLE_ID: u32 = 0x4d53_0064;
const N: &str = "\\Device\\Harddisk0\\Partition4";

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-xapi-cache-seed-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn vfs_over(data: &Path, seed: Vec<PathBuf>) -> Arc<Mutex<vfs::Vfs>> {
    let mut config = vfs::Config::new(data, TITLE_ID);
    config.cache_seed = seed;
    Arc::new(Mutex::new(vfs::Vfs::with_storage(config).unwrap()))
}

fn write(vfs: &Mutex<vfs::Vfs>, name: &str, bytes: &[u8]) {
    let mut v = vfs.lock().unwrap();
    let options = OpenOptions {
        access: Access::ALL,
        disposition: CreateDisposition::OpenIf,
        directory: Some(false),
        ..OpenOptions::default()
    };
    let h = v.open(name, options).unwrap().handle;
    v.write(h, Offset::Absolute(0), bytes).unwrap();
    v.close(h).unwrap();
}

fn read(vfs: &Mutex<vfs::Vfs>, name: &str) -> Option<Vec<u8>> {
    let mut v = vfs.lock().unwrap();
    let options = OpenOptions {
        access: Access::READ,
        directory: Some(false),
        ..OpenOptions::default()
    };
    let h = v.open(name, options).ok()?.handle;
    let mut out = vec![0; 1 << 16];
    let n = v.read(h, Offset::Absolute(0), &mut out).unwrap();
    v.close(h).unwrap();
    out.truncate(n);
    Some(out)
}

fn stat(vfs: &Mutex<vfs::Vfs>, name: &str) -> Option<Metadata> {
    vfs.lock().unwrap().query_path(name).ok()
}

fn record_dir(name: &str) -> String {
    name.bytes()
        .map(|b| format!("{:02x}", b.to_ascii_lowercase()))
        .collect()
}

/// Keeps `names` from a data root's Partition4 as the batch's template
/// does: the files under `disk/`, their records under `metadata/`.
fn keep(data: &Path, names: &[&str], into: &Path) {
    for name in names {
        fs::create_dir_all(into.join("disk")).unwrap();
        fs::copy(
            data.join("disk/Partition4").join(name),
            into.join("disk").join(name),
        )
        .unwrap();
        let record = data
            .join("metadata/Partition4")
            .join(record_dir(name))
            .join("record");
        let to = into.join("metadata").join(record_dir(name));
        fs::create_dir_all(&to).unwrap();
        fs::copy(record, to.join("record")).unwrap();
    }
}

fn slot(fill: u8) -> Vec<u8> {
    let mut b = vec![fill; 0x800 + 4096];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    b
}

/// A producer run's N: (slots 2 and 3 with their records), kept as two
/// template parts. Returns the parts and the slots' metadata as the
/// producer saw it.
fn template(t: &TempDir) -> (Vec<PathBuf>, Metadata, Metadata) {
    let producer = t.0.join("producer");
    let vfs = vfs_over(&producer, Vec::new());
    write(&vfs, &format!("{N}\\cache002.map"), &slot(2));
    write(&vfs, &format!("{N}\\cache003.map"), &slot(3));
    vfs::flush_pending_metadata();
    let menu = stat(&vfs, &format!("{N}\\cache002.map")).unwrap();
    let map = stat(&vfs, &format!("{N}\\cache003.map")).unwrap();
    drop(vfs);
    let parts = vec![
        t.0.join("tpl/cache/mainmenu"),
        t.0.join("tpl/cache/lockout"),
    ];
    keep(&producer, &["cache002.map"], &parts[0]);
    keep(&producer, &["cache003.map"], &parts[1]);
    (parts, menu, map)
}

#[test]
fn a_seeded_n_holds_the_slots_and_records_after_its_format() {
    let t = TempDir::new();
    let (parts, menu, map) = template(&t);
    let data = t.0.join("run");
    let vfs = vfs_over(&data, parts.clone());
    // Left over from before the boot: the format removes it.
    write(&vfs, &format!("{N}\\cache004.map"), b"stale campaign");
    let mut storage = VfsStorage::new(Arc::clone(&vfs));
    storage.mount_cache(4).unwrap();

    assert_eq!(read(&vfs, "\\??\\N:\\cache002.map"), Some(slot(2)));
    assert_eq!(read(&vfs, "\\??\\N:\\cache003.map"), Some(slot(3)));
    assert_eq!(read(&vfs, "\\??\\N:\\cache004.map"), None);
    // The records came along: the files' times are the producer's.
    for (name, want) in [("cache002.map", &menu), ("cache003.map", &map)] {
        let got = stat(&vfs, &format!("\\??\\N:\\{name}")).unwrap();
        assert_eq!(got.basic.attributes, want.basic.attributes, "{name}");
        assert_eq!(got.basic.creation_time, want.basic.creation_time, "{name}");
        assert_eq!(
            got.basic.last_write_time, want.basic.last_write_time,
            "{name}"
        );
        assert_eq!(got.end_of_file, want.end_of_file, "{name}");
        assert!(
            data.join("metadata/Partition4")
                .join(record_dir(name))
                .join("record")
                .is_file()
        );
    }
    // The template is untouched by the run's writes.
    write(&vfs, "\\??\\N:\\cache003.map", &slot(9));
    assert_eq!(
        fs::read(parts[1].join("disk/cache003.map")).unwrap(),
        slot(3)
    );
    // Each later format seeds again from the template.
    storage.mount_cache(4).unwrap();
    assert_eq!(read(&vfs, "\\??\\N:\\cache003.map"), Some(slot(3)));
}

#[test]
fn an_unseeded_n_is_empty_after_its_format() {
    let t = TempDir::new();
    let data = t.0.join("run");
    let vfs = vfs_over(&data, Vec::new());
    write(&vfs, &format!("{N}\\cache002.map"), &slot(2));
    let mut storage = VfsStorage::new(Arc::clone(&vfs));
    storage.mount_cache(4).unwrap();
    assert_eq!(read(&vfs, "\\??\\N:\\cache002.map"), None);
    assert!(
        fs::read_dir(data.join("disk/Partition4"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(vfs.lock().unwrap().seed_device(N), Ok(0));
}

#[test]
fn only_n_is_seeded_and_never_over_an_open_file() {
    let t = TempDir::new();
    let (parts, _, _) = template(&t);
    let data = t.0.join("run");
    let vfs = vfs_over(&data, parts);
    let mut storage = VfsStorage::new(Arc::clone(&vfs));
    // Z:'s mount with a format seeds nothing.
    storage.mount_utility(5, true).unwrap();
    assert_eq!(read(&vfs, "\\??\\Z:\\cache002.map"), None);
    storage.mount_cache(4).unwrap();
    // A file open on the volume refuses the seed, as it refuses a format.
    let open = {
        let mut v = vfs.lock().unwrap();
        let options = OpenOptions {
            access: Access::READ,
            directory: Some(false),
            ..OpenOptions::default()
        };
        v.open("\\??\\N:\\cache002.map", options).unwrap().handle
    };
    assert_eq!(
        vfs.lock().unwrap().seed_device(N),
        Err(vfs::Error::SharingViolation)
    );
    vfs.lock().unwrap().close(open).unwrap();
}

#[test]
fn a_seed_that_fails_leaves_n_empty() {
    let t = TempDir::new();
    let (mut parts, _, _) = template(&t);
    // Two seeds with the same file collide: the second one fails.
    parts.push(parts[0].clone());
    let data = t.0.join("run");
    let vfs = vfs_over(&data, parts);
    let mut storage = VfsStorage::new(Arc::clone(&vfs));
    // The mount itself succeeds: the title finds an empty N: and copies.
    storage.mount_cache(4).unwrap();
    assert_eq!(read(&vfs, "\\??\\N:\\cache002.map"), None);
    assert_eq!(read(&vfs, "\\??\\N:\\cache003.map"), None);
}
