//! `maps.test.*`: purpose-built test maps from recipes, built from the
//! user's own disc and loaded by the running game (docs/game/test-maps.md).
//!
//! - `maps.test.create` checks the recipe, then builds it on a thread of its
//!   own: the base map from the disc, built by the [`Recipes`] the caller
//!   gives (boot's `test_maps_recipes`, over `scenario_edit::recipe`)
//!   into `<data>/maps/test-<name>/`, then the offers are installed again
//!   (`map_select::packages_changed`, else `new_maps::install_at_boot`) and
//!   the VFS rescans, so the map has a multiplayer slot of its own, as a
//!   Live download does (docs/game/maps.md §3).
//! - `maps.test.load` finds that slot in the list and queues a local
//!   multiplayer game on it with the level boot's steps
//!   (`level_boot::steps`, `level_boot::fill_options`), made by the agent's
//!   guest calls at the next tick (`agent::sim::calls::Call::QueueGame`).
//! - `maps.test.list` and `maps.test.remove` read and delete them.
//!
//! Build reports (what was placed where, tag names from the user's disc)
//! are kept in the cache directory (`test-maps/<MapId>.json`), never in
//! the repository.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use agent::sim::calls::{self, Arg, Call, GameStep};
use agent::{Access, Agent, GameCtx};
use agent_proto::test_maps::{
    CreateParams, CreateResult, ListParams, ListResult, Listed, LoadResult, NameParams,
    RecipeInfo, RemoveResult, State, TestMap,
};
use agent_proto::{RpcError, code, method};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::level_boot::{self, Kind, Request, Step};
use crate::new_maps::Disc as _;

/// Ticks `maps.test.load` waits for the guest calls (one tick does them).
const CALL_TICKS: u64 = 30;
/// A test map's package directory starts with this.
const PREFIX: &str = "test-";

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| invalid(e.to_string()))
}

/// A recipe, resolved: what the methods need to know before a build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub base: String,
    /// The package's directory under the maps directory.
    pub package_dir: String,
}

/// A disc map to build on: a file, or a region of the disc image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseMap {
    pub path: PathBuf,
    /// `(offset, length)` inside `path`, or `None` for the whole file.
    pub region: Option<(u64, u64)>,
}

/// A finished build: the package's `MapId` and the build report.
#[derive(Clone, Debug)]
pub struct Built {
    pub map_id: String,
    pub report: Value,
}

/// Recipes and their builder. `game` may not use the scenario editor, so
/// the app that links it implements this (boot's `test_maps_recipes`) and
/// hands it to [`install`]. A recipe is named (`Value::String`, a built-in)
/// or given whole (`Value::Object`), on its own base or `base`; a
/// `String` error is shown to the caller.
pub trait Recipes: Send + Sync + 'static {
    /// The built-in recipes, for `maps.test.list`.
    fn built_in(&self) -> Result<Vec<RecipeInfo>, String>;
    /// Checks a recipe and names its package.
    fn resolve(&self, recipe: &Value, base: Option<&str>) -> Result<Resolved, String>;
    /// Builds the recipe into a package under `data_root`'s maps directory.
    fn build(
        &self,
        recipe: &Value,
        base: Option<&str>,
        source: BaseMap,
        data_root: &Path,
    ) -> Result<Built, String>;
}

/// Where test maps come from and go, and what builds them.
#[derive(Clone)]
struct Paths {
    disc: PathBuf,
    data_root: PathBuf,
    recipes: Arc<dyn Recipes>,
}

impl Paths {
    fn maps(&self) -> PathBuf {
        content::store::maps_dir(&self.data_root)
    }
    fn content_root(&self) -> PathBuf {
        vfs::Config::new(&self.data_root, crate::new_maps::HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR)
    }
}

/// The state the methods share: maps building or failed this session, and
/// the disc's multiplayer list slots (read once).
#[derive(Default)]
struct Jobs {
    maps: BTreeMap<String, TestMap>,
    slots: Option<Vec<Vec<u8>>>,
}

type Shared = Arc<Mutex<Jobs>>;

fn lock(j: &Shared) -> MutexGuard<'_, Jobs> {
    j.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where a build's report is kept.
fn report_path(map_id: &str) -> PathBuf {
    agent_proto::cache_dir()
        .join("test-maps")
        .join(format!("{map_id}.json"))
}

/// The test map packages on disk (`<maps>/test-*`), with their reports.
fn on_disk(maps: &Path) -> Vec<TestMap> {
    let mut out: Vec<TestMap> = std::fs::read_dir(maps)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir = e.file_name().to_string_lossy().into_owned();
            let name = dir.strip_prefix(PREFIX)?.to_owned();
            let text =
                std::fs::read_to_string(e.path().join(content::manifest::MANIFEST_FILE)).ok()?;
            let m = content::Manifest::from_toml(&text).ok()?;
            let id = m.map_id().to_string();
            let built = std::fs::read(report_path(&id))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            Some(TestMap {
                name,
                base: m.base.map.clone(),
                state: State::Ready,
                dir,
                map_id: Some(id),
                built,
                ..TestMap::default()
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The multiplayer list's entry for a package: its alias ends with the
/// first 20 hex digits of its `MapId` (`new_maps`'s `alias`).
fn listed_in<'a>(
    maps: impl IntoIterator<Item = &'a crate::map_select::Map>,
    map_id: &str,
) -> Option<Listed> {
    let tail = map_id.get(..20)?.to_ascii_lowercase();
    maps.into_iter()
        .find(|m| m.short_name().ends_with(&tail))
        .map(|m| Listed {
            id: m.id,
            path: m.path.clone(),
        })
}

/// Every test map: those on disk, with this session's builds over them,
/// each with its slot when the list has one.
fn all_maps(paths: &Paths, jobs: &Shared) -> Vec<TestMap> {
    let mut by_name: BTreeMap<String, TestMap> = on_disk(&paths.maps())
        .into_iter()
        .map(|m| (m.name.clone(), m))
        .collect();
    let slots = {
        let mut j = lock(jobs);
        for (n, m) in &j.maps {
            if m.state != State::Ready || !by_name.contains_key(n) {
                by_name.insert(n.clone(), m.clone());
            }
        }
        if j.slots.is_none() {
            j.slots = crate::new_maps::IsoDisc::new(&paths.disc).slots().ok();
        }
        j.slots.clone().unwrap_or_default()
    };
    let cat = crate::map_select::read(&slots, &paths.content_root());
    by_name
        .into_values()
        .map(|mut m| {
            if m.state == State::Ready
                && let Some(id) = &m.map_id
            {
                m.listed = listed_in(cat.all(), id);
            }
            m
        })
        .collect()
}

/// The disc's map `base` as a source: a region of the image, or the file
/// itself in an extracted disc.
fn base_source(disc: &Path, base: &str) -> Result<BaseMap, String> {
    let file = crate::new_maps::IsoDisc::new(disc)
        .map_file(base)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("the disc has no maps\\{base}.map"))?;
    let whole = file.offset == 0
        && std::fs::metadata(&file.path).is_ok_and(|m| m.len() == file.len);
    Ok(BaseMap {
        region: (!whole).then_some((file.offset, file.len)),
        path: file.path,
    })
}

/// Builds `r` into its package (replacing an old one when asked) and keeps
/// the report. The package's `MapId` and the report.
fn build(
    paths: &Paths,
    cp: &CreateParams,
    r: &Resolved,
    replace: bool,
) -> Result<(String, Value), String> {
    let dir = paths.maps().join(&r.package_dir);
    if dir.exists() {
        if !replace {
            return Err(format!("{} exists (replace: true rebuilds it)", dir.display()));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
    }
    let source = base_source(&paths.disc, &r.base)?;
    let Built { map_id: id, report: built } = paths
        .recipes
        .build(&cp.recipe, cp.base.as_deref(), source, &paths.data_root)?;
    let report = report_path(&id);
    if let Some(parent) = report.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_vec_pretty(&built) {
        let _ = std::fs::write(&report, text);
    }
    Ok((id, built))
}

/// Installs the offers again and has the VFS look, as a Live download does.
fn publish(paths: &Paths, vfs: Option<&Arc<Mutex<vfs::Vfs>>>) {
    if crate::map_select::prepared() {
        crate::map_select::packages_changed(&paths.disc);
    } else {
        crate::new_maps::install_at_boot(&paths.data_root, &paths.disc);
    }
    if let Some(v) = vfs {
        v.lock().unwrap_or_else(|e| e.into_inner()).rescan_maps();
    }
}

/// Records the level boot's writes instead of making them.
#[derive(Default)]
struct Writes(Vec<(u32, Vec<u8>)>);

impl crate::layout_glue::Gm for Writes {
    fn rd(&self, _: u32, _: &mut [u8]) -> bool {
        false
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.push((addr, data.to_vec()));
        true
    }
}

/// The level boot's steps for `req` as guest-call steps over an options
/// block at offset 0 (`level_boot::call_of` with the block's address
/// given at the call). `None` when the options can't hold the request.
pub fn game_steps(req: &Request) -> Option<Vec<GameStep>> {
    let call = |target: u32, args: Vec<Arg>, eax: Option<Arg>, esi: Option<Arg>, report| {
        GameStep::Call {
            target,
            args,
            eax,
            esi,
            report,
        }
    };
    let opts = Arg::Options(0);
    let mut out = Vec::new();
    for step in level_boot::steps(req.kind) {
        let s = match step {
            Step::New => call(level_boot::OPTIONS_NEW, vec![], None, Some(opts), None),
            Step::Fill => {
                let mut w = Writes::default();
                if !level_boot::fill_options(&mut w, 0, req) {
                    return None;
                }
                out.extend(
                    w.0.into_iter()
                        .map(|(offset, bytes)| GameStep::Write { offset, bytes }),
                );
                continue;
            }
            Step::Variant => call(
                level_boot::VARIANT_DEFAULTS,
                vec![
                    Arg::Options(level_boot::OPT_VARIANT),
                    Arg::Value(level_boot::GAME_TYPE_SLAYER),
                ],
                None,
                None,
                None,
            ),
            Step::Players => call(
                level_boot::OPTIONS_PLAYERS,
                vec![Arg::Value(1), opts],
                None,
                None,
                None,
            ),
            Step::Check => call(level_boot::OPTIONS_CHECK, vec![opts], None, None, Some("check")),
            Step::Queue => call(level_boot::OPTIONS_QUEUE, vec![], Some(opts), None, None),
        };
        out.push(s);
    }
    Some(out)
}

/// The multiplayer request for a listed test map: a local game, default
/// Slayer, one player, as the level boot starts a disc map.
fn request(name: &str, listed: &Listed) -> Request {
    Request {
        name: name.to_owned(),
        scenario: listed.path.clone(),
        campaign: u32::MAX,
        map: listed.id,
        difficulty: level_boot::Difficulty::default(),
        kind: Kind::Multiplayer,
    }
}

fn check_name(name: &str) -> Result<(), RpcError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("name {name:?}: a test map's name (a-z, 0-9, '-')")))
    }
}

/// Registers `maps.test.list`, `create`, `load` and `remove` on `agent`.
/// `calls` is the agent's guest-call queue; without it `load` answers
/// unavailable. `recipes` builds the maps.
pub fn install(
    agent: &Agent,
    recipes: Arc<dyn Recipes>,
    calls: Option<calls::Shared>,
    disc: &Path,
    data_root: &Path,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
) {
    let paths = Paths {
        disc: disc.to_owned(),
        data_root: data_root.to_owned(),
        recipes,
    };
    let jobs = Shared::default();

    let (p, j) = (paths.clone(), jobs.clone());
    agent.add_method(method::MAPS_TEST_LIST, Access::Read, move |v| {
        let ListParams { name } = params(v)?;
        let recipes = p.recipes.built_in().map_err(unavailable)?;
        let mut maps = all_maps(&p, &j);
        if let Some(n) = &name {
            maps.retain(|m| &m.name == n);
            if maps.is_empty() {
                return Err(invalid(format!("no test map {n:?}")));
            }
        }
        Ok(json!(ListResult { recipes, maps }))
    });

    let (p, j, v_create) = (paths.clone(), jobs.clone(), vfs.clone());
    agent.add_method(method::MAPS_TEST_CREATE, Access::Act, move |v| {
        let cp: CreateParams = params(v)?;
        let r = p.recipes.resolve(&cp.recipe, cp.base.as_deref()).map_err(invalid)?;
        let dir = r.package_dir.clone();
        {
            let mut jobs = lock(&j);
            if jobs
                .maps
                .get(&r.name)
                .is_some_and(|m| m.state == State::Building)
            {
                return Err(RpcError::new(code::BUSY, format!("{} is building", r.name)));
            }
            if p.maps().join(&dir).exists() && !cp.replace {
                return Err(invalid(format!(
                    "test map {} exists (replace: true rebuilds it; maps.test.load loads it)",
                    r.name
                )));
            }
            jobs.maps.insert(
                r.name.clone(),
                TestMap {
                    name: r.name.clone(),
                    base: r.base.clone(),
                    state: State::Building,
                    dir: dir.clone(),
                    ..TestMap::default()
                },
            );
        }
        let (p2, j2, vfs) = (p.clone(), j.clone(), v_create.clone());
        let (r2, replace) = (r.clone(), cp.replace);
        let cp2 = cp.clone();
        let spawned = std::thread::Builder::new()
            .name("test map build".into())
            .spawn(move || {
                let out = build(&p2, &cp2, &r2, replace);
                if out.is_ok() {
                    publish(&p2, vfs.as_ref());
                }
                let mut jobs = lock(&j2);
                let Some(m) = jobs.maps.get_mut(&r2.name) else { return };
                match out {
                    Ok((id, built)) => {
                        eprintln!("test maps: {} built ({id})", r2.name);
                        m.state = State::Ready;
                        m.map_id = Some(id);
                        m.built = Some(built);
                    }
                    Err(e) => {
                        eprintln!("test maps: {} failed: {e}", r2.name);
                        m.state = State::Failed;
                        m.error = Some(e);
                    }
                }
            });
        if let Err(e) = spawned {
            lock(&j).maps.remove(&r.name);
            return Err(unavailable(format!("starting the build: {e}")));
        }
        Ok(json!(CreateResult {
            name: r.name,
            state: State::Building,
            dir,
        }))
    });

    let (p, j) = (paths.clone(), jobs.clone());
    agent.add_game_method(method::MAPS_TEST_LOAD, Access::Act, move |v, ctx| {
        let NameParams { name } = params(v)?;
        check_name(&name)?;
        let m = all_maps(&p, &j)
            .into_iter()
            .find(|m| m.name == name)
            .ok_or_else(|| invalid(format!("no test map {name:?} (maps.test.create builds one)")))?;
        match m.state {
            State::Building => {
                return Err(RpcError::new(code::BUSY, format!("{name} is still building")));
            }
            State::Failed => {
                return Err(unavailable(format!(
                    "{name} failed to build: {}",
                    m.error.unwrap_or_default()
                )));
            }
            State::Ready => {}
        }
        let listed = m.listed.ok_or_else(|| {
            unavailable(format!(
                "{name} has no slot in the multiplayer list (the list may be full: \
                 docs/game/maps.md §4); remove other packages or test maps"
            ))
        })?;
        let steps = game_steps(&request(&name, &listed))
            .ok_or_else(|| invalid(format!("{}: the path is longer than the options hold", listed.path)))?;
        let q = calls.clone().ok_or_else(|| {
            unavailable(
                "the game's own functions aren't reachable here: the agent-calls feature isn't \
                 installed",
            )
        })?;
        let ticket = calls::lock(&q)
            .submit(Call::QueueGame { steps })
            .map_err(unavailable)?;
        let expect: Vec<Value> = m
            .built
            .as_ref()
            .and_then(|b| b.get("expect"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        eprintln!("test maps: queueing {name} ({}, map {:#x})", listed.path, listed.id);
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = calls::lock(&q).take(ticket)?;
                Some(out.map_err(unavailable).map(|v| {
                    json!(LoadResult {
                        name: name.clone(),
                        map: listed.clone(),
                        check: v["check"].as_bool().unwrap_or(false),
                        queued: v["queued"].as_bool().unwrap_or(false),
                        expect: expect.clone(),
                    })
                }))
            }),
        );
        Ok(Value::Null)
    });

    let (p, j) = (paths, jobs);
    agent.add_method(method::MAPS_TEST_REMOVE, Access::Act, move |v| {
        let NameParams { name } = params(v)?;
        check_name(&name)?;
        if lock(&j)
            .maps
            .get(&name)
            .is_some_and(|m| m.state == State::Building)
        {
            return Err(RpcError::new(code::BUSY, format!("{name} is building")));
        }
        let dir = p.maps().join(format!("{PREFIX}{name}"));
        let removed = dir.is_dir();
        if removed {
            if let Some(id) = on_disk(&p.maps())
                .into_iter()
                .find(|m| m.name == name)
                .and_then(|m| m.map_id)
            {
                let _ = std::fs::remove_file(report_path(&id));
            }
            std::fs::remove_dir_all(&dir)
                .map_err(|e| unavailable(format!("removing {}: {e}", dir.display())))?;
            publish(&p, vfs.as_ref());
        }
        lock(&j).maps.remove(&name);
        Ok(json!(RemoveResult { name, removed }))
    });
}

#[cfg(test)]
mod tests;
