//! Drive the same bounded, verified replay operations used by the end-game
//! job. The checkpoint is local. Dry runs never change server state.
use crate::live_cmd::{self, Args, Client};
use crate::live_query::path_part;
use crate::live_recovery::Signer;
use anyhow::{Context, Result, bail, ensure};
use live_proto::Page;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const ROOT: &str = "/admin/replay-archives";
const USAGE: &str = "h2 live frames migrate --dry-run|--apply|--verify [--server URL] [--as GAMERTAG | --data DIR] [--checkpoint FILE] [--delay-ms N]";
const ROLLBACK: &str = "h2 live frames rollback GAME --archive-sha256 HASH --backup-sha256 HASH [--server URL] [--as GAMERTAG | --data DIR]";

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    DryRun,
    Apply,
    Verify,
}
impl Mode {
    fn read(args: &mut Args) -> Result<Self> {
        let chosen = [
            (args.flag("--dry-run"), Self::DryRun),
            (args.flag("--apply"), Self::Apply),
            (args.flag("--verify"), Self::Verify),
        ];
        let mut modes = chosen.into_iter().filter_map(|(b, m)| b.then_some(m));
        let first = modes.next().context(USAGE)?;
        ensure!(modes.next().is_none(), "{USAGE}");
        Ok(first)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
struct Totals {
    frames: u64,
    source_bytes: u64,
    archive_bytes: u64,
    segments: u64,
}
impl Totals {
    fn add(&mut self, r: &Self) -> Result<()> {
        macro_rules! field {
            ($f:ident) => {
                self.$f = self
                    .$f
                    .checked_add(r.$f)
                    .context("migration totals overflow")?;
            };
        }
        field!(frames);
        field!(source_bytes);
        field!(archive_bytes);
        field!(segments);
        Ok(())
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Game {
    game_id: String,
    state: String,
    source_position: u64,
    chunks: u64,
    legacy_frames: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Job {
    game: Game,
    cursor: Option<String>,
    totals: Totals,
}
#[derive(Deserialize)]
struct Reply {
    game_id: String,
    state: String,
    done: bool,
    next: Option<String>,
    source_position: u64,
    source_bytes: u64,
    archive_bytes: u64,
    frames: u64,
    segment: Option<serde_json::Value>,
    totals: Option<Totals>,
    archive_sha256: Option<String>,
    backup_sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Failed {
    job: Job,
    reason: String,
}
#[derive(Serialize, Deserialize)]
struct Checkpoint {
    format: u32,
    server: String,
    identity: String,
    mode: Mode,
    cursor: Option<String>,
    scanned: bool,
    pending: VecDeque<Job>,
    failed: Vec<Failed>,
    migrated: u64,
    skipped: u64,
    #[serde(default)]
    unpublished: u64,
    chunks: u64,
    legacy_frames: u64,
    totals: Totals,
}
impl Checkpoint {
    fn new(server: String, identity: String, mode: Mode) -> Self {
        Self {
            format: 1,
            server,
            identity,
            mode,
            cursor: None,
            scanned: false,
            pending: VecDeque::new(),
            failed: vec![],
            migrated: 0,
            skipped: 0,
            unpublished: 0,
            chunks: 0,
            legacy_frames: 0,
            totals: Totals::default(),
        }
    }
    fn completed(&self) -> bool {
        self.scanned && self.pending.is_empty() && self.failed.is_empty()
    }
    fn retry(&mut self) {
        self.pending.extend(self.failed.drain(..).map(|f| f.job));
    }
    fn page(&mut self, page: Page<Game>) -> Result<()> {
        ensure!(
            page.next.is_none() || page.next != self.cursor,
            "inventory cursor did not advance"
        );
        self.cursor = page.next;
        self.scanned = self.cursor.is_none();
        for game in page.items {
            let eligible = match (self.mode, game.state.as_str()) {
                (_, "live" | "empty") => false,
                (Mode::DryRun | Mode::Apply, "published") => false,
                (Mode::Verify, "published") => true,
                (Mode::Verify, "raw" | "staging" | "rolled_back") => {
                    self.unpublished += 1;
                    false
                }
                (Mode::DryRun | Mode::Apply, "raw" | "staging" | "rolled_back") => true,
                _ => bail!("unsupported recording state {:?}", game.state),
            };
            if eligible {
                self.chunks += game.chunks;
                self.legacy_frames += game.legacy_frames;
                self.pending.push_back(Job {
                    game,
                    cursor: None,
                    totals: Totals::default(),
                });
            } else {
                self.skipped += 1;
            }
        }
        Ok(())
    }
    fn accept(&mut self, response: Reply) -> Result<()> {
        let job = self.pending.front_mut().context("no active game")?;
        ensure!(
            response.game_id == job.game.game_id
                && response.source_position == job.game.source_position,
            "server returned a different game or source revision"
        );
        ensure!(
            response.done == response.next.is_none()
                && (response.done || response.next != job.cursor),
            "invalid step continuation"
        );
        if response.done && self.mode != Mode::DryRun {
            ensure!(
                response.state == "published"
                    && valid_digest(response.archive_sha256.as_deref())
                    && valid_digest(response.backup_sha256.as_deref()),
                "missing verified archive or backup"
            );
        }
        if let Some(totals) = response.totals {
            job.totals = totals;
        } else {
            ensure!(
                self.mode == Mode::DryRun,
                "server omitted cumulative counts"
            );
            job.totals.add(&Totals {
                frames: response.frames,
                source_bytes: response.source_bytes,
                archive_bytes: response.archive_bytes,
                segments: response.segment.is_some() as u64,
            })?;
        }
        job.cursor = response.next;
        if response.done {
            self.totals.add(&job.totals)?;
            println!(
                "{}  {} frames  {} -> {} B  archive={}  backup={}",
                job.game.game_id,
                job.totals.frames,
                job.totals.source_bytes,
                job.totals.archive_bytes,
                response.archive_sha256.as_deref().unwrap_or("-"),
                response.backup_sha256.as_deref().unwrap_or("-")
            );
            self.pending.pop_front();
            self.migrated += 1;
        }
        Ok(())
    }
}
fn valid_digest(s: Option<&str>) -> bool {
    s.is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

struct Remote {
    client: Client,
    signer: Signer,
    delay: Duration,
    last: Option<Instant>,
}
impl Remote {
    fn from_args(args: &mut Args) -> Result<Self> {
        let delay = args
            .take("--delay-ms")?
            .map_or(Ok(250u64), |d| d.parse())
            .context("--delay-ms needs a whole number")?;
        let client = Client::connect(args.take("--server")?)?;
        let signer = Signer::from_args(args, &client)?;
        Ok(Self {
            client,
            signer,
            delay: Duration::from_millis(delay),
            last: None,
        })
    }
    fn request<T: DeserializeOwned>(
        &mut self,
        request: ureq::Request,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        if let Some(last) = self.last {
            std::thread::sleep(self.delay.saturating_sub(last.elapsed()));
        }
        let method = if body.is_some() {
            live_server::admin::LXM_ADMIN_WRITE
        } else {
            live_server::invites::LXM_ADMIN_READ
        };
        let token = self.signer.token(&self.client, method)?;
        let req = self.client.authorized(request, token)?;
        let data = body.as_ref().map(serde_json::to_vec).transpose()?;
        self.last = Some(Instant::now());
        Ok(serde_json::from_str(&live_cmd::call(
            req.set("content-type", "application/json"),
            data.as_deref(),
        )?)?)
    }
    fn page(&mut self, cursor: Option<&str>) -> Result<Page<Game>> {
        let mut req = self
            .client
            .agent
            .get(&self.client.url(ROOT))
            .query("limit", "32");
        if let Some(c) = cursor {
            req = req.query("cursor", c);
        }
        self.request(req, None)
    }
    fn post<T: DeserializeOwned>(
        &mut self,
        id: &str,
        op: &str,
        data: serde_json::Value,
    ) -> Result<T> {
        let req = self
            .client
            .agent
            .post(&self.client.url(&format!("{ROOT}/{}/{op}", path_part(id))));
        self.request(req, Some(data))
    }
    fn step(&mut self, mode: Mode, job: &Job) -> Result<Reply> {
        self.post(&job.game.game_id,"step",serde_json::json!({"mode":mode,"cursor":job.cursor,"source_position":job.game.source_position}))
    }
}

fn save(path: &Path, state: &Checkpoint) -> Result<()> {
    let temp = path.with_file_name(format!(
        "{}.new",
        path.file_name()
            .context("missing checkpoint filename")?
            .to_string_lossy()
    ));
    let mut file = fs::File::create(&temp)?;
    serde_json::to_writer(&mut file, state)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, path)?;
    #[cfg(unix)]
    fs::File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}

pub(crate) fn migrate(mut args: Args) -> Result<()> {
    let mode = Mode::read(&mut args)?;
    let path = args.take("--checkpoint")?;
    let mut remote = Remote::from_args(&mut args)?;
    ensure!(args.positional_for(USAGE)?.is_empty(), "{USAGE}");
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let digest = Sha256::digest(remote.client.base.as_bytes());
            let host = format!("{digest:x}");
            live_cmd::live_dir()?
                .join("migration")
                .join(format!("replays-{}-{mode:?}.json", &host[..16]))
        }
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_added_extension("lock"))?;
    lock.try_lock()
        .context("another process is using this checkpoint")?;
    let mut state = match fs::read(&path) {
        Ok(b) => {
            let s: Checkpoint =
                serde_json::from_slice(&b).context("invalid migration checkpoint")?;
            ensure!(
                s.format == 1
                    && s.mode == mode
                    && s.server == remote.client.base
                    && s.identity == remote.client.discovery.id,
                "checkpoint belongs to a different server, mode or format"
            );
            if s.completed() {
                Checkpoint::new(s.server, s.identity, mode)
            } else {
                s
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Checkpoint::new(
            remote.client.base.clone(),
            remote.client.discovery.id.clone(),
            mode,
        ),
        Err(e) => return Err(e.into()),
    };
    state.retry();
    eprintln!(
        "{mode:?} on {}  checkpoint {}",
        remote.client.base,
        path.display()
    );
    loop {
        if let Some(job) = state.pending.front() {
            let result = remote.step(mode, job).and_then(|reply| state.accept(reply));
            if let Err(e) = result {
                // Transport/auth failures can affect every later game. The
                // durable cursor stays put; invoking the same command retries.
                if !matches!(live_cmd::status_of(&e), Some(400 | 404 | 409 | 422)) {
                    return Err(e.context(format!("resume with checkpoint {}", path.display())));
                }
                let job = state.pending.pop_front().unwrap();
                eprintln!("FAILED {}: {e:#}", job.game.game_id);
                state.failed.push(Failed {
                    job,
                    reason: format!("{e:#}"),
                });
            }
        } else if !state.scanned {
            let page = remote.page(state.cursor.as_deref())?;
            state.page(page)?;
        } else {
            break;
        }
        save(&path, &state)?;
    }
    println!(
        "{} games complete, {} skipped, {} failed; {} chunk objects, {} legacy rows; {} segments, {} frames, {} -> {} bytes",
        state.migrated,
        state.skipped,
        state.failed.len(),
        state.chunks,
        state.legacy_frames,
        state.totals.segments,
        state.totals.frames,
        state.totals.source_bytes,
        state.totals.archive_bytes
    );
    if mode == Mode::Verify {
        println!(
            "{} finished recordings still have an unpublished archive",
            state.unpublished
        );
    }
    ensure!(
        state.failed.is_empty(),
        "failed game IDs and last verified cursors saved in {}",
        path.display()
    );
    Ok(())
}
pub(crate) fn rollback(mut args: Args) -> Result<()> {
    let archive = args.take("--archive-sha256")?.context(ROLLBACK)?;
    let backup = args.take("--backup-sha256")?.context(ROLLBACK)?;
    ensure!(
        valid_digest(Some(&archive)) && valid_digest(Some(&backup)),
        "both SHA-256 digests must have 64 hex digits"
    );
    let mut remote = Remote::from_args(&mut args)?;
    let [game]: [String; 1] = args
        .positional_for(ROLLBACK)?
        .try_into()
        .map_err(|_| anyhow::anyhow!(ROLLBACK))?;
    let reply: serde_json::Value = remote.post(
        &game,
        "rollback",
        serde_json::json!({"archive_sha256":archive,"backup_sha256":backup}),
    )?;
    println!("{reply}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(mode: Mode) -> Checkpoint {
        let mut s = Checkpoint::new("http://localhost".into(), "server".into(), mode);
        let items = ["published", "raw", "live", "empty"]
            .into_iter()
            .map(|k| Game {
                game_id: k.into(),
                state: k.into(),
                source_position: 12,
                chunks: 3,
                legacy_frames: 7,
            })
            .collect();
        s.page(Page {
            items,
            next: None,
            position: 14,
        })
        .unwrap();
        s
    }
    fn reply(done: bool) -> Reply {
        Reply {
            game_id: "raw".into(),
            state: if done { "published" } else { "staging" }.into(),
            done,
            next: (!done).then(|| "opaque".into()),
            source_position: 12,
            source_bytes: 70,
            archive_bytes: 20,
            frames: 4,
            segment: Some(serde_json::json!(0)),
            totals: Some(Totals {
                source_bytes: if done { 140 } else { 70 },
                archive_bytes: if done { 40 } else { 20 },
                frames: if done { 8 } else { 4 },
                segments: if done { 2 } else { 1 },
            }),
            archive_sha256: Some("a".repeat(64)),
            backup_sha256: Some("b".repeat(64)),
        }
    }
    #[test]
    fn published_skipped_on_apply_and_resumed_totals_count_once() {
        let mut s = state(Mode::Apply);
        assert_eq!(s.skipped, 3);
        assert_eq!(s.pending.len(), 1);
        s.accept(reply(false)).unwrap();
        let mut resumed: Checkpoint =
            serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(resumed.pending[0].cursor.as_deref(), Some("opaque"));
        resumed.accept(reply(true)).unwrap();
        assert!(resumed.completed());
        assert_eq!(resumed.totals.source_bytes, 140);
        assert_eq!(resumed.totals.frames, 8);
        assert_eq!(resumed.migrated, 1);
    }
    #[test]
    fn stale_or_incomplete_receipts_keep_the_original_revision_and_cursor() {
        let mut s = state(Mode::Apply);
        let mut stale = reply(true);
        stale.source_position = 13;
        assert!(s.accept(stale).is_err());
        let mut unverified = reply(true);
        unverified.backup_sha256 = None;
        assert!(s.accept(unverified).is_err());
        assert_eq!(s.pending[0].game.source_position, 12);
        assert_eq!(s.pending[0].cursor, None);
        assert_eq!(s.totals.frames, 0);
    }
    #[test]
    fn dry_run_accumulates_per_step_and_verify_visits_published() {
        let mut s = state(Mode::DryRun);
        for done in [false, true] {
            let mut r = reply(done);
            r.totals = None;
            r.archive_sha256 = None;
            r.backup_sha256 = None;
            s.accept(r).unwrap();
        }
        assert_eq!(s.totals.frames, 8);
        assert_eq!(s.totals.segments, 2);
        let v = state(Mode::Verify);
        assert!(v.pending.iter().any(|j| j.game.state == "published"));
        assert_eq!(v.pending.len(), 1);
        assert_eq!(v.unpublished, 1);
    }
}

#[cfg(test)]
mod server_test {
    use super::*;
    use live_proto::{game, sign::EventBody};
    use live_server::{ServerIdentity, store::NewEvent};
    use serde_json::json;

    #[test]
    fn migration_command_keeps_an_unreadable_signed_source_and_finishes_the_other_game() {
        let dir = std::env::temp_dir().join(format!(
            "h2-replay-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data_dir = dir.join("server");
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &data_dir,
            Default::default(),
        )
        .unwrap();
        let op = ServerIdentity::load(&data_dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut signed = Vec::new();
        for (id, source) in [
            (
                "historic",
                json!({"game_id":"historic","tick":0,"players":[{"index":0,"alive":true,"yaw":0.0,"pos":[0.1,-0.0,1.0/65536.0]}]}),
            ),
            ("broken", json!({"packed":"not base64","tick":0})),
        ] {
            let started = game::GameStarted {
                game_id: id.into(),
                host: op.author(),
                game: game::NewGame {
                    map: "lockout".into(),
                    mode: "multiplayer".into(),
                    ..Default::default()
                },
            };
            for (kind, payload) in [
                (game::GAME_STARTED, json!(started)),
                (game::GAME_FRAME, source),
                (game::GAME_ENDED, json!({"tick":2,"reason":"finished"})),
            ] {
                let body = EventBody::at_unix(
                    &op.key,
                    &op.author(),
                    &game::stream(id),
                    kind,
                    payload,
                    live_cmd::now(),
                );
                signed.push(NewEvent {
                    jws: body.sign(&op.key).into(),
                    jws_index: None,
                    body,
                });
            }
        }
        let rows = rt
            .block_on(server.live.store().append(signed, &[]))
            .unwrap();
        let seal = rows.last().unwrap().position;
        rt.block_on(async {
            let mut applied = server.live.applied();
            tokio::time::timeout(Duration::from_secs(5), applied.wait_for(|n| *n >= seal))
                .await
                .unwrap()
                .unwrap();
        });
        let command = |mode: &str| {
            let cp = dir.join(format!("{}.json", &mode[2..]));
            let result = live_cmd::run(vec![
                "frames".into(),
                "migrate".into(),
                mode.into(),
                "--server".into(),
                server.url.clone(),
                "--data".into(),
                data_dir.to_str().unwrap().into(),
                "--delay-ms".into(),
                "0".into(),
                "--checkpoint".into(),
                cp.to_str().unwrap().into(),
            ]);
            let progress: Checkpoint = serde_json::from_slice(&fs::read(cp).unwrap()).unwrap();
            (result, progress)
        };
        let (dry, plan) = command("--dry-run");
        assert!(dry.is_err());
        assert_eq!(plan.migrated, 1);
        assert_eq!(rt.block_on(server.live.store().head()).unwrap(), seal);
        let (apply, published) = command("--apply");
        assert!(apply.is_err());
        assert_eq!(published.migrated, 1);
        assert_eq!(published.failed[0].job.game.game_id, "broken");
        let applied_head = rt.block_on(server.live.store().head()).unwrap();
        let (retry, again) = command("--apply");
        assert!(retry.is_err());
        assert_eq!(again.migrated, 1);
        assert_eq!(
            rt.block_on(server.live.store().head()).unwrap(),
            applied_head
        );
        let (verify, done) = command("--verify");
        verify.unwrap();
        assert_eq!(done.totals.frames, 1);
        assert_eq!(done.unpublished, 1);
        let originals = rt
            .block_on(server.live.store().read(&live_server::store::Query {
                after: 0,
                limit: rows.len(),
                stream: None,
                types: vec![],
                exclude: vec![],
            }))
            .unwrap();
        for (before, after) in rows.iter().zip(&originals) {
            assert_eq!(before.to_wire(), after.to_wire());
        }
        assert_eq!(originals.len(), rows.len());
        drop(server);
        fs::remove_dir_all(dir).unwrap();
    }
}
