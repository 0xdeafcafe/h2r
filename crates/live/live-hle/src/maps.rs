//! Joining a session on a custom map (docs/game/maps.md §5, "As built"): before a
//! join, the session's `map_id` is compared with what this machine has; a
//! published package's missing layers are downloaded from the server,
//! checked against their SHA-256 and written as a package under
//! `<data>/maps`. A join that cannot be had (no disc map for the base, the
//! content taken down, a ranked session on an unpinned map) is refused
//! with a reason before any map load.
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use content::manifest::{MANIFEST_FILE, relative};
use content::{Library, Manifest, MapId, Sha256};
use live_proto::maps::{HopperPin, MapAnswer, ranked_flags};
use live_proto::session::NewSession;

/// The largest blob a client downloads (a whole user-made map).
pub const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;
/// The largest JSON answer read (a manifest, the pins).
const MAX_JSON: u64 = 1024 * 1024;
/// Downloaded packages are `<data>/maps/live-<first 16 hex of the MapId>`.
pub const DIR_PREFIX: &str = "live-";

/// The user's own maps, for the base of a package and for retail ids.
pub trait Bases: Send + Sync {
    /// SHA-256 of the user's own `map`: the disc's `maps\<map>.map`, or for
    /// a Multiplayer Map Pack map ([`content::map_pack`]) its copy
    /// installed by `h2 import-mappack`. None when there is neither.
    fn disc_hash(&self, map: &str) -> Option<Sha256>;

    /// Whether all Map Pack maps are installed for the retail title to load.
    fn has_map_pack(&self) -> bool {
        false
    }
}

/// [`Bases`] over a directory of `.map` files (the disc's `maps`), each
/// hashed once on first ask.
pub struct DiscMaps {
    dir: PathBuf,
    cache: Mutex<HashMap<String, Option<Sha256>>>,
}

impl DiscMaps {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        DiscMaps {
            dir: dir.into(),
            cache: Mutex::default(),
        }
    }
}

impl Bases for DiscMaps {
    fn disc_hash(&self, map: &str) -> Option<Sha256> {
        let key = map.to_ascii_lowercase();
        if let Some(h) = self.cache.lock().unwrap().get(&key) {
            return *h;
        }
        let found = std::fs::read_dir(&self.dir).ok().and_then(|entries| {
            entries.flatten().find_map(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                (name == format!("{key}.map")).then(|| e.path())
            })
        });
        let hash = found.and_then(|p| hash_file(&p).ok());
        self.cache.lock().unwrap().insert(key, hash);
        hash
    }
}

fn hash_file(path: &Path) -> std::io::Result<Sha256> {
    let mut f = std::fs::File::open(path)?;
    let mut h = content::Hasher::default();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finish());
        }
        h.update(&buf[..n]);
    }
}

/// What a GET to the Live API answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    Ok(Vec<u8>),
    NotFound,
    /// 410, with the server's reason.
    Gone(String),
}

/// Unauthenticated GETs on the Live API (`path` like `/maps/<id>`).
pub trait Fetch {
    fn get(&self, path: &str, cap: u64) -> Result<Got, String>;
    /// [`Fetch::get`] as the signed-in account, so the server counts a
    /// download (`GET /v1/maps/{id}` with a fresh `read` token,
    /// docs/live/live-api.md "The mod repository"). Without an account it
    /// is a plain GET.
    fn get_counted(&self, path: &str, cap: u64) -> Result<Got, String> {
        self.get(path, cap)
    }
}

/// The agent a map download uses: a connect and a per-read timeout, and no
/// timeout on the whole request, so a big layer on a slow line finishes as
/// long as bytes keep coming. The Live worker's own agent times the whole
/// request out (`ServerOptions::request_timeout`, 5 s), which no layer of
/// more than a few MB fits in.
pub fn download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30))
        .redirects(0)
        .build()
}

/// A fresh single-use `read` bearer token for each call, `None` when there
/// is no account to make one for.
pub type Reader = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// [`Fetch`] over HTTP with `ureq`, against an API base like
/// `http://host:8074/v1`; `reader` makes the token of a counted GET.
pub struct HttpFetch<'a> {
    pub agent: &'a ureq::Agent,
    pub api: &'a str,
    pub reader: Option<Reader>,
}

impl Fetch for HttpFetch<'_> {
    fn get(&self, path: &str, cap: u64) -> Result<Got, String> {
        self.send(path, cap, None)
    }
    fn get_counted(&self, path: &str, cap: u64) -> Result<Got, String> {
        let token = self.reader.as_ref().and_then(|r| r());
        self.send(path, cap, token)
    }
}

impl HttpFetch<'_> {
    /// Reads private catalogues and layers on behalf of a local account.
    pub fn get_authenticated(&self, path: &str, cap: u64, token: String) -> Result<Got, String> {
        if !live_proto::bearer_allowed(&format!("{}{path}", self.api)) {
            return Err("the Live service needs HTTPS for authenticated downloads".into());
        }
        self.send(path, cap, Some(token))
    }

    fn send(&self, path: &str, cap: u64, token: Option<String>) -> Result<Got, String> {
        let mut req = self.agent.get(&format!("{}{path}", self.api));
        if let Some(t) = token {
            req = req.set("authorization", &format!("Bearer {t}"));
        }
        match req.call() {
            Ok(resp) => {
                let mut bytes = Vec::new();
                resp.into_reader()
                    .take(cap + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("reading {path}: {e}"))?;
                if bytes.len() as u64 > cap {
                    return Err(format!("{path} is larger than {cap} bytes"));
                }
                Ok(Got::Ok(bytes))
            }
            Err(ureq::Error::Status(404, _)) => Ok(Got::NotFound),
            Err(ureq::Error::Status(410, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                let why = serde_json::from_str::<live_proto::ErrorBody>(&text)
                    .map(|e| e.message)
                    .unwrap_or(text);
                Ok(Got::Gone(why))
            }
            Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code} for {path}")),
            Err(ureq::Error::Transport(t)) => Err(format!("reaching the server: {t}")),
        }
    }
}

/// Why a join on a session's map is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("this game is on {name}, built on {map}, and your disc's {map} is not the one it needs")]
    NoBase { name: String, map: String },
    /// The base is a Multiplayer Map Pack map, and the map pack isn't
    /// installed here (`h2 import-mappack`).
    #[error(
        "this game is on {name}, built on {map} from the Multiplayer Map Pack, which isn't installed here (h2 import-mappack)"
    )]
    NoMapPack { name: String, map: String },
    #[error("this game's map was taken down by the server's admins: {0}")]
    TakenDown(String),
    #[error("this game's map ({0}) is not published on the server, so it cannot be downloaded")]
    Unpublished(MapId),
    #[error("this matchmaking game is on a map its playlist does not allow ({0})")]
    Ranked(MapId),
    #[error("downloading this game's map failed: {0}")]
    Download(String),
    #[error("{0}")]
    Unavailable(String),
}

/// What a join needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// The disc's own map: retail, or a session with no `map_id`.
    Nothing,
    /// A package here already has the map.
    Local(PathBuf),
    /// Layers to fetch: `bytes` in all.
    Download {
        answer: Box<MapAnswer>,
        bytes: u64,
    },
}

/// Told the package directory after an install or removal.
pub type Installed = Box<dyn Fn(&Path) + Send + Sync>;
pub type Ready = Box<dyn Fn(MapId) -> Result<(), String> + Send + Sync>;

/// The join-time map check and download, for one maps directory.
pub struct MapGate {
    maps_dir: PathBuf,
    bases: Arc<dyn Bases>,
    has_map_pack: bool,
    installed: Mutex<Option<Installed>>,
    ready: Mutex<Option<Ready>>,
}

impl std::fmt::Debug for MapGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapGate")
            .field("maps_dir", &self.maps_dir)
            .finish_non_exhaustive()
    }
}

fn json<T: serde::de::DeserializeOwned>(f: &dyn Fetch, path: &str) -> Result<Option<T>, Refusal> {
    match f.get(path, MAX_JSON).map_err(Refusal::Download)? {
        Got::Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Refusal::Download(format!("{path}: {e}"))),
        Got::NotFound => Ok(None),
        Got::Gone(why) => Err(Refusal::TakenDown(why)),
    }
}

impl MapGate {
    /// `maps_dir` is `<data>/maps`.
    pub fn new(maps_dir: impl Into<PathBuf>, bases: Arc<dyn Bases>) -> Self {
        let has_map_pack = bases.has_map_pack();
        MapGate {
            maps_dir: maps_dir.into(),
            bases,
            has_map_pack,
            installed: Mutex::new(None),
            ready: Mutex::new(None),
        }
    }

    /// Call `f` after a download or removal so the boot updates its offers.
    pub fn on_installed(&self, f: Installed) {
        *self.installed.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Check that the package has a playable slot on this console.
    pub fn on_prepare(&self, f: Ready) {
        *self.ready.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    fn installed(&self, dir: &Path) {
        if let Some(f) = &*self.installed.lock().unwrap_or_else(|e| e.into_inner()) {
            f(dir);
        }
    }

    pub fn maps_dir(&self) -> &Path {
        &self.maps_dir
    }

    /// Installation is established at startup, when the title mounts its maps.
    pub fn has_map_pack(&self) -> bool {
        self.has_map_pack
    }

    /// The package here that has map `id`, if any (the mod repository's
    /// "Installed").
    pub fn package_of(&self, id: &MapId) -> Option<PathBuf> {
        self.local(id)
    }

    /// The retail `MapId` of the disc's `map`.
    pub fn retail_id(&self, map: &str) -> Option<MapId> {
        self.bases
            .disc_hash(map)
            .map(|h| MapId::retail(&map.to_ascii_lowercase(), h))
    }

    fn local(&self, id: &MapId) -> Option<PathBuf> {
        Library::open(&self.maps_dir)
            .packages()
            .iter()
            .find(|p| p.id == *id)
            .map(|p| p.dir.clone())
    }

    /// What joining `ad` needs, without downloading anything: the server's
    /// word on the map (taken down, unknown), the disc's base, the ranked
    /// rule, and what is already here.
    pub fn check(&self, f: &dyn Fetch, ad: &NewSession) -> Result<Need, Refusal> {
        let Some(id) = ad.map_id else {
            return Ok(Need::Nothing);
        };
        let retail = ad
            .map
            .as_deref()
            .and_then(|m| self.retail_id(m))
            .is_some_and(|r| r == id);
        if ranked_flags(&ad.flags) && !retail {
            #[derive(serde::Deserialize)]
            struct Pins {
                pins: Vec<HopperPin>,
            }
            let pinned = match json::<Pins>(f, "/hoppers/pins")? {
                Some(p) => ad
                    .hopper_id
                    .is_some_and(|h| p.pins.iter().any(|p| p.hopper_id == h && p.allows(&id))),
                None => false,
            };
            if !pinned {
                return Err(Refusal::Ranked(id));
            }
        }
        self.check_id(f, id, retail)
    }

    /// What having map `id` needs, without downloading anything: the
    /// server's word on it, the disc's base, and what is already here.
    /// `retail`: `id` is the disc's own map (nothing to fetch when the
    /// server does not know it).
    fn check_id(&self, f: &dyn Fetch, id: MapId, retail: bool) -> Result<Need, Refusal> {
        let Some(answer) = json::<MapAnswer>(f, &format!("/maps/{id}"))? else {
            if retail {
                return Ok(Need::Nothing);
            }
            return match self.local(&id) {
                Some(dir) => Ok(Need::Local(dir)),
                None => Err(Refusal::Unpublished(id)),
            };
        };
        let m = &answer.map.manifest;
        if MapId::of(m) != id {
            return Err(Refusal::Download(format!(
                "the server's manifest for {id} is another map's"
            )));
        }
        if !m.base.derived {
            let have = self.bases.disc_hash(&m.base.map);
            if have != m.base.sha256 {
                let (name, map) = (m.package.name.clone(), m.base.map.clone());
                return Err(match have {
                    None if content::map_pack::is_map(&map) => Refusal::NoMapPack { name, map },
                    _ => Refusal::NoBase { name, map },
                });
            }
        }
        if let Some(dir) = self.local(&id) {
            return Ok(Need::Local(dir));
        }
        let dir = self.package_dir(&id);
        let bytes = answer
            .map
            .summary
            .layers
            .iter()
            .zip(m.patches.iter().chain(&m.packs))
            .filter(|(_, l)| !has(&dir, &l.file, &l.sha256))
            .map(|(info, _)| info.size)
            .sum();
        Ok(Need::Download {
            answer: Box::new(answer),
            bytes,
        })
    }

    /// Where the package for `id` is written.
    pub fn package_dir(&self, id: &MapId) -> PathBuf {
        self.maps_dir
            .join(format!("{DIR_PREFIX}{}", &id.to_string()[..16]))
    }

    /// [`check`](Self::check), then fetch what is missing, verify every
    /// byte, and write the package. Verify it has a slot before joining.
    pub fn prepare(&self, f: &dyn Fetch, ad: &NewSession) -> Result<Option<PathBuf>, Refusal> {
        let need = self.check(f, ad)?;
        let dir = self.fetch(f, need, ad.map_id, &mut |_, _| {})?;
        if let (Some(_), Some(id), Some(ready)) = (
            &dir,
            ad.map_id,
            self.ready
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref(),
        ) {
            ready(id).map_err(Refusal::Unavailable)?;
        }
        Ok(dir)
    }

    /// Install published map `id` from the mod repository (docs/game/maps.md
    /// §5, "Mod downloads"): the join's checks and download, with no
    /// session. `progress` is told the bytes written so far and the bytes
    /// to fetch in all, after each layer. Returns the package directory.
    pub fn install(
        &self,
        f: &dyn Fetch,
        id: MapId,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, Refusal> {
        let need = self.check_id(f, id, false)?;
        self.fetch(f, need, Some(id), progress)?
            .ok_or(Refusal::Unpublished(id))
    }

    /// Removes the package for map `id` that a download put here (the Mods
    /// screen's Remove, docs/game/maps.md §5 "A mod's page"): only a
    /// `live-<id>` directory of the maps directory, never a package the
    /// player put there by hand. The boot then removes its offer.
    /// Returns the directory removed.
    pub fn remove(&self, id: &MapId) -> Result<PathBuf, String> {
        let lib = Library::open(&self.maps_dir);
        let pkg = lib
            .packages()
            .iter()
            .find(|p| p.id == *id)
            .ok_or_else(|| format!("map {id} is not installed"))?;
        let dir = pkg.dir.clone();
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with(DIR_PREFIX) || dir.parent() != Some(self.maps_dir.as_path()) {
            return Err(format!(
                "{} was not downloaded from Live; remove it by hand",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.installed(&dir);
        Ok(dir)
    }

    /// Fetch and verify any missing layers, then tell the boot.
    fn fetch(
        &self,
        f: &dyn Fetch,
        need: Need,
        map_id: Option<MapId>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Option<PathBuf>, Refusal> {
        let (answer, total) = match need {
            Need::Nothing => return Ok(None),
            Need::Local(dir) => return Ok(Some(dir)),
            Need::Download { answer, bytes } => (answer, bytes),
        };
        let mut done = 0u64;
        progress(done, total);
        let m: &Manifest = &answer.map.manifest;
        let id = MapId::of(m);
        let dir = self.package_dir(&id);
        let io = |e: std::io::Error| Refusal::Download(format!("{}: {e}", dir.display()));
        std::fs::create_dir_all(&dir).map_err(io)?;
        for (file, sha) in m
            .patches
            .iter()
            .chain(&m.packs)
            .map(|l| (&l.file, &l.sha256))
        {
            if has(&dir, file, sha) {
                continue;
            }
            let bytes = self.get_layer(f, file, sha)?;
            let rel = relative(file).map_err(|e| Refusal::Download(e.to_string()))?;
            write_atomic(&dir.join(rel), &bytes).map_err(io)?;
            done += bytes.len() as u64;
            progress(done, total.max(done));
        }
        // The preview is display data, outside the MapId: one the server
        // cannot give (an admin hid it, a bad copy) is left out of the
        // package written, never the reason a map cannot be had.
        let mut written = m.clone();
        if let Some((file, sha)) = m
            .package
            .image
            .as_ref()
            .zip(answer.map.summary.image.as_ref())
            && !has(&dir, file, sha)
        {
            let got = self.get_layer(f, file, sha).and_then(|bytes| {
                let rel = relative(file).map_err(|e| Refusal::Download(e.to_string()))?;
                write_atomic(&dir.join(rel), &bytes).map_err(io)
            });
            if let Err(why) = got {
                eprintln!("live: map {id}: no preview image ({why})");
                written.package.image = None;
            }
        } else if m.package.image.is_some() && answer.map.summary.image.is_none() {
            written.package.image = None;
        }
        debug_assert_eq!(MapId::of(&written), id);
        write_atomic(&dir.join(MANIFEST_FILE), written.to_toml().as_bytes()).map_err(io)?;
        if self.local(&id).is_none() {
            return Err(Refusal::Download(format!(
                "the package written to {} does not resolve to {id}",
                dir.display()
            )));
        }
        self.installed(&dir);
        self.count(f, map_id);
        Ok(Some(dir))
    }

    /// The blob `sha` (the package's `file`), checked against its hash.
    fn get_layer(&self, f: &dyn Fetch, file: &str, sha: &Sha256) -> Result<Vec<u8>, Refusal> {
        let bytes = match f
            .get(&format!("/content/{sha}"), MAX_DOWNLOAD)
            .map_err(Refusal::Download)?
        {
            Got::Ok(b) => b,
            Got::NotFound => {
                return Err(Refusal::Download(format!(
                    "the server no longer has {file}"
                )));
            }
            Got::Gone(why) => return Err(Refusal::TakenDown(why)),
        };
        if Sha256::of(&bytes) != *sha {
            return Err(Refusal::Download(format!(
                "{file} from the server does not match its SHA-256"
            )));
        }
        Ok(bytes)
    }

    /// Tells the server this account has the map now: the manifest asked
    /// for again as a counted GET ([`Fetch::get_counted`]). The server
    /// counts each account once per package, so a reinstall or a second
    /// join adds nothing. Its answer is not needed; a failure only goes
    /// uncounted.
    fn count(&self, f: &dyn Fetch, map_id: Option<MapId>) {
        if let Some(id) = map_id
            && let Err(e) = f.get_counted(&format!("/maps/{id}"), MAX_JSON)
        {
            eprintln!("live: map {id}: the download was not counted: {e}");
        }
    }
}

/// Whether `dir/file` holds bytes hashing to `sha`.
fn has(dir: &Path, file: &str, sha: &Sha256) -> bool {
    relative(file)
        .ok()
        .and_then(|r| hash_file(&dir.join(r)).ok())
        .is_some_and(|h| h == *sha)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One name per write, not per process: a join and a Mods screen
    // install of the same map may write the same file at once.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("part-{}-{n}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::manifest::Layer;
    use live_proto::maps::{BaseInfo, LayerInfo, MapDetail, MapSummary, Who};
    use std::collections::BTreeMap;

    struct Disc(HashMap<String, Sha256>);
    impl Bases for Disc {
        fn disc_hash(&self, map: &str) -> Option<Sha256> {
            self.0.get(map).copied()
        }
    }

    /// A fake server: paths to answers, and every path asked for.
    #[derive(Default)]
    struct Server {
        answers: HashMap<String, Got>,
        asked: Mutex<Vec<String>>,
        counted: Mutex<Vec<String>>,
    }
    impl Fetch for Server {
        fn get(&self, path: &str, _cap: u64) -> Result<Got, String> {
            self.asked.lock().unwrap().push(path.into());
            Ok(self.answers.get(path).cloned().unwrap_or(Got::NotFound))
        }
        fn get_counted(&self, path: &str, cap: u64) -> Result<Got, String> {
            self.counted.lock().unwrap().push(path.into());
            self.get(path, cap)
        }
    }

    const BASE: Sha256 = Sha256([0x11; 32]);

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2-hle-maps-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn modded(patch: &[u8]) -> Manifest {
        let mut m = Manifest::retail("lockout", BASE);
        m.package.name = "Lockout Snipers".into();
        m.patches = vec![Layer {
            file: "patches/snipers.ips".into(),
            sha256: Sha256::of(patch),
        }];
        m
    }

    fn answer(m: &Manifest, size: u64) -> MapAnswer {
        MapAnswer {
            map: MapDetail {
                summary: MapSummary {
                    mod_pack: None,
                    map_id: MapId::of(m),
                    name: m.package.name.clone(),
                    description: None,
                    author: Who {
                        xuid: live_proto::Xuid(9),
                        gamertag: "Modder".into(),
                    },
                    base: BaseInfo {
                        map: m.base.map.clone(),
                        sha256: m.base.sha256,
                        derived: m.base.derived,
                    },
                    layers: vec![LayerInfo {
                        sha256: m.patches[0].sha256,
                        size,
                        kind: "patch".into(),
                    }],
                    size,
                    image: None,
                    features: vec![],
                    gametypes: vec![],
                    version: None,
                    geometry: None,
                    published_at: "2026-11-16T00:00:00Z".into(),
                    position: 1,
                    downloads: 0,
                    credit: None,
                    category: None,
                    source_url: None,
                    credits: None,
                    images: vec![],
                    contents: vec![],
                    pack: None,
                    pack_name: None,
                },
                manifest: m.clone(),
                report: None,
                pack_info: None,
            },
            missing: vec![],
            position: 1,
        }
    }

    fn ad(flags: &str, map_id: Option<MapId>) -> NewSession {
        NewSession {
            addr: None,
            port: 3074,
            host_address: vec![0; 36],
            slots: Default::default(),
            attributes: BTreeMap::new(),
            flags: flags.into(),
            hopper_id: Some(2),
            map: Some("lockout".into()),
            name: None,
            language: None,
            visibility: Default::default(),
            map_id,
            skulls: None,
            campaign: None,
            ping_ms: None,
            variant_ext: None,
            browser: None,
        }
    }

    fn gate(maps: &Path, disc: bool) -> MapGate {
        let mut d = HashMap::new();
        if disc {
            d.insert("lockout".to_string(), BASE);
        }
        MapGate::new(maps, Arc::new(Disc(d)))
    }

    fn server_with(m: &Manifest, patch: &[u8]) -> Server {
        let id = MapId::of(m);
        let mut s = Server::default();
        s.answers.insert(
            format!("/maps/{id}"),
            Got::Ok(serde_json::to_vec(&answer(m, patch.len() as u64)).unwrap()),
        );
        s.answers.insert(
            format!("/content/{}", Sha256::of(patch)),
            Got::Ok(patch.to_vec()),
        );
        s
    }

    #[test]
    fn retail_and_unannounced_maps_need_nothing() {
        let maps = dir("retail");
        let g = gate(&maps, true);
        let s = Server::default();
        assert_eq!(g.check(&s, &ad("custom", None)).unwrap(), Need::Nothing);
        let retail = MapId::retail("lockout", BASE);
        assert_eq!(g.check(&s, &ad("ranked", Some(retail))).unwrap(), Need::Nothing);
        g.on_installed(Box::new(|_| panic!("nothing was installed")));
        assert_eq!(g.prepare(&s, &ad("custom", Some(retail))).unwrap(), None);
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn a_published_mod_is_downloaded_without_activating_its_base() {
        let maps = dir("download");
        let patch = b"PATCH invented";
        let m = modded(patch);
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        let g = gate(&maps, true);
        let told: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let sink = Arc::clone(&told);
        g.on_installed(Box::new(move |d| sink.lock().unwrap().push(d.to_owned())));
        match g.check(&s, &ad("custom", Some(id))).unwrap() {
            Need::Download { bytes, .. } => assert_eq!(bytes, patch.len() as u64),
            other => panic!("{other:?}"),
        }
        assert!(told.lock().unwrap().is_empty(), "a check installs nothing");
        let dir = g.prepare(&s, &ad("custom", Some(id))).unwrap().unwrap();
        assert_eq!(dir, maps.join(format!("live-{}", &id.to_string()[..16])));
        assert_eq!(*told.lock().unwrap(), std::slice::from_ref(&dir), "the boot is told");
        assert_eq!(std::fs::read(dir.join("patches/snipers.ips")).unwrap(), patch);
        let lib = Library::open(&maps);
        assert_eq!(lib.packages()[0].id, id);
        assert!(!maps.join("active.toml").exists());
        // Now it is here: nothing more is fetched.
        s.asked.lock().unwrap().clear();
        assert_eq!(g.check(&s, &ad("custom", Some(id))).unwrap(), Need::Local(lib.packages()[0].dir.clone()));
        assert!(!s.asked.lock().unwrap().iter().any(|p| p.starts_with("/content/")));
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn a_taken_down_image_does_not_block_the_map() {
        let maps = dir("image");
        let patch = b"PATCH invented with a picture";
        let mut m = modded(patch);
        m.package.image = Some("preview.png".into());
        let id = MapId::of(&m);
        let img = Sha256([0x55; 32]);
        let mut s = server_with(&m, patch);
        let mut a = answer(&m, patch.len() as u64);
        a.map.summary.image = Some(img);
        s.answers.insert(
            format!("/maps/{id}"),
            Got::Ok(serde_json::to_vec(&a).unwrap()),
        );
        s.answers.insert(
            format!("/content/{img}"),
            Got::Gone("an admin hid it".into()),
        );
        let g = gate(&maps, true);
        let got = g.prepare(&s, &ad("custom", Some(id))).unwrap().unwrap();
        assert!(
            s.asked.lock().unwrap().contains(&format!("/content/{img}")),
            "it was tried"
        );
        let lib = Library::open(&maps);
        assert_eq!(lib.packages().len(), 1);
        assert_eq!(lib.packages()[0].id, id, "the same map, with no picture");
        assert_eq!(lib.packages()[0].manifest.package.image, None);
        assert!(!got.join("preview.png").exists());

        // A good picture is kept.
        let maps2 = dir("image-ok");
        let png = b"\x89PNG\r\n\x1a\ninvented".to_vec();
        let mut a = answer(&m, patch.len() as u64);
        a.map.summary.image = Some(Sha256::of(&png));
        let mut s = server_with(&m, patch);
        s.answers.insert(
            format!("/maps/{id}"),
            Got::Ok(serde_json::to_vec(&a).unwrap()),
        );
        s.answers.insert(
            format!("/content/{}", Sha256::of(&png)),
            Got::Ok(png.clone()),
        );
        let dir = gate(&maps2, true)
            .prepare(&s, &ad("custom", Some(id)))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(dir.join("preview.png")).unwrap(), png);
        let lib = Library::open(&maps2);
        assert_eq!(
            lib.packages()[0].manifest.package.image.as_deref(),
            Some("preview.png")
        );
        std::fs::remove_dir_all(&maps).unwrap();
        std::fs::remove_dir_all(&maps2).unwrap();
    }

    #[test]
    fn preparing_a_package_already_served_writes_rescans_and_counts_nothing() {
        let maps = dir("again");
        let patch = b"PATCH invented, asked for again";
        let m = modded(patch);
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        let g = gate(&maps, true);
        let told: Arc<Mutex<usize>> = Arc::default();
        let sink = Arc::clone(&told);
        g.on_installed(Box::new(move |_| *sink.lock().unwrap() += 1));
        g.prepare(&s, &ad("custom", Some(id))).unwrap().unwrap();
        assert_eq!(
            (*told.lock().unwrap(), s.counted.lock().unwrap().len()),
            (1, 1)
        );
        let active = maps.join("active.toml");
        let legacy = "lockout = \"someone-else\"\n";
        std::fs::write(&active, legacy).unwrap();
        for _ in 0..5 {
            g.prepare(&s, &ad("custom", Some(id))).unwrap().unwrap();
        }
        assert_eq!(*told.lock().unwrap(), 1, "no rescan");
        assert_eq!(s.counted.lock().unwrap().len(), 1, "no counted GETs");
        assert_eq!(std::fs::read_to_string(&active).unwrap(), legacy);
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn a_downloaded_map_without_a_slot_refuses_the_join() {
        let maps = dir("full");
        let patch = b"PATCH invented for a full list";
        let m = modded(patch);
        let id = m.map_id();
        let s = server_with(&m, patch);
        let g = gate(&maps, true);
        g.on_prepare(Box::new(move |wanted| {
            assert_eq!(wanted, id);
            Err("the map list is full".into())
        }));
        assert_eq!(
            g.prepare(&s, &ad("custom", Some(id))),
            Err(Refusal::Unavailable("the map list is full".into()))
        );
        assert!(g.package_of(&id).is_some());
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn writes_from_two_threads_do_not_trip_over_each_other() {
        let d = dir("race");
        let path = d.join("same.bin");
        let threads: Vec<_> = (0..8u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        write_atomic(&path, &[i; 4096]).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() == 4096 && bytes.iter().all(|b| *b == bytes[0]));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no parts left");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A one-request HTTP server that sends `body` in `chunks` pieces,
    /// `gap` apart.
    fn trickle(body: Vec<u8>, chunks: usize, gap: std::time::Duration) -> String {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = c.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            c.write_all(head.as_bytes()).unwrap();
            for piece in body.chunks(body.len().div_ceil(chunks)) {
                std::thread::sleep(gap);
                if c.write_all(piece).is_err() {
                    return;
                }
                let _ = c.flush();
            }
        });
        api
    }

    #[test]
    fn a_download_outlasts_the_workers_whole_request_timeout() {
        let body = vec![7u8; 64 * 1024];
        let gap = std::time::Duration::from_millis(250);
        // The worker's agent: the whole request in 500 ms.
        let worker = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_millis(500))
            .build();
        let api = trickle(body.clone(), 6, gap);
        let f = HttpFetch {
            agent: &worker,
            api: &api,
            reader: None,
        };
        assert!(
            f.get("/content/x", MAX_DOWNLOAD).is_err(),
            "times out mid-body"
        );
        // The download agent: bytes keep coming, so it finishes.
        let agent = download_agent();
        let api = trickle(body.clone(), 6, gap);
        let f = HttpFetch {
            agent: &agent,
            api: &api,
            reader: None,
        };
        assert_eq!(f.get("/content/x", MAX_DOWNLOAD).unwrap(), Got::Ok(body));
    }

    #[test]
    fn the_mod_repository_installs_a_map_by_id_with_progress() {
        let maps = dir("install");
        let patch = b"PATCH invented for the mod repository";
        let m = modded(patch);
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        let g = gate(&maps, true);
        assert_eq!(g.package_of(&id), None);
        let told: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let sink = Arc::clone(&told);
        g.on_installed(Box::new(move |d| sink.lock().unwrap().push(d.to_owned())));
        let mut seen = Vec::new();
        let dir = g.install(&s, id, &mut |done, total| seen.push((done, total))).unwrap();
        let n = patch.len() as u64;
        assert_eq!(seen, vec![(0, n), (n, n)], "before and after the one layer");
        assert_eq!(std::fs::read(dir.join("patches/snipers.ips")).unwrap(), patch);
        assert_eq!(g.package_of(&id), Some(dir.clone()));
        assert_eq!(*told.lock().unwrap(), std::slice::from_ref(&dir), "the boot is told");
        let manifest = format!("/maps/{id}");
        assert_eq!(
            *s.counted.lock().unwrap(),
            std::slice::from_ref(&manifest),
            "the download is counted once installed"
        );
        assert!(!maps.join("active.toml").exists());
        // Installed already: nothing is fetched again.
        s.asked.lock().unwrap().clear();
        let mut again = Vec::new();
        assert_eq!(g.install(&s, id, &mut |d, t| again.push((d, t))).unwrap(), dir);
        assert!(again.is_empty());
        assert!(!s.asked.lock().unwrap().iter().any(|p| p.starts_with("/content/")));
        // Already the one served: not counted again (the server would
        // count an account once per package anyway).
        assert_eq!(s.counted.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn the_mod_repository_says_which_disc_map_is_missing() {
        let maps = dir("install-nobase");
        let patch = b"PATCH invented";
        let m = modded(patch);
        let s = server_with(&m, patch);
        let err = gate(&maps, false).install(&s, MapId::of(&m), &mut |_, _| {}).unwrap_err();
        assert!(s.counted.lock().unwrap().is_empty(), "nothing installed: nothing counted");
        assert_eq!(
            err,
            Refusal::NoBase {
                name: "Lockout Snipers".into(),
                map: "lockout".into()
            }
        );
        // Not on the server at all.
        let gone = Server::default();
        let err = gate(&maps, true).install(&gone, MapId::of(&m), &mut |_, _| {}).unwrap_err();
        assert_eq!(err, Refusal::Unpublished(MapId::of(&m)));
        std::fs::remove_dir_all(&maps).unwrap();
    }

    /// A mod on a Map Pack base (Relic, `dune`): installed when the map
    /// pack's copy hashes as the manifest says, refused as `NoMapPack`
    /// when the map pack isn't installed, `NoBase` when it is but differs.
    #[test]
    fn a_map_pack_base_needs_the_installed_map_pack() {
        let maps = dir("install-mappack");
        let patch = b"PATCH invented for relic";
        let mut m = modded(patch);
        m.base.map = "dune".into();
        m.package.name = "Relic Rockets".into();
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        let pack = |h: Option<Sha256>| {
            let d: HashMap<String, Sha256> =
                h.map(|h| ("dune".to_string(), h)).into_iter().collect();
            MapGate::new(&maps, Arc::new(Disc(d)))
        };
        let err = pack(None).install(&s, id, &mut |_, _| {}).unwrap_err();
        assert_eq!(
            err,
            Refusal::NoMapPack {
                name: "Relic Rockets".into(),
                map: "dune".into()
            }
        );
        assert!(err.to_string().contains("Multiplayer Map Pack"), "{err}");
        assert!(err.to_string().contains("h2 import-mappack"), "{err}");
        assert!(matches!(
            pack(Some(Sha256([0x22; 32]))).install(&s, id, &mut |_, _| {}),
            Err(Refusal::NoBase { .. })
        ));
        assert_eq!(Library::open(&maps).packages().len(), 0, "nothing written");
        let dir = pack(Some(BASE)).install(&s, id, &mut |_, _| {}).unwrap();
        assert_eq!(
            std::fs::read(dir.join("patches/snipers.ips")).unwrap(),
            patch
        );
        assert!(!maps.join("active.toml").exists());
        // A guest joining a game on it gets the same package.
        let mut join = ad("custom", Some(id));
        join.map = Some("dune".into());
        let here = Library::open(&maps).packages()[0].dir.clone();
        assert_eq!(pack(Some(BASE)).prepare(&s, &join).unwrap(), Some(here));
        assert!(matches!(
            pack(None).check(&s, &join),
            Err(Refusal::NoMapPack { .. })
        ));
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn bad_bytes_from_the_server_are_refused() {
        let maps = dir("badbytes");
        let patch = b"PATCH invented";
        let m = modded(patch);
        let id = MapId::of(&m);
        let mut s = server_with(&m, patch);
        s.answers.insert(
            format!("/content/{}", Sha256::of(patch)),
            Got::Ok(b"PATCH tampered".to_vec()),
        );
        let err = gate(&maps, true).prepare(&s, &ad("custom", Some(id))).unwrap_err();
        assert!(matches!(err, Refusal::Download(ref w) if w.contains("SHA-256")), "{err}");
        assert!(Library::open(&maps).is_empty(), "no package is left half written");
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn refusals_say_why() {
        let maps = dir("refuse");
        let patch = b"PATCH invented";
        let m = modded(patch);
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        // No disc map for the base.
        let err = gate(&maps, false).check(&s, &ad("custom", Some(id))).unwrap_err();
        assert_eq!(
            err,
            Refusal::NoBase {
                name: "Lockout Snipers".into(),
                map: "lockout".into()
            }
        );
        assert!(err.to_string().contains("your disc's lockout"));
        // Taken down.
        let mut gone = Server::default();
        gone.answers
            .insert(format!("/maps/{id}"), Got::Gone("map is blocked by an admin".into()));
        let err = gate(&maps, true).check(&gone, &ad("custom", Some(id))).unwrap_err();
        assert!(matches!(err, Refusal::TakenDown(_)));
        // Unknown to the server and not here.
        let err = gate(&maps, true)
            .check(&Server::default(), &ad("custom", Some(MapId([5; 32]))))
            .unwrap_err();
        assert!(matches!(err, Refusal::Unpublished(_)));
        // Ranked: only retail or pinned.
        let err = gate(&maps, true).check(&s, &ad("ranked", Some(id))).unwrap_err();
        assert_eq!(err, Refusal::Ranked(id));
        let mut pinned = server_with(&m, patch);
        pinned.answers.insert(
            "/hoppers/pins".into(),
            Got::Ok(
                serde_json::to_vec(&serde_json::json!({"pins":[{"hopper_id":2,"map_ids":[id]}],"position":1}))
                    .unwrap(),
            ),
        );
        assert!(matches!(
            gate(&maps, true).check(&pinned, &ad("hopper", Some(id))).unwrap(),
            Need::Download { .. }
        ));
        std::fs::remove_dir_all(&maps).unwrap();
    }

    #[test]
    fn disc_maps_hash_once_and_match_any_case() {
        let d = dir("disc");
        std::fs::write(d.join("Lockout.map"), b"daeh invented").unwrap();
        let disc = DiscMaps::new(&d);
        assert_eq!(disc.disc_hash("lockout"), Some(Sha256::of(b"daeh invented")));
        std::fs::remove_file(d.join("Lockout.map")).unwrap();
        assert_eq!(disc.disc_hash("LOCKOUT"), Some(Sha256::of(b"daeh invented")), "cached");
        assert_eq!(disc.disc_hash("midship"), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_downloaded_mod_is_removed_without_touching_legacy_choices() {
        let maps = dir("remove");
        let patch = b"PATCH invented to be removed";
        let m = modded(patch);
        let id = MapId::of(&m);
        let s = server_with(&m, patch);
        let g = gate(&maps, true);
        let dir = g.install(&s, id, &mut |_, _| {}).unwrap();
        let told: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let sink = Arc::clone(&told);
        g.on_installed(Box::new(move |d| sink.lock().unwrap().push(d.to_owned())));
        let active = maps.join("active.toml");
        let legacy = "lockout = \"some-old-choice\"\n";
        std::fs::write(&active, legacy).unwrap();
        assert_eq!(g.remove(&id).unwrap(), dir);
        assert!(!dir.exists());
        assert_eq!(g.package_of(&id), None);
        assert_eq!(std::fs::read_to_string(&active).unwrap(), legacy);
        assert_eq!(*told.lock().unwrap(), [dir], "the boot rescans");
        assert!(g.remove(&id).unwrap_err().contains("not installed"));
        // A package put here by hand is never removed.
        let own = maps.join("my-lockout");
        std::fs::create_dir_all(&own).unwrap();
        let by_hand = b"PATCH invented by hand";
        std::fs::write(own.join("patches.ips"), by_hand).unwrap();
        let mut hand = m.clone();
        hand.patches[0].file = "patches.ips".into();
        hand.patches[0].sha256 = Sha256::of(by_hand);
        std::fs::write(own.join(MANIFEST_FILE), hand.to_toml()).unwrap();
        let hand_id = MapId::of(&hand);
        assert_eq!(g.package_of(&hand_id), Some(own.clone()));
        assert!(g.remove(&hand_id).unwrap_err().contains("by hand"));
        assert!(own.exists());
        std::fs::remove_dir_all(&maps).unwrap();
    }
}
