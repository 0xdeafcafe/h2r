//! `h2 live store copy` (and `h2-live-server store copy`): moves a server's
//! state between backends, for switching an existing server to SQLite and
//! R2 (docs/live/live-hosting.md, "Free hosting").
//!
//! - **The log**, `--from DB --to DB`: every event with its position,
//!   version and hash kept ([`EventStore::import`]), so cursors clients hold
//!   and checkpoints stay valid; then the title-file blobs and the durable
//!   subscribers' checkpoints. Each batch is checked (stream versions, the
//!   hash chain) and written atomically.
//! - **Blobs**, `--from-blobs LOC --to-blobs LOC`: everything under
//!   [`PREFIXES`] (custom content, cloud saves and their indexes).
//!
//! Both are idempotent and resumable: run it again and it carries on where
//! it stopped. The log copy picks up after the destination's head, once it
//! has checked that the destination's last event is the source's event at
//! that position (a different log is refused, not merged); a blob already
//! there with the same size is skipped. Stop the server first, or copy
//! again after stopping it, so nothing written meanwhile is left behind.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::blobs::{BlobStore, Location, Meta};
use crate::store::{EventStore, Query, StoreError};

/// What the blob copy moves: the server's blobs, nothing else in the data
/// directory (its key, its database).
pub const PREFIXES: &[&str] = &["content/", "storage/"];

const BATCH: usize = 500;

/// What [`copy_log`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LogReport {
    /// Events written now.
    pub events: u64,
    /// The destination's head before: what it had already.
    pub already: u64,
    pub blobs: usize,
    pub checkpoints: usize,
}

/// Copies `from`'s log into `to`, from `to`'s head on. `progress` hears
/// (copied so far, source head) after each batch.
pub async fn copy_log(
    from: &dyn EventStore,
    to: &dyn EventStore,
    mut progress: impl FnMut(u64, u64),
) -> Result<LogReport, StoreError> {
    let source_head = from.head().await?;
    let mut cursor = to.head().await?;
    let mut report = LogReport::default();
    if cursor > 0 {
        let at = |s: &'static str| StoreError::Import {
            position: cursor,
            reason: format!(
                "the destination holds a different log ({s}); copy into an empty database"
            ),
        };
        let mine = to.read(&Query::after(cursor - 1, 1)).await?;
        let theirs = from.read(&Query::after(cursor - 1, 1)).await?;
        match (mine.first(), theirs.first()) {
            (Some(m), Some(t)) if m.position == cursor && t.position == cursor => {
                if m.hash != t.hash || m.jws != t.jws {
                    return Err(at("its last event differs"));
                }
            }
            _ => return Err(at("the source has no event at its head")),
        }
        report.already = cursor;
    }
    loop {
        let batch = from.read(&Query::after(cursor, BATCH)).await?;
        let Some(last) = batch.last() else { break };
        cursor = last.position;
        to.import(&batch).await?;
        report.events += batch.len() as u64;
        progress(report.events, source_head);
    }
    for sha in from.blob_names().await? {
        if to.blob(&sha).await?.is_none()
            && let Some(bytes) = from.blob(&sha).await?
        {
            to.put_blob(&sha, &bytes).await?;
            report.blobs += 1;
        }
    }
    for (name, position) in from.checkpoints().await? {
        if to
            .load_checkpoint(&name)
            .await?
            .is_none_or(|p| p < position)
        {
            to.save_checkpoint(&name, position).await?;
            report.checkpoints += 1;
        }
    }
    Ok(report)
}

/// What [`copy_blobs`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlobReport {
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
}

/// Whether a key is a write's leftover rather than a blob: a temporary
/// upload (`.../tmp/...`, where servers before the blob store kept them).
fn leftover(key: &str) -> bool {
    key.split('/').any(|s| s == "tmp")
}

/// Copies every blob under `prefixes` from `from` to `to`, skipping those
/// `to` has at the same size.
pub async fn copy_blobs(
    from: &BlobStore,
    to: &BlobStore,
    prefixes: &[&str],
    mut progress: impl FnMut(&str),
) -> crate::blobs::Result<BlobReport> {
    let mut report = BlobReport::default();
    for prefix in prefixes {
        for info in from.list(prefix).await? {
            if leftover(&info.key) {
                continue;
            }
            if to
                .head(&info.key)
                .await?
                .is_some_and(|i| i.size == info.size)
            {
                report.skipped += 1;
                continue;
            }
            let Some(bytes) = from.get(&info.key).await? else {
                continue;
            };
            let meta = Meta {
                content_type: Some(
                    if info.key.ends_with(".json") {
                        "application/json"
                    } else {
                        "application/octet-stream"
                    }
                    .into(),
                ),
                ..Meta::default()
            };
            report.bytes += bytes.len() as u64;
            to.put(&info.key, bytes, &meta).await?;
            report.copied += 1;
            progress(&info.key);
        }
    }
    Ok(report)
}

pub const USAGE: &str =
    "usage: store copy [--from DB --to DB] [--from-blobs LOC --to-blobs LOC] [--data DIR]

  --from DB, --to DB      copy the event log: sqlite://PATH, postgres://...,
                          or file:DIR (the embedded server's log, source only)
  --from-blobs LOC        copy content and cloud saves: a directory (the
  --to-blobs LOC          server's data directory) or s3://BUCKET[/PREFIX]
                          (endpoint and keys from $H2_LIVE_S3_ENDPOINT,
                          $H2_LIVE_S3_ACCESS_KEY_ID, $H2_LIVE_S3_SECRET_ACCESS_KEY)
  --data DIR              where sqlite paths without a directory go, and
                          --from-blobs's default

Idempotent and resumable: run it again to carry on. Stop the server first.";

/// Opens the event store a `--db`-style URL names.
pub async fn open_db(url: &str) -> Result<Arc<dyn EventStore>> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let pg = crate::pg::PostgresStore::connect_creating(url)
            .await
            .map_err(|e| crate::db_error(url, &e))?;
        return Ok(Arc::new(pg));
    }
    if let Some(path) = crate::sqlite::path_of(url) {
        let s = crate::sqlite::SqliteStore::open(&path)
            .await
            .with_context(|| format!("opening {}", path.display()))?;
        return Ok(Arc::new(s));
    }
    if let Some(dir) = url.strip_prefix("file:") {
        let dir = dir.strip_prefix("//").unwrap_or(dir);
        let f = crate::filestore::FileStore::open(Path::new(dir))
            .await
            .with_context(|| format!("opening {dir}"))?;
        return Ok(Arc::new(f));
    }
    bail!("{url:?}: a database is sqlite://PATH or postgres://HOST/NAME")
}

/// `store copy`'s arguments (after `store copy`).
pub async fn run(args: Vec<String>, env: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    let mut args = crate::standalone::Args(args);
    if args.flag("--help") || args.flag("-h") {
        bail!("{USAGE}");
    }
    let from = args.take("--from")?;
    let to = args.take("--to")?;
    let from_blobs = args.take("--from-blobs")?;
    let to_blobs = args.take("--to-blobs")?;
    let data: Option<PathBuf> = args.take("--data")?.map(PathBuf::from);
    if let Some(extra) = args.0.first() {
        bail!("unexpected {extra}\n\n{USAGE}");
    }
    let log = match (from, to) {
        (Some(f), Some(t)) => Some((f, t)),
        (None, None) => None,
        _ => bail!("--from and --to go together\n\n{USAGE}"),
    };
    let blobs = match (from_blobs, to_blobs, &data) {
        (f, Some(t), d) => Some((
            f.or_else(|| d.as_ref().map(|d| d.display().to_string()))
                .context("--to-blobs needs --from-blobs (or --data, the directory they are in)")?,
            t,
        )),
        (Some(_), None, _) => bail!("--from-blobs needs --to-blobs"),
        (None, None, _) => None,
    };
    if log.is_none() && blobs.is_none() {
        bail!("nothing to copy\n\n{USAGE}");
    }
    if let Some((from, to)) = log {
        if from == to {
            bail!("--from and --to are the same database");
        }
        let source = open_db(&from).await?;
        let dest = open_db(&to).await?;
        println!(
            "store copy: log {} ({}) -> {} ({})",
            crate::redact(&from),
            source.kind(),
            crate::redact(&to),
            dest.kind()
        );
        let mut last_print = std::time::Instant::now();
        let report = copy_log(&*source, &*dest, |done, of| {
            if last_print.elapsed().as_secs() >= 2 {
                println!("store copy: {done} events (source head {of})");
                last_print = std::time::Instant::now();
            }
        })
        .await?;
        println!(
            "store copy: log done: {} events copied (the destination was at {}), {} title-file blobs, {} checkpoints; head {}",
            report.events,
            report.already,
            report.blobs,
            report.checkpoints,
            dest.head().await?
        );
    }
    if let Some((from, to)) = blobs {
        let source = BlobStore::open(&Location::parse(&from)?, env, false)?;
        let dest = BlobStore::open(&Location::parse(&to)?, env, false)?;
        println!(
            "store copy: blobs {} -> {}",
            source.describe(),
            dest.describe()
        );
        let mut n = 0usize;
        let report = copy_blobs(&source, &dest, PREFIXES, |key| {
            n += 1;
            if n.is_multiple_of(100) {
                println!("store copy: {n} blobs ({key})");
            }
        })
        .await?;
        println!(
            "store copy: blobs done: {} copied ({} bytes), {} already there",
            report.copied, report.bytes, report.skipped
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, conformance::event};
    use live_proto::sign::KeyPair;

    async fn fill(store: &dyn EventStore, key: &KeyPair, n: u64) {
        for i in 0..n {
            let stream = format!("s{}", i % 3);
            store
                .append(vec![event(key, &stream, i)], &[])
                .await
                .unwrap();
        }
    }

    async fn all(store: &dyn EventStore) -> Vec<crate::store::RecordedEvent> {
        store.read(&Query::after(0, usize::MAX)).await.unwrap()
    }

    /// Positions with gaps (Postgres's sequence skips numbers on rollback)
    /// come through unchanged, and appends after a copy carry on the chain.
    #[tokio::test]
    async fn the_log_moves_with_positions_versions_and_hashes() {
        let key = KeyPair::generate();
        let path = crate::sqlite::tests::temp_db("copy-src");
        let src = crate::sqlite::SqliteStore::open(&path).await.unwrap();
        fill(&src, &key, 7).await;
        // A gap: a position taken and never committed.
        sqlx::query("update sqlite_sequence set seq = seq + 5 where name = 'events'")
            .execute(src.pool())
            .await
            .unwrap();
        fill(&src, &key, 5).await;
        src.put_blob("title-sha", b"motd").await.unwrap();
        src.save_checkpoint("projector", 9).await.unwrap();
        let before = all(&src).await;
        assert_eq!(before.len(), 12);
        assert!(
            before.windows(2).any(|w| w[1].position > w[0].position + 1),
            "a gap"
        );

        let dpath = crate::sqlite::tests::temp_db("copy-dst");
        let dst = crate::sqlite::SqliteStore::open(&dpath).await.unwrap();
        let mut heard = 0;
        let r = copy_log(&src, &dst, |_, _| heard += 1).await.unwrap();
        assert_eq!((r.events, r.blobs, r.checkpoints), (12, 1, 1));
        assert!(heard > 0);
        assert_eq!(all(&dst).await, before);
        assert_eq!(
            dst.blob("title-sha").await.unwrap().as_deref(),
            Some(&b"motd"[..])
        );
        assert_eq!(dst.load_checkpoint("projector").await.unwrap(), Some(9));

        // Run again: nothing to do.
        let again = copy_log(&src, &dst, |_, _| {}).await.unwrap();
        assert_eq!((again.events, again.blobs), (0, 0));
        assert_eq!(again.already, src.head().await.unwrap());

        // The source grew (the server ran on): a second copy resumes.
        fill(&src, &key, 4).await;
        let more = copy_log(&src, &dst, |_, _| {}).await.unwrap();
        assert_eq!(more.events, 4);
        assert_eq!(all(&dst).await, all(&src).await);

        // The copy is a working log: appends continue versions and chains,
        // after the highest position.
        let next = dst.append(vec![event(&key, "s0", 99)], &[]).await.unwrap();
        let src_s0 = src
            .read(&Query {
                stream: Some("s0".into()),
                ..Query::after(0, 100)
            })
            .await
            .unwrap();
        let last = src_s0.last().unwrap();
        assert_eq!(next[0].version, last.version + 1);
        assert_eq!(
            next[0].hash,
            live_proto::sign::chain_hash(Some(&last.hash), &next[0].jws)
        );
        assert!(next[0].position > src.head().await.unwrap());
        for p in [&path, &dpath] {
            std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
        }
    }

    #[tokio::test]
    async fn a_different_log_is_refused_not_merged() {
        let key = KeyPair::generate();
        let (a, b) = (MemoryStore::new(), MemoryStore::new());
        fill(&a, &key, 3).await;
        fill(&b, &key, 2).await;
        let err = copy_log(&a, &b, |_, _| {}).await.unwrap_err();
        assert!(err.to_string().contains("different log"), "{err}");
        assert_eq!(all(&b).await.len(), 2, "nothing written");
    }

    #[tokio::test]
    async fn import_checks_what_it_is_given() {
        let key = KeyPair::generate();
        let src = MemoryStore::new();
        fill(&src, &key, 4).await;
        let mut events = all(&src).await;
        let path = crate::sqlite::tests::temp_db("import");
        let dst = crate::sqlite::SqliteStore::open(&path).await.unwrap();
        // A broken chain is refused, and the batch writes nothing.
        let mut bad = events.clone();
        bad[3].hash = "0".repeat(64);
        let err = dst.import(&bad).await.unwrap_err();
        assert!(
            matches!(err, StoreError::Import { position: 4, .. }),
            "{err}"
        );
        assert_eq!(dst.head().await.unwrap(), 0);
        // So is a skipped version.
        let mut skip = events.clone();
        skip.remove(0);
        assert!(dst.import(&skip).await.is_err());
        dst.import(&events).await.unwrap();
        // And anything at or before the head.
        events.truncate(1);
        assert!(dst.import(&events).await.is_err());
        // A store that numbers its own positions takes no import.
        let file_dir = crate::blobs::tests::temp_dir("filestore");
        let file = crate::filestore::FileStore::open(&file_dir).await.unwrap();
        assert!(file.import(&events).await.is_err());
        drop(file);
        std::fs::remove_dir_all(&file_dir).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// The embedded server's file log copies into SQLite.
    #[tokio::test]
    async fn the_file_log_copies_into_sqlite() {
        let key = KeyPair::generate();
        let dir = crate::blobs::tests::temp_dir("file-src");
        let file = crate::filestore::FileStore::open(&dir).await.unwrap();
        fill(&file, &key, 5).await;
        file.put_blob("abc", b"x").await.unwrap();
        let path = crate::sqlite::tests::temp_db("from-file");
        let dst = crate::sqlite::SqliteStore::open(&path).await.unwrap();
        let r = copy_log(&file, &dst, |_, _| {}).await.unwrap();
        assert_eq!((r.events, r.blobs), (5, 1));
        assert_eq!(all(&dst).await, all(&file).await);
        drop(file);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn blobs_move_between_stores_and_resume() {
        let root = crate::blobs::tests::temp_dir("copy-blobs");
        let from = BlobStore::dir(&root).unwrap();
        for (k, v) in [
            ("content/aa/aa11", &b"map"[..]),
            ("storage/7/index.json", b"{}"),
            ("storage/7/blobs/bb/bb22", b"save"),
            ("content/tmp/half-upload", b"junk"),
        ] {
            from.put(k, bytes::Bytes::copy_from_slice(v), &Meta::default())
                .await
                .unwrap();
        }
        // The rest of the data directory is not blobs.
        std::fs::write(root.join("server.key"), b"secret").unwrap();
        std::fs::write(root.join("live.db"), b"db").unwrap();
        let to = BlobStore::memory();
        // Already there from an earlier, interrupted run.
        to.put(
            "content/aa/aa11",
            bytes::Bytes::from_static(b"map"),
            &Meta::default(),
        )
        .await
        .unwrap();
        let mut seen = Vec::new();
        let r = copy_blobs(&from, &to, PREFIXES, |k| seen.push(k.to_string()))
            .await
            .unwrap();
        assert_eq!((r.copied, r.skipped), (2, 1));
        assert_eq!(seen, ["storage/7/blobs/bb/bb22", "storage/7/index.json"]);
        assert_eq!(
            &to.get("storage/7/blobs/bb/bb22").await.unwrap().unwrap()[..],
            b"save"
        );
        assert_eq!(to.get("content/tmp/half-upload").await.unwrap(), None);
        assert!(
            to.list("")
                .await
                .unwrap()
                .iter()
                .all(|i| !i.key.contains("server.key"))
        );
        let again = copy_blobs(&from, &to, PREFIXES, |_| {}).await.unwrap();
        assert_eq!((again.copied, again.skipped), (0, 3));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn the_command_line() {
        let none = |_: &str| None;
        let run = |a: &[&str]| run(a.iter().map(|s| s.to_string()).collect(), &none);
        let err = |r: Result<()>| format!("{:#}", r.unwrap_err());
        assert!(err(run(&[]).await).contains("nothing to copy"));
        assert!(err(run(&["--from", "sqlite://a.db"]).await).contains("go together"));
        assert!(err(run(&["--from-blobs", "/x"]).await).contains("needs --to-blobs"));
        assert!(
            err(run(&["--from", "mysql://x", "--to", "sqlite://b"]).await)
                .contains("sqlite://PATH or postgres://")
        );
        assert!(
            err(run(&["--from", "sqlite://a", "--to", "sqlite://a"]).await)
                .contains("same database")
        );

        let key = KeyPair::generate();
        let dir = crate::blobs::tests::temp_dir("cli");
        let src = dir.join("src.db");
        let dst = dir.join("a b/dst.db");
        let s = crate::sqlite::SqliteStore::open(&src).await.unwrap();
        fill(&s, &key, 3).await;
        drop(s);
        BlobStore::dir(&dir)
            .unwrap()
            .put(
                "content/cc/cc33",
                bytes::Bytes::from_static(b"c"),
                &Meta::default(),
            )
            .await
            .unwrap();
        let out = dir.join("out");
        run(&[
            "--from",
            &crate::sqlite::url_for(&src),
            "--to",
            &crate::sqlite::url_for(&dst),
            "--data",
            dir.to_str().unwrap(),
            "--to-blobs",
            out.to_str().unwrap(),
        ])
        .await
        .unwrap();
        let d = crate::sqlite::SqliteStore::open(&dst).await.unwrap();
        assert_eq!(d.head().await.unwrap(), 3);
        assert_eq!(std::fs::read(out.join("content/cc/cc33")).unwrap(), b"c");
        drop(d);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
