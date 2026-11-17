//! A durable event store in a directory, for the embedded server (`h2 play
//! --live-local`) when there is no Postgres: the log survives restarts, so
//! accounts, friends, published title files and games are kept.
//!
//! ```text
//! <dir>/events.jsonl      one line per append: {"events":[{"jws","jws_index"}..]}
//!                         (a batch's JWS once: its later events on the
//!                         line leave `jws` out)
//! <dir>/blobs/<sha256>    title-file bytes, content-addressed
//! <dir>/checkpoints.json  {"name": position}
//! <dir>/lock              held exclusively while the store is open
//! ```
//!
//! The log keeps only what the author signed; positions, versions and the
//! hash chain are recomputed on open by replaying it into a
//! [`MemoryStore`], which then serves every read. A batch is one line, so
//! a torn last line (a crash mid-write) loses that whole batch and nothing
//! else: it is cut off on open. Each append is flushed with `fsync` before
//! it returns.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::store::{
    BatchDecoder, EventStore, MemoryStore, NewEvent, Query, RecordedEvent, StoreError, Wake,
};

#[derive(Serialize, Deserialize)]
struct Line {
    events: Vec<Stored>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    /// Left out (empty) for an element of the batch the event before it
    /// on the line is in. Lines from before that always have it.
    #[serde(default, skip_serializing_if = "str::is_empty")]
    jws: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jws_index: Option<usize>,
}

pub struct FileStore {
    dir: PathBuf,
    mem: MemoryStore,
    /// The open log. Held across the in-memory append and the disk write
    /// so lines are in position order.
    log: tokio::sync::Mutex<File>,
    _lock: File,
}

fn io(e: io::Error) -> StoreError {
    StoreError::Io(e)
}

impl FileStore {
    /// Opens (creating if needed) the store in `dir` and replays its log.
    /// Fails if another process has it open.
    pub async fn open(dir: &Path) -> Result<Self, StoreError> {
        fs::create_dir_all(dir.join("blobs")).map_err(io)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))
            .map_err(io)?;
        // A server that just stopped (a quick relaunch, or a restart within
        // one process, whose runtime shuts down in the background) may still
        // hold the lock for a moment: wait up to two seconds before giving up.
        let mut tries = 0;
        while let Err(e) = lock.try_lock() {
            tries += 1;
            if tries > 40 {
                return Err(io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("{} is in use by another server ({e})", dir.display()),
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let mut log = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .append(true)
            .open(dir.join("events.jsonl"))
            .map_err(io)?;
        let mem = MemoryStore::new();
        let mut kept = 0u64;
        let mut reader = BufReader::new(&log);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(io)?;
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            let head = mem.head().await?;
            let parsed: Line = serde_json::from_str(&line).map_err(|e| StoreError::Corrupt {
                position: head + 1,
                reason: format!("{}: {e}", dir.join("events.jsonl").display()),
            })?;
            let mut events: Vec<NewEvent> = Vec::with_capacity(parsed.events.len());
            let mut batches = BatchDecoder::default();
            for s in parsed.events {
                let position = head + events.len() as u64 + 1;
                let jws: std::sync::Arc<str> = match (s.jws.is_empty(), events.last()) {
                    (false, _) => s.jws.into(),
                    // The same batch as the event before.
                    (true, Some(prev)) if s.jws_index.is_some() && prev.jws_index.is_some() => {
                        std::sync::Arc::clone(&prev.jws)
                    }
                    _ => {
                        return Err(StoreError::Corrupt {
                            position,
                            reason: "an event without a JWS".into(),
                        });
                    }
                };
                events.push(batches.decode(&jws, s.jws_index).map_err(|e| {
                    StoreError::Corrupt {
                        position,
                        reason: e.to_string(),
                    }
                })?);
            }
            mem.append(events, &[]).await?;
            kept += n as u64;
        }
        drop(reader);
        // A torn last line: the append it belonged to never returned.
        if log.metadata().map_err(io)?.len() != kept {
            log.set_len(kept).map_err(io)?;
            log.sync_data().map_err(io)?;
        }
        log.seek(SeekFrom::End(0)).map_err(io)?;
        if let Ok(raw) = fs::read(dir.join("checkpoints.json")) {
            let saved: HashMap<String, u64> =
                serde_json::from_slice(&raw).map_err(|e| StoreError::Corrupt {
                    position: 0,
                    reason: format!("checkpoints.json: {e}"),
                })?;
            for (name, position) in saved {
                mem.save_checkpoint(&name, position).await?;
            }
        }
        Ok(FileStore {
            dir: dir.to_path_buf(),
            mem,
            log: tokio::sync::Mutex::new(log),
            _lock: lock,
        })
    }

    fn blob_path(&self, sha256: &str) -> Result<PathBuf, StoreError> {
        let ok = !sha256.is_empty()
            && sha256.len() <= 128
            && sha256
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !ok {
            return Err(io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "blob keys are [A-Za-z0-9_-]",
            )));
        }
        Ok(self.dir.join("blobs").join(sha256))
    }
}

/// Writes `bytes` to `path` through a temporary file and a rename, so a
/// reader sees the old contents or the new, never half.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_data()?;
    fs::rename(&tmp, path)
}

#[async_trait]
impl EventStore for FileStore {
    fn kind(&self) -> &'static str {
        "file"
    }

    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        let mut log = self.log.lock().await;
        let line = Line {
            events: events
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let same_batch = i > 0
                        && e.jws_index.is_some()
                        && events[i - 1].jws_index.is_some()
                        && events[i - 1].jws == e.jws;
                    Stored {
                        jws: if same_batch {
                            String::new()
                        } else {
                            e.jws.to_string()
                        },
                        jws_index: e.jws_index,
                    }
                })
                .collect(),
        };
        let mut bytes = serde_json::to_vec(&line).expect("serializes");
        bytes.push(b'\n');
        // Checked in memory first (a conflict writes nothing), then made
        // durable before anyone is told it happened.
        let recorded = self.mem.append(events, expect).await?;
        log.write_all(&bytes).map_err(io)?;
        log.sync_data().map_err(io)?;
        Ok(recorded)
    }

    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        self.mem.read(query).await
    }

    async fn head(&self) -> Result<u64, StoreError> {
        self.mem.head().await
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.blob_path(sha256)?;
        if !path.is_file() {
            replace(&path, bytes).map_err(io)?;
        }
        Ok(())
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match fs::read(self.blob_path(sha256)?) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        self.mem.load_checkpoint(name).await
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        // Under the log lock: one writer of checkpoints.json at a time.
        let _log = self.log.lock().await;
        self.mem.save_checkpoint(name, position).await?;
        let all = self.mem.checkpoint_map();
        replace(
            &self.dir.join("checkpoints.json"),
            &serde_json::to_vec(&all).expect("serializes"),
        )
        .map_err(io)
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        self.mem.wake().await
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        let mut names = Vec::new();
        for entry in fs::read_dir(self.dir.join("blobs")).map_err(io)? {
            let name = entry.map_err(io)?.file_name();
            // Skip a write's temporary file (`<sha>.tmp-<pid>`).
            if let Some(name) = name.to_str().filter(|n| !n.contains('.')) {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        self.mem.checkpoints().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::conformance;
    use live_proto::sign::KeyPair;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2-filestore-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[tokio::test]
    async fn file_store_conforms() {
        let d = dir("conform");
        conformance::run(&FileStore::open(&d).await.unwrap(), "file").await;
        fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn everything_survives_a_reopen() {
        let d = dir("reopen");
        let key = KeyPair::generate();
        let (before, all) = {
            let s = FileStore::open(&d).await.unwrap();
            s.append(
                vec![
                    conformance::event(&key, "a", 0),
                    conformance::event(&key, "b", 1),
                ],
                &[],
            )
            .await
            .unwrap();
            s.append(vec![conformance::event(&key, "a", 2)], &[("a".into(), 1)])
                .await
                .unwrap();
            s.put_blob("abc123", b"motd bytes").await.unwrap();
            s.save_checkpoint("games", 3).await.unwrap();
            // A second open of the same directory is refused while this
            // one holds it.
            assert!(matches!(FileStore::open(&d).await, Err(StoreError::Io(_))));
            (
                s.head().await.unwrap(),
                s.read(&Query::after(0, 100)).await.unwrap(),
            )
        };
        let s = FileStore::open(&d).await.unwrap();
        assert_eq!(s.head().await.unwrap(), before);
        assert_eq!(s.read(&Query::after(0, 100)).await.unwrap(), all);
        assert_eq!(
            s.blob("abc123").await.unwrap().as_deref(),
            Some(&b"motd bytes"[..])
        );
        assert_eq!(s.blob("missing").await.unwrap(), None);
        assert!(s.blob("../escape").await.is_err());
        assert_eq!(s.load_checkpoint("games").await.unwrap(), Some(3));
        // Versions continue where they were: the stream is at 2.
        let next = s
            .append(vec![conformance::event(&key, "a", 4)], &[("a".into(), 2)])
            .await
            .unwrap();
        assert_eq!((next[0].position, next[0].version), (4, 2));
        assert_eq!(
            next[0].hash,
            live_proto::sign::chain_hash(Some(&all[2].hash), &next[0].jws)
        );
        drop(s);
        fs::remove_dir_all(&d).unwrap();
    }

    /// A log from before batches were written once (each event of a batch
    /// on the line with the whole JWS) replays as before, and a new batch
    /// goes on its line once.
    #[tokio::test]
    async fn a_log_from_before_batches_were_written_once_still_verifies() {
        let d = dir("legacy");
        let key = KeyPair::generate();
        let legacy = conformance::legacy_log(&key, "game-legacy");
        fs::create_dir_all(&d).unwrap();
        let line = |events: &[RecordedEvent]| {
            let events: Vec<_> = events
                .iter()
                .map(|e| serde_json::json!({"jws": &*e.jws, "jws_index": e.jws_index}))
                .collect();
            serde_json::json!({ "events": events }).to_string() + "\n"
        };
        fs::write(
            d.join("events.jsonl"),
            line(&legacy[..1]) + &line(&legacy[1..]),
        )
        .unwrap();
        let s = FileStore::open(&d).await.unwrap();
        conformance::upgraded(&s, &key, &legacy).await;
        let all = s.read(&Query::after(0, 100)).await.unwrap();
        drop(s);
        let text = fs::read_to_string(d.join("events.jsonl")).unwrap();
        let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        let jws: Vec<_> = last["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.get("jws").is_some())
            .collect();
        assert_eq!(jws, [true, false], "the batch's JWS once");
        // And it all replays.
        let s = FileStore::open(&d).await.unwrap();
        assert_eq!(s.read(&Query::after(0, 100)).await.unwrap(), all);
        drop(s);
        fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn a_torn_last_line_is_dropped_whole() {
        let d = dir("torn");
        let key = KeyPair::generate();
        {
            let s = FileStore::open(&d).await.unwrap();
            s.append(vec![conformance::event(&key, "a", 0)], &[])
                .await
                .unwrap();
        }
        let path = d.join("events.jsonl");
        let good = fs::read(&path).unwrap();
        let mut torn = good.clone();
        torn.extend_from_slice(br#"{"events":[{"jws":"eyJ"#);
        fs::write(&path, &torn).unwrap();
        let s = FileStore::open(&d).await.unwrap();
        assert_eq!(s.head().await.unwrap(), 1);
        assert_eq!(fs::read(&path).unwrap(), good);
        // And appends after the cut are whole lines again.
        s.append(vec![conformance::event(&key, "a", 1)], &[])
            .await
            .unwrap();
        drop(s);
        let s = FileStore::open(&d).await.unwrap();
        assert_eq!(s.head().await.unwrap(), 2);
        drop(s);
        fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn a_conflict_writes_nothing_to_disk() {
        let d = dir("conflict");
        let key = KeyPair::generate();
        let s = FileStore::open(&d).await.unwrap();
        s.append(vec![conformance::event(&key, "a", 0)], &[])
            .await
            .unwrap();
        let len = fs::metadata(d.join("events.jsonl")).unwrap().len();
        assert!(matches!(
            s.append(vec![conformance::event(&key, "a", 1)], &[("a".into(), 0)])
                .await,
            Err(StoreError::Conflict { .. })
        ));
        assert_eq!(fs::metadata(d.join("events.jsonl")).unwrap().len(), len);
        drop(s);
        fs::remove_dir_all(&d).unwrap();
    }
}
