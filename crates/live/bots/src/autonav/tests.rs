use super::*;
use crate::los::Walls;
use crate::nav::{NavMesh, Poly};

/// A synthetic cache header: the signatures, the name, the kind and a
/// checksum word.
fn header(name: &str, kind: u32, checksum: u32) -> Vec<u8> {
    let mut h = vec![0u8; MAP_HEADER_LEN];
    h[..4].copy_from_slice(b"daeh");
    h[4..8].copy_from_slice(&8u32.to_le_bytes());
    h[0x140..0x144].copy_from_slice(&kind.to_le_bytes());
    h[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
    h[0x2d0..0x2d4].copy_from_slice(&checksum.to_le_bytes());
    h[0x7fc..].copy_from_slice(b"toof");
    h
}

fn data(bsp: f32) -> MapData {
    MapData {
        places: Default::default(),
        nav: NavMesh::new(
            vec![Poly::new(vec![[bsp, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]])],
            vec![],
        ),
        walls: Walls::new(vec![[[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]]]),
    }
}

type Log = Arc<Mutex<Vec<String>>>;

struct Fake {
    key: String,
    header: Vec<u8>,
    bsps: usize,
    builds: Log,
}

impl Source for Fake {
    fn key(&self) -> &str {
        &self.key
    }
    fn header(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.header.clone())
    }
    fn build(&mut self) -> Result<Vec<MapData>, String> {
        self.builds.lock().unwrap().push(self.key.clone());
        if self.bsps == 0 {
            return Err("no collision".into());
        }
        Ok((0..self.bsps).map(|k| data(k as f32)).collect())
    }
}

/// `(key, kind, checksum)` maps, all one BSP.
fn scan_of(maps: Arc<Mutex<Vec<(String, u32, u32)>>>, builds: Log) -> Scan {
    Box::new(move || {
        maps.lock()
            .unwrap()
            .iter()
            .map(|(k, kind, sum)| {
                Box::new(Fake {
                    key: k.clone(),
                    header: header(k, *kind, *sum),
                    bsps: 1,
                    builds: Arc::clone(&builds),
                }) as Box<dyn Source>
            })
            .collect()
    })
}

fn maps(list: &[(&str, u32, u32)]) -> Arc<Mutex<Vec<(String, u32, u32)>>> {
    Arc::new(Mutex::new(
        list.iter().map(|&(k, a, b)| (k.to_owned(), a, b)).collect(),
    ))
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bots-autonav-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const MP: u32 = 1;
const CAMPAIGN: u32 = 0;

#[test]
fn the_switch_is_on_unless_set_to_off() {
    assert!(enabled(None));
    assert!(enabled(Some("")));
    assert!(enabled(Some("1")));
    assert!(enabled(Some("on")));
    for off in ["0", " 0 ", "off", "OFF", "false", "no"] {
        assert!(!enabled(Some(off)), "{off:?}");
    }
}

#[test]
fn the_header_gives_name_kind_and_a_fingerprint_of_every_byte() {
    let h = MapHeader::parse(&header("lockout", MP, 7)).unwrap();
    assert_eq!(h.name, "lockout");
    assert!(h.multiplayer);
    assert!(!MapHeader::parse(&header("01a_tutorial", CAMPAIGN, 7)).unwrap().multiplayer);
    // An edit changes the checksum, so the fingerprint.
    let other = MapHeader::parse(&header("lockout", MP, 8)).unwrap();
    assert_ne!(h.fingerprint, other.fingerprint);
    assert_eq!(h.fingerprint, MapHeader::parse(&header("lockout", MP, 7)).unwrap().fingerprint);
    assert!(MapHeader::parse(&[0u8; 16]).is_err());
    assert!(MapHeader::parse(&vec![0u8; MAP_HEADER_LEN]).is_err());
    assert!(MapHeader::parse(&header("", MP, 7)).is_err());
}

#[test]
fn staleness_missing_fresh_other_map_old_generator_old_format_damaged() {
    let dir = tmp("status");
    assert_eq!(status(&dir, "lockout", 1), Status::Missing);
    write_map(&dir, "lockout", 1, &[data(0.0)]).unwrap();
    assert_eq!(status(&dir, "lockout", 1), Status::Fresh);
    assert_eq!(status(&dir, "lockout", 2), Status::OtherMap);
    let file = dir.join(cache::file_name("lockout", 0));
    let old = Stamp {
        generator: cache::GENERATOR.wrapping_sub(1),
        fingerprint: 1,
    };
    std::fs::write(&file, data(0.0).to_bytes(old)).unwrap();
    assert_eq!(status(&dir, "lockout", 1), Status::OldGenerator(cache::GENERATOR.wrapping_sub(1)));
    // A version 1 file (no stamp).
    let mut v1 = cache::MAGIC.to_vec();
    v1.extend_from_slice(&1u32.to_le_bytes());
    v1.extend_from_slice(&[0u8; 16]);
    std::fs::write(&file, &v1).unwrap();
    assert_eq!(status(&dir, "lockout", 1), Status::OldFormat(1));
    std::fs::write(&file, b"garbage, not a bots file").unwrap();
    assert_eq!(status(&dir, "lockout", 1), Status::Damaged);
    std::fs::write(&file, &cache::MAGIC[..]).unwrap();
    assert_eq!(status(&dir, "lockout", 1), Status::Damaged);
    for s in [Status::Missing, Status::Damaged, Status::OldFormat(1), Status::OldGenerator(0), Status::OtherMap] {
        assert!(s.stale(), "{s:?}");
    }
    assert!(!Status::Fresh.stale());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect()
}

#[test]
fn the_write_is_atomic_and_leaves_no_temporary_file() {
    let dir = tmp("atomic");
    let path = dir.join("lockout.0.bots");
    std::fs::write(&path, b"old").unwrap();
    // A temporary file left by a crash is overwritten, never read.
    std::fs::write(dir.join(format!(".lockout.0.bots.{}.tmp", std::process::id())), b"half").unwrap();
    write_atomic(&path, b"new and whole").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new and whole");
    assert!(leftovers(&dir).is_empty());
    // A rename that fails (a directory in the way) leaves what was there
    // and no temporary file.
    let blocked = dir.join("blocked.0.bots");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), b"x").unwrap();
    assert!(write_atomic(&blocked, b"new").is_err());
    assert!(blocked.join("keep").is_file());
    assert!(leftovers(&dir).is_empty());
    // No directory: an error, nothing written.
    assert!(write_atomic(&dir.join("gone").join("x.bots"), b"new").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_map_is_written_bsp_zero_last() {
    let dir = tmp("order");
    let files = write_map(&dir, "deltatowers", 9, &[data(0.0), data(1.0), data(2.0)]).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["deltatowers.1.bots", "deltatowers.2.bots", "deltatowers.0.bots"]);
    for (k, f) in [(1, &files[0]), (2, &files[1]), (0, &files[2])] {
        let (d, s) = MapData::from_bytes(&std::fs::read(f).unwrap()).unwrap();
        assert_eq!(d, data(k as f32));
        assert_eq!(s, Stamp::now(9));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_queue_puts_the_wanted_map_first_and_keeps_one_per_key() {
    let builds: Log = Arc::default();
    let mut q = Queue::default();
    let mut scan = scan_of(maps(&[("a", MP, 0), ("b", MP, 0), ("c", MP, 0)]), Arc::clone(&builds));
    for s in scan() {
        q.push(s);
    }
    q.prioritise("C");
    assert_eq!(q.keys(), ["c", "a", "b"]);
    // Pushed again: replaced, at the back.
    q.push(scan().remove(0));
    assert_eq!(q.keys(), ["c", "b", "a"]);
    q.prioritise("nothing");
    assert_eq!(q.keys(), ["c", "b", "a"]);
}

#[test]
fn a_pass_builds_the_current_map_first_then_the_stale_ones_and_skips_the_rest() {
    let dir = tmp("pass");
    let builds: Log = Arc::default();
    let list = maps(&[
        ("ascension", MP, 1),
        ("lockout", MP, 1),
        ("01a_tutorial", CAMPAIGN, 1),
        ("zanzibar", MP, 1),
    ]);
    let lines: Log = Arc::default();
    let built_seen: Log = Arc::default();
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let (l, b) = (Arc::clone(&lines), Arc::clone(&built_seen));
    let sum = w
        .first(Some("zanzibar".into()))
        .log(move |s| l.lock().unwrap().push(s.to_owned()))
        .on_built(move |name, _| b.lock().unwrap().push(name.to_owned()))
        .run();
    assert_eq!(*builds.lock().unwrap(), ["zanzibar", "ascension", "lockout"]);
    assert_eq!(*built_seen.lock().unwrap(), ["zanzibar", "ascension", "lockout"]);
    assert_eq!(sum.built, ["zanzibar", "ascension", "lockout"]);
    assert_eq!((sum.fresh, sum.skipped), (0, 1));
    let lines = lines.lock().unwrap().clone();
    // One line a map and the summary.
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].starts_with("bots: built the nav mesh for zanzibar (none yet)"), "{}", lines[0]);
    assert!(lines[3].starts_with("bots: nav meshes: 3 built, 0 up to date"), "{}", lines[3]);
    assert!(!dir.join(cache::file_name("01a_tutorial", 0)).exists());

    // Again: all up to date, nothing built.
    builds.lock().unwrap().clear();
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let sum = w.log(|_| {}).run();
    assert!(builds.lock().unwrap().is_empty());
    assert_eq!((sum.built.len(), sum.fresh), (0, 3));

    // Lockout edited: only it is built again.
    list.lock().unwrap()[1].2 = 2;
    let lines: Log = Arc::default();
    let l = Arc::clone(&lines);
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let sum = w.log(move |s| l.lock().unwrap().push(s.to_owned())).run();
    assert_eq!(*builds.lock().unwrap(), ["lockout"]);
    assert_eq!(sum.fresh, 2);
    assert!(lines.lock().unwrap()[0].contains("(the map changed)"));

    // Forced (`--force`): built though fresh; multiplayer only.
    builds.lock().unwrap().clear();
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let sum = w.force(true).log(|_| {}).run();
    assert_eq!((sum.built.len(), sum.skipped), (3, 1));
    // And any kind (`h2 bots nav <map>`).
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let sum = w.force(true).any_kind(true).log(|_| {}).run();
    assert_eq!(sum.built.len(), 4);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_map_is_counted_and_the_rest_go_on() {
    let dir = tmp("fail");
    let builds: Log = Arc::default();
    let b = Arc::clone(&builds);
    let scan: Scan = Box::new(move || {
        vec![
            Box::new(Fake { key: "bad".into(), header: header("bad", MP, 0), bsps: 0, builds: Arc::clone(&b) })
                as Box<dyn Source>,
            Box::new(Fake { key: "junk".into(), header: vec![1, 2, 3], bsps: 1, builds: Arc::clone(&b) }),
            Box::new(Fake { key: "good".into(), header: header("good", MP, 0), bsps: 1, builds: Arc::clone(&b) }),
        ]
    });
    let (w, _) = Worker::new(dir.clone(), scan);
    let sum = w.log(|_| {}).run();
    assert_eq!(sum.built, ["good"]);
    assert_eq!(sum.failed, ["bad", "junk"]);
    assert_eq!(status(&dir, "bad", 0), Status::Missing);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_wanted_map_installed_after_boot_is_built_while_the_worker_waits() {
    let dir = tmp("want");
    let builds: Log = Arc::default();
    let list = maps(&[("lockout", MP, 1)]);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let (w, h) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let w = w
        .stay(true)
        .log(|_| {})
        .on_built(move |name, _| tx.send(name.to_owned()).unwrap());
    let t = std::thread::spawn(move || w.run());
    let wait = |rx: &std::sync::mpsc::Receiver<String>| rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(wait(&rx), "lockout");
    // A community map arrives; the game loads it.
    list.lock().unwrap().push(("new_map".into(), MP, 1));
    h.want("new_map");
    assert_eq!(wait(&rx), "new_map");
    // Wanted again unchanged: looked at, not built.
    h.want("lockout");
    // Lockout replaced by a package: built again when wanted.
    list.lock().unwrap()[0].2 = 5;
    h.want("lockout");
    assert_eq!(wait(&rx), "lockout");
    h.stop();
    let sum = t.join().unwrap();
    assert_eq!(sum.built, ["lockout", "new_map", "lockout"]);
    assert_eq!(*builds.lock().unwrap(), ["lockout", "new_map", "lockout"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_workers_data_wins_over_a_file_read_as_the_map_loads() {
    let mut l = Loaded::default();
    // Nothing loaded: nothing goes in.
    assert!(!l.accept("lockout", true));
    l.changed("lockout");
    assert!(!l.accept("zanzibar", false));
    assert!(!l.accept("zanzibar", true));
    // The file first, then the worker's: both go in.
    assert!(l.accept("lockout", false));
    assert!(l.accept("Lockout", true));
    // A file read late does not replace what the worker built.
    assert!(!l.accept("lockout", false));
    assert!(l.accept("lockout", true));
    // The next map starts over.
    l.changed("zanzibar");
    assert!(l.accept("zanzibar", false));
}
