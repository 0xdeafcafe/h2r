//! The admin role and the admin-only title-file editors (docs/live/live-admin.md).
//!
//! An admin may publish title files (the MOTD and the hopper/playlist file)
//! where otherwise only the server operator's key can. Admins come from two
//! places:
//!
//! - **Configuration**, read once at startup: `h2 live serve --admin
//!   <gamertag>` (repeatable) plus `$H2_LIVE_ADMINS` (comma-separated). No
//!   gamertag is an admin by default; there are no built-in admin names.
//!   Configuration cannot be revoked at runtime.
//! - **The log**: `h2 live admin grant|revoke <gamertag>` appends
//!   server-signed [`ADMIN_GRANTED`]/[`ADMIN_REVOKED`] events on the
//!   account stream, which [`Admins`] projects in memory (and replays at
//!   startup, so grants survive a restart).
//!
//! The embedded server `h2 play --live-local` starts also makes the account it is
//! about to play as an admin for its lifetime ([`Admins::grant_runtime`]):
//! it is the owner's own machine.
//!
//! The HTTP handlers here are wired in `http.rs`; the JSON they speak is
//! the editable fields only — the binary formats live in `live_proto`
//! (`motd`, `hopper`) and every byte the editors do not model is kept from
//! the published file, so editing is lossless.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use live_proto::hopper::clankers::ClankerHoppers;
use live_proto::hopper::defaults::{self, RETAIL_MAPS};
use live_proto::hopper::variant::VariantRecord;
use live_proto::hopper::{
    Descriptor, Entry, HALO_2_HOPPER_FILE, HOPPERS, Hopper, HopperError, HopperFile, HopperText,
    MAX_RECORDS, RECORD, Record, VARIANT_AT, check_descriptor, set_slot_entry,
};
use live_proto::motd::Motd;
use live_proto::sign;
use live_proto::{
    Author, ErrorCode, Gamertag, HALO_2_MOTD_FILE, PublishedFile, TitleId, WireEvent, Xuid, lxm,
    streams, types, valid_file_name,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::app::{ApiError, Live, MAX_TITLE_FILE, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::model::State as ModelState;
use crate::store::RecordedEvent;

/// `AdminGranted` event type, on the account stream of the account made an
/// admin. Server-signed; written by [`grant`].
pub const ADMIN_GRANTED: &str = "AdminGranted";
/// `AdminRevoked` event type, on the account stream of the account that
/// stops being an admin.
pub const ADMIN_REVOKED: &str = "AdminRevoked";
/// The token method `h2 live admin grant|revoke` signs (operator only).
pub const LXM_ADMIN_WRITE: &str = "admin.write";
/// The environment variable `h2 live serve` and the embedded server read
/// configured admin gamertags from.
pub const ENV_ADMINS: &str = "H2_LIVE_ADMINS";

type ApiResult<T> = Result<T, ApiError>;

fn forbidden(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Forbidden, m)
}

fn conflict(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, m)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn body_bytes(body: Result<Bytes, BytesRejection>) -> ApiResult<Bytes> {
    body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::new(ErrorCode::TooLarge, "request body too large")
        }
        _ => invalid(e.body_text()),
    })
}

fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    (status, axum::Json(body)).into_response()
}

fn hopper_error(e: HopperError) -> ApiError {
    invalid(e.to_string())
}

/// The admin set: gamertags configured at startup, plus accounts granted
/// through the log. Cheap to read (an in-memory map), applied by the
/// dispatcher like the read model is.
pub struct Admins {
    /// Lowercased gamertag -> display spelling.
    configured: BTreeMap<String, String>,
    /// Xuids granted through events (or, for the embedded server's playing
    /// account, at runtime), with their gamertags for messages.
    granted: RwLock<HashMap<Xuid, String>>,
}

impl Admins {
    pub fn new(configured: Vec<Gamertag>) -> Self {
        Admins {
            configured: configured
                .into_iter()
                .map(|t| (t.key(), t.to_string()))
                .collect(),
            granted: RwLock::new(HashMap::new()),
        }
    }

    /// Parses and merges admin gamertags from `H2_LIVE_ADMINS`
    /// (comma-separated, trimmed) and the `--admin` flags. Case-insensitive
    /// duplicates collapse; the first spelling is kept. An invalid gamertag
    /// is an error, so a typo cannot silently widen who is an admin.
    pub fn from_flags_and_env(flags: &[String]) -> Result<Vec<Gamertag>, String> {
        let mut specs: Vec<String> = std::env::var(ENV_ADMINS)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        specs.extend(flags.iter().cloned());
        let mut seen: BTreeMap<String, Gamertag> = BTreeMap::new();
        for spec in specs {
            let tag = Gamertag::new(&spec).map_err(|e| format!("{spec:?}: {e}"))?;
            seen.entry(tag.key()).or_insert(tag);
        }
        Ok(seen.into_values().collect())
    }

    /// The configured admin gamertags, as spelled, alphabetically (the
    /// startup notice, and `GET /v1/admins`).
    pub fn configured(&self) -> Vec<String> {
        self.configured.values().cloned().collect()
    }

    /// Whether `tag` is an admin by startup configuration alone.
    pub fn is_configured(&self, tag: &Gamertag) -> bool {
        self.configured.contains_key(&tag.key())
    }

    fn is_configured_str(&self, tag: &str) -> bool {
        self.configured.contains_key(&tag.to_ascii_lowercase())
    }

    /// Makes `xuid` an admin until the process stops, without an event:
    /// the embedded server's playing account, which is re-granted on every
    /// `h2 play --live-local` (docs/live/live-admin.md).
    pub fn grant_runtime(&self, xuid: Xuid, tag: &str) {
        self.granted
            .write()
            .unwrap()
            .entry(xuid)
            .or_insert_with(|| tag.to_string());
    }

    /// Whether this author is an admin. The server key always is; an
    /// account is when its gamertag is configured or its xuid granted.
    pub fn is_admin(&self, author: &Author, s: &ModelState) -> bool {
        match author {
            Author::Server(_) => true,
            Author::Account { xuid, .. } => self.is_admin_xuid(*xuid, s),
        }
    }

    /// Whether account `xuid` (whose gamertag the state supplies) is an
    /// admin.
    pub fn is_admin_xuid(&self, xuid: Xuid, s: &ModelState) -> bool {
        self.granted.read().unwrap().contains_key(&xuid)
            || s.accounts
                .get(&xuid)
                .is_some_and(|a| self.is_configured(&a.gamertag))
    }

    /// Whether the account with this gamertag and xuid is an admin.
    pub fn is_admin_of(&self, tag: &Gamertag, xuid: Xuid) -> bool {
        self.granted.read().unwrap().contains_key(&xuid) || self.is_configured(tag)
    }

    /// The granted admins' xuids and gamertags, sorted by gamertag.
    pub fn granted_list(&self) -> Vec<(Xuid, String)> {
        let mut v: Vec<_> = self
            .granted
            .read()
            .unwrap()
            .iter()
            .map(|(x, t)| (*x, t.clone()))
            .collect();
        v.sort_by_key(|a| a.1.to_lowercase());
        v
    }
}

impl Subscriber for Admins {
    fn name(&self) -> &str {
        "admins"
    }

    fn apply(&self, event: &RecordedEvent) {
        let body = &event.body;
        let Some(xuid) = body
            .data
            .get("xuid")
            .and_then(Value::as_str)
            .and_then(|s| Xuid::parse(s).ok())
        else {
            return;
        };
        if body.stream != streams::account(xuid) {
            return;
        }
        match body.kind.as_str() {
            ADMIN_GRANTED => {
                let tag = body
                    .data
                    .get("gamertag")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.granted.write().unwrap().insert(xuid, tag);
            }
            ADMIN_REVOKED => {
                self.granted.write().unwrap().remove(&xuid);
            }
            _ => {}
        }
    }
}

/// `POST /v1/admins/{gamertag}`: the operator makes an account an admin.
/// Appends a server-signed [`ADMIN_GRANTED`] event on the account stream.
pub async fn grant(
    live: &Live,
    bearer: Option<&str>,
    tag: &Gamertag,
) -> Result<WireEvent, ApiError> {
    operator(live, bearer)?;
    if live.admins.is_configured(tag) {
        return Err(conflict(format!(
            "{tag} is an admin by configuration (--admin or {ENV_ADMINS}); nothing to grant"
        )));
    }
    let xuid = account_xuid(live, tag)?;
    if live.admins.is_admin_of(tag, xuid) {
        return Err(conflict(format!("{tag} is already an admin")));
    }
    append_admin_event(live, xuid, tag, ADMIN_GRANTED).await
}

/// `DELETE /v1/admins/{gamertag}`: the operator ends a grant. Configuration
/// cannot be revoked at runtime.
pub async fn revoke(
    live: &Live,
    bearer: Option<&str>,
    tag: &Gamertag,
) -> Result<WireEvent, ApiError> {
    operator(live, bearer)?;
    if live.admins.is_configured(tag) {
        return Err(conflict(format!(
            "{tag} is an admin by configuration (--admin or {ENV_ADMINS}); remove it there and restart"
        )));
    }
    let xuid = account_xuid(live, tag)?;
    if !live.admins.is_admin_of(tag, xuid) {
        return Err(conflict(format!("{tag} is not an admin")));
    }
    append_admin_event(live, xuid, tag, ADMIN_REVOKED).await
}

/// The operator: a token signed by the server key.
pub(crate) fn operator(live: &Live, bearer: Option<&str>) -> Result<Author, ApiError> {
    let author = live.authenticate(bearer, LXM_ADMIN_WRITE)?;
    if !matches!(author, Author::Server(_)) {
        return Err(forbidden(
            "only the server operator grants and revokes admins",
        ));
    }
    live.rate_limit(&author, LXM_ADMIN_WRITE)?;
    Ok(author)
}

fn account_xuid(live: &Live, tag: &Gamertag) -> Result<Xuid, ApiError> {
    live.model
        .read()
        .account_by_tag(tag)
        .map(|a| a.xuid)
        .ok_or_else(|| not_found("no such account"))
}

async fn append_admin_event(
    live: &Live,
    xuid: Xuid,
    tag: &Gamertag,
    kind: &str,
) -> Result<WireEvent, ApiError> {
    let event = live.server_event(
        streams::account(xuid),
        kind,
        serde_json::json!({"xuid": xuid, "gamertag": tag}),
    );
    let recorded = live.store().append(vec![event], &[]).await?;
    let event = recorded.into_iter().next().expect("one appended");
    live.wait_applied(event.position).await?;
    Ok(event.to_wire())
}

/// `GET /v1/admins`: the configured and granted admins, for the web app and
/// `h2 live q`. Public unless the server is `--private`, like other reads.
pub async fn list_admins(State(live): State<Arc<Live>>, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let items: Vec<Value> = live
        .admins
        .configured()
        .into_iter()
        .map(|tag| serde_json::json!({"gamertag": tag, "source": "configured"}))
        .chain(
            live.admins
                .granted_list()
                .into_iter()
                .filter_map(|(xuid, tag)| {
                    (!live.admins.is_configured_str(&tag)).then(
                        || serde_json::json!({"gamertag": tag, "xuid": xuid, "source": "granted"}),
                    )
                }),
        )
        .collect();
    Ok(json(
        StatusCode::OK,
        &serde_json::json!({"items": items, "position": live.position()}),
    ))
}

/// Whether the authenticated author may publish title files: the server
/// operator (a server-key token) or an admin account (self-issued).
pub fn require_admin(live: &Live, author: &Author) -> Result<(), ApiError> {
    if live.admins.is_admin(author, &live.model.read()) {
        Ok(())
    } else {
        Err(forbidden(
            "publishing title files needs the operator or an admin",
        ))
    }
}

/// The tail of `Live::publish_file`: store the blob, append the
/// server-signed `TitleFilePublished` event, wait for the read model.
/// Callers have already checked who may publish.
pub async fn publish(
    live: &Live,
    title: TitleId,
    name: &str,
    bytes: &[u8],
) -> Result<PublishedFile, ApiError> {
    if !valid_file_name(name) {
        return Err(invalid("file names are [A-Za-z0-9_.-]{1,64}"));
    }
    if bytes.len() > MAX_TITLE_FILE {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            "title files are at most 2 MiB",
        ));
    }
    let meta = PublishedFile {
        title_id: title,
        name: name.into(),
        sha256: sign::hex(&Sha256::digest(bytes)),
        size: bytes.len(),
    };
    live.store().put_blob(&meta.sha256, bytes).await?;
    let data = serde_json::to_value(&meta).expect("serializes");
    let event = live.server_event(
        streams::title_file(title, name),
        types::TITLE_FILE_PUBLISHED,
        data,
    );
    let recorded = live.store().append(vec![event], &[]).await?;
    live.wait_applied(recorded[0].position).await?;
    Ok(meta)
}

// ---------- the MOTD editor ----------

/// The MOTD as JSON: the editable fields only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotdDoc {
    pub header: String,
    pub messages: Vec<String>,
}

impl From<Motd> for MotdDoc {
    fn from(m: Motd) -> Self {
        MotdDoc {
            header: m.header,
            messages: m.messages,
        }
    }
}

impl MotdDoc {
    fn motd(&self) -> Motd {
        Motd {
            header: self.header.clone(),
            messages: self.messages.clone(),
        }
    }
}

fn title_id(raw: &str) -> ApiResult<TitleId> {
    TitleId::parse(raw).map_err(|_| invalid("title ids are 8 hex digits"))
}

/// The published file for `title`, or a 404 that says which file is missing.
async fn published_file(
    live: &Live,
    title: TitleId,
    name: &str,
) -> Result<(PublishedFile, Vec<u8>), ApiError> {
    match live.file(title, name).await {
        Ok(found) => Ok(found),
        Err(e) if e.code == ErrorCode::NotFound => {
            Err(not_found(format!("no {name} published for this title")))
        }
        Err(e) => Err(e),
    }
}

/// `GET /v1/titles/{title}/motd`: the published message of the day as text.
pub async fn get_motd(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(title): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let title = title_id(&title)?;
    let (_, bytes) = published_file(&live, title, HALO_2_MOTD_FILE).await?;
    let motd = Motd::parse_legacy(&bytes).map_err(|e| {
        invalid(format!(
            "the published file does not parse ({e}); republish it"
        ))
    })?;
    Ok(json(StatusCode::OK, &MotdDoc::from(motd)))
}

/// `PUT /v1/titles/{title}/motd`: admin-only. Builds the binary from the
/// text and publishes it exactly as the operator path does.
pub async fn put_motd(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(title): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let title = title_id(&title)?;
    let author = live.authenticate(bearer(&headers), lxm::TITLES_WRITE)?;
    require_admin(&live, &author)?;
    live.rate_limit(&author, lxm::TITLES_WRITE)?;
    let bytes = body_bytes(body)?;
    let doc: MotdDoc = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("motd: {e}")))?;
    let built = doc.motd().build().map_err(|e| invalid(e.to_string()))?;
    let published = publish(&live, title, HALO_2_MOTD_FILE, &built).await?;
    Ok(json(
        StatusCode::OK,
        &serde_json::json!({"published": published, "motd": doc}),
    ))
}

// ---------- the hopper (playlist) builder ----------

/// One hopper as JSON, with its records. `type` is the hopper type 1..5,
/// or 0 for an unused slot (`unused`: no hopper to the game; create one
/// with `template` in the PUT body).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HopperDoc {
    pub index: usize,
    #[serde(rename = "type")]
    pub hopper_type: u32,
    pub unused: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_map_pack_maps: bool,
    pub status: u32,
    pub record_count: u32,
    pub name: String,
    /// The title and description the playlist list shows (English slot).
    pub title: String,
    pub description: String,
    pub ranges: [u32; 3],
    pub dwords: Vec<u32>,
    pub records: Vec<RecordDoc>,
}

/// One record of a hopper: the fields the builder edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordDoc {
    pub id: u32,
    pub string: String,
    pub flags: u32,
    pub dwords: [u32; 2],
    /// What the record's game variant says about itself (read-only).
    #[serde(default)]
    pub variant: Option<VariantDoc>,
    /// The key of the built-in variant ([`BuiltinVariantDoc`]) whose bytes
    /// the record's variant is exactly, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
}

/// A record's game variant: name, game-engine index, team play.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantDoc {
    pub name: String,
    pub engine: u32,
    pub team: bool,
}

/// A built-in game variant an entry can be given by `key` (`variant` in a
/// record of the PUT body): the stock record of its game type with the
/// default lineup's changes ([`defaults::builtin_variants`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuiltinVariantDoc {
    pub key: String,
    pub name: String,
    pub engine: u32,
    pub team: bool,
}

/// A playlist template (`template` in the PUT body): a default playlist
/// whose descriptor a new playlist takes ([`defaults::templates`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateDoc {
    /// The default playlist's id; the new playlist gets a free one.
    pub id: u32,
    #[serde(rename = "type")]
    pub hopper_type: u32,
    pub title: String,
    pub description: String,
    pub min_players: Option<u32>,
    pub max_players: Option<u32>,
    /// The descriptor's fields as [`HopperDoc`] has them, so a builder's
    /// working copy of a new playlist is whole (and sending them back
    /// changes nothing).
    pub ranges: [u32; 3],
    pub dwords: Vec<u32>,
    /// The template's first entry: a map id and a built-in variant key, a
    /// valid first entry for a new playlist of this kind.
    pub entry: TemplateEntryDoc,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateEntryDoc {
    pub id: u32,
    pub variant: String,
}

/// A retail multiplayer map by its entry id ([`RETAIL_MAPS`]): the
/// builder's map list when the owner's catalog is not there.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapDoc {
    pub id: u32,
    pub name: String,
}

/// `GET /v1/titles/{title}/hopper`: the hoppers, and what the builder can
/// pick from without the owner's catalog: the built-in variants, the
/// playlist templates and the retail maps.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HoppersDoc {
    pub hoppers: Vec<HopperDoc>,
    pub variants: Vec<BuiltinVariantDoc>,
    pub templates: Vec<TemplateDoc>,
    pub maps: Vec<MapDoc>,
    pub position: u64,
}

/// The built-in variants, as [`HoppersDoc::variants`] lists them.
pub fn builtin_variant_docs() -> Vec<BuiltinVariantDoc> {
    defaults::builtin_variants()
        .into_iter()
        .map(|(key, v)| BuiltinVariantDoc {
            key: key.into(),
            name: v.name(),
            engine: v.engine(),
            team: v.team(),
        })
        .collect()
}

/// The playlist templates, as [`HoppersDoc::templates`] lists them.
pub fn template_docs() -> Vec<TemplateDoc> {
    let mut scratch = HopperFile::blank();
    defaults::templates()
        .into_iter()
        .filter_map(|t| {
            let d = t.descriptor();
            let first = t.entries().into_iter().next()?;
            scratch.create_from(0, &t).ok()?;
            let h = scratch.hopper(0).ok()?;
            Some(TemplateDoc {
                id: t.playlist_id,
                hopper_type: d.hopper_type,
                title: t.title.into(),
                description: t.description.into(),
                min_players: d.min_players(),
                max_players: d.max_players(),
                ranges: h.ranges,
                dwords: h.dwords,
                entry: TemplateEntryDoc {
                    id: first.map_id,
                    variant: defaults::builtin_key(&first.variant)?.into(),
                },
            })
        })
        .collect()
}

fn record_builtin(file: &HopperFile, i: usize, j: usize) -> Option<String> {
    let slot = file.slot(i, j).ok()?;
    let v = VariantRecord::from_bytes(&slot[VARIANT_AT..RECORD])?;
    defaults::builtin_key(&v).map(String::from)
}

fn hoppers_doc(live: &Live, file: &HopperFile) -> ApiResult<HoppersDoc> {
    let hoppers = (0..HOPPERS)
        .map(|i| {
            let h = file.hopper(i).map_err(hopper_error)?;
            let text = file.text(i).map_err(hopper_error)?;
            let records = file
                .records(i)
                .map_err(hopper_error)?
                .into_iter()
                .enumerate()
                .map(|(j, r)| {
                    let v = file.variant(i, j).map_err(hopper_error)?;
                    Ok(RecordDoc {
                        id: r.id,
                        string: r.string,
                        flags: r.flags,
                        dwords: r.dwords,
                        variant: Some(VariantDoc {
                            name: v.name,
                            engine: v.engine,
                            team: v.team,
                        }),
                        builtin: record_builtin(file, i, j),
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            Ok(HopperDoc {
                index: i,
                hopper_type: h.hopper_type,
                unused: h.hopper_type == 0,
                has_map_pack_maps: file.contains_addon_maps(i).map_err(hopper_error)?,
                status: h.status,
                record_count: h.record_count,
                name: h.name,
                title: text.title,
                description: text.description,
                ranges: h.ranges,
                dwords: h.dwords,
                records,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(HoppersDoc {
        hoppers,
        variants: builtin_variant_docs(),
        templates: template_docs(),
        maps: RETAIL_MAPS
            .iter()
            .map(|(id, name)| MapDoc {
                id: *id,
                name: (*name).into(),
            })
            .collect(),
        position: live.position(),
    })
}

/// `PUT /v1/titles/{title}/hopper` body: the hoppers to change. Hoppers not
/// listed keep their bytes; within a listed one, fields left out keep
/// theirs. An unused slot (type 0) is made a playlist with `template`, and
/// a playlist made an unused slot again with `clear`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HoppersPatch {
    pub hoppers: Vec<HopperPatch>,
}

/// One hopper's changes. `records`, when present, replaces the whole list:
/// every entry is written into slot order, taking its unknown bytes (the
/// template blobs the game fills in) from `from` — the slot the record
/// comes from in the file as it stands — or from its own position when
/// `from` is absent. That is how reorders, removals and copies keep every
/// byte the builder cannot edit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HopperPatch {
    pub index: usize,
    /// `type`, as the GET names it (`hopper_type` is read too).
    #[serde(default, rename = "type", alias = "hopper_type")]
    pub hopper_type: Option<u32>,
    /// Makes the hopper a playlist of this template's kind first
    /// ([`TemplateDoc`]; its id): the whole descriptor, with a free
    /// playlist id. Required to fill an unused slot; the other fields then
    /// apply on top.
    #[serde(default)]
    pub template: Option<u32>,
    /// Makes the hopper an unused slot (descriptor and entries zeroed).
    /// Nothing else may be sent with it.
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub status: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    /// Written into every language slot.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub ranges: Option<[u32; 3]>,
    #[serde(default)]
    pub dwords: Option<Vec<u32>>,
    #[serde(default)]
    pub record_count: Option<u32>,
    #[serde(default)]
    pub records: Option<Vec<RecordPatch>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecordPatch {
    pub id: u32,
    pub string: String,
    pub flags: u32,
    pub dwords: [u32; 2],
    /// The current slot whose whole bytes this entry keeps (0-based, into
    /// the hopper's records as they are now). Absent: this entry's own
    /// position.
    #[serde(default)]
    pub from: Option<usize>,
    /// With `from`: the hopper that slot is in (the file as it stood before
    /// this patch). Absent: this hopper. How a record, game variant and all,
    /// is copied from one playlist into another.
    #[serde(default)]
    pub from_hopper: Option<usize>,
    /// A built-in variant's key ([`BuiltinVariantDoc`]): the entry is
    /// built afresh from the map id (`id`), the weight (`dwords[0]`) and
    /// that variant. `string`, `flags` and `dwords[1]` lie inside the
    /// variant and are ignored; `from` may not be given with it.
    #[serde(default)]
    pub variant: Option<String>,
}

/// Applies a patch to the parsed file. Pure, so it is easy to test; the
/// handler publishes the result. This is the one place a hopper edit is
/// checked, for the native server and the Worker alike (both serve this
/// crate's router). Hoppers the patch does not name are not checked: an
/// unused slot stays unused, whatever its zeros.
pub fn apply_patch(file: &mut HopperFile, patch: &HoppersPatch) -> ApiResult<()> {
    let original = file.clone();
    let mut seen = [false; HOPPERS];
    for h in &patch.hoppers {
        if h.index >= HOPPERS {
            return Err(invalid(format!(
                "hopper index {} is out of range (there are 16)",
                h.index
            )));
        }
        if seen[h.index] {
            return Err(invalid(format!("hopper {} appears twice", h.index)));
        }
        seen[h.index] = true;
        let n = h.index + 1;
        if h.clear {
            let other = h.template.is_some()
                || h.records.is_some()
                || h.hopper_type.is_some()
                || h.title.is_some();
            if other {
                return Err(invalid(format!(
                    "playlist {n}: `clear` empties the slot; send nothing else with it"
                )));
            }
            file.clear_hopper(h.index).map_err(hopper_error)?;
            continue;
        }
        // A descriptor the game took before must still be one it takes.
        let before = original.descriptor(h.index).map_err(hopper_error)?;
        let mut keep_valid = before.hopper_type != 0 && check_descriptor(&before).is_empty();
        if let Some(id) = h.template {
            let t = defaults::template(id).ok_or_else(|| {
                invalid(format!("playlist {n}: there is no playlist template {id}"))
            })?;
            file.create_from(h.index, &t).map_err(hopper_error)?;
            keep_valid = true;
            if h.records.is_none() && file.block_record_count(h.index).map_err(hopper_error)? == 0 {
                return Err(invalid(format!(
                    "playlist {n}: a new playlist needs at least one entry"
                )));
            }
        } else if file.is_unused(h.index).map_err(hopper_error)? {
            return Err(invalid(format!(
                "playlist {n} is an unused slot: create it from a template (`template`) first"
            )));
        }
        let current = file.hopper(h.index).map_err(hopper_error)?;
        let mut next = Hopper {
            hopper_type: h.hopper_type.unwrap_or(current.hopper_type),
            status: h.status.unwrap_or(current.status),
            record_count: h.record_count.unwrap_or(current.record_count),
            name: h.name.clone().unwrap_or(current.name),
            ranges: h.ranges.unwrap_or(current.ranges),
            dwords: h.dwords.clone().unwrap_or(current.dwords),
        };
        if let Some(records) = &h.records {
            let n = records.len();
            if !(1..=MAX_RECORDS).contains(&n) {
                return Err(invalid(format!(
                    "hopper {} needs 1 to 100 records, got {}",
                    h.index, n
                )));
            }
            if let Some(c) = h.record_count
                && c as usize != n
            {
                return Err(invalid(format!(
                    "hopper {} says record_count {c} but lists {n} records",
                    h.index
                )));
            }
            if !records.iter().any(|record| live_proto::hopper::is_base_map(record.id)) {
                return Err(invalid(format!(
                    "Playlist {n} needs a Halo 2 disc map for players without the Map Pack."
                )));
            }
            // The slots as they stood, so `from` names them.
            let mut bytes = Vec::with_capacity(n);
            for (j, r) in records.iter().enumerate() {
                if let Some(key) = &r.variant {
                    bytes.push(builtin_slot(h.index, j, r, key)?);
                    continue;
                }
                let hopper = r.from_hopper.unwrap_or(h.index);
                if hopper >= HOPPERS {
                    return Err(invalid(format!(
                        "from_hopper {hopper} is out of range (there are 16)"
                    )));
                }
                let src = r.from.unwrap_or(j);
                let old = original.hopper(hopper).map_err(hopper_error)?.record_count as usize;
                if src >= old.min(MAX_RECORDS) {
                    return Err(invalid(format!(
                        "from: slot {src} is not one of hopper {hopper}'s {old} records"
                    )));
                }
                bytes.push(original.slot(hopper, src).map_err(hopper_error)?.to_vec());
            }
            for (j, b) in bytes.iter().enumerate() {
                file.set_slot(h.index, j, b).map_err(hopper_error)?;
            }
            file.set_record_count(h.index, n as u32)
                .map_err(hopper_error)?;
            for (j, r) in records.iter().enumerate() {
                // A built-in variant's entry is whole already: its raw
                // fields are the variant's own bytes.
                if r.variant.is_some() {
                    continue;
                }
                file.set_record(
                    h.index,
                    j,
                    &Record {
                        id: r.id,
                        string: r.string.clone(),
                        flags: r.flags,
                        dwords: r.dwords,
                    },
                )
                .map_err(hopper_error)?;
            }
            next.record_count = n as u32;
        }
        file.set_hopper(h.index, &next).map_err(hopper_error)?;
        if h.title.is_some() || h.description.is_some() {
            let now = file.text(h.index).map_err(hopper_error)?;
            let text = HopperText {
                title: h.title.clone().unwrap_or(now.title),
                description: h.description.clone().unwrap_or(now.description),
            };
            file.set_text(h.index, &text).map_err(hopper_error)?;
        }
        if h.records.is_none()
            && let Some(c) = h.record_count
        {
            file.set_record_count(h.index, c).map_err(hopper_error)?;
        }
        if keep_valid {
            let d = file.descriptor(h.index).map_err(hopper_error)?;
            let problems = check_descriptor(&d);
            if !problems.is_empty() {
                let what: Vec<String> = problems.into_iter().map(|(_, w)| w).collect();
                return Err(invalid(format!(
                    "playlist {n}: the game would not load this playlist: {}",
                    what.join("; ")
                )));
            }
        }
    }
    Ok(())
}

/// The 0x37c bytes of entry `j` of hopper `i` built from a built-in
/// variant: map id, weight and variant, every other byte zero (as the
/// default set writes its entries).
fn builtin_slot(i: usize, j: usize, r: &RecordPatch, key: &str) -> ApiResult<Vec<u8>> {
    let at = || format!("playlist {} entry {}", i + 1, j + 1);
    if r.from.is_some() || r.from_hopper.is_some() {
        return Err(invalid(format!(
            "{}: `variant` builds the entry afresh; `from` cannot be given with it",
            at()
        )));
    }
    let variant = defaults::builtin_variant(key)
        .ok_or_else(|| invalid(format!("{}: there is no built-in variant {key:?}", at())))?;
    let mut slot = vec![0; RECORD];
    set_slot_entry(
        &mut slot,
        &Entry {
            map_id: r.id,
            weight: r.dwords[0],
            map_name: String::new(),
            variant,
        },
    )
    .map_err(hopper_error)?;
    Ok(slot)
}

/// `GET /v1/titles/{title}/hopper`: the playlists as JSON. Halo 2 with no
/// file published reads the built-in lineup ([`crate::default_hopper`]);
/// other titles 404 until a file is imported.
pub async fn get_hopper(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(title): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let title = title_id(&title)?;
    let (_, bytes) = published_file(&live, title, HALO_2_HOPPER_FILE)
        .await
        .map_err(|e| {
            if e.code == ErrorCode::NotFound {
                not_found(format!(
                    "no {HALO_2_HOPPER_FILE} published for this title; import a base image \
                     with `h2 live hopper import` first"
                ))
            } else {
                e
            }
        })?;
    let file = HopperFile::parse_legacy(&bytes).map_err(|e| {
        invalid(format!(
            "the published {HALO_2_HOPPER_FILE} does not parse ({e}); import a base image"
        ))
    })?;
    Ok(json(StatusCode::OK, &hoppers_doc(&live, &file)?))
}

/// `GET /v1/hoppers/clankers`: which of Halo 2's served hoppers are
/// clanker hoppers (docs/game/bots.md §9.4 item 1), by descriptor index.
/// The marks are the default lineup's
/// ([`live_proto::hopper::defaults::Playlist::clankers`]), kept where the
/// file served now (the published one, or the built-in lineup) has the same
/// playlist id at that index. No file, or one that does not parse, has
/// none. Read like the pins; the file is read as ranking reads it
/// ([`Live::hoppers`]).
pub async fn get_clankers(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let file = live.hoppers().await;
    Ok(json(StatusCode::OK, &clanker_hoppers(file.as_ref())))
}

/// The clanker marks of a served hopper file ([`get_clankers`]); none
/// without one.
pub fn clanker_hoppers(file: Option<&HopperFile>) -> ClankerHoppers {
    let Some(file) = file else {
        return ClankerHoppers::default();
    };
    let served: Vec<Descriptor> = (0..HOPPERS)
        .filter_map(|i| file.descriptor(i).ok())
        .collect();
    ClankerHoppers::of_defaults(&served)
}

/// `PUT /v1/titles/{title}/hopper`: admin-only. Applies the patch to the
/// published file (or the built-in lineup, for Halo 2 with none), keeping
/// every byte the patch does not name, and publishes the result.
pub async fn put_hopper(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(title): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let title = title_id(&title)?;
    let author = live.authenticate(bearer(&headers), lxm::TITLES_WRITE)?;
    require_admin(&live, &author)?;
    live.rate_limit(&author, lxm::TITLES_WRITE)?;
    let bytes = body_bytes(body)?;
    let (_, current) = published_file(&live, title, HALO_2_HOPPER_FILE)
        .await
        .map_err(|e| {
            if e.code == ErrorCode::NotFound {
                not_found(format!(
                    "no {HALO_2_HOPPER_FILE} published for this title; import a base image \
                     with `h2 live hopper import` first"
                ))
            } else {
                e
            }
        })?;
    let mut file = HopperFile::parse_legacy(&current)
        .map_err(|e| invalid(format!("the published file does not parse ({e})")))?;
    let patch: HoppersPatch =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("hopper: {e}")))?;
    apply_patch(&mut file, &patch)?;
    let built = file.build_checked().map_err(|e| invalid(e.to_string()))?;
    let published = publish(&live, title, HALO_2_HOPPER_FILE, &built).await?;
    let doc = hoppers_doc(&live, &file)?;
    Ok(json(
        StatusCode::OK,
        &serde_json::json!({
            "published": published,
            "hoppers": doc.hoppers,
            "position": doc.position,
        }),
    ))
}

// ---------- route handlers ----------

/// `POST /v1/admins/{gamertag}`.
pub async fn grant_admin(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    let tag = Gamertag::new(&tag).map_err(|_| not_found("no such account"))?;
    let event = grant(&live, bearer(&headers), &tag).await?;
    Ok(json(StatusCode::CREATED, &event))
}

/// `DELETE /v1/admins/{gamertag}`.
pub async fn revoke_admin(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    let tag = Gamertag::new(&tag).map_err(|_| not_found("no such account"))?;
    let event = revoke(&live, bearer(&headers), &tag).await?;
    Ok(json(StatusCode::OK, &event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Account;
    use live_proto::HALO_2_TITLE_ID;
    use live_proto::sign::{AccountRequest, EventBody, KeyPair, issue_token};
    use serde_json::json;

    // A log the Admins subscriber is fed by hand, like model.rs's tests.
    struct Log {
        key: KeyPair,
        position: u64,
    }

    impl Log {
        fn event(&mut self, stream: &str, kind: &str, data: Value) -> RecordedEvent {
            self.position += 1;
            let body = EventBody::new(
                &self.key,
                "server:h",
                stream,
                kind,
                data,
                time::OffsetDateTime::now_utc(),
            );
            RecordedEvent {
                position: self.position,
                version: 0,
                hash: String::new(),
                jws: body.sign(&self.key).into(),
                jws_index: None,
                body,
            }
        }
    }

    fn state_with(xuid: Xuid, tag: &str) -> ModelState {
        let mut s = ModelState::default();
        s.accounts.insert(
            xuid,
            Account {
                xuid,
                gamertag: Gamertag::new(tag).unwrap(),
                created_at: String::new(),
                position: 1,
            },
        );
        s
    }

    fn author(xuid: Xuid) -> Author {
        Author::Account {
            xuid,
            server: "h".into(),
        }
    }

    const A: Xuid = Xuid(0x0009_0000_0000_0001);
    const B: Xuid = Xuid(0x0009_0000_0000_0002);

    #[test]
    fn admins_project_from_the_log() {
        let admins = Admins::new(vec![Gamertag::new("Boss").unwrap()]);
        let mut log = Log {
            key: KeyPair::generate(),
            position: 0,
        };
        let state = state_with(A, "NoIcE");
        let account = |x: Xuid| streams::account(x).to_string();

        // Configured by gamertag, case-insensitively.
        let boss = state_with(B, "BOSS");
        assert!(admins.is_admin(&author(B), &boss));
        assert!(!admins.is_admin(&author(A), &state), "not granted yet");

        // A grant on another account's stream does not count.
        admins.apply(&log.event(
            &account(B),
            ADMIN_GRANTED,
            json!({"xuid": A, "gamertag": "NoIcE"}),
        ));
        assert!(!admins.is_admin(&author(A), &state));
        // The real one, with a matching xuid, does.
        admins.apply(&log.event(
            &account(A),
            ADMIN_GRANTED,
            json!({"xuid": A, "gamertag": "NoIcE"}),
        ));
        assert!(admins.is_admin(&author(A), &state));
        assert!(admins.is_admin_of(&Gamertag::new("noice").unwrap(), A));
        assert_eq!(admins.granted_list(), vec![(A, "NoIcE".into())]);
        // Revoked again, on the same stream.
        admins.apply(&log.event(&account(A), ADMIN_REVOKED, json!({"xuid": A})));
        assert!(!admins.is_admin(&author(A), &state));
        // A grant whose data xuid is not the stream's is skipped.
        admins.apply(&log.event(
            &account(A),
            ADMIN_GRANTED,
            json!({"xuid": B, "gamertag": "X"}),
        ));
        assert!(!admins.is_admin(&author(A), &state));

        // The server key is always allowed.
        assert!(admins.is_admin(&Author::Server("h".into()), &state));
        // The runtime grant (the embedded server's playing account).
        admins.grant_runtime(B, "BOSS");
        assert!(admins.is_admin(&author(B), &boss));
    }

    #[test]
    fn admin_lists_parse_and_merge() {
        // Pin the env var so a shell that happens to have it set (or unset)
        // does not change the test. Nothing else in this binary reads it;
        // this test is the only writer.
        unsafe { std::env::set_var(ENV_ADMINS, "") };
        let flags = ["afr".to_string(), "AFR".into(), "Bob".into()];
        let admins = Admins::new(Admins::from_flags_and_env(&flags).unwrap());
        assert_eq!(
            admins.configured(),
            vec!["afr".to_string(), "Bob".to_string()]
        );
        assert!(admins.is_configured(&Gamertag::new("Afr").unwrap()));
        // The env var contributes too.
        unsafe { std::env::set_var(ENV_ADMINS, " Carol , bob") };
        let merged = Admins::from_flags_and_env(&["afr".to_string()]).unwrap();
        let names: Vec<String> = merged.iter().map(|t| t.to_string()).collect();
        assert_eq!(
            names,
            vec!["afr".to_string(), "bob".to_string(), "Carol".to_string()]
        );
        // An invalid gamertag is an error naming it, not a silent skip.
        unsafe { std::env::set_var(ENV_ADMINS, "9lives") };
        let err = Admins::from_flags_and_env(&[]).unwrap_err();
        assert!(err.contains("9lives"), "{err}");
        unsafe { std::env::remove_var(ENV_ADMINS) };
    }

    #[test]
    fn motd_doc_is_the_editable_fields() {
        let doc = MotdDoc {
            header: "Halo 2 is back".into(),
            messages: vec!["Welcome.".into()],
        };
        assert_eq!(
            serde_json::to_value(&doc).unwrap(),
            json!({"header": "Halo 2 is back", "messages": ["Welcome."]})
        );
        let round = serde_json::from_value::<MotdDoc>(serde_json::to_value(&doc).unwrap()).unwrap();
        assert_eq!(round.motd().build().unwrap(), doc.motd().build().unwrap());
    }

    fn base_file() -> HopperFile {
        let mut file = HopperFile::blank();
        file.set_hopper(
            0,
            &Hopper {
                hopper_type: 4,
                status: 1,
                record_count: 2,
                name: "Big Team Battle".into(),
                ranges: [1, 8, 6],
                dwords: vec![0x7530_07a4; 18],
            },
        )
        .unwrap();
        file.set_record_count(0, 2).unwrap();
        for (j, (id, s)) in [(10u32, "one"), (20, "two")].into_iter().enumerate() {
            file.set_record(
                0,
                j,
                &Record {
                    id,
                    string: s.into(),
                    flags: 0,
                    dwords: [j as u32, 0],
                },
            )
            .unwrap();
        }
        file
    }

    fn patch(index: usize, records: Vec<RecordPatch>) -> HoppersPatch {
        HoppersPatch {
            hoppers: vec![HopperPatch {
                index,
                hopper_type: None,
                template: None,
                clear: false,
                status: None,
                name: None,
                title: None,
                description: None,
                ranges: None,
                dwords: None,
                record_count: None,
                records: Some(records),
            }],
        }
    }

    fn rec(id: u32, string: &str, from: Option<usize>) -> RecordPatch {
        RecordPatch {
            id,
            string: string.into(),
            flags: 0,
            dwords: [0, 0],
            from,
            from_hopper: None,
            variant: None,
        }
    }

    #[test]
    fn patches_edit_reorder_and_add_records() {
        let mut file = base_file();
        // Markers in the template blobs at +0x08 of each slot (bytes
        // set_record never writes), to see whole slots move. 0x6140 is
        // where hopper 0's data block starts (16 * 0x614).
        let block = 0x6140;
        let slot1_blob = block + 0x48 + live_proto::hopper::RECORD + 0x08;
        file.image_mut()[slot1_blob] = 0x5a;

        // Reorder (1, 0), renaming the hopper.
        let mut p = patch(
            0,
            vec![rec(20, "two first", Some(1)), rec(10, "one second", Some(0))],
        );
        p.hoppers[0].name = Some("Big Team".into());
        apply_patch(&mut file, &p).unwrap();
        let h = file.hopper(0).unwrap();
        assert_eq!((h.name.as_str(), h.record_count), ("Big Team", 2));
        let records = file.records(0).unwrap();
        assert_eq!(records[0].id, 20);
        assert_eq!(records[0].string, "two first");
        assert_eq!(records[1].string, "one second");
        // Slot 1's blob moved to slot 0 with its record.
        assert_eq!(file.image()[block + 0x48 + 0x08], 0x5a);
        assert_eq!(file.image()[slot1_blob], 0);

        // Add a third entry by copying slot 1.
        let p = patch(
            0,
            vec![
                rec(20, "two first", Some(0)),
                rec(10, "one second", Some(1)),
                rec(30, "a copy of one", Some(1)),
            ],
        );
        apply_patch(&mut file, &p).unwrap();
        assert_eq!(file.hopper(0).unwrap().record_count, 3);
        assert_eq!(file.block_record_count(0).unwrap(), 3);
        assert_eq!(file.records(0).unwrap()[2].string, "a copy of one");
        // Removing is listing fewer: the count drops, the bytes stay.
        let p = patch(0, vec![rec(20, "two first", Some(0))]);
        apply_patch(&mut file, &p).unwrap();
        assert_eq!(file.hopper(0).unwrap().record_count, 1);
        assert_eq!(file.records(0).unwrap().len(), 1);
    }

    #[test]
    fn patches_set_text_and_copy_records_across_hoppers() {
        let mut file = base_file();
        // Hopper 1 gets one record whose variant carries a marker.
        file.set_hopper(
            1,
            &Hopper {
                record_count: 1,
                ..file.hopper(0).unwrap()
            },
        )
        .unwrap();
        file.set_record_count(1, 1).unwrap();
        let slot = 0x6140 + live_proto::hopper::BLOCK + 0x48;
        file.image_mut()[slot + 0x24c + 0x44] = 9; // engine: Assault
        file.image_mut()[slot + 0x24c + 0x04] = b'A';
        let mut copy = rec(7, "", Some(0));
        copy.from_hopper = Some(1);
        let mut p = patch(0, vec![rec(10, "one", None), copy]);
        p.hoppers[0].title = Some("Team Snipers".into());
        apply_patch(&mut file, &p).unwrap();
        assert_eq!(file.text(0).unwrap().title, "Team Snipers");
        assert_eq!(file.records(0).unwrap()[1].id, 7);
        let v = file.variant(0, 1).unwrap();
        assert_eq!((v.engine, v.name.as_str()), (9, "A"));
        // The source hopper is untouched.
        assert_eq!(file.variant(1, 0).unwrap().engine, 9);
        // Out-of-range sources are refused.
        let mut bad = rec(7, "", Some(1));
        bad.from_hopper = Some(1);
        assert!(apply_patch(&mut file, &patch(0, vec![bad.clone()])).is_err());
        bad.from_hopper = Some(HOPPERS);
        assert!(apply_patch(&mut file, &patch(0, vec![bad])).is_err());
        let mut long = patch(0, vec![rec(10, "one", None)]);
        long.hoppers[0].title = Some("x".repeat(16));
        assert!(apply_patch(&mut file, &long).is_err());
    }

    #[test]
    fn patches_validate() {
        let mut file = base_file();
        let plain = |index: usize| HopperPatch {
            index,
            hopper_type: None,
            template: None,
            clear: false,
            status: None,
            name: None,
            title: None,
            description: None,
            ranges: None,
            dwords: None,
            record_count: None,
            records: None,
        };
        // Index out of range, and the same hopper twice.
        assert!(
            apply_patch(
                &mut file,
                &HoppersPatch {
                    hoppers: vec![plain(HOPPERS)]
                }
            )
            .is_err()
        );
        assert!(
            apply_patch(
                &mut file,
                &HoppersPatch {
                    hoppers: vec![plain(1), plain(1)]
                }
            )
            .is_err()
        );
        // Zero records is invalid to the game, and 101 would be too.
        assert!(apply_patch(&mut file, &patch(0, vec![])).is_err());
        // A record_count that disagrees with the list.
        let mut mismatch = patch(0, vec![rec(10, "a", None)]);
        mismatch.hoppers[0].record_count = Some(2);
        assert!(apply_patch(&mut file, &mismatch).is_err());
        // `from` beyond the current records.
        assert!(apply_patch(&mut file, &patch(0, vec![rec(10, "a", Some(9))])).is_err());
        // Field values the game would reject.
        let mut bad = HoppersPatch {
            hoppers: vec![HopperPatch {
                name: Some("café".into()),
                ..plain(0)
            }],
        };
        assert!(apply_patch(&mut file, &bad).is_err());
        bad.hoppers[0].name = Some("x".repeat(0x85));
        assert!(apply_patch(&mut file, &bad).is_err());
        // A valid patch of the remaining fields applies, and an unlisted
        // hopper keeps its name.
        let good = HoppersPatch {
            hoppers: vec![HopperPatch {
                hopper_type: Some(2),
                ranges: Some([1, 4, 2]),
                name: Some("Fine".into()),
                ..plain(0)
            }],
        };
        apply_patch(&mut file, &good).unwrap();
        assert_eq!(file.hopper(0).unwrap().hopper_type, 2);
        assert_eq!(file.hopper(0).unwrap().name, "Fine");
        assert_eq!(file.hopper(1).unwrap().name, "");
    }

    #[test]
    fn hopper_doc_shapes_the_json() {
        let file = base_file();
        let h = file.hopper(0).unwrap();
        let records = file.records(0).unwrap();
        let doc = HopperDoc {
            index: 0,
            hopper_type: h.hopper_type,
            unused: false,
            has_map_pack_maps: false,
            status: h.status,
            record_count: h.record_count,
            name: h.name.clone(),
            title: "Big Team".into(),
            description: String::new(),
            ranges: h.ranges,
            dwords: h.dwords.clone(),
            records: records
                .into_iter()
                .map(|r| RecordDoc {
                    id: r.id,
                    string: r.string,
                    flags: r.flags,
                    dwords: r.dwords,
                    variant: None,
                    builtin: None,
                })
                .collect(),
        };
        let expected = json!({
            "index": 0, "type": 4, "unused": false, "status": 1, "record_count": 2,
            "name": "Big Team Battle", "title": "Big Team", "description": "",
            "ranges": [1, 8, 6],
            "dwords": serde_json::json!(vec![0x7530_07a4u32; 18]),
            "records": [
                {"id": 10, "string": "one", "flags": 0, "dwords": [0, 0], "variant": null},
                {"id": 20, "string": "two", "flags": 0, "dwords": [1, 0], "variant": null}
            ]
        });
        assert_eq!(serde_json::to_value(&doc).unwrap(), expected);
        // The patch side accepts exactly these fields plus `from`.
        let back: HoppersPatch = serde_json::from_value(json!({
            "hoppers": [{
                "index": 0, "type": 4, "status": 1, "record_count": 2,
                "name": "Big Team Battle", "ranges": [1, 8, 6],
                "records": [
                    {"id": 1, "string": "one", "flags": 0, "dwords": [0, 0]},
                    {"id": 2, "string": "two", "flags": 0, "dwords": [1, 0], "from": 0}
                ]
            }]
        }))
        .unwrap();
        assert_eq!(back.hoppers[0].records.as_ref().unwrap()[1].from, Some(0));
        // `type` is the field the GET names (the web sends it), and the
        // older `hopper_type` still reads.
        assert_eq!(back.hoppers[0].hopper_type, Some(4));
        let old: HoppersPatch =
            serde_json::from_value(json!({"hoppers": [{"index": 0, "hopper_type": 2}]})).unwrap();
        assert_eq!(old.hoppers[0].hopper_type, Some(2));
    }

    fn record(id: u32, weight: u32, variant: &str) -> RecordPatch {
        RecordPatch {
            id,
            string: String::new(),
            flags: 0,
            dwords: [weight, 0],
            from: None,
            from_hopper: None,
            variant: Some(variant.into()),
        }
    }

    /// Entry `j` of hopper `i` as it stands, kept in place.
    fn keep(file: &HopperFile, i: usize, j: usize) -> RecordPatch {
        let r = file.record(i, j).unwrap();
        RecordPatch {
            id: r.id,
            string: r.string,
            flags: r.flags,
            dwords: r.dwords,
            from: Some(j),
            from_hopper: None,
            variant: None,
        }
    }

    fn create(index: usize, template: u32, title: &str, records: Vec<RecordPatch>) -> HoppersPatch {
        let mut p = patch(index, records);
        p.hoppers[0].template = Some(template);
        p.hoppers[0].title = Some(title.into());
        p
    }

    #[test]
    fn an_unused_slot_becomes_a_playlist_the_game_loads_from_a_template() {
        let mut file = HopperFile::default_set();
        let i = defaults::playlists().len();
        assert!(file.is_unused(i).unwrap());
        // Untouched unused slots never block an edit elsewhere.
        let mut other = patch(0, vec![keep(&file, 0, 0)]);
        other.hoppers[0].title = Some("Rumble".into());
        apply_patch(&mut file, &other).unwrap();
        // Naming an unused slot without a template is refused, clearly.
        let mut named = patch(i, vec![record(50, 1, "team_slayer")]);
        named.hoppers[0].title = Some("Modtacular".into());
        let e = apply_patch(&mut file, &named).unwrap_err();
        assert!(e.to_string().contains("unused slot"), "{e}");
        // With a template and no entries: refused, saying so.
        let mut empty = patch(i, vec![]);
        empty.hoppers[0].records = None;
        empty.hoppers[0].template = Some(2);
        let e = apply_patch(&mut file, &empty).unwrap_err();
        assert!(e.to_string().contains("at least one entry"), "{e}");
        // With a template and a built-in variant entry: a playlist the
        // game's validator takes, with a fresh playlist id.
        let p = create(
            i,
            2,
            "Modtacular",
            vec![record(50, 5, "team_slayer"), record(20, 1, "classic_ctf")],
        );
        apply_patch(&mut file, &p).unwrap();
        assert!(!file.is_unused(i).unwrap());
        let d = file.descriptor(i).unwrap();
        assert_eq!(check_descriptor(&d), vec![]);
        assert_eq!(d.hopper_type, 4);
        assert_eq!(d.playlist_id, 12);
        assert_eq!(file.text(i).unwrap().title, "Modtacular");
        let entries = file.entries(i).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].map_id, entries[0].weight), (50, 5));
        assert_eq!(entries[0].variant, defaults::variants::team_slayer());
        assert_eq!(entries[1].variant, defaults::variants::classic_ctf());
        assert_eq!(
            file.problems(&defaults::retail_map_ids()),
            vec![],
            "the whole file is one the game loads"
        );
        // The GET reads it back as a playlist with built-in keys.
        let rec_doc = record_builtin(&file, i, 1);
        assert_eq!(rec_doc.as_deref(), Some("classic_ctf"));
        // It survives a build and parse.
        let back = HopperFile::parse(&file.build_checked().unwrap()).unwrap();
        assert!(back.image() == file.image());
    }

    #[test]
    fn map_pack_rotation_needs_a_disc_fallback() {
        let mut file = HopperFile::default_set();
        let addon = record(0x12_345, 1, "rumble_slayer");
        let only_addon = patch(0, vec![addon.clone()]);
        let error = apply_patch(&mut file, &only_addon).unwrap_err();
        assert!(error.to_string().contains("needs a Halo 2 disc map"));
        let mixed = patch(0, vec![keep(&file, 0, 0), addon]);
        apply_patch(&mut file, &mixed).unwrap();
        assert!(file.contains_addon_maps(0).unwrap());
        assert_eq!(file.entry(0, 1).unwrap().map_id, 0x12_345);
    }

    #[test]
    fn entries_take_built_in_variants_and_bad_keys_are_refused() {
        let mut file = HopperFile::default_set();
        // Entry 0 keeps its slot; entry 1 becomes Territories on Lockout.
        let p = patch(0, vec![keep(&file, 0, 0), record(50, 3, "territories")]);
        let before = file.slot(0, 0).unwrap().to_vec();
        apply_patch(&mut file, &p).unwrap();
        assert_eq!(file.slot(0, 0).unwrap(), &before[..]);
        let e = file.entry(0, 1).unwrap();
        assert_eq!((e.map_id, e.weight), (50, 3));
        assert_eq!(e.variant, defaults::variants::territories());
        // Unknown keys and `from` beside `variant` are refused.
        let mut bad = patch(0, vec![record(50, 1, "team_swords")]);
        assert!(apply_patch(&mut file, &bad).is_err());
        bad = patch(0, vec![record(50, 1, "territories")]);
        bad.hoppers[0].records.as_mut().unwrap()[0].from = Some(0);
        assert!(apply_patch(&mut file, &bad).is_err());
    }

    #[test]
    fn clearing_a_playlist_leaves_an_unused_slot_and_valid_ones_stay_valid() {
        let mut file = HopperFile::default_set();
        let i = defaults::playlists().len();
        apply_patch(
            &mut file,
            &create(i, 1, "Solo", vec![record(10, 1, "rumble_slayer")]),
        )
        .unwrap();
        let mut clear = patch(i, vec![]);
        clear.hoppers[0].records = None;
        clear.hoppers[0].clear = true;
        apply_patch(&mut file, &clear).unwrap();
        assert!(file.image() == HopperFile::default_set().image());
        // `clear` with anything else is refused.
        let mut both = patch(0, vec![keep(&file, 0, 0)]);
        both.hoppers[0].clear = true;
        assert!(apply_patch(&mut file, &both).is_err());
        // A type change that breaks a playlist the game loaded is refused
        // (Rumble Pit's free-for-all sizes are not team sizes).
        let mut retype = patch(0, vec![]);
        retype.hoppers[0].records = None;
        retype.hoppers[0].hopper_type = Some(4);
        let e = apply_patch(&mut file, &retype).unwrap_err();
        assert!(e.to_string().contains("would not load"), "{e}");
        // An unknown template is refused.
        let e = apply_patch(
            &mut file,
            &create(i, 99, "X", vec![record(10, 1, "oddball")]),
        )
        .unwrap_err();
        assert!(e.to_string().contains("template 99"), "{e}");
    }

    #[test]
    fn the_get_lists_variants_templates_and_maps() {
        let v = builtin_variant_docs();
        assert!(
            v.iter()
                .any(|d| d.key == "team_slayer" && d.name == "Team Slayer" && d.team)
        );
        let t = template_docs();
        assert_eq!(t.len(), 8);
        for d in &t {
            assert!((1..=5).contains(&d.hopper_type), "{}", d.title);
            assert!(v.iter().any(|b| b.key == d.entry.variant));
            assert!(RETAIL_MAPS.iter().any(|(id, _)| *id == d.entry.id));
        }
        assert_eq!(t[0].title, "Rumble Pit");
        assert_eq!(t[0].ranges, [4, 8, 0], "fewest and most players");
        assert_eq!(t[0].dwords.len(), 18);
        // What the web builder sends for a new playlist: the template and
        // its own fields back, which change nothing.
        let mut file = HopperFile::default_set();
        let i = defaults::playlists().len();
        for d in &t {
            let p = HoppersPatch {
                hoppers: vec![HopperPatch {
                    index: i,
                    hopper_type: Some(d.hopper_type),
                    template: Some(d.id),
                    clear: false,
                    status: Some(0),
                    name: Some(String::new()),
                    title: Some(d.title.clone()),
                    description: Some(d.description.clone()),
                    ranges: Some(d.ranges),
                    dwords: Some(d.dwords.clone()),
                    record_count: Some(1),
                    records: Some(vec![record(d.entry.id, 1, &d.entry.variant)]),
                }],
            };
            apply_patch(&mut file, &p).unwrap();
            let got = file.descriptor(i).unwrap();
            assert_eq!(check_descriptor(&got), vec![], "{}", d.title);
            assert_eq!(got.hopper_type, d.hopper_type);
            assert_eq!(file.problems(&defaults::retail_map_ids()), vec![]);
        }
        assert_eq!((t[0].min_players, t[0].max_players), (Some(4), Some(8)));
    }

    // Needs the `Live.admins` and `Policy.admins` wiring (docs/live/live-admin.md,
    // "Wiring"); exercises grant, revoke and the admin publish path end to
    // end against the in-memory store.
    #[tokio::test]
    async fn grant_revoke_and_admin_publish() {
        let dir = std::env::temp_dir().join(format!(
            "h2live-admin-{}-{}",
            std::process::id(),
            sign::random_id()
        ));
        let identity = crate::identity::ServerIdentity::open(&dir, Some("live.test")).unwrap();
        let operator = crate::identity::ServerIdentity::load(&dir).unwrap();
        let policy = crate::app::Policy {
            admins: vec![Gamertag::new("afr").unwrap()],
            ..crate::app::Policy::default()
        };
        let live = Live::start(Arc::new(crate::store::MemoryStore::new()), identity, policy)
            .await
            .unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();

        // Two accounts: afr (configured) and NoIcE (to be granted).
        let key = KeyPair::generate();
        let created = live
            .create_account(&AccountRequest::sign(&key, "NoIcE", None, "live.test", now))
            .await
            .unwrap();
        let nice = Gamertag::new("NoIcE").unwrap();

        let op_token =
            |method: &str| issue_token(&operator.key, &operator.author(), "live.test", method, now);

        // Not an admin yet: the titles.write check refuses the account.
        let nice_token = issue_token(&key, &created.id, "live.test", lxm::TITLES_WRITE, now);
        let authed = live
            .authenticate(Some(nice_token.as_str()), lxm::TITLES_WRITE)
            .unwrap();
        assert!(require_admin(&live, &authed).is_err());

        // Grant is operator-only, even with a token for the method.
        let nice_admin = issue_token(&key, &created.id, "live.test", LXM_ADMIN_WRITE, now);
        let err = grant(&live, Some(nice_admin.as_str()), &nice)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        grant(&live, Some(op_token(LXM_ADMIN_WRITE).as_str()), &nice)
            .await
            .unwrap();
        require_admin(&live, &authed).unwrap();

        // afr is an admin by configuration: granting is a conflict, and so
        // is revoking it.
        let err = grant(
            &live,
            Some(op_token(LXM_ADMIN_WRITE).as_str()),
            &Gamertag::new("afr").unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        let err = revoke(
            &live,
            Some(op_token(LXM_ADMIN_WRITE).as_str()),
            &Gamertag::new("afr").unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        // Unknown gamertags are 404s.
        let err = grant(
            &live,
            Some(op_token(LXM_ADMIN_WRITE).as_str()),
            &Gamertag::new("ghost").unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);

        // The granted admin publishes the MOTD; the file reads back.
        let motd = Motd {
            header: "Hi".into(),
            messages: vec!["Welcome back.".into()],
        };
        publish(
            &live,
            HALO_2_TITLE_ID,
            HALO_2_MOTD_FILE,
            &motd.build().unwrap(),
        )
        .await
        .unwrap();
        let (_, bytes) = live.file(HALO_2_TITLE_ID, HALO_2_MOTD_FILE).await.unwrap();
        assert_eq!(Motd::parse(&bytes).unwrap(), motd);

        // Revoke: the admin right ends, and a second revoke conflicts.
        revoke(&live, Some(op_token(LXM_ADMIN_WRITE).as_str()), &nice)
            .await
            .unwrap();
        assert!(require_admin(&live, &authed).is_err());
        let err = revoke(&live, Some(op_token(LXM_ADMIN_WRITE).as_str()), &nice)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);

        drop(live);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clanker_marks_follow_the_served_file() {
        // The built-in lineup: its three clanker playlists, by index.
        let defaults = HopperFile::default_set();
        let marks = clanker_hoppers(Some(&defaults));
        let ids: Vec<(u32, u32)> = marks
            .hoppers
            .iter()
            .map(|h| (h.hopper_id, h.playlist_id))
            .collect();
        assert_eq!(ids, [(8, 9), (9, 10), (10, 11)]);
        // No file: none; nor a blank one.
        assert_eq!(clanker_hoppers(None), ClankerHoppers::default());
        assert_eq!(
            clanker_hoppers(Some(&HopperFile::blank())),
            ClankerHoppers::default()
        );
        // A file that moved a clanker playlist's id loses that mark.
        let mut file = HopperFile::default_set();
        let mut d = file.descriptor(9).unwrap();
        d.playlist_id = 40;
        file.set_descriptor(9, &d).unwrap();
        let marks = clanker_hoppers(Some(&file));
        assert!(marks.contains(8) && !marks.contains(9) && marks.contains(10));
    }
}
