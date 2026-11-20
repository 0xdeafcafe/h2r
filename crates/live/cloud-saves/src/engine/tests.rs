//! The engine against a fake server, over invented save trees in
//! temporary data roots; the worker against a real VFS over one.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::local::tests::{FakeSettings, temp};
use crate::remote::fake::FakeRemote;
use crate::local::ProfileSettings;
use crate::{KIND_SETTINGS, local};

const META: &str = "0123456789AB/SaveMeta.xbx";
const PROFILE: &str = "0123456789AB/profile";
const VARIANT: &str = "BBBBBBBBBBBB/slayer";

/// `SaveMeta.xbx` naming `name`, as the game writes it.
fn meta(name: &str) -> Vec<u8> {
    vfs::save_meta::encode_save_meta(&name.encode_utf16().collect::<Vec<_>>())
}

/// One machine: its data root and engine.
struct Machine {
    data: PathBuf,
    engine: Engine<FakeRemote>,
}

impl Machine {
    fn new(name: &str, remote: &FakeRemote) -> Self {
        Self::with_settings(name, remote, None)
    }

    fn with_settings(name: &str, remote: &FakeRemote, settings: Option<Arc<dyn local::ProfileSettings>>) -> Self {
        let data = temp(name);
        let engine = Self::engine(&data, remote, settings);
        Machine { data, engine }
    }

    fn engine(
        data: &std::path::Path,
        remote: &FakeRemote,
        settings: Option<Arc<dyn local::ProfileSettings>>,
    ) -> Engine<FakeRemote> {
        Engine::new(
            Local::new(data, settings),
            remote.clone(),
            Places::new(data, "live.test", "0009000000000001"),
        )
        .unwrap()
    }

    fn write(&self, name: &str, bytes: &[u8], modified: i64) {
        self.engine.local.write(&Key::save(name), bytes, modified).unwrap();
    }

    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.engine.local.read(&Key::save(name)).unwrap()
    }

    fn remove(&self, name: &str) {
        self.engine.local.remove(&Key::save(name)).unwrap();
    }

    fn conflicts(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let root = self.engine.places().dir.join("conflicts");
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    let rel = e.path().strip_prefix(&root).unwrap().to_string_lossy().into_owned();
                    // Drop the time stamp: `<stamp>-<side>/...` -> `<side>/...`.
                    let rel = rel.split_once('-').unwrap().1.to_string();
                    out.push((rel, std::fs::read(e.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

#[test]
fn stamps_are_utc_dates() {
    assert_eq!(stamp(0), "19700101T000000Z");
    assert_eq!(stamp(1_763_294_405_000), "20251116T120005Z");
    assert_eq!(stamp(951_782_400_000), "20000229T000000Z");
}

#[test]
fn a_new_machine_gets_the_accounts_saves_before_the_game_starts() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.write(PROFILE, b"profile bytes", 1000);
    a.write(VARIANT, b"slayer variant", 1000);
    let r = a.engine.sync();
    assert_eq!((r.pushed, r.pulled, r.offline), (3, 0, false), "{r:?}");
    assert_eq!(remote.bytes(PROFILE).unwrap(), b"profile bytes");

    let mut b = Machine::new("b", &remote);
    let r = b.engine.sync();
    assert_eq!((r.pulled, r.pushed, r.conflicts), (3, 0, 0), "{r:?}");
    assert_eq!(b.read(META).unwrap(), meta("Chief"));
    assert_eq!(b.read(VARIANT).unwrap(), b"slayer variant");
    // The modification time comes along, so a later conflict compares the
    // writers' times.
    let items = b.engine.local.scan().unwrap();
    assert_eq!(items[&Key::save(META)].modified, 1000);
    // Nothing more to do on either.
    let writes = remote.writes().len();
    assert_eq!(b.engine.sync().summary(), "up to date");
    assert_eq!(a.engine.sync().summary(), "up to date");
    assert_eq!(remote.writes().len(), writes);
}

#[test]
fn changes_travel_both_ways_and_only_changed_files_upload() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.write(PROFILE, b"v1", 1000);
    a.engine.sync();
    let mut b = Machine::new("b", &remote);
    b.engine.sync();
    // B plays and saves the profile; only that file goes up.
    b.write(PROFILE, b"v2", 2000);
    let before = remote.writes().len();
    let r = b.engine.push(&[Key::save("0123456789AB")]);
    assert_eq!(r.pushed, 1, "{r:?}");
    assert_eq!(remote.writes()[before..], [(Key::save(PROFILE), 2)]);
    // A signs in again and gets it.
    let r = a.engine.sync();
    assert_eq!((r.pulled, r.conflicts), (1, 0), "{r:?}");
    assert_eq!(a.read(PROFILE).unwrap(), b"v2");
}

#[test]
fn a_conflict_keeps_the_newer_and_a_copy_of_the_other() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.write(PROFILE, b"v1", 1000);
    a.engine.sync();
    let mut b = Machine::new("b", &remote);
    b.engine.sync();
    // Both play offline from the same start; B's save is later and
    // reaches the server first.
    a.write(PROFILE, b"from a", 5000);
    b.write(PROFILE, b"from b", 6000);
    b.engine.push(&[Key::save("0123456789AB")]);
    let r = a.engine.sync();
    assert_eq!((r.pulled, r.conflicts), (1, 1), "{r:?}");
    assert!(r.lines.iter().any(|l| l.contains("CONFLICT")), "{:?}", r.lines);
    assert_eq!(a.read(PROFILE).unwrap(), b"from b");
    assert_eq!(
        a.conflicts(),
        [("this-machine/save/0123456789AB/profile".to_string(), b"from a".to_vec())]
    );
    // Now the other way: A saves later than B's next save.
    b.write(PROFILE, b"b again", 7000);
    b.engine.push(&[Key::save("0123456789AB")]);
    a.write(PROFILE, b"a again", 8000);
    let r = a.engine.sync();
    assert_eq!((r.pushed, r.conflicts), (1, 1), "{r:?}");
    assert_eq!(remote.bytes(PROFILE).unwrap(), b"a again");
    assert!(
        a.conflicts()
            .contains(&("other-machine/save/0123456789AB/profile".to_string(), b"b again".to_vec()))
    );
    // B gets A's, with nothing of its own to keep (it did not change since).
    let r = b.engine.sync();
    assert_eq!((r.pulled, r.conflicts), (1, 0), "{r:?}");
    assert_eq!(b.read(PROFILE).unwrap(), b"a again");
}

#[test]
fn deletions_travel_and_leave_a_copy() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(VARIANT, b"slayer", 1000);
    a.engine.sync();
    let mut b = Machine::new("b", &remote);
    b.engine.sync();
    a.remove(VARIANT);
    let r = a.engine.push(&[Key::save("BBBBBBBBBBBB")]);
    assert_eq!(r.deleted, 1, "{r:?}");
    assert_eq!(remote.bytes(VARIANT), None);
    let r = b.engine.sync();
    assert_eq!(r.removed, 1, "{r:?}");
    assert_eq!(b.read(VARIANT), None);
    assert!(!b.engine.local.udata.join("BBBBBBBBBBBB").exists());
    assert_eq!(
        b.conflicts(),
        [("deleted-elsewhere/save/BBBBBBBBBBBB/slayer".to_string(), b"slayer".to_vec())]
    );
}

#[test]
fn offline_changes_queue_survive_a_restart_and_upload_later() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(VARIANT, b"slayer", 1000);
    a.engine.sync();
    remote.set_offline(true);
    // Signing in with the server down: nothing lost, the change queued.
    a.write(VARIANT, b"slayer 2", 2000);
    a.write(META, &meta("Chief"), 2000);
    let r = a.engine.sync();
    assert!(r.offline);
    assert_eq!(r.queued, 2, "{r:?}");
    assert!(r.summary().contains("2 waiting"), "{}", r.summary());
    // Played on, still down.
    a.write(PROFILE, b"profile", 3000);
    let r = a.engine.push(&[Key::save("0123456789AB")]);
    assert!(r.offline);
    assert_eq!(r.pushed, 0);
    // The game quit; the queue is on disk.
    let queue = a.engine.state().queue.clone();
    assert_eq!(
        queue.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
        ["0123456789AB", "BBBBBBBBBBBB"]
    );
    let mut again = Machine::engine(&a.data, &remote, None);
    assert_eq!(again.state().queue, queue);
    // The server is back: an empty push flushes the queue.
    remote.set_offline(false);
    let r = again.push(&[]);
    assert_eq!((r.pushed, r.queued, r.offline), (3, 0, false), "{r:?}");
    assert_eq!(remote.bytes(VARIANT).unwrap(), b"slayer 2");
    assert_eq!(remote.bytes(PROFILE).unwrap(), b"profile");
    assert!(again.state().queue.is_empty());
}

#[test]
fn a_push_in_play_never_overwrites_another_machines_change() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(PROFILE, b"v1", 1000);
    a.engine.sync();
    // Another machine saved meanwhile.
    remote.seed(PROFILE, b"elsewhere", 9000);
    a.write(PROFILE, b"here", 2000);
    let r = a.engine.push(&[Key::save("0123456789AB")]);
    assert_eq!((r.pushed, r.deferred), (0, 1), "{r:?}");
    assert_eq!(remote.bytes(PROFILE).unwrap(), b"elsewhere");
    assert_eq!(a.read(PROFILE).unwrap(), b"here", "nothing written under the game");
    // The next sign-in sorts it out: the other machine's is newer.
    let r = a.engine.sync();
    assert_eq!(r.conflicts, 1, "{r:?}");
    assert_eq!(a.read(PROFILE).unwrap(), b"elsewhere");
    assert_eq!(a.conflicts()[0].1, b"here");
}

#[test]
fn an_emptied_save_meta_is_mended_from_the_cloud_and_never_uploaded() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.write(PROFILE, b"profile", 1000);
    a.engine.sync();
    let mut b = Machine::new("b", &remote);
    b.engine.sync();
    // B is killed between XCreateSaveGame's truncate and its write: the
    // file is empty and newer than anything.
    b.write(META, b"", 9000);
    let writes = remote.writes().len();
    // In play, the push refuses it.
    let r = b.engine.push(&[Key::save("0123456789AB")]);
    assert_eq!((r.pushed, r.damaged), (0, 1), "{r:?}");
    assert!(r.lines.iter().any(|l| l.contains("not uploaded")), "{:?}", r.lines);
    // At the next sign-in the cloud's good copy comes back over it.
    let r = b.engine.sync();
    assert_eq!((r.pulled, r.pushed, r.conflicts, r.deleted), (1, 0, 0, 0), "{r:?}");
    assert_eq!(b.read(META).unwrap(), meta("Chief"));
    assert_eq!(remote.bytes(META).unwrap(), meta("Chief"));
    assert_eq!(remote.writes().len(), writes, "nothing went up");
    assert!(b.conflicts().is_empty());
    assert_eq!(b.engine.sync().summary(), "up to date");
    // A torn one on a machine that never synced: the same.
    let mut c = Machine::new("c", &remote);
    c.write(META, &meta("Chief")[..7], 9000);
    let r = c.engine.sync();
    assert_eq!((r.pulled, r.pushed, r.conflicts), (2, 0, 0), "{r:?}");
    assert_eq!(c.read(META).unwrap(), meta("Chief"));
    assert_eq!(remote.writes().len(), writes);
}

#[test]
fn a_damaged_meta_in_the_cloud_is_not_written_here() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.engine.sync();
    // An older build pushed an emptied file.
    remote.seed(META, b"", 9000);
    let mut b = Machine::new("b", &remote);
    let r = b.engine.sync();
    assert_eq!((r.pulled, r.damaged), (0, 1), "{r:?}");
    assert!(
        r.lines.iter().any(|l| l.contains("not downloaded") && l.contains("damaged")),
        "{:?}",
        r.lines
    );
    assert_eq!(b.read(META), None);
    // A machine with the good copy puts it back.
    let r = a.engine.sync();
    assert_eq!((r.pulled, r.pushed, r.damaged), (0, 1, 1), "{r:?}");
    assert_eq!(a.read(META).unwrap(), meta("Chief"));
    assert_eq!(remote.bytes(META).unwrap(), meta("Chief"));
    let r = b.engine.sync();
    assert_eq!(r.pulled, 1, "{r:?}");
    assert_eq!(b.read(META).unwrap(), meta("Chief"));
}

#[test]
fn a_good_save_meta_edit_is_still_uploaded() {
    let remote = FakeRemote::default();
    let mut a = Machine::new("a", &remote);
    a.write(META, &meta("Chief"), 1000);
    a.engine.sync();
    let mut no_copy = meta("Chief");
    no_copy.extend("NoCopy=1\r\n".encode_utf16().flat_map(u16::to_le_bytes));
    a.write(META, &no_copy, 2000);
    let r = a.engine.push(&[Key::save("0123456789AB")]);
    assert_eq!((r.pushed, r.damaged), (1, 0), "{r:?}");
    assert_eq!(remote.bytes(META).unwrap(), no_copy);
    let r = a.engine.sync();
    assert_eq!(r.summary(), "up to date", "{r:?}");
}

#[test]
fn an_older_server_leaves_saves_alone() {
    struct Old;
    impl Remote for Old {
        fn list(&self) -> crate::remote::RemoteResult<Vec<StorageItem>> {
            Err(RemoteError::Unsupported)
        }
        fn get(&self, _: &Key) -> crate::remote::RemoteResult<Vec<u8>> {
            unreachable!()
        }
        fn put(&self, _: &Key, _: &[u8], _: u64, _: i64) -> crate::remote::RemoteResult<StorageItem> {
            unreachable!()
        }
        fn delete(&self, _: &Key, _: u64) -> crate::remote::RemoteResult<StorageItem> {
            unreachable!()
        }
    }
    let data = temp("old");
    let mut e = Engine::new(Local::new(&data, None), Old, Places::new(&data, "s", "x")).unwrap();
    let r = e.sync();
    assert!(r.unsupported);
    assert!(r.summary().contains("no cloud saves"));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn profile_settings_sync_through_the_store() {
    let remote = FakeRemote::default();
    let sa = Arc::new(FakeSettings::default());
    sa.values.lock().unwrap().insert(
        "Chief".into(),
        BTreeMap::from([("skulls.collected".to_string(), serde_json::json!(5))]),
    );
    let mut a = Machine::with_settings("a", &remote, Some(sa.clone()));
    assert_eq!(a.engine.sync().pushed, 1);
    let sb = Arc::new(FakeSettings::default());
    let mut b = Machine::with_settings("b", &remote, Some(sb.clone()));
    assert_eq!(b.engine.sync().pulled, 1);
    assert_eq!(sb.snapshot()["Chief"]["skulls.collected"], serde_json::json!(5));
    // A change in play is noticed and pushed.
    assert!(b.engine.settings_changed().is_empty());
    sb.values
        .lock()
        .unwrap()
        .get_mut("Chief")
        .unwrap()
        .insert("skulls.collected".into(), serde_json::json!(7));
    let changed = b.engine.settings_changed();
    assert_eq!(changed, [Key::new(KIND_SETTINGS, &local::profile_item("Chief"))]);
    assert_eq!(b.engine.push(&changed).pushed, 1);
    assert_eq!(a.engine.sync().pulled, 1);
    assert_eq!(sa.snapshot()["Chief"]["skulls.collected"], serde_json::json!(7));
}

/// U: through a real VFS, as the game writes it.
fn vfs_over(data: &std::path::Path) -> vfs::Vfs {
    let config = vfs::Config::new(data, 0x4D530064);
    let mut v = vfs::Vfs::with_storage(config.clone()).unwrap();
    v.create_standard_links(&config).unwrap();
    v
}

fn write_through(v: &mut vfs::Vfs, path: &str, bytes: &[u8]) {
    let h = v
        .open(
            path,
            vfs::OpenOptions {
                access: vfs::Access::ALL,
                disposition: vfs::CreateDisposition::OverwriteIf,
                directory: Some(false),
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    v.write(h, vfs::Offset::Absolute(0), bytes).unwrap();
    v.close(h).unwrap();
}

fn make_dir(v: &mut vfs::Vfs, path: &str) {
    let h = v
        .open(
            path,
            vfs::OpenOptions {
                access: vfs::Access::ALL,
                disposition: vfs::CreateDisposition::OpenIf,
                directory: Some(true),
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    v.close(h).unwrap();
}

fn fast() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(10),
        quiet: Duration::from_millis(200),
        max_delay: Duration::from_secs(5),
        retry: Duration::from_secs(60),
        retry_max: Duration::from_secs(60),
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_worker_pushes_what_the_game_writes_through_the_vfs() {
    let remote = FakeRemote::default();
    let data = temp("worker");
    let mut engine = Machine::engine(&data, &remote, None);
    engine.sync();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&lines);
    let worker = Worker::spawn(engine, fast(), Box::new(move |l| log.lock().unwrap().push(l.into()))).unwrap();
    let mut v = vfs_over(&data);
    v.set_change_observer(Some(worker.observer()));
    // XCreateSaveGame, then the payload in two writes, in the guest's case.
    make_dir(&mut v, "U:\\0123456789AB");
    write_through(&mut v, "U:\\0123456789AB\\SaveMeta.xbx", &meta("Chief"));
    write_through(&mut v, "U:\\0123456789ab\\profile", b"part one");
    write_through(&mut v, "U:\\0123456789AB\\profile", b"part two");
    // Cache partitions and T: are not saves.
    write_through(&mut v, "Z:\\cache.bin", b"cache");
    write_through(&mut v, "T:\\title.bin", b"title");
    wait_for("the save to upload", || remote.bytes(PROFILE).is_some() && remote.bytes(META).is_some());
    assert_eq!(remote.bytes(PROFILE).unwrap(), b"part two");
    // Debounced: the profile went up once, not once per write.
    let profile_writes = remote.writes().iter().filter(|(k, _)| k.name == PROFILE).count();
    assert_eq!(profile_writes, 1);
    assert_eq!(remote.writes().len(), 2, "{:?}", remote.writes());
    // A deletion in the game deletes in the cloud.
    let h = v
        .open(
            "U:\\0123456789AB\\profile",
            vfs::OpenOptions {
                access: vfs::Access::DELETE,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    v.set_delete_pending(h, true).unwrap();
    v.close(h).unwrap();
    wait_for("the deletion", || remote.bytes(PROFILE).is_none());
    // Offline: a save made now is queued, and stopping keeps the queue.
    remote.set_offline(true);
    make_dir(&mut v, "U:\\BBBBBBBBBBBB");
    write_through(&mut v, "U:\\BBBBBBBBBBBB\\slayer", b"variant");
    v.set_change_observer(None);
    let engine = worker.stop().unwrap();
    assert_eq!(
        engine.state().queue.iter().cloned().collect::<Vec<_>>(),
        [Key::save("BBBBBBBBBBBB")]
    );
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("cannot be reached")));
    drop(v);
    let _ = std::fs::remove_dir_all(&data);
}
