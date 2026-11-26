//! The second object of a Live game's upload: the host's film file
//! (`H2FILM4!`, docs/game/input-film.md "Layout"), beside the stats stream
//! the rest of this module ingests. Live stores it as the bytes it was sent.
//! It reads the 64-byte header, inflates the seed once to tie the file to
//! its game, and walks the block lengths and the trailer. No tick chunk,
//! start point or game start is inflated.
use axum::{
    body::{Body, to_bytes},
    http::StatusCode,
};
use live_proto::film::{
    PART_BYTES, SINGLE_UPLOAD_BYTES,
    sharing::{Compat, header_id},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use sha2_checkpoint::{Digest as _, Sha256};

use super::{ApiResult, Request, blob_error, too_large};
use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::blobs::{BlobError, BlobStore, Meta, Seen};
use crate::films::inspect::checkpoint::digest;
use crate::host_identity::{FilmReservation, lease_id};

/// The signed receipt: the stored film file of one game.
pub(in crate::films) const STORED: &str = "HostFilmFileStored";
/// Film files and their parts. Outside `films/<game>`, which the stats
/// stream's cleanup sweeps down to its one chosen object.
pub(in crate::films) const PREFIX: &str = "input-films/";

const MAGIC: &[u8; 8] = b"H2FILM4!";
const VERSION: u16 = 4;
const HEADER: usize = 64;
const MAX_SEED: usize = 1 << 20;
const FLAG_SEALED: u16 = 2;
const FLAG_START_POINT: u16 = 4;
const FLAG_GAME_START: u16 = 8;
const KIND_TICKS: u8 = 0;
const KIND_START: u8 = 2;
const KIND_TAIL: u8 = 3;
const KIND_GAME: u8 = 4;
/// `film::columns::FRAME_CAP`, `film::file::MAX_START` and
/// `film::file::MAX_GAME_START`.
const MAX_TICK_FRAME: u32 = 192 << 10;
const MAX_START: u32 = 4 << 20;
const MAX_GAME_START: u32 = 8 << 20;

/// What Live keeps of a stored film file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FilmFile {
    pub blob: String,
    pub size: u64,
    pub sha256: String,
    pub ticks: u32,
}

/// The one inflation the ingest makes. Tests count the calls.
pub(in crate::films) trait Inflate {
    fn inflate(&mut self, packed: &[u8], limit: usize) -> ApiResult<Vec<u8>>;
}

pub(in crate::films) struct Zstd;
impl Inflate for Zstd {
    fn inflate(&mut self, packed: &[u8], limit: usize) -> ApiResult<Vec<u8>> {
        crate::films::inspect::inflate_limited(packed, limit)
    }
}

/// A walk over the file's bytes as they arrive, in any number of pieces.
/// It survives between numbered parts in the staging cursor.
#[derive(Default, Serialize, Deserialize)]
pub(in crate::films) struct Scan {
    pos: u64,
    flags: u16,
    ticks: u32,
    trailer: u64,
    next_block: u64,
    prefix: Vec<u8>,
    chunks: u32,
    count: Option<u32>,
    count_bytes: Vec<u8>,
    #[serde(with = "digest")]
    content: Sha256,
    #[serde(with = "digest")]
    offsets: Sha256,
    #[serde(with = "digest")]
    listed: Sha256,
    /// An account's own film (`feed_account`): its film ID and what it
    /// needs to play, from the seed.
    #[serde(default)]
    account: Option<(String, Compat)>,
}

/// Whose film a scan checks: a Live game's, sent by its host, or an
/// account's own, shared into its film library (`films/shared`).
#[derive(Clone, Copy)]
enum Tie<'a> {
    Game(&'a str),
    Account,
}

/// The checked file: what goes into the receipt beside its key.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::films) struct Checked {
    pub size: u64,
    pub sha256: String,
    pub ticks: u32,
}

fn bad(m: &str) -> ApiError {
    invalid(format!("film file: {m}"))
}

impl Scan {
    /// The next piece of the file, which starts where the last one ended.
    /// The header and the seed come in the first piece.
    pub fn feed(&mut self, game: &str, bytes: &[u8], inflate: &mut (dyn Inflate + Send)) -> ApiResult<()> {
        self.walk(Tie::Game(game), bytes, inflate)
    }

    /// [`Scan::feed`] for a film an account shares into its library: the
    /// seed ties the file to the film ID its stats stream names, and must
    /// name no machine or account (`film::file::Seed::shared`).
    pub fn feed_account(&mut self, bytes: &[u8], inflate: &mut (dyn Inflate + Send)) -> ApiResult<()> {
        self.walk(Tie::Account, bytes, inflate)
    }

    /// An account film's ID and compatibility, once its seed was read.
    pub fn account(&self) -> Option<&(String, Compat)> {
        self.account.as_ref()
    }

    fn walk(&mut self, tie: Tie<'_>, bytes: &[u8], inflate: &mut (dyn Inflate + Send)) -> ApiResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let start = self.pos;
        let end = start + bytes.len() as u64;
        let mut p = start;
        if start == 0 {
            p = self.header(tie, bytes, inflate)?;
        }
        let at = |abs: u64| (abs - start) as usize;
        while p < end {
            if p < self.trailer {
                if p < self.next_block {
                    p = self.next_block.min(end);
                    continue;
                }
                let block = self.next_block;
                let take = (5 - self.prefix.len()).min((end - p) as usize);
                self.prefix.extend_from_slice(&bytes[at(p)..at(p) + take]);
                p += take as u64;
                if self.prefix.len() < 5 {
                    continue;
                }
                if block + 5 > self.trailer {
                    return Err(bad("a block runs past the trailer"));
                }
                let kind = self.prefix[0];
                let stored = u32::from_le_bytes(self.prefix[1..5].try_into().expect("four bytes"));
                let fits = match kind {
                    KIND_TICKS | KIND_TAIL => (1..=MAX_TICK_FRAME).contains(&stored),
                    KIND_START => {
                        self.flags & FLAG_START_POINT != 0 && (1..=MAX_START).contains(&stored)
                    }
                    KIND_GAME => {
                        self.flags & FLAG_GAME_START != 0 && (1..=MAX_GAME_START).contains(&stored)
                    }
                    _ => return Err(bad("a block of unknown kind")),
                };
                if !fits {
                    return Err(bad(
                        "a block has a bad size or a kind its header does not name",
                    ));
                }
                if kind == KIND_TICKS {
                    self.offsets.update(block.to_le_bytes());
                    self.chunks += 1;
                }
                self.next_block = block + 5 + u64::from(stored);
                if self.next_block > self.trailer {
                    return Err(bad("a block runs past the trailer"));
                }
                self.prefix.clear();
            } else if self.count.is_none() {
                if self.next_block != self.trailer || !self.prefix.is_empty() {
                    return Err(bad("the blocks do not end at the trailer"));
                }
                let take = (4 - self.count_bytes.len()).min((end - p) as usize);
                self.count_bytes
                    .extend_from_slice(&bytes[at(p)..at(p) + take]);
                p += take as u64;
                if self.count_bytes.len() == 4 {
                    let count =
                        u32::from_le_bytes(self.count_bytes[..].try_into().expect("four bytes"));
                    if count != self.chunks {
                        return Err(bad("the trailer disagrees with its blocks"));
                    }
                    self.count = Some(count);
                }
            } else {
                self.listed.update(&bytes[at(p)..]);
                p = end;
            }
        }
        self.content.update(bytes);
        self.pos = end;
        Ok(())
    }

    /// Check the header and the seed; returns where the blocks start.
    fn header(&mut self, tie: Tie<'_>, bytes: &[u8], inflate: &mut (dyn Inflate + Send)) -> ApiResult<u64> {
        let h = bytes
            .get(..HEADER)
            .ok_or_else(|| bad("the header must come in the first part"))?;
        let u16_at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().expect("four bytes"));
        if h[..8] != *MAGIC || u16_at(8) != VERSION {
            return Err(bad("not a version 4 film file"));
        }
        let flags = u16_at(10);
        if flags & FLAG_SEALED == 0 || flags & !(FLAG_SEALED | FLAG_START_POINT | FLAG_GAME_START) != 0
        {
            return Err(bad("the film is not sealed or has unknown flags"));
        }
        if h[12] == 0 || h[13] != 16 || u16_at(14) != 256 {
            return Err(bad("the film has other rate, seat or chunk limits"));
        }
        if h[48..64].iter().any(|&b| b != 0) {
            return Err(bad("the film names a dictionary"));
        }
        let seed_len = u32_at(36) as usize;
        let trailer = u64::from_le_bytes(h[40..48].try_into().expect("eight bytes"));
        let seed_end = (HEADER + seed_len) as u64;
        if seed_len == 0 || seed_len > MAX_SEED || seed_end > trailer {
            return Err(bad("the seed is empty, too large or past the trailer"));
        }
        let packed = bytes
            .get(HEADER..HEADER + seed_len)
            .ok_or_else(|| bad("the seed must come in the first part"))?;
        let raw = inflate.inflate(packed, MAX_SEED)?;
        let seed: Value = serde_json::from_slice(&raw).map_err(|_| bad("the seed is not JSON"))?;
        let film = seed["header"]["meta"]["id"]
            .as_str()
            .ok_or_else(|| bad("the seed names no film"))?;
        if h[16..32] != header_id(film) {
            return Err(bad("the seed is not this film's"));
        }
        match tie {
            Tie::Game(game) => {
                if seed["header"]["game_id"].as_str() != Some(game) {
                    return Err(bad("the film belongs to another game"));
                }
            }
            Tie::Account => self.account = Some((film.to_owned(), account_seed(&seed)?)),
        }
        self.flags = flags;
        self.ticks = u32_at(32);
        self.trailer = trailer;
        self.next_block = seed_end;
        Ok(seed_end)
    }
}

/// The seed of a film an account shares: no network address, lease, local
/// map path, host or account number (`film::file::Seed::shared`), and the
/// compatibility its library row shows.
fn account_seed(seed: &Value) -> ApiResult<Compat> {
    let header = &seed["header"];
    let descriptor = &seed["descriptor"];
    let named = |v: &Value| !v.is_null() && v.as_str() != Some("");
    if named(&header["xnet_addr"])
        || named(&header["lease_id"])
        || named(&descriptor["host_id"])
        || named(&descriptor["map"]["cache_path"])
        || descriptor["roster"]
            .as_array()
            .is_some_and(|seats| seats.iter().any(|seat| !seat["xuid"].is_null()))
    {
        return Err(bad("the seed names a machine or an account"));
    }
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    let compat = Compat {
        xbe: text(&header["xbe_sha256"]),
        contract: text(&header["guest_image"]["contract_sha256"]),
        abi: text(&header["runtime_stamp"]),
        hooks: text(&header["simulation_hooks"]),
    };
    if compat.xbe.is_empty() || compat.abi.is_empty() || compat.abi.len() > 64 {
        return Err(bad("the seed names no executable or simulation"));
    }
    if !live_proto::game::valid_id(header["meta"]["id"].as_str().unwrap_or_default()) {
        return Err(bad("the seed's film ID is not valid"));
    }
    Ok(compat)
}

impl Scan {
    /// The whole file has been fed: its trailer lists exactly the tick
    /// chunks the walk found, and it ends after them.
    pub fn finish(&self, size: u64) -> ApiResult<Checked> {
        let Some(count) = self.count else {
            return Err(bad("the file ends before its trailer"));
        };
        if self.pos != size || self.trailer + 4 + 8 * u64::from(count) != size {
            return Err(bad("the file does not end after its trailer"));
        }
        if self.offsets.clone().finalize() != self.listed.clone().finalize() {
            return Err(bad("the trailer lists other chunks"));
        }
        Ok(Checked {
            size,
            sha256: live_proto::sign::hex(&self.content.clone().finalize()),
            ticks: self.ticks,
        })
    }
}

/// A multipart upload's staging cursor.
#[derive(Serialize, Deserialize)]
struct Cursor {
    lease: Option<String>,
    blob: String,
    size: u64,
    hashes: Vec<String>,
    scan: Scan,
}

fn cursor_key(id: &str) -> String {
    format!("input-staging/{id}/film")
}

fn fresh_key(id: &str, multipart: bool) -> String {
    let suffix = if multipart { "parts" } else { "h2f" };
    format!("{PREFIX}{id}/{}.{suffix}", live_proto::sign::random_id())
}

/// Store a whole film file as sent, after checking it.
pub(in crate::films) async fn accept(
    store: &BlobStore,
    key: &str,
    game: &str,
    bytes: axum::body::Bytes,
    inflate: &mut (dyn Inflate + Send),
) -> ApiResult<Checked> {
    let mut scan = Scan::default();
    scan.feed(game, &bytes, inflate)?;
    let checked = scan.finish(bytes.len() as u64)?;
    store
        .put(key, bytes, &Meta::default())
        .await
        .map_err(blob_error)?;
    Ok(checked)
}

fn owner_lease(grant: &FilmReservation) -> Option<String> {
    grant.publisher().host_lease_id().map(str::to_owned)
}

/// The lease may write this game's film: its own game, or the one its
/// reservation names before the game starts.
fn check_owner(live: &Live, id: &str, grant: &FilmReservation) -> ApiResult<()> {
    let model = live.model.read();
    let lease = owner_lease(grant);
    let host = model
        .films
        .standalone_host_blob(id)
        .map(|(host, _)| host.to_owned())
        .or_else(|| model.games.get(id).map(|g| g.started.host.clone()));
    match host {
        Some(host) if lease_id(&host).map(str::to_owned) != lease => {
            Err(forbidden("the game belongs to another host"))
        }
        Some(_) => Ok(()),
        None if id == grant.canonical_film_id() => Ok(()),
        None => Err(not_found("no such assigned game")),
    }
}

fn published(live: &Live, id: &str) -> Option<FilmFile> {
    live.model.read().films.film_files.get(id).cloned()
}

/// Sign the receipt. The first film file of a game is the one kept.
async fn publish(live: &Live, id: &str, file: FilmFile) -> ApiResult<()> {
    let _guard = live.film_writes.lock().await;
    let version = {
        let model = live.model.read();
        if let Some(kept) = model.films.film_files.get(id) {
            return if kept.sha256 == file.sha256 {
                Ok(())
            } else {
                Err(conflict("another film file is stored for this game"))
            };
        }
        model.films.next_version
    };
    live.append_film(version, STORED, json!({"game": id, "file": file}))
        .await
}

fn same(kept: &FilmFile, size: u64, sha256: &str) -> ApiResult<StatusCode> {
    if kept.size == size && kept.sha256 == sha256 {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(conflict("another film file is stored for this game"))
    }
}

/// `?object=film` on the host film routes: a whole file in one POST, or
/// numbered parts and a completion, as the stats stream is sent. Every
/// accepted request answers 204; the film needs no continuation.
pub(super) async fn upload(
    live: &Live,
    store: &BlobStore,
    id: &str,
    grant: &FilmReservation,
    request: Request,
    body: Body,
    declared: Option<u64>,
) -> ApiResult<StatusCode> {
    check_owner(live, id, grant)?;
    let limit = match request {
        Request::Single => SINGLE_UPLOAD_BYTES,
        Request::Part(_) => PART_BYTES,
        Request::Complete(..) | Request::Continue => 0,
    };
    let bytes = to_bytes(body, limit as usize)
        .await
        .map_err(|_| too_large())?;
    if declared.is_some_and(|n| n != bytes.len() as u64) {
        return Err(invalid("the film upload was cut off"));
    }
    match request {
        Request::Single => {
            if bytes.is_empty() {
                return Err(invalid("empty film file"));
            }
            let key = fresh_key(id, false);
            if let Some(kept) = published(live, id) {
                let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
                return same(&kept, bytes.len() as u64, &sha);
            }
            let checked = accept(store, &key, id, bytes, &mut Zstd).await?;
            let file = FilmFile {
                blob: key.clone(),
                size: checked.size,
                sha256: checked.sha256,
                ticks: checked.ticks,
            };
            if let Err(error) = publish(live, id, file).await {
                let _ = store.delete(&key).await;
                return Err(error);
            }
            Ok(StatusCode::NO_CONTENT)
        }
        Request::Part(n) => part(live, store, id, grant, n, bytes).await,
        Request::Complete(size, sha256) => {
            if let Some(kept) = published(live, id) {
                return same(&kept, size, &sha256);
            }
            let (cursor, _) = load(store, id).await?;
            let cursor = cursor
                .filter(|c| c.lease == owner_lease(grant) && c.size == size)
                .ok_or_else(|| invalid("missing film file parts or wrong size"))?;
            let checked = cursor.scan.finish(size)?;
            if checked.sha256 != sha256 {
                return Err(invalid("film file digest mismatch"));
            }
            crate::films::parts::publish(store, &cursor.blob, size, &cursor.hashes)
                .await
                .map_err(blob_error)?;
            publish(
                live,
                id,
                FilmFile {
                    blob: cursor.blob,
                    size,
                    sha256,
                    ticks: checked.ticks,
                },
            )
            .await?;
            let _ = store.delete(&cursor_key(id)).await;
            Ok(StatusCode::NO_CONTENT)
        }
        Request::Continue => match published(live, id) {
            Some(_) => Ok(StatusCode::NO_CONTENT),
            None => Err(invalid("no staged film file")),
        },
    }
}

async fn load(store: &BlobStore, id: &str) -> ApiResult<(Option<Cursor>, Seen)> {
    match store.get_seen(&cursor_key(id)).await.map_err(blob_error)? {
        Some((bytes, seen)) => Ok((
            Some(serde_json::from_slice(&bytes).map_err(|_| invalid("invalid film file cursor"))?),
            seen,
        )),
        None => Ok((None, Seen::default())),
    }
}

async fn part(
    live: &Live,
    store: &BlobStore,
    id: &str,
    grant: &FilmReservation,
    n: u64,
    bytes: axum::body::Bytes,
) -> ApiResult<StatusCode> {
    if published(live, id).is_some() {
        // The completion that follows says whether it is the same file.
        return Ok(StatusCode::NO_CONTENT);
    }
    if bytes.is_empty() {
        return Err(invalid("empty film file part"));
    }
    let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    let lease = owner_lease(grant);
    let (previous, seen) = load(store, id).await?;
    let mut cursor = match previous {
        Some(c) if c.lease == lease && (n != 0 || c.hashes.first() == Some(&digest)) => c,
        Some(c) if n != 0 && c.lease != lease => {
            return Err(forbidden(
                "the film file belongs to another host reservation",
            ));
        }
        _ if n == 0 => Cursor {
            lease,
            blob: fresh_key(id, true),
            size: 0,
            hashes: Vec::new(),
            scan: Scan::default(),
        },
        _ => return Err(conflict("send film file part zero first")),
    };
    if let Some(stored) = cursor.hashes.get(n as usize) {
        return if *stored == digest {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(conflict("film file part differs from the accepted bytes"))
        };
    }
    if n as usize != cursor.hashes.len() || cursor.size != n * PART_BYTES {
        return Err(conflict("send consecutive film file parts"));
    }
    cursor.scan.feed(id, &bytes, &mut Zstd)?;
    let size = bytes.len() as u64;
    store
        .put(
            &crate::films::parts::part_key(&cursor.blob, n, &digest),
            bytes,
            &Meta::default(),
        )
        .await
        .map_err(blob_error)?;
    cursor.size += size;
    cursor.hashes.push(digest);
    let saved = serde_json::to_vec(&cursor).map_err(|_| invalid("encoding film file cursor"))?;
    store
        .put_if(&cursor_key(id), saved.into(), &seen, &Meta::default())
        .await
        .map_err(|e| match e {
            BlobError::Changed(_) => conflict("film file upload advanced on another server; retry"),
            e => blob_error(e),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
pub(in crate::films) mod tests;
