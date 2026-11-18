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

/// The user's own disc maps, for the base of a package and for retail ids.
pub trait Bases: Send + Sync {
    /// SHA-256 of the disc's `maps\<map>.map`, if the disc has it.
    fn disc_hash(&self, map: &str) -> Option<Sha256>;
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
}

/// [`Fetch`] over HTTP with `ureq`, against an API base like
/// `http://host:8074/v1`.
pub struct HttpFetch<'a> {
    pub agent: &'a ureq::Agent,
    pub api: &'a str,
}

impl Fetch for HttpFetch<'_> {
    fn get(&self, path: &str, cap: u64) -> Result<Got, String> {
        match self.agent.get(&format!("{}{path}", self.api)).call() {
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
    #[error("this game's map was taken down by the server's admins: {0}")]
    TakenDown(String),
    #[error("this game's map ({0}) is not published on the server, so it cannot be downloaded")]
    Unpublished(MapId),
    #[error("this matchmaking game is on a map its playlist does not allow ({0})")]
    Ranked(MapId),
    #[error("downloading this game's map failed: {0}")]
    Download(String),
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

/// Told the package directory after [`MapGate::prepare`] wrote or chose a
/// package (see [`MapGate::on_installed`]).
pub type Installed = Box<dyn Fn(&Path) + Send + Sync>;

/// The join-time map check and download, for one maps directory.
pub struct MapGate {
    maps_dir: PathBuf,
    bases: Arc<dyn Bases>,
    installed: Mutex<Option<Installed>>,
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
        MapGate {
            maps_dir: maps_dir.into(),
            bases,
            installed: Mutex::new(None),
        }
    }

    /// Call `f` after each [`prepare`](Self::prepare) that wrote a package
    /// or named one in `active.toml`: the boot rescans the VFS's packages
    /// (`vfs::Vfs::rescan_maps`), so the map is served at its next open.
    /// Replaces any earlier callback.
    pub fn on_installed(&self, f: Installed) {
        *self.installed.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    fn installed(&self, dir: &Path) {
        if let Some(f) = &*self.installed.lock().unwrap_or_else(|e| e.into_inner()) {
            f(dir);
        }
    }

    pub fn maps_dir(&self) -> &Path {
        &self.maps_dir
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
        if !m.base.derived && self.bases.disc_hash(&m.base.map) != m.base.sha256 {
            return Err(Refusal::NoBase {
                name: m.package.name.clone(),
                map: m.base.map.clone(),
            });
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
    /// byte, write the package, and make it the one served for its map
    /// (`active.toml`). Returns the package directory (None: the disc's
    /// map).
    pub fn prepare(&self, f: &dyn Fetch, ad: &NewSession) -> Result<Option<PathBuf>, Refusal> {
        let (answer, _) = match self.check(f, ad)? {
            Need::Nothing => return Ok(None),
            Need::Local(dir) => {
                self.activate(&dir, ad)?;
                self.installed(&dir);
                return Ok(Some(dir));
            }
            Need::Download { answer, bytes } => (answer, bytes),
        };
        let m: &Manifest = &answer.map.manifest;
        let id = MapId::of(m);
        let dir = self.package_dir(&id);
        let io = |e: std::io::Error| Refusal::Download(format!("{}: {e}", dir.display()));
        std::fs::create_dir_all(&dir).map_err(io)?;
        let image = m
            .package
            .image
            .as_ref()
            .zip(answer.map.summary.image.as_ref());
        for (file, sha) in m
            .patches
            .iter()
            .chain(&m.packs)
            .map(|l| (&l.file, &l.sha256))
            .chain(image)
        {
            if has(&dir, file, sha) {
                continue;
            }
            let bytes = match f
                .get(&format!("/content/{sha}"), MAX_DOWNLOAD)
                .map_err(Refusal::Download)?
            {
                Got::Ok(b) => b,
                Got::NotFound => {
                    return Err(Refusal::Download(format!("the server no longer has {file}")));
                }
                Got::Gone(why) => return Err(Refusal::TakenDown(why)),
            };
            if Sha256::of(&bytes) != *sha {
                return Err(Refusal::Download(format!(
                    "{file} from the server does not match its SHA-256"
                )));
            }
            let rel = relative(file).map_err(|e| Refusal::Download(e.to_string()))?;
            write_atomic(&dir.join(rel), &bytes).map_err(io)?;
        }
        write_atomic(&dir.join(MANIFEST_FILE), m.to_toml().as_bytes()).map_err(io)?;
        if self.local(&id).is_none() {
            return Err(Refusal::Download(format!(
                "the package written to {} does not resolve to {id}",
                dir.display()
            )));
        }
        self.activate(&dir, ad)?;
        self.installed(&dir);
        Ok(Some(dir))
    }

    /// Names `dir` in `active.toml` for its map, so the vfs serves it
    /// rather than another package for the same map (docs/game/maps.md §7,
    /// "Choice rule").
    fn activate(&self, dir: &Path, ad: &NewSession) -> Result<(), Refusal> {
        let Some(pkg) = Library::open(&self.maps_dir)
            .packages()
            .iter()
            .find(|p| Some(p.id) == ad.map_id)
            .cloned()
        else {
            return Ok(());
        };
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let path = self.maps_dir.join(content::ACTIVE_FILE);
        let mut table: toml::Table = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or_default();
        let key = pkg.manifest.base.map.to_ascii_lowercase();
        if table.get(&key).and_then(|v| v.as_str()) == Some(name.as_str()) {
            return Ok(());
        }
        table.insert(key, toml::Value::String(name));
        write_atomic(&path, toml::to_string(&table).unwrap_or_default().as_bytes())
            .map_err(|e| Refusal::Download(format!("{}: {e}", path.display())))
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
    let tmp = path.with_extension(format!("part-{}", std::process::id()));
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
    }
    impl Fetch for Server {
        fn get(&self, path: &str, _cap: u64) -> Result<Got, String> {
            self.asked.lock().unwrap().push(path.into());
            Ok(self.answers.get(path).cloned().unwrap_or(Got::NotFound))
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
                    geometry: None,
                    published_at: "2026-11-16T00:00:00Z".into(),
                    position: 1,
                    downloads: 0,
                    credit: None,
                    category: None,
                    source_url: None,
                },
                manifest: m.clone(),
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
    fn a_published_mod_is_downloaded_verified_and_activated() {
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
        let active = std::fs::read_to_string(maps.join("active.toml")).unwrap();
        assert!(active.contains(&format!("lockout = \"live-{}\"", &id.to_string()[..16])), "{active}");
        // Now it is here: nothing more is fetched.
        s.asked.lock().unwrap().clear();
        assert_eq!(g.check(&s, &ad("custom", Some(id))).unwrap(), Need::Local(lib.packages()[0].dir.clone()));
        assert!(!s.asked.lock().unwrap().iter().any(|p| p.starts_with("/content/")));
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
}
