//! World-space diagnostics captured by the owner of a rendered guest image.
//! Tag decoding runs on a worker. The image holds selected lines and can be
//! shown again without consulting live guest memory or the current toggles.
use crate::{Access, Agent, CollisionTrace, CollisionTraces, GameStatus, Reader};
use agent_proto::debug::{
    RANGE, TRACE_TICKS, View, ViewParams, ViewState, ViewsParams, ViewsResult, method,
};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::game::{GameGlobals, GameTime, PlayerGlobals, cache_checksum};
use guest_layout::sound::SoundListener;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

mod debug_hitboxes;
mod debug_static;
pub use debug_hitboxes::Hitboxes as DebugHitboxes;
use debug_hitboxes::Hitboxes;
pub use debug_static::StaticViews as DebugBsp;
use debug_static::StaticViews;

type Point = [f32; 3];
type Edge = [Point; 2];
const MAX_LINES: usize = 2048;

/// Level collision a live commit replaced (`edit.collision.commit`,
/// docs/editor/realtime-edits.md, "Collision"), by structure BSP datum.
/// The views are built from the map file, which does not hold it, so a
/// built BSP layer has its structure collision lines replaced by these
/// ([`StaticViews::replace_structure`]) instead of being read again.
#[derive(Default)]
pub struct CollisionEdits {
    inner: Mutex<(u64, HashMap<u32, Arc<tags::collision::Bsp>>)>,
}

impl CollisionEdits {
    fn lock(&self) -> MutexGuard<'_, (u64, HashMap<u32, Arc<tags::collision::Bsp>>)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Structure `datum`'s level collision is now `bsp`.
    pub fn replace(&self, datum: u32, bsp: tags::collision::Bsp) {
        let mut inner = self.lock();
        inner.0 += 1;
        inner.1.insert(datum, Arc::new(bsp));
    }

    /// A new map: nothing is replaced.
    pub fn clear(&self) {
        let mut inner = self.lock();
        if !inner.1.is_empty() {
            inner.0 += 1;
            inner.1.clear();
        }
    }

    pub(crate) fn snapshot(&self) -> (u64, HashMap<u32, Arc<tags::collision::Bsp>>) {
        self.lock().clone()
    }
}

/// `views` with its structure's level collision replaced, when `edited`
/// holds it.
fn apply_edits(views: &mut Arc<StaticViews>, edited: &HashMap<u32, Arc<tags::collision::Bsp>>) {
    if let Some(bsp) = views.structure.and_then(|s| edited.get(&s)) {
        Arc::make_mut(views).replace_structure(bsp);
    }
}

/// Opens the mounted cache and its shared resources on the loader thread.
pub trait DebugMaps: Sync + Send {
    fn generation(&self) -> u64;
    fn bsp(&self, name: &str, checksum: Option<u32>, index: usize) -> Result<DebugBsp, String>;
    fn hitboxes(&self, name: &str, checksum: Option<u32>) -> Result<DebugHitboxes, String>;
}

#[derive(Clone, Copy)]
struct Options {
    enabled: [bool; 7],
    range: u16,
    trace_ticks: u16,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: [false; 7],
            range: 48,
            trace_ticks: 30,
        }
    }
}

impl Options {
    fn on(self, view: View) -> bool {
        self.enabled[view as usize]
    }
    fn any(self) -> bool {
        self.enabled.contains(&true)
    }
    fn needs_bsp(self) -> bool {
        View::ALL
            .into_iter()
            .any(|v| self.on(v) && !matches!(v, View::Hitboxes | View::Traces))
    }
}

#[derive(Default)]
struct State {
    options: Options,
    frame: Arc<Frame>,
}

#[derive(Default)]
struct Frame {
    bsp: Option<Arc<StaticViews>>,
    active_bsp: Option<u32>,
    checksum: Option<u32>,
    models: Option<Arc<Hitboxes>>,
    history: Vec<CollisionTrace>,
    issues: [Option<String>; 7],
}

#[derive(Default)]
pub struct DebugViews {
    active: AtomicBool,
    state: Mutex<State>,
}

impl DebugViews {
    /// Hot path for the render source when all views are off.
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn capture(&self, guest: &dyn GuestMemory) -> Option<DebugImage> {
        let (options, frame) = {
            let state = self.state();
            if !state.options.any() {
                return None;
            }
            (state.options, state.frame.clone())
        };
        // The render owner can have an older map or BSP than the
        // simulation thread which decoded these tags.
        let bsp = GameGlobals::current(guest)
            .ok()??
            .active_bsp(guest)
            .ok()
            .and_then(|index| u32::try_from(index).ok())?;
        if frame.active_bsp != Some(bsp) || frame.checksum != Some(cache_checksum(guest).ok()?) {
            return None;
        }
        let eyes = seat_positions(guest);
        let range = f32::from(options.range);
        let mut image = DebugImage::default();
        for view in View::ALL.into_iter().filter(|&v| options.on(v)) {
            match view {
                View::Traces => {
                    if !frame.history.is_empty()
                        && let Some(time) = GameTime::current(guest).ok().flatten()
                        && let Ok(tick) = time.tick(guest)
                    {
                        let tick = u64::from(tick);
                        image.add_traces(&frame.history, tick, options.trace_ticks, &eyes, range);
                    }
                }
                View::Hitboxes => {
                    if let Some(models) = &frame.models {
                        // Give each seat its own object walk budget. An earlier
                        // object's mesh near seat zero cannot starve seat one.
                        let groups = if eyes.len() <= 1 {
                            let edges = models.edges(guest, &eyes, range, MAX_LINES);
                            select(&edges, &eyes, range, MAX_LINES)
                        } else {
                            let mut seats = Vec::with_capacity(eyes.len());
                            for (i, &eye) in eyes.iter().enumerate() {
                                let mut edges = models.edges(guest, &[eye], range, MAX_LINES);
                                edges.retain(|edge| nearest(*edge, &eyes, range) == Some(i));
                                seats.push(edges);
                            }
                            share(seats, MAX_LINES)
                        };
                        image.add(view, groups);
                    }
                }
                _ => {
                    if let Some(bsp) = &frame.bsp {
                        let edges = match view {
                            View::GeometryEdges => &bsp.geometry_edges,
                            View::TriggerVolumes => &bsp.triggers,
                            View::KillVolumes => &bsp.kill_volumes,
                            View::InvisibleBarriers => &bsp.barriers,
                            View::CollisionGeometry => &bsp.collision,
                            _ => unreachable!(),
                        };
                        image.add(view, select(edges, &eyes, range, MAX_LINES));
                    }
                }
            }
        }
        (!image.layers.is_empty() || !image.traces.is_empty()).then_some(image)
    }
}

/// Read only mapped local users, including splits with holes in the seat map.
/// The sound engine copies its observer camera into each active listener
/// (game/src/bot_glue/audio.rs, audio.listeners). A seat without a listener
/// uses its live unit's world-space bounding sphere centre.
fn seat_positions(guest: &dyn GuestMemory) -> Vec<Point> {
    let seats = PlayerGlobals::current(guest)
        .ok()
        .flatten()
        .and_then(|p| p.local_players(guest).ok());
    let Some(seats) = seats else { return vec![] };
    let sound = guest
        .u32(guest_layout::points::SOUND_GLOBALS.base)
        .ok()
        .filter(|&p| p != 0);
    let players = guest_layout::view::players::array(guest).ok().flatten();
    seats
        .into_iter()
        .enumerate()
        .filter_map(|(i, datum)| {
            let id = guest_layout::DatumHandle::from_raw(datum.raw())?;
            let listener = || {
                let listener = SoundListener::for_user(sound?, i)?;
                if listener.active(guest).ok()? == 0 {
                    return None;
                }
                let position = listener.position(guest).ok()?;
                position.iter().all(|n| n.is_finite()).then_some(position)
            };
            listener().or_else(|| {
                let player = players.as_ref()?.get(guest, id).ok()??;
                let unit = player.unit(guest).ok()??;
                let obj = sim_state::object(guest, sim_state::ObjectId(unit.raw())).ok()??;
                let p = obj.bounding_sphere().ok()?.center;
                p.iter().all(|n| n.is_finite()).then_some(p)
            })
        })
        .collect()
}

fn nearest([a, b]: Edge, eyes: &[Point], range: f32) -> Option<usize> {
    let dot = |a: Point, b: Point| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
    let delta = std::array::from_fn(|i| b[i] - a[i]);
    let length_sq = dot(delta, delta);
    let mut best = None;
    let mut best_sq = range * range;
    for (i, eye) in eyes.iter().enumerate() {
        let offset = std::array::from_fn(|i| eye[i] - a[i]);
        let t = if length_sq == 0.0 {
            0.0
        } else {
            (dot(offset, delta) / length_sq).clamp(0.0, 1.0)
        };
        let residual = std::array::from_fn(|i| offset[i] - t * delta[i]);
        let sq = dot(residual, residual);
        if sq < best_sq || (best.is_none() && sq == best_sq) {
            best_sq = sq;
            best = Some(i);
        }
    }
    best
}

/// Round robin among disjoint sets. Seats with fewer lines release the rest
/// of their share to the others. No duplicate edges across overlapping eyes.
fn share(mut seats: Vec<Vec<Edge>>, limit: usize) -> Vec<Edge> {
    if seats.len() == 1 {
        let mut edges = seats.pop().unwrap();
        edges.truncate(limit);
        return edges;
    }
    let total = limit.min(seats.iter().map(Vec::len).sum());
    let mut out = Vec::with_capacity(total);
    let mut rows: Vec<_> = seats.into_iter().map(Vec::into_iter).collect();
    while out.len() < total {
        for row in &mut rows {
            if let Some(edge) = row.next() {
                out.push(edge);
                if out.len() == total {
                    break;
                }
            }
        }
    }
    out
}

fn select(edges: &[Edge], eyes: &[Point], range: f32, limit: usize) -> Vec<Edge> {
    if eyes.is_empty() {
        return edges[..edges.len().min(limit)].to_vec();
    }
    let mut seats = vec![vec![]; eyes.len()];
    let mut full = 0;
    for &edge in edges {
        if let Some(i) = nearest(edge, eyes, range)
            && seats[i].len() < limit
        {
            seats[i].push(edge);
            if seats[i].len() == limit {
                full += 1;
                if full == seats.len() {
                    break;
                }
            }
        }
    }
    share(seats, limit)
}

#[derive(Default, Debug)]
pub struct DebugImage {
    layers: Vec<(View, Vec<Edge>)>,
    traces: Vec<TraceLines>,
}

#[derive(Debug)]
struct TraceLines {
    segments: [Edge; 4],
    len: usize,
    color: Point,
    dashed: bool,
}

impl DebugImage {
    pub fn lines(&self, visit: &mut dyn FnMut(&[Edge], Point, bool)) {
        for (view, edges) in &self.layers {
            visit(edges, view.color(), false);
        }
        for trace in &self.traces {
            visit(&trace.segments[..trace.len], trace.color, trace.dashed);
        }
    }

    fn add(&mut self, view: View, edges: Vec<Edge>) {
        if !edges.is_empty() {
            self.layers.push((view, edges));
        }
    }

    fn add_traces(
        &mut self,
        rays: &[CollisionTrace],
        tick: u64,
        ticks: u16,
        eyes: &[Point],
        range: f32,
    ) {
        let mut remaining = MAX_LINES;
        for ray in rays {
            let segment = [ray.start, ray.end];
            let Some(age) = tick.checked_sub(ray.tick).filter(|&a| a < u64::from(ticks)) else {
                continue;
            };
            if !eyes.is_empty() && nearest(segment, eyes, range).is_none() {
                continue;
            }
            let mut segments = [segment; 4];
            let mut len = 1;
            if let Some(hit) = ray.hit {
                for axis in 0..3 {
                    let mut edge = [hit; 2];
                    edge[0][axis] -= 0.08;
                    edge[1][axis] += 0.08;
                    segments[len] = edge;
                    len += 1;
                }
            }
            if len > remaining {
                break;
            }
            let fade = 1.0 - age as f32 / f32::from(ticks);
            self.traces.push(TraceLines {
                segments,
                len,
                color: View::Traces.color().map(|c| c * fade),
                dashed: ray.hit.is_none(),
            });
            remaining -= len;
        }
    }
}

#[derive(Clone, Copy)]
enum Layer {
    Bsp(usize),
    Hitboxes,
}
enum Loaded {
    Bsp(StaticViews),
    Hitboxes(Hitboxes),
}
struct Job {
    epoch: u64,
    layer: Layer,
    thread: JoinHandle<Result<Loaded, String>>,
}

#[derive(Default)]
struct Cache {
    name: Option<String>,
    generation: u64,
    epoch: u64,
    checksum: Option<u32>,
    last_tick: Option<u64>,
    active_bsp: Option<u32>,
    bsps: HashMap<usize, Result<Arc<StaticViews>, String>>,
    models: Option<Result<Arc<Hitboxes>, String>>,
    job: Option<Job>,
    /// The [`CollisionEdits`] generation `bsps` has applied; `None` after
    /// a layer loads, so the next refresh applies the edits to it.
    edits: Option<u64>,
}

impl Cache {
    fn store(&mut self, layer: Layer, result: Result<Loaded, String>) {
        match layer {
            Layer::Bsp(id) => {
                self.bsps.insert(
                    id,
                    result.map(|r| match r {
                        Loaded::Bsp(bsp) => Arc::new(bsp),
                        _ => unreachable!(),
                    }),
                );
            }
            Layer::Hitboxes => {
                self.models = Some(result.map(|r| match r {
                    Loaded::Hitboxes(models) => Arc::new(models),
                    _ => unreachable!(),
                }))
            }
        }
    }

    fn load(&mut self, maps: &Arc<dyn DebugMaps>, name: &str, layer: Layer) {
        let maps = maps.clone();
        let name = name.to_owned();
        let checksum = self.checksum;
        match background::spawn("debug view geometry", move || match layer {
            Layer::Bsp(i) => maps.bsp(&name, checksum, i).map(Loaded::Bsp),
            Layer::Hitboxes => maps.hitboxes(&name, checksum).map(Loaded::Hitboxes),
        }) {
            Ok(thread) => {
                self.job = Some(Job {
                    epoch: self.epoch,
                    layer,
                    thread,
                })
            }
            Err(e) => self.store(layer, Err(e.to_string())),
        }
    }

    fn refresh(
        &mut self,
        maps: Option<&Arc<dyn DebugMaps>>,
        game: &GameStatus,
        reader: &mut dyn Reader,
        options: Options,
        traces: &CollisionTraces,
        edits: &CollisionEdits,
    ) -> Frame {
        let generation = maps.map_or(0, |m| m.generation());
        let checksum = reader.live_metadata().1;
        if self.name != game.map || checksum != self.checksum || generation != self.generation {
            self.bsps.clear();
            self.models = None;
            self.epoch += 1;
            self.generation = generation;
            self.name.clone_from(&game.map);
            self.checksum = checksum;
            traces.clear();
        }
        if self.active_bsp != game.bsp || self.last_tick.zip(game.tick).is_some_and(|(a, b)| b < a)
        {
            self.active_bsp = game.bsp;
            traces.clear();
        }
        self.last_tick = game.tick;
        if self.job.as_ref().is_some_and(|j| j.thread.is_finished()) {
            let job = self.job.take().unwrap();
            let result = job
                .thread
                .join()
                .unwrap_or_else(|_| Err("debug geometry loader stopped".into()));
            if job.epoch == self.epoch {
                self.store(job.layer, result);
                self.edits = None;
            }
        }
        let (generation, edited) = edits.snapshot();
        if self.edits != Some(generation) {
            self.edits = Some(generation);
            for views in self.bsps.values_mut().filter_map(|r| r.as_mut().ok()) {
                apply_edits(views, &edited);
            }
        }
        let index = game.bsp.map(|i| i as usize);
        if self.job.is_none()
            && let (Some(maps), Some(name)) = (maps, &game.map)
        {
            if options.needs_bsp()
                && let Some(i) = index
                && !self.bsps.contains_key(&i)
            {
                self.load(maps, name, Layer::Bsp(i));
            } else if options.on(View::Hitboxes) && self.models.is_none() {
                self.load(maps, name, Layer::Hitboxes);
            }
        }
        let mut frame = Frame {
            checksum: self.checksum,
            active_bsp: self.active_bsp,
            models: self.models.as_ref().and_then(|r| r.as_ref().ok()).cloned(),
            ..Default::default()
        };
        let bsp = index.and_then(|i| self.bsps.get(&i));
        if let Some(Ok(bsp)) = bsp {
            frame.bsp = Some(bsp.clone());
        }
        for v in View::ALL.into_iter().filter(|&v| options.on(v)) {
            let why = match v {
                _ if game.map.is_none() => Some("waiting for a loaded map"),
                View::Hitboxes => match &self.models {
                    None => Some("loading hitbox models"),
                    Some(Err(e)) => Some(e.as_str()),
                    Some(Ok(_)) => None,
                },
                View::Traces => None,
                _ => match bsp {
                    None if index.is_none() => Some("waiting for an active BSP"),
                    None => Some("loading debug geometry"),
                    Some(Err(e)) => Some(e.as_str()),
                    Some(Ok(bsp)) => bsp.refusal(v),
                },
            };
            frame.issues[v as usize] = why.map(str::to_owned);
        }
        frame
    }
}

pub(crate) struct Observer {
    views: Arc<DebugViews>,
    cache: Mutex<Cache>,
    maps: Option<Arc<dyn DebugMaps>>,
    traces: Arc<CollisionTraces>,
    edits: Arc<CollisionEdits>,
}

impl Observer {
    pub(crate) fn active(&self) -> bool {
        self.views.active()
    }

    pub(crate) fn refresh(&self, reader: &mut dyn Reader, game: &GameStatus) {
        let options = self.views.state().options;
        if !options.any() {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut frame = cache.refresh(self.maps.as_ref(), game, reader, options, &self.traces, &self.edits);
        if options.on(View::Traces)
            && let Some(tick) = game.tick
        {
            // Couple rays to this map/BSP before publishing. A render owner
            // holding an older Frame cannot see a new map's trace history.
            frame.history = self.traces.recent(tick, *TRACE_TICKS.end());
        }
        self.views.state().frame = Arc::new(frame);
    }
}

struct Methods {
    views: Arc<DebugViews>,
    unavailable: Option<&'static str>,
    has_maps: bool,
    traces: Arc<CollisionTraces>,
}

impl Methods {
    fn refusal(&self, view: View) -> Option<&'static str> {
        self.unavailable.or_else(|| {
            if view == View::Traces {
                self.traces.unavailable()
            } else {
                (!self.has_maps).then_some("host does not supply map geometry")
            }
        })
    }

    fn list(&self) -> ViewsResult {
        let (options, frame) = {
            let state = self.views.state();
            (state.options, state.frame.clone())
        };
        let views = View::ALL
            .into_iter()
            .map(|name| {
                let on = options.on(name);
                let why = self.refusal(name).map(str::to_owned).or_else(|| {
                    if on {
                        frame.issues[name as usize].clone()
                    } else {
                        None
                    }
                });
                ViewState {
                    name,
                    label: name.label().into(),
                    on,
                    why,
                }
            })
            .collect();
        ViewsResult {
            views,
            range: options.range,
            trace_ticks: options.trace_ticks,
        }
    }

    fn set(&self, p: ViewParams) -> Result<ViewsResult, RpcError> {
        if let ViewParams::Toggle { name, on } = p {
            if on && let Some(why) = self.refusal(name) {
                return Err(RpcError::new(code::UNAVAILABLE, why));
            }
            if name == View::Traces {
                self.traces
                    .set_enabled(on)
                    .map_err(|why| RpcError::new(code::UNAVAILABLE, why))?;
            }
        }
        {
            let mut state = self.views.state();
            match p {
                ViewParams::Toggle { name, on } => {
                    if !state.options.any() && on {
                        state.frame = Arc::default();
                    }
                    state.options.enabled[name as usize] = on;
                }
                ViewParams::Range { range } if RANGE.contains(&range) => {
                    state.options.range = range
                }
                ViewParams::TraceTicks { trace_ticks } if TRACE_TICKS.contains(&trace_ticks) => {
                    state.options.trace_ticks = trace_ticks
                }
                ViewParams::Range { .. } => {
                    return Err(invalid("range must be 1 to 512 world units"));
                }
                ViewParams::TraceTicks { .. } => {
                    return Err(invalid("trace_ticks must be 1 to 300"));
                }
            }
            self.views
                .active
                .store(state.options.any(), Ordering::Release);
        }
        Ok(self.list())
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

/// Install the read and control methods even when the host cannot present them.
pub fn install(
    agent: &Agent,
    views: Option<Arc<DebugViews>>,
    maps: Option<Arc<dyn DebugMaps>>,
    unavailable: Option<&'static str>,
) {
    let unavailable = unavailable.or_else(|| {
        views
            .is_none()
            .then_some("this host has no debug view presenter")
    });
    let views = views.unwrap_or_default();
    let traces = agent.collision_traces();
    let methods = Arc::new(Methods {
        views: views.clone(),
        unavailable,
        has_maps: maps.is_some(),
        traces: traces.clone(),
    });
    let list = methods.clone();
    agent.add_method(method::VIEWS, Access::Read, move |params| {
        let _: ViewsParams = serde_json::from_value(if params.is_null() {
            serde_json::json!({})
        } else {
            params
        })
        .map_err(|e| invalid(e.to_string()))?;
        Ok(serde_json::to_value(list.list()).unwrap())
    });
    agent.add_method(method::VIEW, Access::Act, move |params| {
        let params = serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
        Ok(serde_json::to_value(methods.set(params)?).unwrap())
    });
    agent.set_debug_views(Observer {
        views,
        cache: Mutex::default(),
        maps,
        traces,
        edits: agent.collision_edits(),
    });
}

#[cfg(test)]
mod tests;
