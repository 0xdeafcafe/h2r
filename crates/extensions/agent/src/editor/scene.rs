use agent_proto::editor::{
    MAX_MARKS, Mark, MarkBox, MarkGeometry, MarkStyle, MarksParams, MarksResult, RefusedMark,
};
use guest_layout::GuestMemory;
use sim_state::ObjectId;

use super::*;
use crate::Reader;

type Point = [f32; 3];
type Edge = [Point; 2];

#[derive(Clone, Debug, PartialEq)]
pub struct MarkShape {
    pub edges: Vec<Edge>,
    pub anchor: Point,
    pub color: [f32; 3],
    pub style: Option<MarkStyle>,
    pub label: Option<String>,
}

#[derive(Default)]
pub(super) struct Scene {
    marks: Option<Marks>,
    next_generation: u64,
}

pub(crate) struct MarkObserver {
    link: EditorLink,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl MarkObserver {
    pub(crate) fn on_tick(&self, reader: &mut dyn Reader) {
        self.link.retire_marks(reader, &*self.clock);
    }

    pub(crate) fn invalidate(&self) {
        self.link.invalidate_marks();
    }

    pub(crate) fn set_no_tick_hook(&self) {
        self.link.0.no_tick_observer.store(true, Ordering::Relaxed);
        self.invalidate();
    }
}

struct Marks {
    objects: Vec<(ObjectId, Appearance)>,
    fixed: Vec<MarkShape>,
    generation: u64,
    expires: Instant,
}

#[derive(PartialEq)]
struct Appearance {
    color: [f32; 3],
    style: Option<MarkStyle>,
    label: Option<String>,
}

/// World-space bounds sampled from the guest memory that renders an image.
#[derive(Debug)]
pub struct MarkFrame {
    generation: u64,
    pub shapes: Vec<MarkShape>,
}

impl MarkFrame {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum MarkAvailability {
    Waiting = 0,
    Ready,
    NoWorldView,
    RawFrame,
    Loading,
    Synthetic,
    NativeOff,
    SceneOff,
}

impl MarkAvailability {
    fn why(value: u8) -> Option<&'static str> {
        match value {
            n if n == Self::Ready as u8 => None,
            n if n == Self::NoWorldView as u8 => Some("this guest frame has no captured world camera"),
            n if n == Self::RawFrame as u8 => Some("raw frame has no world camera"),
            n if n == Self::Loading as u8 => Some("the window is showing a loading picture"),
            n if n == Self::Synthetic as u8 => Some("synthetic frame has no camera for marks"),
            n if n == Self::NativeOff as u8 => Some("native rendering is off"),
            n if n == Self::SceneOff as u8 => Some("frame scene capture is off"),
            _ => Some("waiting for a guest frame with marks"),
        }
    }
}

impl Appearance {
    fn shape(&self, anchor: Point, edges: Vec<Edge>) -> MarkShape {
        MarkShape {
            edges,
            anchor,
            color: self.color,
            style: self.style,
            label: self.label.clone(),
        }
    }
}

fn add(a: Point, b: Point) -> Point {
    std::array::from_fn(|i| a[i] + b[i])
}

fn scale(v: Point, k: f32) -> Point {
    v.map(|n| n * k)
}

fn cross([x, y, z]: Point, [a, b, c]: Point) -> Point {
    [y * c - z * b, z * a - x * c, x * b - y * a]
}

fn unit(v: Point) -> Option<Point> {
    let length = v[0].hypot(v[1]).hypot(v[2]);
    (length > 1e-6 && length.is_finite()).then(|| v.map(|n| n / length))
}

fn basis(forward: Point, up: Point) -> Option<(Point, Point, Point)> {
    let forward = unit(forward)?;
    let right = unit(cross(forward, unit(up)?))?;
    Some((forward, right, cross(right, forward)))
}

fn box_edges(corner: Point, axes: [Point; 3]) -> Vec<Edge> {
    let corners: [Point; 8] = std::array::from_fn(|bits| {
        axes.iter()
            .enumerate()
            .filter(|&(axis, _)| bits & (1 << axis) != 0)
            .fold(corner, |p, (_, &axis)| add(p, axis))
    });
    let mut edges = Vec::with_capacity(12);
    for bits in 0..8 {
        for axis in [1, 2, 4] {
            if bits & axis == 0 {
                edges.push([corners[bits], corners[bits | axis]]);
            }
        }
    }
    edges
}

fn object_shape(
    guest: &dyn GuestMemory,
    id: ObjectId,
    appearance: &Appearance,
) -> Result<MarkShape, &'static str> {
    let object = sim_state::object(guest, id)
        .map_err(|_| "object pool is unreadable")?
        .ok_or("object is gone")?;
    let sphere = object
        .bounding_sphere()
        .map_err(|_| "object bounds are unreadable")?;
    let parent = object.parent().map_err(|_| "object parent is unreadable")?;
    let axes = if parent.is_some() {
        // A child body stores its axes in parent space. Its first render
        // node supplies world axes when it has one.
        let mut root = [sim_state::NodeMatrix::default()];
        let count = object
            .nodes(&mut root)
            .map_err(|_| "object nodes are unreadable")?;
        if count != 0 {
            let [_, fx, fy, fz, _, _, _, ux, uy, uz, _, _, _] = root[0].0;
            basis([fx, fy, fz], [ux, uy, uz])
        } else {
            basis([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
        }
    } else {
        let transform = object
            .transform()
            .map_err(|_| "object transform is unreadable")?;
        basis(transform.forward, transform.up)
    };
    let (forward, right, up) = axes.ok_or("object has invalid axes")?;
    if !sphere.center.into_iter().all(f32::is_finite)
        || !sphere.radius.is_finite()
        || sphere.radius <= 0.0
    {
        return Err("object has invalid bounds");
    }
    // ObjectView already reports the sphere in world space, including scale.
    let axes = [forward, right, up];
    let corner = axes
        .iter()
        .fold(sphere.center, |p, &v| add(p, scale(v, -sphere.radius)));
    let edges = box_edges(corner, axes.map(|v| scale(v, 2.0 * sphere.radius)));
    Ok(appearance.shape(sphere.center, edges))
}

fn box_shape(frame: MarkBox, appearance: &Appearance) -> Result<MarkShape, RpcError> {
    let (forward, right, up) = basis(frame.forward, frame.up)
        .ok_or_else(|| invalid("box needs nonparallel forward and up axes"))?;
    if !frame.origin.into_iter().all(f32::is_finite)
        || !frame.size.into_iter().all(|n| n.is_finite() && n > 0.0)
    {
        return Err(invalid("box needs a finite origin and positive size"));
    }
    // A placement volume is anchored at its origin, with full lengths along
    // forward, left and up (the editor's scenario volume frame).
    let frame_axes = [forward, scale(right, -1.0), up];
    let axes = std::array::from_fn(|i| scale(frame_axes[i], frame.size[i]));
    let center = axes.iter().fold(frame.origin, |p, &v| add(p, scale(v, 0.5)));
    Ok(appearance.shape(center, box_edges(frame.origin, axes)))
}

fn point_shape(point: Point, appearance: &Appearance) -> Result<MarkShape, RpcError> {
    if !point.into_iter().all(f32::is_finite) {
        return Err(invalid("point must be finite"));
    }
    let edges = (0..3)
        .map(|axis| {
            let mut a = point;
            let mut b = point;
            a[axis] -= 0.125;
            b[axis] += 0.125;
            [a, b]
        })
        .collect();
    Ok(appearance.shape(point, edges))
}

fn ttl(value: Option<u64>, default: u64) -> Result<Duration, RpcError> {
    let ms = value.unwrap_or(default);
    if !TTL_MS_RANGE.contains(&ms) {
        return Err(invalid(format!(
            "ttl_ms must be {} to {}",
            TTL_MS_RANGE.start(),
            TTL_MS_RANGE.end()
        )));
    }
    Ok(Duration::from_millis(ms))
}

enum ValidMark {
    Object(ObjectId, Appearance),
    Shape(MarkShape),
}

fn mark(mark: Mark) -> Result<ValidMark, RpcError> {
    if !mark.color.into_iter().all(|n| (0.0..=1.0).contains(&n)) {
        return Err(invalid("mark color components must be in 0..=1"));
    }
    let appearance = Appearance {
        color: mark.color,
        style: mark.style,
        label: mark.label,
    };
    match mark.geometry {
        MarkGeometry::Object(id) => {
            if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid("object must be an eight-digit hex handle"));
            }
            let id = u32::from_str_radix(&id, 16).map_err(|e| invalid(e.to_string()))?;
            Ok(ValidMark::Object(ObjectId(id), appearance))
        }
        MarkGeometry::Box(frame) => Ok(ValidMark::Shape(box_shape(frame, &appearance)?)),
        MarkGeometry::Point(point) => Ok(ValidMark::Shape(point_shape(point, &appearance)?)),
    }
}

impl EditorLink {
    fn replace_marks(
        &self,
        params: MarksParams,
        now: Instant,
        guest: Option<&dyn GuestMemory>,
        windowed: bool,
    ) -> Result<MarksResult, RpcError> {
        let expires = now + ttl(params.ttl_ms, LINK_TTL_MS)?;
        let mut refused = Vec::new();
        let mut objects = Vec::new();
        let mut fixed = Vec::new();
        let no_tick = self.0.no_tick_observer.load(Ordering::Relaxed);
        for (index, entry) in params.marks.into_iter().enumerate() {
            match mark(entry)? {
                ValidMark::Shape(shape) => fixed.push(shape),
                ValidMark::Object(id, appearance) => {
                    let checked = if no_tick {
                        Err("object marks need the agent tick observer")
                    } else {
                        guest
                            .ok_or("object pool is unreadable")
                            .and_then(|guest| object_shape(guest, id, &appearance))
                            .map(|_| ())
                    };
                    match checked {
                        Ok(()) => objects.push((id, appearance)),
                        Err(why) => refused.push(RefusedMark {
                            index,
                            why: why.into(),
                        }),
                    }
                }
            }
        }
        let held = objects.len() + fixed.len();
        let wake = {
            let mut state = self.state();
            let previous = state.scene.marks.as_ref().filter(|marks| {
                now < marks.expires && marks.objects == objects && marks.fixed == fixed
            });
            let generation = if let Some(previous) = previous {
                previous.generation
            } else {
                state.scene.next_generation = state.scene.next_generation.wrapping_add(1).max(1);
                state.scene.next_generation
            };
            state.scene.marks = (held > 0).then_some(Marks {
                objects,
                fixed,
                generation,
                expires,
            });
            self.0
                .marks_generation
                .store(if held > 0 { generation } else { 0 }, Ordering::Release);
            state.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
        let why = if !windowed {
            Some(NO_WINDOW)
        } else if held == 0 {
            None
        } else {
            self.mark_unavailable()
        };
        Ok(MarksResult {
            held,
            drawn: held > 0 && why.is_none(),
            why: why.map(str::to_owned),
            refused,
        })
    }

    /// A read-only sample of the leased set. The render worker's guest
    /// view can be older than the game thread's.
    pub fn capture_marks(
        &self,
        guest: &dyn GuestMemory,
        now: impl FnOnce() -> Instant,
    ) -> Option<MarkFrame> {
        let generation = self.0.marks_generation.load(Ordering::Acquire);
        if generation == 0 {
            return None;
        }
        let state = self.state();
        let marks = state.scene.marks.as_ref()?;
        if generation != marks.generation || now() >= marks.expires {
            return None;
        }
        let mut shapes = Vec::with_capacity(marks.objects.len() + marks.fixed.len());
        shapes.extend(marks.objects.iter().filter_map(|(id, appearance)| {
            object_shape(guest, *id, appearance).ok()
        }));
        shapes.extend(marks.fixed.iter().cloned());
        Some(MarkFrame { generation, shapes })
    }

    /// Checks the lease of a queued or repeated image without re-resolving it.
    pub fn frame_mark_deadline(
        &self,
        frame: &MarkFrame,
        now: impl FnOnce() -> Instant,
    ) -> Option<Instant> {
        if self.0.marks_generation.load(Ordering::Acquire) != frame.generation
            || frame.shapes.is_empty()
        {
            return None;
        }
        let mut state = self.state();
        let marks = state.scene.marks.as_ref()?;
        if marks.generation != frame.generation {
            return None;
        }
        if now() >= marks.expires {
            self.clear_marks(&mut state);
            let wake = state.wake.clone();
            drop(state);
            if let Some(wake) = wake {
                wake();
            }
            return None;
        }
        Some(marks.expires)
    }

    fn retire_marks(&self, reader: &mut dyn Reader, now: &dyn Fn() -> Instant) {
        if self.0.marks_generation.load(Ordering::Acquire) == 0 {
            return;
        }
        let now = now();
        let mut state = self.state();
        let scene = &mut state.scene;
        let Some(marks) = &mut scene.marks else {
            return;
        };
        if now >= marks.expires {
            self.clear_marks(&mut state);
        } else {
            let count = marks.objects.len();
            if count == 0 {
                return;
            }
            reader.with_guest(&mut |guest| {
                marks.objects.retain(|(id, _)| !matches!(sim_state::object(guest, *id), Ok(None)));
            });
            if marks.objects.len() == count {
                return;
            }
            scene.next_generation = scene.next_generation.wrapping_add(1).max(1);
            if marks.objects.is_empty() && marks.fixed.is_empty() {
                self.clear_marks(&mut state);
            } else {
                marks.generation = scene.next_generation;
                self.0.marks_generation.store(marks.generation, Ordering::Release);
            }
        }
        let wake = state.wake.clone();
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
    }

    fn clear_marks(&self, state: &mut State) {
        state.scene.marks = None;
        self.0.marks_generation.store(0, Ordering::Release);
    }

    fn invalidate_marks(&self) {
        if self.0.marks_generation.load(Ordering::Acquire) == 0 {
            return;
        }
        let wake = {
            let mut state = self.state();
            self.clear_marks(&mut state);
            state.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }

    pub fn set_mark_frame_status(&self, status: MarkAvailability) {
        debug_assert!(!matches!(status, MarkAvailability::Ready));
        self.0.mark_camera.store(status as u8, Ordering::Release);
    }

    pub fn set_drawn_mark_generation(&self, generation: u64) {
        self.0
            .drawn_mark_generation
            .store(generation, Ordering::Release);
        self.0
            .mark_camera
            .store(MarkAvailability::Ready as u8, Ordering::Release);
    }

    pub fn set_mark_capture_disabled(&self, reason: MarkAvailability) {
        self.0.mark_capture_disabled.store(reason as u8, Ordering::Relaxed);
    }

    fn mark_unavailable(&self) -> Option<&'static str> {
        let status = self.0.mark_camera.load(Ordering::Acquire);
        if status == MarkAvailability::Ready as u8
            && self.0.drawn_mark_generation.load(Ordering::Acquire)
                != self.0.marks_generation.load(Ordering::Acquire)
        {
            return Some("waiting for a guest frame with this mark set");
        }
        if status == MarkAvailability::Waiting as u8 || status == MarkAvailability::NoWorldView as u8
        {
            let disabled = self.0.mark_capture_disabled.load(Ordering::Relaxed);
            if disabled != 0 {
                return MarkAvailability::why(disabled);
            }
        }
        MarkAvailability::why(status)
    }

    #[cfg(test)]
    pub(super) fn marks(&self, now: Instant) -> Option<(Vec<MarkShape>, Instant)> {
        let guest = guest_layout::testing::FakeGuest::new();
        let frame = self.capture_marks(&guest, || now)?;
        let expires = self.frame_mark_deadline(&frame, || now)?;
        Some((frame.shapes, expires))
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    if !params.is_object() {
        return Err(invalid("params must be an object"));
    }
    serde_json::from_value(params).map_err(|e| invalid(e.to_string()))
}

/// Adds the marks method, including for headless hosts.
pub fn install_scene(agent: &Agent, window: Option<EditorLink>) {
    install_scene_with_clock(agent, window, Instant::now);
}

pub(super) fn install_scene_with_clock(
    agent: &Agent,
    window: Option<EditorLink>,
    clock: impl Fn() -> Instant + Send + Sync + 'static,
) {
    let windowed = window.is_some();
    let link = window.unwrap_or_default();
    let clock = Arc::new(clock);
    agent.set_editor_marks(MarkObserver {
        link: link.clone(),
        clock: clock.clone(),
    });
    let marks = link.clone();
    let now = clock.clone();
    agent.add_game_method(method::MARKS, Access::Read, move |params, ctx| {
        let params: MarksParams = decode(params)?;
        if params.marks.len() > MAX_MARKS {
            return Err(invalid(format!("at most {MAX_MARKS} marks")));
        }
        let at = now();
        let needs_guest = !marks.0.no_tick_observer.load(Ordering::Relaxed)
            && params
                .marks
                .iter()
                .any(|m| matches!(&m.geometry, MarkGeometry::Object(_)));
        let result = if needs_guest {
            let mut pending = Some(params);
            let mut reply = None;
            ctx.reader.with_guest(&mut |guest| {
                if let Some(params) = pending.take() {
                    reply = Some(marks.replace_marks(params, at, Some(guest), windowed));
                }
            });
            match pending {
                Some(params) => marks.replace_marks(params, at, None, windowed)?,
                None => reply.expect("the guest reader answered")?,
            }
        } else {
            marks.replace_marks(params, at, None, windowed)?
        };
        Ok(serde_json::to_value(result).expect("marks result is JSON"))
    });
}
