//! `maps.test.*`: purpose-built test maps from recipes, built from the
//! user's own disc and loaded by the running game (docs/game/test-maps.md).
//!
//! - `maps.test.create` checks the recipe, then builds it on a thread of its
//!   own: the base map from the disc, `scenario_edit::recipe::build_package`
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
use scenario_edit::recipe::{self, Recipe};
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

/// Where test maps come from and go.
#[derive(Clone, Debug)]
struct Paths {
    disc: PathBuf,
    data_root: PathBuf,
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

/// The recipe a `maps.test.create` names: a built-in by name or a whole
/// recipe, on its own base or `base`.
fn recipe_of(p: &CreateParams) -> Result<Recipe, RpcError> {
    let r = match &p.recipe {
        Value::String(name) => recipe::built_in_named(name),
        v @ Value::Object(_) => Recipe::from_json(v.clone()),
        _ => return Err(invalid("recipe: a built-in recipe's name, or a recipe object")),
    }
    .map_err(|e| invalid(e.to_string()))?;
    match &p.base {
        Some(b) => r.on_base(b.trim()).map_err(|e| invalid(e.to_string())),
        None => Ok(r),
    }
}

fn info(r: &Recipe) -> RecipeInfo {
    RecipeInfo {
        name: r.name.clone(),
        title: r.title.clone(),
        base: r.base.clone(),
        fidelity: match r.fidelity {
            recipe::Fidelity::Full => "full".into(),
            recipe::Fidelity::Approximate => "approximate".into(),
        },
        purpose: r.purpose.clone(),
        note: r.note.clone(),
    }
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
fn base_source(disc: &Path, base: &str) -> Result<scenario_edit::MapSource, String> {
    let file = crate::new_maps::IsoDisc::new(disc)
        .map_file(base)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("the disc has no maps\\{base}.map"))?;
    let whole = file.offset == 0
        && std::fs::metadata(&file.path).is_ok_and(|m| m.len() == file.len);
    Ok(if whole {
        scenario_edit::MapSource::file(file.path)
    } else {
        scenario_edit::MapSource::region(file.path, Some((file.offset, file.len)))
    })
}

/// Builds `r` into its package (replacing an old one when asked) and keeps
/// the report. The package's `MapId` and the report.
fn build(paths: &Paths, r: &Recipe, replace: bool) -> Result<(String, recipe::Built), String> {
    let dir = paths.maps().join(r.package_dir());
    if dir.exists() {
        if !replace {
            return Err(format!("{} exists (replace: true rebuilds it)", dir.display()));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
    }
    let source = base_source(&paths.disc, &r.base)?;
    let (built, saved) =
        recipe::build_package(source, r, &paths.data_root).map_err(|e| e.to_string())?;
    let id = saved.id.to_string();
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
pub(crate) fn game_steps(req: &Request) -> Option<Vec<GameStep>> {
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
/// unavailable.
pub fn install(
    agent: &Agent,
    calls: Option<calls::Shared>,
    disc: &Path,
    data_root: &Path,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
) {
    let paths = Paths {
        disc: disc.to_owned(),
        data_root: data_root.to_owned(),
    };
    let jobs = Shared::default();

    let (p, j) = (paths.clone(), jobs.clone());
    agent.add_method(method::MAPS_TEST_LIST, Access::Read, move |v| {
        let ListParams { name } = params(v)?;
        let recipes = recipe::built_in()
            .map_err(|e| unavailable(e.to_string()))?
            .iter()
            .map(info)
            .collect();
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
        let r = recipe_of(&cp)?;
        let dir = r.package_dir();
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
        let spawned = std::thread::Builder::new()
            .name("test map build".into())
            .spawn(move || {
                let out = build(&p2, &r2, replace);
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
                        m.built = serde_json::to_value(&built).ok();
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
