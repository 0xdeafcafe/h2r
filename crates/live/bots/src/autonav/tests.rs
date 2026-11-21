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
    assert_eq!(h.kind, map::CacheType::Multiplayer);
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
    // The previously interrupted write is ignored.
    let abandoned = dir.join(".lockout.0.bots.abandoned.tmp");
    std::fs::write(&abandoned, b"half").unwrap();
    write_atomic(&path, b"new and whole").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new and whole");
    assert_eq!(std::fs::read(&abandoned).unwrap(), b"half");
    std::fs::remove_file(abandoned).unwrap();
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
fn same_process_atomic_writers_do_not_share_temporary_files() {
    let dir = tmp("writers");
    let path = dir.join("map.cache");
    let a = vec![0xa5; 8192];
    let b = vec![0x9b; 4096];
    std::thread::scope(|scope| {
        let one = scope.spawn(|| write_atomic(&path, &a));
        write_atomic(&path, &b).unwrap();
        one.join().unwrap().unwrap();
    });
    let written = std::fs::read(&path).unwrap();
    assert!(written == a || written == b);
    assert!(leftovers(&dir).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_region_digest_covers_exactly_the_pinned_region() {
    use std::io::Cursor;

    let h = header("rooms", MP, 2);
    let mut bytes = b"container header".to_vec();
    let base = bytes.len();
    bytes.extend_from_slice(&h);
    bytes.extend_from_slice(b"synthetic map");
    let end = bytes.len();
    bytes.extend_from_slice(b"outside the region");
    let expected = Sha256::of(&bytes[base..end]);
    let current = Arc::new(Mutex::new(bytes));
    let opened = Arc::clone(&current);
    let mut region = Region::new(
        "rooms",
        base as u64,
        (end - base) as u64,
        Box::new(move || Ok(Box::new(Cursor::new(opened.lock().unwrap().clone())))),
    );

    assert_eq!(region.header().unwrap(), h);
    current.lock().unwrap()[end - 1] ^= 1;
    assert_eq!(region.cache_digest(), Some(expected));
    assert_eq!(region.header().unwrap(), h, "the changed tail has the same header");
    assert_ne!(region.cache_digest(), Some(expected));
    current.lock().unwrap().truncate(end - 1);
    assert_eq!(region.header().unwrap(), h);
    assert_eq!(region.cache_digest(), None, "never cache a short region");
}

#[test]
fn shared_cache_preserves_all_bsp_bytes_and_rebuilds_on_a_miss() {
    struct CachedFake {
        source: Fake,
        digest: Sha256,
    }

    impl Source for CachedFake {
        fn key(&self) -> &str {
            self.source.key()
        }
        fn header(&mut self) -> Result<Vec<u8>, String> {
            self.source.header()
        }
        fn cache_digest(&mut self) -> Option<Sha256> {
            Some(self.digest)
        }
        fn build(&mut self) -> Result<Vec<MapData>, String> {
            self.source.build()
        }
    }

    let root = tmp("shared");
    let shared = root.join("machine-cache");
    let builds: Log = Arc::default();
    let calls: Log = Arc::default();
    let lines: Log = Arc::default();
    let run = |dir: PathBuf, digest: Sha256, force: bool| {
        let builds = Arc::clone(&builds);
        let source = move || {
            vec![Box::new(CachedFake {
                source: Fake {
                    key: "rooms".into(),
                    header: header("rooms", MP, 21),
                    bsps: 2,
                    builds: Arc::clone(&builds),
                },
                digest,
            }) as Box<dyn Source>]
        };
        let (worker, _) = Worker::new(dir, Box::new(source));
        let (calls, lines) = (Arc::clone(&calls), Arc::clone(&lines));
        let sum = worker
            .shared_cache(Some(shared.clone()))
            .force(force)
            .on_built(move |map, _| calls.lock().unwrap().push(map.into()))
            .log(move |line| lines.lock().unwrap().push(line.into()))
            .run();
        assert_eq!(sum.built, ["rooms"]);
        assert!(sum.failed.is_empty());
    };
    let bytes = |dir: &Path| {
        (0..2)
            .flat_map(|k| [cache::file_name("rooms", k), crate::vis::file_name("rooms", k)])
            .map(|name| std::fs::read(dir.join(name)).unwrap())
            .collect::<Vec<_>>()
    };

    let digest = Sha256::of(b"synthetic source");
    let cold = root.join("cold");
    run(cold.clone(), digest, false);
    let original = bytes(&cold);
    assert_eq!(builds.lock().unwrap().len(), 1);

    let warm = root.join("warm");
    run(warm.clone(), digest, false);
    assert_eq!(bytes(&warm), original);
    assert_eq!(builds.lock().unwrap().len(), 1);
    assert!(lines.lock().unwrap().iter().any(|line| line.contains("2 BSPs restored")));

    // A nav file already in the data directory can recover just its grid.
    let main_grid = warm.join(crate::vis::file_name("rooms", 0));
    std::fs::remove_file(main_grid).unwrap();
    run(warm.clone(), digest, false);
    assert_eq!(bytes(&warm), original);
    assert_eq!(builds.lock().unwrap().len(), 1);

    // A changed grid bit or header with its old checksum must be ignored.
    let grid = shared_cache::grid_path(&shared, Sha256::of(&original[0]));
    let mut corrupted = std::fs::read(&grid).unwrap();
    let last_payload_byte = corrupted.len() - 33;
    corrupted[last_payload_byte] ^= 1;
    std::fs::write(&grid, &corrupted).unwrap();
    let repaired = root.join("repaired-grid");
    run(repaired.clone(), digest, false);
    assert_eq!(bytes(&repaired), original);
    assert_eq!(builds.lock().unwrap().len(), 1);
    assert_ne!(std::fs::read(grid).unwrap(), corrupted);

    let other = Sha256::of(b"same map header, different synthetic tail");
    run(root.join("changed-input"), other, false);
    assert_eq!(builds.lock().unwrap().len(), 2);

    std::fs::write(shared_cache::nav_path(&shared, digest), b"truncated nav entry").unwrap();
    run(root.join("repaired-nav"), digest, false);
    assert_eq!(builds.lock().unwrap().len(), 3);

    let forced = root.join("forced");
    lines.lock().unwrap().clear();
    run(forced.clone(), digest, true);
    assert_eq!(bytes(&forced), original);
    assert_eq!(builds.lock().unwrap().len(), 4);
    assert!(lines.lock().unwrap().iter().all(|line| !line.contains("shared cache")));
    assert_eq!(calls.lock().unwrap().len(), 6, "only nav writes offer a new mesh");

    std::fs::remove_dir_all(&shared).unwrap();
    std::fs::write(&shared, b"the cache root is unavailable").unwrap();
    let uncached = root.join("unavailable-cache");
    run(uncached.clone(), digest, false);
    assert_eq!(bytes(&uncached), original);
    assert_eq!(builds.lock().unwrap().len(), 5);
    std::fs::remove_dir_all(root).unwrap();
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
    // Two lines a map (the nav mesh, then its grid) and the summary.
    assert_eq!(lines.len(), 7, "{lines:?}");
    assert!(lines[0].starts_with("bots: built the nav mesh for zanzibar (none yet)"), "{}", lines[0]);
    assert!(lines[1].starts_with("bots: built the visibility grid for zanzibar (none yet)"), "{}", lines[1]);
    assert!(lines[6].starts_with("bots: nav meshes: 3 built, 0 up to date"), "{}", lines[6]);
    for m in ["zanzibar", "ascension", "lockout"] {
        assert_eq!(crate::vis::status(&dir, m), crate::vis::Status::Fresh, "{m}");
    }
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
fn the_menu_and_shared_maps_are_never_used_and_multiplayer_is_built() {
    use map::CacheType as K;
    for name in ["mainmenu", "MainMenu", "shared", "single_player_shared", " shared ", ""] {
        for kind in [None, Some(K::Multiplayer), Some(K::Campaign)] {
            assert_eq!(map_use(name, kind), MapUse::Never, "{name:?} {kind:?}");
        }
    }
    // By kind, whatever the name.
    for kind in [K::MainMenu, K::Shared, K::SinglePlayerShared] {
        assert_eq!(map_use("renamed", Some(kind)), MapUse::Never, "{kind:?}");
    }
    assert_eq!(map_use("lockout", Some(K::Multiplayer)), MapUse::Build);
    // Only the name: the build reads the header before it builds.
    assert_eq!(map_use("lockout", None), MapUse::Build);
    assert_eq!(map_use("01a_tutorial", Some(K::Campaign)), MapUse::LoadOnly);
    assert_eq!(map_use("odd", Some(K::Unknown(9))), MapUse::LoadOnly);
}

#[test]
fn the_worker_never_wants_scans_or_builds_the_menu_and_shared_maps() {
    let dir = tmp("ui");
    let builds: Log = Arc::default();
    let list = maps(&[
        ("mainmenu", 2, 1),
        ("shared", 3, 1),
        ("single_player_shared", 4, 1),
        // Named like a played map, but the header says main menu.
        ("menu_copy", 2, 1),
        ("lockout", MP, 1),
    ]);
    let (w, h) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    h.want("mainmenu");
    h.want("Shared");
    assert!(h.0.wants.lock().unwrap().names.is_empty());
    h.want("lockout");
    assert_eq!(h.0.wants.lock().unwrap().names, ["lockout"]);
    // Even forced and of any kind (`h2 bots nav mainmenu --force`).
    let sum = w.force(true).any_kind(true).log(|_| {}).run();
    assert_eq!(*builds.lock().unwrap(), ["lockout"]);
    assert_eq!(sum.built, ["lockout"]);
    assert_eq!(sum.skipped, 1, "menu_copy, by its header");
    for n in ["mainmenu", "shared", "single_player_shared", "menu_copy"] {
        assert!(!dir.join(cache::file_name(n, 0)).exists(), "{n}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The grid the worker builds is saved where the game loads from
/// (`cache::dir`, as `boot::bot_glue` and `boot::bots_nav_auto` use), and
/// the game's load (`vis::load` over the `.bots` file's walls and stamp)
/// takes it; the next pass leaves the map alone.
#[test]
fn a_built_grid_is_saved_where_the_game_loads_it_and_used() {
    let root = tmp("vis-roundtrip");
    let dir = cache::dir(&root);
    let builds: Log = Arc::default();
    let list = maps(&[("lockout", MP, 1)]);
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    assert_eq!(w.log(|_| {}).run().built, ["lockout"]);
    // As the game does when the map loads.
    let bytes = std::fs::read(cache::dir(&root).join(cache::file_name("lockout", 0))).unwrap();
    let (d, stamp) = MapData::from_bytes(&bytes).unwrap();
    let line = crate::vis::load(&cache::dir(&root), "lockout", 0, stamp, &d.walls);
    assert!(line.starts_with("bots: lockout: visibility grid: "), "{line}");
    let expect = crate::vis::build(&d.nav, &d.walls);
    assert_eq!(d.walls.vis().map(|v| &**v), Some(&expect));
    assert!(!dir.join(crate::vis::part_name("lockout", 0)).exists());
    // Up to date, grid and all: nothing built.
    builds.lock().unwrap().clear();
    let (w, _) = Worker::new(dir.clone(), scan_of(Arc::clone(&list), Arc::clone(&builds)));
    let sum = w.log(|_| {}).run();
    assert!(builds.lock().unwrap().is_empty());
    assert_eq!((sum.built.len(), sum.fresh), (0, 1));
    std::fs::remove_dir_all(&root).unwrap();
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
