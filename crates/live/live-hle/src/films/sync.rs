//! A durable queue scoped to the same server identity and account as cloud
//! saves. The recorder enqueues only files sealed during this account's game.
use super::{Archive, check_id, http_error};
use crate::LiveConfig;
use cloud_saves::{engine::WorkerConfig, state::Places};
use live_proto::{
    Xuid,
    film::{
        PART_BYTES,
        sharing::{Upload, Visibility},
    },
    lxm,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    source: PathBuf,
    upload: Option<Upload>,
    remote: Option<String>,
    next: usize,
    done: bool,
    #[serde(default)]
    pin: Option<bool>,
    #[serde(default)]
    removed: bool,
}

fn save(dir: &Path, job: &Job) -> Result<(), String> {
    let tmp = dir.join("state.tmp");
    fs::write(&tmp, serde_json::to_vec(job).map_err(|e| e.to_string())?)
        .and_then(|_| fs::rename(tmp, dir.join("state.json")))
        .map_err(|e| e.to_string())
}
fn read(dir: &Path) -> Result<Job, String> {
    let f = File::open(dir.join("state.json")).map_err(|e| e.to_string())?;
    serde_json::from_reader(f).map_err(|e| e.to_string())
}
fn lock(dir: &Path) -> Result<File, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"))
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    Ok(file)
}

trait Remote {
    fn reserve(&self, upload: &Upload) -> Result<(String, bool), String>;
    fn part(&self, id: &str, name: &str, n: usize, bytes: &[u8]) -> Result<(), String>;
    fn complete(&self, id: &str) -> Result<bool, String>;
}

impl Remote for Archive {
    fn reserve(&self, upload: &Upload) -> Result<(String, bool), String> {
        let r = self
            .request("POST", &format!("{}/films", self.api), lxm::FILMS_WRITE)
            .set("Content-Type", "application/json")
            .send_string(&serde_json::to_string(upload).map_err(|e| e.to_string())?)
            .map_err(http_error)?;
        let done = r.status() == 201;
        if !done && r.status() != 202 {
            return Err("Unexpected film reservation response".into());
        }
        #[derive(Deserialize)]
        struct Receipt {
            film: String,
        }
        let receipt: Receipt =
            serde_json::from_reader(r.into_reader().take(4096)).map_err(|e| e.to_string())?;
        check_id(&receipt.film)?;
        Ok((receipt.film, done))
    }
    fn part(&self, id: &str, name: &str, n: usize, bytes: &[u8]) -> Result<(), String> {
        check_id(id)?;
        self.request(
            "PUT",
            &format!("{}/films/{id}?object={name}&part={n}", self.api),
            lxm::FILMS_WRITE,
        )
        .send_bytes(bytes)
        .map(|_| ())
        .map_err(http_error)
    }
    fn complete(&self, id: &str) -> Result<bool, String> {
        check_id(id)?;
        let r = self
            .request(
                "POST",
                &format!("{}/films/{id}?continue=1", self.api),
                lxm::FILMS_WRITE,
            )
            .send_bytes(&[])
            .map_err(http_error)?;
        match r.status() {
            201 => Ok(true),
            202 => Ok(false),
            n => Err(format!("Unexpected film publication status {n}")),
        }
    }
}

/// The local player's machine tape contains private guest state. Its
/// library entry stays local even when a previous session queued it.
fn machine_tape(path: &Path) -> Result<bool, String> {
    use std::io::Read;
    let mut magic = [0; 8];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| e.to_string())?;
    Ok(magic == live_proto::film::range::MAGIC || &magic[..4] == b"HEAD")
}

/// Stage the upload of a recorded film: its stats container as it was
/// sealed, and the film file's shared copy, which keeps the game and its
/// ticks and names no machine or account (`film::file::share_copy`). Both
/// are copies, so the library may prune or delete the film meanwhile.
fn prepare_job(dir: &Path, job: &mut Job) -> Result<(), String> {
    if job.upload.is_none() {
        let stats = film::file::stats_of(&job.source);
        if !stats.is_file() {
            return Err("Film needs its stats container".into());
        }
        fs::copy(&stats, dir.join("stats")).map_err(|e| e.to_string())?;
        let out = File::create(dir.join("film")).map_err(|e| e.to_string())?;
        film::file::share_copy(&job.source, out)
            .and_then(|file| Ok(file.sync_all()?))
            .map_err(|e| format!("{e:#}"))?;
        job.upload = Some(Upload {
            film: film::share::object(&dir.join("film")).map_err(|e| e.to_string())?,
            stats: film::share::object(&dir.join("stats")).map_err(|e| e.to_string())?,
            visibility: Visibility::Private,
        });
        save(dir, job)?;
    }
    Ok(())
}

fn transfer(remote: &impl Remote, dir: &Path, job: &mut Job) -> Result<String, String> {
    if !job.done {
        prepare_job(dir, job)?;
        let upload = job.upload.as_ref().unwrap();
        if job.remote.is_none() {
            let (id, done) = remote.reserve(upload)?;
            job.remote = Some(id);
            job.done = done;
            save(dir, job)?;
        }
        let id = job.remote.as_deref().ok_or("missing upload reservation")?;
        if !job.done {
            let mut index = 0;
            // The stats stream first, then the film file, as a host sends
            // them (`film::upload`).
            for (name, object) in [("stats", &upload.stats), ("film", &upload.film)] {
                let mut f = File::open(dir.join(name)).map_err(|e| e.to_string())?;
                for n in 0..object.parts.len() {
                    if index >= job.next {
                        let at = n as u64 * PART_BYTES;
                        let mut bytes = vec![0; (object.size - at).min(PART_BYTES) as usize];
                        f.seek(SeekFrom::Start(at))
                            .and_then(|_| f.read_exact(&mut bytes))
                            .map_err(|e| e.to_string())?;
                        remote.part(id, name, n, &bytes)?;
                        job.next = index + 1;
                        save(dir, job)?;
                    }
                    index += 1;
                }
            }
            while !remote.complete(id)? {}
            job.done = true;
            save(dir, job)?;
        }
        for name in ["film", "stats"] {
            let _ = fs::remove_file(dir.join(name));
        }
    }
    job.remote
        .clone()
        .ok_or_else(|| "missing published film id".into())
}

fn apply_local_change(intent: &Path, dir: &Path, job: &mut Job) -> Result<(), String> {
    let path = intent.join("change.json");
    if !path.is_file() {
        return Ok(());
    }
    let _lock = lock(intent)?;
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    job.pin = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    job.removed |= job.pin.is_none();
    save(dir, job)?;
    fs::remove_file(path).map_err(|e| e.to_string())
}

struct Account {
    config: LiveConfig,
    xuid: Xuid,
    server_id: String,
    places: Places,
    last_error: std::sync::Mutex<Option<String>>,
    wake: mpsc::Sender<()>,
}

#[derive(Clone)]
pub struct Sync(Arc<Account>);

impl Sync {
    pub fn start(
        data: &Path,
        server_id: &str,
        xuid: Xuid,
        config: LiveConfig,
    ) -> Result<Self, String> {
        let places = Places::films(data, server_id, &xuid.to_string());
        let (wake, rx) = mpsc::channel();
        let this = Self(Arc::new(Account {
            config,
            xuid,
            server_id: server_id.into(),
            places,
            last_error: Default::default(),
            wake,
        }));
        let weak = Arc::downgrade(&this.0);
        background::spawn("h2-film-library", move || {
            let timing = WorkerConfig::default();
            let mut delay = timing.retry;
            loop {
                let Some(account) = weak.upgrade() else { break };
                let sync = Sync(account);
                let result = sync.flush();
                *sync.0.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                    result.as_ref().err().cloned();
                drop(sync);
                let wait = match result {
                    Ok(()) => {
                        delay = timing.retry;
                        timing.retry
                    }
                    Err(e) => {
                        eprintln!("film library: {e}");
                        let wait = delay;
                        delay = (delay * 2).min(timing.retry_max);
                        wait
                    }
                };
                if matches!(
                    rx.recv_timeout(wait),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ) {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
        Ok(this)
    }

    fn dir(&self, id: &str) -> Result<PathBuf, String> {
        check_id(id)?;
        Ok(self.0.places.dir.join("recordings").join(id))
    }

    pub fn enqueue(&self, id: &str, source: &Path) -> Result<(), String> {
        if machine_tape(source)? {
            return Err("Machine recordings can be saved and watched locally".into());
        }
        let dir = self.dir(id)?;
        if dir.join("state.json").is_file() {
            return Ok(());
        }
        let _lock = lock(&dir)?;
        if !dir.join("state.json").exists() {
            save(
                &dir,
                &Job {
                    source: source.canonicalize().map_err(|e| e.to_string())?,
                    upload: None,
                    remote: None,
                    next: 0,
                    done: false,
                    pin: None,
                    removed: false,
                },
            )?;
        }
        let _ = self.0.wake.send(());
        Ok(())
    }

    fn archive(&self) -> Result<Archive, String> {
        let remote =
            Archive::connect(&self.0.config, Some(self.0.xuid))?.ok_or("Live unavailable")?;
        if remote.server_id != self.0.server_id {
            return Err("Signed-in Live server changed".into());
        }
        Ok(remote)
    }

    pub fn share(&self, id: &str, visibility: Visibility) -> Result<String, String> {
        let dir = self.dir(id)?;
        let _lock = lock(&dir)?;
        let remote = self.archive()?;
        let mut job = read(&dir)?;
        apply_local_change(&dir.join("intent"), &dir, &mut job)?;
        if job.removed {
            return Err("This film was removed from the library".into());
        }
        let id = transfer(&remote, &dir, &mut job)?;
        remote.set_visibility(&id, visibility)?;
        Ok(id)
    }

    /// Persist the user's last edit even if the catalogue is currently offline.
    pub fn change_remote(&self, id: &str, pin: Option<bool>) -> Result<(), String> {
        check_id(id)?;
        let dir = self.0.places.dir.join("changes").join(id);
        let _guard = lock(&dir)?;
        let tmp = dir.join("change.tmp");
        fs::write(&tmp, serde_json::to_vec(&pin).map_err(|e| e.to_string())?)
            .and_then(|()| fs::rename(tmp, dir.join("change.json")))
            .map_err(|e| e.to_string())?;
        let _ = self.0.wake.send(());
        Ok(())
    }

    fn flush_remote_changes(&self) -> Result<(), String> {
        let entries = match fs::read_dir(self.0.places.dir.join("changes")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            r => r.map_err(|e| e.to_string())?,
        };
        let mut archive = None;
        let mut problem = None;
        for entry in entries {
            let dir = entry.map_err(|e| e.to_string())?.path();
            let Some(id) = dir.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            check_id(id)?;
            let path = dir.join("change.json");
            if !path.exists() {
                continue;
            }
            // Another game process may sync this account. Serialize senders
            // separately so recording a new menu action never waits for Live.
            let _sender = lock(&dir.join("sender"))?;
            let bytes = match fs::read(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                r => r.map_err(|e| e.to_string())?,
            };
            let pin: Option<bool> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if archive.is_none() {
                archive = Some(self.archive()?);
            }
            let remote = archive.as_ref().unwrap();
            let result = match pin {
                Some(value) => remote.set_saved(id, value),
                None => remote.remove(id),
            };
            if let Err(e) = result {
                problem = Some(e);
                continue;
            }
            let _guard = lock(&dir)?;
            if fs::read(&path).ok().as_ref() == Some(&bytes) {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        problem.map_or(Ok(()), Err)
    }

    fn flush(&self) -> Result<(), String> {
        let remote_changes = self.flush_remote_changes();
        let entries = match fs::read_dir(self.0.places.dir.join("recordings")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return remote_changes,
            other => other.map_err(|e| e.to_string())?,
        };
        let mut problem = None;
        let mut archive = None;
        for entry in entries {
            let dir = entry.map_err(|e| e.to_string())?.path();
            if !dir.join("state.json").is_file() {
                continue;
            }
            let job = match read(&dir) {
                Ok(job) => job,
                Err(e) => {
                    problem = Some(e);
                    continue;
                }
            };
            let pending = |j: &Job| {
                if j.removed {
                    j.remote.is_some()
                } else {
                    !j.done || j.pin.is_some()
                }
            };
            let intent = dir.join("intent");
            if !pending(&job) && !intent.join("change.json").exists() {
                continue;
            }
            let _lock = lock(&dir)?;
            let mut job = read(&dir)?;
            apply_local_change(&intent, &dir, &mut job)?;
            if !pending(&job) {
                continue;
            }
            if job.remote.is_none() && job.upload.is_none() && machine_tape(&job.source) == Ok(true) {
                // Retire the upload intent while the tape still identifies itself.
                // After normal Recent pruning, its path may no longer exist.
                job.removed = true;
                job.pin = None;
                save(&dir, &job)?;
                continue;
            }
            if !job.done
                && !job.removed
                && let Err(e) = prepare_job(&dir, &mut job)
            {
                problem = Some(e);
                continue;
            }
            if archive.is_none() {
                archive = Some(self.archive()?);
            }
            let remote = archive.as_ref().unwrap();
            let outcome = (|| {
                if job.removed {
                    remote.remove(job.remote.as_deref().unwrap())?;
                    job.remote = None;
                    save(&dir, &job)?;
                } else {
                    let id = transfer(remote, &dir, &mut job)?;
                    if let Some(pin) = job.pin {
                        remote.set_saved(&id, pin)?;
                        job.pin = None;
                        save(&dir, &job)?;
                    }
                }
                Ok(())
            })();
            if let Err(e) = outcome {
                problem = Some(e);
            }
        }
        remote_changes.and(problem.map_or(Ok(()), Err))
    }

    /// A menu change may arrive before the numbered upload has an ID on Live.
    /// Record it durably and let the worker apply it to the same reservation.
    pub fn change_local(&self, id: &str, pin: Option<bool>) -> Result<(), String> {
        let dir = self.dir(id)?;
        if !dir.join("state.json").is_file() {
            return Ok(());
        }
        let intent = dir.join("intent");
        let _lock = lock(&intent)?;
        let tmp = intent.join("change.tmp");
        fs::write(&tmp, serde_json::to_vec(&pin).map_err(|e| e.to_string())?)
            .and_then(|()| fs::rename(tmp, intent.join("change.json")))
            .map_err(|e| e.to_string())?;
        let _ = self.0.wake.send(());
        Ok(())
    }

    pub fn cached_revision(&self, game: &str) -> Option<(u64, String)> {
        check_id(game).ok()?;
        let path = self
            .0
            .places
            .dir
            .join("downloads")
            .join(format!("{game}.json"));
        serde_json::from_reader(File::open(path).ok()?).ok()
    }

    pub fn remember_download(&self, game: &str, revision: &(u64, String)) -> Result<(), String> {
        check_id(game)?;
        let dir = self.0.places.dir.join("downloads");
        let _guard = lock(&dir)?;
        let tmp = dir.join("receipt.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec(revision).map_err(|e| e.to_string())?,
        )
        .and_then(|()| fs::rename(tmp, dir.join(format!("{game}.json"))))
        .map_err(|e| e.to_string())
    }

    pub fn read_library(&self) -> Result<Vec<super::RecentFilm>, String> {
        let path = self.0.places.dir.join("library.json");
        if !path.is_file() {
            return Ok(Vec::new());
        }
        serde_json::from_reader(File::open(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }

    pub fn remember_library(&self, rows: &[super::RecentFilm]) -> Result<(), String> {
        let dir = &self.0.places.dir;
        let _guard = lock(dir)?;
        let tmp = dir.join("library.tmp");
        let mut f = File::create(&tmp).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut f, rows).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        fs::rename(tmp, dir.join("library.json")).map_err(|e| e.to_string())
    }

    pub fn error(&self) -> Option<String> {
        self.0
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn account(&self) -> Xuid {
        self.0.xuid
    }
    pub fn published(&self, id: &str) -> Option<String> {
        let job = read(&self.dir(id).ok()?).ok()?;
        (job.done && !job.removed).then_some(job.remote).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Interrupted {
        offline: Cell<bool>,
        reserves: Cell<usize>,
        sent: RefCell<Vec<(String, usize, Vec<u8>)>>,
    }
    impl Remote for Interrupted {
        fn reserve(&self, upload: &Upload) -> Result<(String, bool), String> {
            assert_eq!(upload.visibility, Visibility::Private);
            self.reserves.set(self.reserves.get() + 1);
            Ok(("remote-id".into(), false))
        }
        fn part(&self, _: &str, object: &str, n: usize, bytes: &[u8]) -> Result<(), String> {
            if object == "film" && self.offline.get() {
                return Err("connection lost".into());
            }
            self.sent
                .borrow_mut()
                .push((object.into(), n, bytes.into()));
            Ok(())
        }
        fn complete(&self, _: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    #[test]
    fn interrupted_background_upload_reopens_account_state_and_resumes_at_next_object() {
        let root = std::env::temp_dir().join(live_proto::sign::random_id());
        let dir = Places::films(&root, "live.test", "account")
            .dir
            .join("recordings/game");
        fs::create_dir_all(&dir).unwrap();
        let source = root.join("original.h2film");
        fs::write(&source, b"original recording").unwrap();
        for name in ["film", "stats"] {
            fs::write(dir.join(name), name.as_bytes()).unwrap();
        }
        let mut job = Job {
            source: source.clone(),
            upload: Some(Upload {
                visibility: Visibility::Private,
                film: film::share::object(&dir.join("film")).unwrap(),
                stats: film::share::object(&dir.join("stats")).unwrap(),
            }),
            remote: None,
            next: 0,
            done: false,
            pin: None,
            removed: false,
        };
        let live = Interrupted::default();
        live.offline.set(true);
        assert!(transfer(&live, &dir, &mut job).is_err());
        let mut job = read(&dir).unwrap();
        assert_eq!(job.next, 1);
        assert!(!job.done);
        live.offline.set(false);
        assert_eq!(transfer(&live, &dir, &mut job).unwrap(), "remote-id");
        assert!(read(&dir).unwrap().done);
        assert_eq!(
            transfer(&live, &dir, &mut read(&dir).unwrap()).unwrap(),
            "remote-id"
        );
        assert_eq!(live.reserves.get(), 1);
        assert_eq!(
            *live.sent.borrow(),
            vec![
                ("stats".into(), 0, b"stats".to_vec()),
                ("film".into(), 0, b"film".to_vec())
            ]
        );
        assert_eq!(fs::read(source).unwrap(), b"original recording");
        fs::remove_dir_all(root).unwrap();
    }

    /// A recorded film's header, as a player's default recording writes it.
    fn recorded_header() -> film::Header {
        serde_json::from_value(serde_json::json!({
            "version": 3,
            "meta": {"id": "local-game", "map": "", "variant": "", "started_ms": 0,
                "length_ticks": 0, "players": [], "source": "Local", "image_stamp": "image"},
            "title_id": 0x4d53_0064u32,
            "xbe_sha256": "a".repeat(64),
            "runtime_stamp": "h2-sim/2",
            "epoch_100ns": 0,
            "lease_id": null,
            "xnet_addr": "192.168.1.20:3074",
            "transfers_per_slice": 5000,
            "lobby": false,
            "lobby_machine_start": false,
            "host_bots": false,
            "telemetry": true,
            "live_service": true,
            "features": "",
            "player_features": true,
            "inputs_in_film": true
        }))
        .unwrap()
    }

    #[test]
    fn a_locally_recorded_film_and_its_stats_are_prepared_and_uploaded_stats_first() {
        use film::columns::{ACTION_BYTES, SEATS};
        let root = std::env::temp_dir().join(live_proto::sign::random_id());
        let library = root.join("films");
        fs::create_dir_all(&library).unwrap();
        let source = library.join("auto-20261126-120000-000.h2film");
        let seed = film::file::Seed {
            header: recorded_header(),
            descriptor: Default::default(),
        };
        let mut w = film::file::FilmWriter::new(File::create(&source).unwrap(), &seed, 30).unwrap();
        for tick in 1..=300u64 {
            let mut seats = [None; SEATS];
            seats[0] = Some([tick as u8; ACTION_BYTES]);
            w.push_tick(film::file::TICK_DIGEST, tick, &seats).unwrap();
        }
        w.finish().unwrap();
        let stats = film::file::stats_of(&source);
        fs::write(&stats, b"the sealed stats container").unwrap();
        let recorded = fs::read(&source).unwrap();

        let dir = Places::films(&root, "live.test", "account")
            .dir
            .join("recordings/local-game");
        fs::create_dir_all(&dir).unwrap();
        let mut job = Job {
            source: source.clone(),
            upload: None,
            remote: None,
            next: 0,
            done: false,
            pin: None,
            removed: false,
        };
        prepare_job(&dir, &mut job).unwrap();
        let upload = job.upload.clone().unwrap();
        assert_eq!(upload.visibility, Visibility::Private);
        let live = Interrupted::default();
        assert_eq!(transfer(&live, &dir, &mut job).unwrap(), "remote-id");
        assert!(read(&dir).unwrap().done);

        let sent = live.sent.borrow();
        let objects: Vec<_> = sent.iter().map(|(name, ..)| name.as_str()).collect();
        let stats_parts = upload.stats.parts.len();
        assert!(objects[..stats_parts].iter().all(|n| *n == "stats"));
        assert!(objects[stats_parts..].iter().all(|n| *n == "film"));
        let object = |name: &str| -> Vec<u8> {
            sent.iter().filter(|(n, ..)| n == name).flat_map(|(.., b)| b.clone()).collect()
        };
        assert_eq!(object("stats"), b"the sealed stats container");
        // The film sent is the shared copy: the game's ticks, no address.
        let film_bytes = object("film");
        assert_eq!(film_bytes.len() as u64, upload.film.size);
        let shared = film::file::FilmReader::open(std::io::Cursor::new(&film_bytes)).unwrap();
        assert!(shared.sealed);
        assert_eq!(shared.length_ticks, 300);
        assert_eq!(shared.seed.header.meta.id, "local-game");
        assert!(shared.seed.header.xnet_addr.is_empty());
        // The library's files are left as they were.
        assert_eq!(fs::read(&source).unwrap(), recorded);
        assert_eq!(fs::read(&stats).unwrap(), b"the sealed stats container");
        assert!(!dir.join("film").exists() && !dir.join("stats").exists());
        drop(sent);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_film_without_its_stats_container_is_not_prepared() {
        let root = std::env::temp_dir().join(live_proto::sign::random_id());
        fs::create_dir_all(&root).unwrap();
        let source = root.join("auto-x.h2film");
        let seed = film::file::Seed { header: recorded_header(), descriptor: Default::default() };
        film::file::FilmWriter::new(File::create(&source).unwrap(), &seed, 30)
            .unwrap()
            .finish()
            .unwrap();
        let dir = root.join("job");
        fs::create_dir_all(&dir).unwrap();
        let mut job = Job {
            source,
            upload: None,
            remote: None,
            next: 0,
            done: false,
            pin: None,
            removed: false,
        };
        assert!(prepare_job(&dir, &mut job).unwrap_err().contains("stats container"));
        assert!(job.upload.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
