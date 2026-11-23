//! Published game variants (docs/live/live-api.md "Game variants",
//! docs/game/rules-engine.md "Workbench").
//!
//! A game variant is a whole `.h2variant` file (`variants::portable`,
//! version 2): name, description, author, the 0x130-byte record and, when
//! the record names a rules script, the script's IR and source. A signed-in
//! account publishes one that passes `Bundle::check`; the server writes the
//! author into it, and the SHA-256 of its canonical bytes is its id. A
//! server-signed `VariantPublished` event on `variant-<id>` holds the file,
//! and a `VariantDeleted` event on the same stream takes it down. The
//! projection below is rebuilt from the log on both homes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

use axum::Extension;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use live_proto::game_variants::{
    self as wire, DELETED, Deleted, KINDS, MAX_PER_AUTHOR, PUBLISHED, Published, Setting, Variant,
    VariantList, Visibility,
};
use live_proto::{Author, ErrorCode, Gamertag, Xuid, lxm};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use variants::portable::{self, Attach, Bundle, PortableError};
use variants::record::{self, GameVariant, Kind};

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::http::{ClientIp, bearer, body_bytes};
use crate::store::{RecordedEvent, StoreError};

type ApiResult<T> = Result<T, ApiError>;

/// The largest file kept: the source (256 KiB, whose JSON escapes may
/// double it) and the IR as base64 (about 88 KiB), with room for the rest.
pub const MAX_BUNDLE: usize = 2 * portable::MAX_SOURCE + 128 * 1024;

/// A request body: a file, possibly as a JSON string of itself.
pub const MAX_BODY: usize = 2 * MAX_BUNDLE + 64 * 1024;

// ---------- the projection ----------

#[derive(Debug, Clone)]
struct Kept {
    published: Published,
    bundle: Bundle,
    created: String,
    deleted: bool,
}

/// Every published variant, by id.
#[derive(Debug, Default)]
pub struct VariantsState {
    by_id: BTreeMap<String, Kept>,
    /// Events on each variant's stream: the version the next append
    /// expects.
    versions: HashMap<String, u64>,
    /// Each author's ids, by last publish, oldest first.
    by_author: HashMap<Xuid, Vec<String>>,
}

impl VariantsState {
    /// An author's variants that are not deleted.
    pub fn count_by(&self, author: Xuid) -> usize {
        self.by_author
            .get(&author)
            .into_iter()
            .flatten()
            .filter(|id| self.by_id.get(*id).is_some_and(|k| !k.deleted))
            .count()
    }

    fn next_version(&self, id: &str) -> u64 {
        self.versions.get(&wire::stream(id)).copied().unwrap_or(0)
    }

    fn apply(&mut self, e: &RecordedEvent) {
        if !e.body.stream.starts_with(wire::STREAM_PREFIX) {
            return;
        }
        *self.versions.entry(e.body.stream.clone()).or_default() += 1;
        if e.body.kind == PUBLISHED {
            let Ok(p) = serde_json::from_value::<Published>(e.body.data.clone()) else {
                return;
            };
            let Ok(bundle) = serde_json::from_str::<Bundle>(&p.bundle) else {
                return;
            };
            if self.by_id.get(&p.id).is_some_and(|k| !k.deleted) {
                return;
            }
            let ids = self.by_author.entry(p.author).or_default();
            ids.retain(|i| *i != p.id);
            ids.push(p.id.clone());
            self.by_id.insert(
                p.id.clone(),
                Kept {
                    published: p,
                    bundle,
                    created: e.body.at.clone(),
                    deleted: false,
                },
            );
        } else if e.body.kind == DELETED {
            let Ok(d) = serde_json::from_value::<Deleted>(e.body.data.clone()) else {
                return;
            };
            if let Some(k) = self.by_id.get_mut(&d.id) {
                k.deleted = true;
            }
        }
    }
}

/// The published variants' projection.
#[derive(Default)]
pub struct Variants {
    state: RwLock<VariantsState>,
}

impl Variants {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, VariantsState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Variants {
    fn name(&self) -> &str {
        "variants"
    }

    fn apply(&self, event: &RecordedEvent) {
        if KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

// ---------- views ----------

/// The record's settings in words: every setting of its game type we have
/// a name for (`record::symbolic_name`), on/off for the flags, then the
/// extension options in one line.
pub fn settings(v: &GameVariant) -> Vec<Setting> {
    let mut out = Vec::new();
    for (id, value) in v.known_settings() {
        let Some(label) = record::symbolic_name(id) else {
            continue;
        };
        let value = match record::field(id).map(|f| f.kind) {
            Some(Kind::Bit16(_) | Kind::Bit32(_) | Kind::Bool8) => {
                if value != 0 { "On" } else { "Off" }.to_string()
            }
            _ => value.to_string(),
        };
        out.push(Setting {
            label: label.to_string(),
            value,
        });
    }
    let e = v.ext();
    if !e.is_default() {
        out.push(Setting {
            label: "Extras".into(),
            value: e.summary(),
        });
    }
    out
}

/// One option a script declares, for the workbench's variant form.
#[derive(Debug, Serialize)]
pub struct OptionView {
    pub name: String,
    pub label: String,
    pub description: Option<String>,
    /// `int`, `bool` or `enum`.
    pub kind: &'static str,
    pub lo: i32,
    pub hi: i32,
    pub default: i32,
    /// An enum's value names, by value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

fn string(ir: &rules::Ir, i: u16) -> String {
    ir.strings.get(usize::from(i)).cloned().unwrap_or_default()
}

/// The options a script declares, in declaration order.
pub fn option_views(ir: &rules::Ir) -> Vec<OptionView> {
    ir.options
        .iter()
        .map(|o| {
            let (kind, values) = match o.ty {
                rules::Ty::Bool => ("bool", None),
                rules::Ty::Enum(e) => (
                    "enum",
                    Some(
                        ir.enums
                            .get(usize::from(e))
                            .map(|d| d.variants.iter().map(|&s| string(ir, s)).collect())
                            .unwrap_or_default(),
                    ),
                ),
                _ => ("int", None),
            };
            OptionView {
                name: string(ir, o.name),
                label: string(ir, o.label),
                description: o.description.map(|d| string(ir, d)),
                kind,
                lo: o.lo,
                hi: o.hi,
                default: o.default,
                values,
            }
        })
        .collect()
}

/// A script's `settings` block row, in words: the table's name and label
/// and the value's text (`rules::settings::value_label`).
#[derive(Debug, Serialize)]
pub struct ScriptSetting {
    pub name: String,
    pub label: String,
    pub value: String,
}

/// One Quick Options mark: a record setting of the table (`setting`) or
/// one of the script's options (`option`).
#[derive(Debug, Serialize)]
pub struct QuickView {
    pub kind: &'static str,
    pub name: String,
    pub label: String,
}

/// The settings block, in the blob's (table) order. A row this build's
/// table lacks is left out.
pub fn script_settings(ir: &rules::Ir) -> Vec<ScriptSetting> {
    ir.settings
        .iter()
        .filter_map(|s| {
            let row = rules::settings::row_at(s.index)?;
            Some(ScriptSetting {
                name: row.name.to_string(),
                label: row.label.to_string(),
                value: rules::settings::value_label(s.index, s.value),
            })
        })
        .collect()
}

/// The Quick Options marks, in the script's order.
pub fn quick_views(ir: &rules::Ir) -> Vec<QuickView> {
    ir.quick
        .iter()
        .filter_map(|q| match *q {
            rules::ir::Quick::Setting(i) => {
                let row = rules::settings::row_at(i)?;
                Some(QuickView {
                    kind: "setting",
                    name: row.name.to_string(),
                    label: row.label.to_string(),
                })
            }
            rules::ir::Quick::Option(i) => {
                let o = ir.options.get(usize::from(i))?;
                Some(QuickView {
                    kind: "option",
                    name: string(ir, o.name),
                    label: string(ir, o.label),
                })
            }
        })
        .collect()
}

/// Who scores the game, as the view names it: `script` or `carrier`.
pub fn scoring_word(ir: &rules::Ir) -> &'static str {
    match ir.header.scoring {
        rules::ir::Scoring::Script => "script",
        rules::ir::Scoring::Carrier => "carrier",
    }
}

/// Option values from a request's JSON, by name. A value is a number, a
/// bool or an enum value's name; anything else is out of every option's
/// range and refused when packed.
pub fn option_values(given: &Map<String, Value>) -> Vec<(String, rules::ir::OptionValue)> {
    use rules::ir::OptionValue;
    given
        .iter()
        .map(|(k, v)| {
            let value = match v {
                Value::Bool(b) => OptionValue::Bool(*b),
                Value::String(s) => OptionValue::Name(s.clone()),
                // A fraction, or a value of another type, is out of every
                // option's range.
                Value::Number(n) => OptionValue::Int(n.as_i64().unwrap_or(i64::MAX)),
                _ => OptionValue::Int(i64::MAX),
            };
            (k.clone(), value)
        })
        .collect()
}

/// Packs option values by name into the record's 32 option bits, in
/// declaration order (docs/game/rules-engine.md, "Carrying a script").
/// An option left out takes its default.
pub fn pack_options(ir: &rules::Ir, given: &Map<String, Value>) -> ApiResult<u32> {
    ir.pack_options(&option_values(given))
        .map_err(|e| invalid(e.to_string()))
}

fn view(live: &Live, k: &Kept) -> Variant {
    let b = &k.bundle;
    let v = b.variant().ok();
    Variant {
        id: k.published.id.clone(),
        name: b.name.clone(),
        description: b.description.clone(),
        author: wire::Author {
            xuid: k.published.author,
            gamertag: live.who_is(k.published.author).gamertag,
        },
        engine: b.engine,
        game_type: portable::game_type_of(b).label(),
        record_hex: b.record_hex.clone(),
        script_hash: b.script.as_ref().map(|s| s.hash.clone()),
        script_name: b.script.as_ref().map(|s| s.name.clone()),
        visibility: k.published.visibility,
        created: k.created.clone(),
        bundle_url: format!("/v1/variants/{}/h2variant", k.published.id),
        settings: v.as_ref().map(settings).unwrap_or_default(),
        eligible: eligible(b),
    }
}

/// Where a bundle may play, by the script policy (`rules::policy`): every
/// kind of game with no script, else the ones its header's capabilities
/// allow. Publishing stays open; this records the answer.
pub fn eligible(b: &Bundle) -> Vec<String> {
    match b.integrity() {
        Ok(c) => match c.script {
            None => rules::policy::PLAYED.iter().map(|c| c.name().to_string()).collect(),
            Some((_, ir)) => rules::policy::eligible(&ir.header.requires, ir.header.api_version)
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        Err(_) => Vec::new(),
    }
}

fn refused(e: PortableError) -> ApiError {
    invalid(format!("the game variant: {e}"))
}

/// Every built-in and example script by IR hash, compiled once.
fn builtins() -> &'static HashMap<String, &'static str> {
    static ALL: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    ALL.get_or_init(|| {
        rules::builtin::ALL
            .iter()
            .map(|b| b.source)
            .chain(rules::builtin::examples().iter().map(|(_, s)| *s))
            .filter_map(|src| Some((rules::compile(src).ok()?.id().to_string(), src)))
            .collect()
    })
}

fn decode_record(hex: &str) -> ApiResult<GameVariant> {
    let hex = hex.trim();
    if hex.len() != 2 * record::RECORD_SIZE || !hex.is_ascii() {
        return Err(invalid("record_hex is 0x130 bytes as 608 hex digits"));
    }
    let bytes = (0..record::RECORD_SIZE)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| invalid("record_hex is not hex"))?;
    GameVariant::from_bytes(&bytes).map_err(|e| invalid(e.to_string()))
}

// ---------- the service ----------

impl Live {
    /// A signed-in account's `content.write` token.
    fn variant_author(&self, bearer: Option<&str>) -> ApiResult<(Author, Xuid)> {
        let author = self.authenticate(bearer, lxm::CONTENT_WRITE)?;
        let xuid = match &author {
            Author::Account { xuid, .. } => *xuid,
            _ => return Err(forbidden("only an account publishes a game variant")),
        };
        self.rate_limit(&author, lxm::CONTENT_WRITE)?;
        Ok((author, xuid))
    }

    /// The script policy (`rules::policy`) for the saved or built-in
    /// script with this full hash in `context`: its first refused
    /// capability, by name and reason.
    fn script_policy(&self, hash: &str, context: rules::policy::Context<'_>) -> ApiResult<()> {
        let attach = self.script_by_hash(hash)?;
        let (ir, _) = rules::verify::load(&attach.ir)
            .map_err(|e| invalid(format!("the script does not load: {e}")))?;
        match rules::policy::check(context, &ir.header.requires, ir.header.api_version)
            .into_iter()
            .next()
        {
            Some(d) => Err(forbidden(format!("the rules script {hash}: {d}"))),
            None => Ok(()),
        }
    }

    /// A session's script against the policy of its kind of game: a
    /// ranked session's `rules` names a script Live knows whose header the
    /// ranked policy takes. Custom and hopper sessions are checked at the
    /// host and at the hopper's edit.
    pub(crate) fn check_session_rules(&self, flags: &str, script: Option<&str>) -> ApiResult<()> {
        match script {
            Some(hash) if flags == "ranked" => {
                self.script_policy(hash, rules::policy::Context::Ranked)
            }
            _ => Ok(()),
        }
    }

    /// A hopper slot's script as its verified IR, for the hopper gate
    /// (`admin::check_pairs`).
    pub(crate) fn slot_script_ir(&self, prefix: &str) -> ApiResult<rules::ir::Ir> {
        let attach = self.script_by_hash(&self.slot_script_hash(prefix)?)?;
        rules::verify::load(&attach.ir)
            .map(|(ir, _)| ir)
            .map_err(|e| invalid(format!("the script does not load: {e}")))
    }

    /// A hopper slot's script, named by the record's hash prefix (16
    /// lowercase hex digits), against `context`: one saved or built-in
    /// script has the prefix, and the context takes its header.
    pub(crate) fn check_slot_script(
        &self,
        prefix: &str,
        context: rules::policy::Context<'_>,
    ) -> ApiResult<()> {
        self.script_policy(&self.slot_script_hash(prefix)?, context)
    }

    /// The hash of the one saved or built-in script a hopper record's hash
    /// prefix (16 lowercase hex digits) names.
    pub(crate) fn slot_script_hash(&self, prefix: &str) -> ApiResult<String> {
        let mut hashes: Vec<String> = builtins()
            .keys()
            .filter(|h| h.starts_with(prefix))
            .cloned()
            .collect();
        hashes.extend(self.rules.read().hashes_with_prefix(prefix));
        hashes.sort();
        hashes.dedup();
        match hashes.as_slice() {
            [] => Err(invalid(format!(
                "no saved or built-in script has hash prefix {prefix}"
            ))),
            [hash] => Ok(hash.clone()),
            _ => Err(invalid(format!(
                "more than one script has hash prefix {prefix}"
            ))),
        }
    }

    /// A saved script's or a built-in's IR and source, by IR hash.
    pub fn script_by_hash(&self, hash: &str) -> ApiResult<Attach> {
        if !live_proto::rules::valid_hash(hash) {
            return Err(invalid("script_hash is 64 lowercase hex digits"));
        }
        let source = match self.saved_rules(hash) {
            Some(s) => s.source,
            None => builtins()
                .get(hash)
                .map(|s| s.to_string())
                .ok_or_else(|| invalid("no saved or built-in script has this hash"))?,
        };
        let ir = rules::compile(&source)
            .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("the saved source: {e}")))?;
        if ir.id().to_string() != hash {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "this build compiles the saved source to another blob",
            ));
        }
        Ok(Attach {
            ir: ir.encode(),
            source: Some(source),
            options: None,
        })
    }

    /// A bundle from the separate fields: a record, its name and
    /// description and, when the record names a rules script, the saved or
    /// built-in script whose hash starts with the record's prefix.
    fn bundle_from_fields(
        &self,
        name: &str,
        description: &str,
        record_hex: &str,
        script_hash: Option<&str>,
    ) -> ApiResult<Bundle> {
        let v = decode_record(record_hex)?;
        let script = match v.ext_state() {
            variants::ext::Block::Ext(e) if e.script.mode == variants::ext::Mode::Rules => {
                Some(e.script)
            }
            _ => None,
        };
        let attach = match (script, script_hash) {
            (None, None) => None,
            (None, Some(_)) => {
                return Err(invalid("script_hash is given, but the record names no script"));
            }
            (Some(_), None) => return Err(refused(PortableError::ScriptMissing)),
            (Some(s), Some(hash)) => {
                let mut a = self.script_by_hash(hash)?;
                let prefix: String = s.prefix.iter().map(|b| format!("{b:02x}")).collect();
                if !hash.starts_with(&prefix) {
                    return Err(refused(PortableError::ScriptPrefix));
                }
                a.options = Some(s.options);
                Some(a)
            }
        };
        Bundle::new(&v, name, description, attach).map_err(refused)
    }

    /// A bundle for a script: the script's own variant
    /// (`portable::record_for_named`: the carrier's stock record, the
    /// script's `settings` written, the request's name and the script
    /// reference with the
    /// given option values). The description is the request's, or the
    /// script's `about` when the request gives none.
    pub fn bundle_for_script(
        &self,
        mut attach: Attach,
        name: &str,
        description: &str,
        options: &Map<String, Value>,
    ) -> ApiResult<Bundle> {
        let (ir, _) = rules::verify::load(&attach.ir)
            .map_err(|e| invalid(format!("the script does not load: {e}")))?;
        // The route's own option refusals come first, in its words.
        attach.options = Some(pack_options(&ir, options)?);
        let v = portable::record_for_named(&ir, name, &option_values(options)).map_err(refused)?;
        let description = if description.trim().is_empty() {
            ir.header.about.as_str()
        } else {
            description
        };
        Bundle::new(&v, name, description, Some(attach)).map_err(refused)
    }

    /// Publishes a bundle as `xuid`: writes the author in, checks it and
    /// keeps it. Returns the variant and whether this request published it.
    pub async fn publish_variant(
        &self,
        xuid: Xuid,
        mut bundle: Bundle,
        visibility: Visibility,
    ) -> ApiResult<(Variant, bool)> {
        bundle.author = Some(portable::Author {
            xuid: xuid.to_string(),
            gamertag: self.who_is(xuid).gamertag,
        });
        bundle.check().map_err(refused)?;
        let text = bundle.canonical();
        if text.len() > MAX_BUNDLE {
            return Err(ApiError::new(
                ErrorCode::TooLarge,
                format!("a game variant file is at most {MAX_BUNDLE} bytes"),
            ));
        }
        let id = bundle.id();
        let _guard = self.variant_writes.lock().await;
        if let Some(v) = self.published_variant(&id) {
            return Ok((v, false));
        }
        let expect = {
            let s = self.variants.read();
            if s.count_by(xuid) >= MAX_PER_AUTHOR {
                return Err(conflict(format!(
                    "an account publishes at most {MAX_PER_AUTHOR} game variants"
                )));
            }
            s.next_version(&id)
        };
        let data = Published {
            id: id.clone(),
            author: xuid,
            visibility,
            bundle: text,
        };
        let stream = wire::stream(&id);
        let event = self.server_event(
            stream.clone(),
            PUBLISHED,
            serde_json::to_value(data).expect("serializes"),
        );
        match self.store.append(vec![event], &[(stream, expect)]).await {
            Ok(recorded) => self.wait_applied(recorded[0].position).await?,
            // Another server on the log wrote this stream first.
            Err(StoreError::Conflict { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        self.published_variant(&id)
            .map(|v| (v, true))
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the variant is not yet readable"))
    }

    /// `GET /v1/variants/{id}`: a variant that is not deleted.
    pub fn published_variant(&self, id: &str) -> Option<Variant> {
        let s = self.variants.read();
        let k = s.by_id.get(id).filter(|k| !k.deleted)?;
        Some(view(self, k))
    }

    /// The canonical file of a variant that is not deleted.
    pub fn variant_file(&self, id: &str) -> Option<(String, String)> {
        let s = self.variants.read();
        let k = s.by_id.get(id).filter(|k| !k.deleted)?;
        Some((k.bundle.name.clone(), k.published.bundle.clone()))
    }

    /// `GET /v1/variants?author=`: an author's variants, newest first;
    /// unlisted ones only when `caller` is the author.
    pub fn variants_by(&self, author: &str, caller: Option<Xuid>) -> ApiResult<VariantList> {
        let xuid = match Xuid::parse(author) {
            Ok(x) if author.len() == 16 => x,
            _ => {
                let tag = Gamertag::new(author).map_err(|_| not_found("no such account"))?;
                self.model
                    .read()
                    .account_by_tag(&tag)
                    .map(|a| a.xuid)
                    .ok_or_else(|| not_found("no such account"))?
            }
        };
        let own = caller == Some(xuid);
        let s = self.variants.read();
        let items = s
            .by_author
            .get(&xuid)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|id| s.by_id.get(id))
            .filter(|k| !k.deleted && (own || k.published.visibility == Visibility::Public))
            .map(|k| view(self, k))
            .collect();
        Ok(VariantList { items })
    }

    /// `DELETE /v1/variants/{id}`: its author, or an admin, takes it down.
    pub async fn delete_variant(&self, bearer: Option<&str>, id: &str) -> ApiResult<()> {
        let (author, xuid) = self.variant_author(bearer)?;
        let _guard = self.variant_writes.lock().await;
        let expect = {
            let s = self.variants.read();
            let k = s
                .by_id
                .get(id)
                .filter(|k| !k.deleted)
                .ok_or_else(|| not_found("no such game variant"))?;
            if k.published.author != xuid && crate::admin::require_admin(self, &author).is_err() {
                return Err(forbidden("only its author deletes a game variant"));
            }
            s.next_version(id)
        };
        let stream = wire::stream(id);
        let event = self.server_event(
            stream.clone(),
            DELETED,
            serde_json::to_value(Deleted {
                id: id.to_string(),
                by: xuid,
            })
            .expect("serializes"),
        );
        match self.store.append(vec![event], &[(stream, expect)]).await {
            Ok(recorded) => self.wait_applied(recorded[0].position).await,
            Err(StoreError::Conflict { .. }) => Err(conflict("the variant changed; try again")),
            Err(e) => Err(e.into()),
        }
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

fn parse<T: for<'de> Deserialize<'de>>(body: Result<Bytes, BytesRejection>) -> ApiResult<T> {
    let bytes = body_bytes(body)?;
    serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))
}

#[derive(Debug, Deserialize)]
struct PublishBody {
    /// The `.h2variant` file, as an object or a string of it.
    #[serde(default)]
    bundle: Option<Value>,
    #[serde(default)]
    visibility: Visibility,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    record_hex: Option<String>,
    #[serde(default)]
    script_hash: Option<String>,
}

fn published(v: Variant, new: bool) -> Response {
    let status = if new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    crate::http::json(status, &v)
}

async fn post_publish(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let b: PublishBody = parse(body)?;
    let (_, xuid) = live.variant_author(bearer(&headers))?;
    let bundle = match (&b.bundle, &b.record_hex) {
        (Some(file), _) => {
            let text = match file {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            portable::read(&text).map_err(refused)?
        }
        (None, Some(hex)) => live.bundle_from_fields(
            b.name.as_deref().unwrap_or(""),
            b.description.as_deref().unwrap_or(""),
            hex,
            b.script_hash.as_deref(),
        )?,
        (None, None) => return Err(invalid("give a bundle (the .h2variant file) or record_hex")),
    };
    let (v, new) = live.publish_variant(xuid, bundle, b.visibility).await?;
    Ok(published(v, new))
}

#[derive(Debug, Deserialize)]
struct FromScriptBody {
    #[serde(default)]
    script_hash: Option<String>,
    #[serde(default)]
    source: Option<String>,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    visibility: Visibility,
    #[serde(default)]
    options: Map<String, Value>,
}

impl FromScriptBody {
    fn bundle(&self, live: &Live) -> ApiResult<Bundle> {
        let attach = match (&self.script_hash, &self.source) {
            (Some(h), _) => live.script_by_hash(h)?,
            (None, Some(src)) => Attach::from_source(src).map_err(refused)?,
            (None, None) => return Err(invalid("give script_hash or source")),
        };
        live.bundle_for_script(attach, &self.name, &self.description, &self.options)
    }
}

async fn post_from_script(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let b: FromScriptBody = parse(body)?;
    let (_, xuid) = live.variant_author(bearer(&headers))?;
    let bundle = b.bundle(&live)?;
    let (v, new) = live.publish_variant(xuid, bundle, b.visibility).await?;
    Ok(published(v, new))
}

/// A file for a script with no account: the workbench's download.
async fn post_bundle(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    live.limit_workbench(&ip)?;
    let b: FromScriptBody = parse(body)?;
    let bundle = b.bundle(&live)?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({
            "id": bundle.id(),
            "file_name": portable::file_name(&bundle.name),
            "bundle": bundle,
        }),
    ))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    author: Option<String>,
}

async fn get_list(
    State(live): Shared,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Response> {
    let caller = live.read_caller(bearer(&headers))?;
    let author = q
        .author
        .filter(|a| !a.trim().is_empty())
        .ok_or_else(|| invalid("?author= (a gamertag or xuid) is required"))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &live.variants_by(author.trim(), caller)?,
    ))
}

fn id_param(id: &str) -> ApiResult<&str> {
    if wire::valid_id(id) {
        Ok(id)
    } else {
        Err(not_found("no such game variant"))
    }
}

async fn get_variant(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let v = live
        .published_variant(id_param(&id)?)
        .ok_or_else(|| not_found("no such game variant"))?;
    Ok(crate::http::json(StatusCode::OK, &v))
}

async fn delete_variant(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    live.delete_variant(bearer(&headers), id_param(&id)?).await?;
    Ok(crate::http::json(StatusCode::OK, &json!({"ok": true, "id": id})))
}

/// `attachment` with an ASCII file name and the UTF-8 one (RFC 6266).
fn disposition(name: &str) -> HeaderValue {
    let file = portable::file_name(name);
    let ascii: String = file
        .chars()
        .map(|c| if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' { c } else { '_' })
        .collect();
    let utf8 = percent_encoding::utf8_percent_encode(&file, percent_encoding::NON_ALPHANUMERIC);
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{utf8}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"variant.h2variant\""))
}

/// The `.h2variant` file: the canonical bytes its id hashes. Not kept by
/// shared caches (`caching::policy`), so a delete takes it down at once.
async fn get_file(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let (name, text) = live
        .variant_file(id_param(&id)?)
        .ok_or_else(|| not_found("no such game variant"))?;
    let mut resp = text.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CONTENT_DISPOSITION, disposition(&name));
    Ok(resp)
}

/// The game variant routes, with their own body cap.
pub(crate) fn routes() -> Router<Arc<Live>> {
    Router::new()
        .route("/v1/variants", get(get_list).post(post_publish))
        .route("/v1/variants/from-script", post(post_from_script))
        .route("/v1/variants/bundle", post(post_bundle))
        .route("/v1/variants/{id}", get(get_variant).delete(delete_variant))
        .route("/v1/variants/{id}/h2variant", get(get_file))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use variants::portable::PortScript;
    use variants::record::GameType;

    fn fists() -> GameVariant {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set_name("Fists").unwrap();
        v
    }

    #[test]
    fn eligibility_follows_the_scripts_capabilities() {
        let plain = portable::Bundle::new(&fists(), "Fists", "", None).unwrap();
        assert_eq!(eligible(&plain), ["custom", "hopper", "ranked"]);
        let slayer = portable::port(&fists(), None, None, PortScript::Builtin).unwrap();
        assert_eq!(eligible(&slayer), ["custom", "hopper", "ranked"]);
        let builder = "rules \"Builder\" 1\ncarrier slayer\nrequires forge.move\n\non kill(victim, killer) { if killer != none { score(killer, 1) } }\n";
        let forge = portable::port(&fists(), None, None, PortScript::Source(builder.into())).unwrap();
        assert_eq!(eligible(&forge), ["custom", "hopper"]);
    }

    #[test]
    fn options_pack_by_name_with_the_routes_refusals() {
        let ir = rules::compile(rules::builtin::SLAYER).unwrap();
        let map = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(pack_options(&ir, &Map::new()).unwrap(), ir.default_options());
        let three = pack_options(&ir, &map(json!({"kill_points": 3, "suicide_costs": false}))).unwrap();
        assert_ne!(three, ir.default_options());
        for (bad, want) in [
            (json!({"nope": 1}), "the script has no option \"nope\""),
            (json!({"kill_points": 9}), "option \"kill_points\" takes 1..=5"),
            (json!({"kill_points": 1.5}), "option \"kill_points\" takes 1..=5"),
            (json!({"kill_points": null}), "option \"kill_points\" takes 1..=5"),
            (json!({"kill_points": "x"}), "option \"kill_points\" has no value \"x\""),
        ] {
            assert_eq!(pack_options(&ir, &map(bad.clone())).unwrap_err().message, want, "{bad}");
        }
    }

    /// The server's port of the game's defaults writer and the editor's
    /// agree on every type: the same 0x130 bytes. Slice 2 left the stock
    /// record named by our type label with description number 0xff.
    #[test]
    fn stock_agrees_with_the_hopper_stock_record() {
        use live_proto::hopper::variant::{GameType as HopperType, VariantRecord};
        for t in HopperType::ALL {
            let ours = GameType::from_engine(t.engine() as i32);
            let label = ours.label();
            let hopper = VariantRecord::stock(t, &label);
            let editor = GameVariant::stock(ours).expect("the editor offers this type");
            assert_eq!(editor.bytes()[..], hopper.bytes()[..], "{label}");
        }
    }

    #[test]
    fn the_script_view_words_settings_and_quick() {
        let duel = rules::compile(rules::builtin::DUEL).unwrap();
        assert_eq!(scoring_word(&duel), "carrier");
        let settings = script_settings(&duel);
        assert_eq!(settings.iter().find(|s| s.name == "score_limit").unwrap().value, "15");
        let quick = quick_views(&duel);
        assert_eq!(quick.len(), duel.quick.len());
        assert_eq!((quick[0].kind, quick[0].name.as_str()), ("setting", "score_limit"));
        let slayer = rules::compile(rules::builtin::SLAYER).unwrap();
        assert_eq!(scoring_word(&slayer), "script");
        assert!(script_settings(&slayer).is_empty());
    }
}
