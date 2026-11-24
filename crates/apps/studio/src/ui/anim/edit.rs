//! The timeline and the frame-based curve view of one animation document.
//! Undo and Redo are the graph's tab's (`Action::UndoDoc`): a document's
//! keys are steps of it beside the tag's own writes.

use std::ops::RangeInclusive;

use anim_edit::{
    AnimDoc, Gesture, Interp, Key, angles_near, angles_to_rotation, rotation_to_angles,
    tangent_from_angles, tangent_to_angles,
};
use dear_imgui_rs::{MouseButton, Ui};
use map::TagId;
use tags::anim_keys::Kind;

use crate::app::anim::{AnimRead, Transport};
use crate::app::{Action, Doc};
use crate::fields::curve::Plot;
use crate::theme;

const LABEL: f32 = 136.0;
const ROW: f32 = 21.0;
const AXES: [[f32; 4]; 3] = [theme::ACCENT, theme::OK, theme::WARN];

#[derive(Clone, Copy)]
struct Span {
    anchor: usize,
    head: usize,
}

impl Span {
    fn single(frame: usize) -> Self {
        Self {
            anchor: frame,
            head: frame,
        }
    }

    fn range(self) -> RangeInclusive<usize> {
        self.anchor.min(self.head)..=self.anchor.max(self.head)
    }

    fn map(self, f: impl Fn(usize) -> usize) -> Self {
        Self {
            anchor: f(self.anchor),
            head: f(self.head),
        }
    }
}

struct TimeDrag {
    gesture: Gesture,
    revision: u64,
    span: Span,
    track: Vec<Key>,
    at: f32,
    grabbed: usize,
}

#[derive(Clone, Copy)]
struct CurveKey {
    key: Key,
    value: [f32; 3],
    slope: [f32; 3],
}

struct CurveDrag {
    gesture: Gesture,
    revision: u64,
    key: CurveKey,
    axis: usize,
    handle: Handle,
    start_y: f32,
    plot: Plot,
}

#[derive(Clone, Copy)]
enum Handle {
    Value,
    Tangent(f32),
}

pub(super) struct State {
    pub node: usize,
    pub channel: Kind,
    curves: bool,
    axis: usize,
    selection: Option<Span>,
    time_drag: Option<TimeDrag>,
    curve_drag: Option<CurveDrag>,
    clipboard: Option<(Kind, Vec<Key>)>,
    time_scale: f32,
    problem: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            node: 0,
            channel: Kind::Translation,
            curves: false,
            axis: 0,
            selection: None,
            time_drag: None,
            curve_drag: None,
            clipboard: None,
            time_scale: 2.0,
            problem: None,
        }
    }
}

impl State {
    pub fn reset_selection(&mut self) {
        self.node = 0;
        self.selection = None;
        self.problem = None;
        self.end_pointer_edit();
    }

    pub fn end_pointer_edit(&mut self) {
        self.time_drag = None;
        self.curve_drag = None;
    }

    pub fn select_track(&mut self, node: usize, channel: Kind) {
        if (self.node, self.channel) != (node, channel) {
            self.node = node;
            if self.channel != channel {
                self.axis = 0;
            }
            self.channel = channel;
            self.selection = None;
            self.problem = None;
            self.end_pointer_edit();
        }
    }

    pub fn record_result(&mut self, result: Result<bool, String>) {
        self.problem = result.err();
    }

    /// Undo and Redo, and the gizmo's channel. The graph's tab is the
    /// document: Undo takes the newest of its steps, this document's key or
    /// the tag's own write, as Cmd+Z does (docs/editor/studio-flow.md,
    /// "Shared patterns"), so the buttons run `Action::UndoDoc` and
    /// `Action::RedoDoc` on it; `can` is what it can do. The action asked
    /// for.
    pub fn controls(&mut self, ui: &Ui, tag: TagId, can: (bool, bool)) -> Option<Action> {
        let mut asked = None;
        let disabled = ui.begin_disabled_with_cond(!can.0);
        if ui.button("Undo##anim") {
            asked = Some(Action::UndoDoc(Doc::Tag(tag)));
            self.problem = None;
        }
        drop(disabled);
        ui.same_line();
        let disabled = ui.begin_disabled_with_cond(!can.1);
        if ui.button("Redo##anim") {
            asked = Some(Action::RedoDoc(Doc::Tag(tag)));
            self.problem = None;
        }
        drop(disabled);
        for (kind, label) in [
            (Kind::Translation, "Move"),
            (Kind::Rotation, "Rotate"),
            (Kind::Scale, "Scale"),
        ] {
            ui.same_line();
            if self.channel == kind {
                ui.text_colored(theme::WIDGET_FOCUS, label);
            } else if ui.small_button(format!("{label}##anim-gizmo")) {
                self.select_track(self.node, kind);
            }
        }
        asked
    }

    pub fn draw(
        &mut self,
        ui: &Ui,
        doc: &mut AnimDoc,
        transport: &mut Transport,
        reduction: Option<&AnimRead>,
    ) {
        ui.separator();
        if self.curves {
            if ui.small_button("Timeline##anim-tab") {
                self.curves = false;
            }
        } else {
            ui.text_colored(theme::WIDGET_FOCUS, "Timeline");
        }
        ui.same_line();
        if self.curves {
            ui.text_colored(theme::WIDGET_FOCUS, "Curves");
        } else if ui.small_button("Curves##anim-tab") {
            self.curves = true;
        }
        if let Some(read) = reduction {
            let t = read.keys.tolerance;
            ui.text_disabled(format!(
                "Source reduction tolerance · {:.3}° / {:.4} units / {:.4} scale",
                t.rotation.to_degrees(),
                t.translation,
                t.scale,
            ));
            for note in &read.notes {
                ui.text_disabled(note);
            }
        }
        if doc.nodes().is_empty() {
            ui.text_disabled("This graph has no nodes");
            return;
        }
        if self.curves {
            self.curves(ui, doc, transport);
        } else {
            self.timeline(ui, doc, transport, reduction);
            self.timeline_tools(ui, doc, transport);
        }
        self.interpolation(ui, doc, transport);
        if let Some(problem) = &self.problem {
            ui.text_colored(theme::WARN, problem);
        }
    }

    fn select_key(&mut self, node: usize, kind: Kind, frame: usize, extend: bool) {
        self.select_track(node, kind);
        self.selection = match (extend, self.selection) {
            (true, Some(s)) => Some(Span {
                anchor: s.anchor,
                head: frame,
            }),
            _ => Some(Span::single(frame)),
        };
    }

    fn timeline(
        &mut self,
        ui: &Ui,
        doc: &mut AnimDoc,
        transport: &mut Transport,
        reduction: Option<&AnimRead>,
    ) {
        let width = ui.content_region_avail()[0].max(LABEL + 80.0);
        let frames = doc.frames();
        let last = frames.saturating_sub(1).max(1) as f32;
        let origin = ui.cursor_screen_pos();
        let track = width - LABEL - 8.0;
        let x = |f: f32| origin[0] + LABEL + f / last * track;
        let to_frame = |x: f32| (x - origin[0] - LABEL) / track * last;
        let height = ROW * (doc.nodes().len() + 2) as f32;
        ui.invisible_button("##anim-timeline", [width, height]);
        let mouse = ui.io().mouse_pos();
        if ui.is_item_clicked() {
            let row = ((mouse[1] - origin[1]) / ROW) as usize;
            let at = to_frame(mouse[0]).round().clamp(0.0, (frames - 1) as f32) as usize;
            if let Some(node) = row.checked_sub(1).filter(|&n| n < doc.nodes().len()) {
                let band = (((mouse[1] - origin[1]) / ROW).fract() * 3.0) as usize;
                let kind = Kind::ALL[band.min(2)];
                let hit = doc
                    .keys(node, kind)
                    .iter()
                    .min_by(|a, b| {
                        (x(a.frame as f32) - mouse[0])
                            .abs()
                            .total_cmp(&(x(b.frame as f32) - mouse[0]).abs())
                    })
                    .filter(|k| (x(k.frame as f32) - mouse[0]).abs() <= 5.0)
                    .copied();
                if let Some(key) = hit {
                    let extend = ui.io().key_shift();
                    let within = (self.node, self.channel) == (node, kind)
                        && self
                            .selection
                            .is_some_and(|s| s.range().contains(&key.frame));
                    if extend || !within {
                        self.select_key(node, kind, key.frame, extend);
                    }
                    if !extend && let Some(span) = self.selection {
                        self.time_drag = Some(TimeDrag {
                            gesture: doc.begin_gesture(),
                            revision: doc.revision(),
                            span,
                            track: doc.keys(node, kind).to_vec(),
                            at: to_frame(mouse[0]),
                            grabbed: key.frame,
                        });
                    }
                    transport.scrub(key.frame as f32, frames);
                } else {
                    self.select_track(node, kind);
                    self.selection = None;
                    if mouse[0] >= x(0.0) {
                        transport.scrub(at as f32, frames);
                    }
                }
            } else if row == 0 {
                transport.scrub(at as f32, frames);
            }
        } else if ui.is_item_active() && ui.is_mouse_down(MouseButton::Left) {
            if let Some(drag) = &mut self.time_drag
                && drag.revision == doc.revision()
            {
                let range = drag.span.range();
                let lo = -(*range.start() as isize);
                let hi = (frames - 1 - range.end()) as isize;
                let shift = ((to_frame(mouse[0]) - drag.at).round() as isize).clamp(lo, hi);
                let move_frame = |f: usize| f.saturating_add_signed(shift);
                let mut keys = drag.track.clone();
                for key in &mut keys {
                    if range.contains(&key.frame) {
                        key.frame = move_frame(key.frame);
                    }
                }
                keys.sort_unstable_by_key(|key| key.frame);
                let result = doc.replace_keys(self.node, self.channel, keys, Some(drag.gesture));
                if result.is_ok() {
                    drag.revision = doc.revision();
                    self.selection = Some(drag.span.map(move_frame));
                    transport.scrub(move_frame(drag.grabbed) as f32, frames);
                }
                self.record_result(result);
            } else if self.time_drag.is_none() && mouse[0] >= x(0.0) {
                transport.scrub(to_frame(mouse[0]).round(), frames);
            }
        }

        let dl = ui.get_window_draw_list();
        dl.add_rect(
            origin,
            [origin[0] + width, origin[1] + height],
            theme::RAISED,
        )
        .filled(true)
        .build();
        for frame in (0..frames).step_by(ruler_step(frames, track)) {
            let fx = x(frame as f32);
            dl.add_line(
                [fx, origin[1] + 15.0],
                [fx, origin[1] + height],
                theme::RULE,
            )
            .build();
            dl.add_text([fx + 2.0, origin[1] + 2.0], theme::DIM, &frame.to_string());
        }
        for (node, n) in doc.nodes().iter().enumerate() {
            let top = origin[1] + ROW * (node + 1) as f32;
            let name = n.name.clone().unwrap_or_else(|| format!("Node {node}"));
            let mut label = 6.0;
            let mut parent = n.parent;
            while label < 24.0 {
                let Some(p) = usize::try_from(parent)
                    .ok()
                    .and_then(|i| doc.nodes().get(i))
                else {
                    break;
                };
                label += 3.0;
                parent = p.parent;
            }
            let colour = if self.node == node {
                theme::WIDGET_FOCUS
            } else {
                theme::TEXT
            };
            dl.add_text([origin[0] + label, top + 2.0], colour, &name);
            for (i, kind) in Kind::ALL.into_iter().enumerate() {
                let active = (self.node, self.channel) == (node, kind);
                let y = top + ROW * (i as f32 + 0.5) / 3.0;
                let colour = AXES[i];
                let keys = doc.keys(node, kind);
                if active {
                    dl.add_line([x(0.0), y], [x(last), y], theme::DIM).build();
                }
                if let (Some(a), Some(b)) = (keys.first(), keys.last()) {
                    let mut line = colour;
                    line[3] = 0.45;
                    dl.add_line([x(a.frame as f32), y], [x(b.frame as f32), y], line)
                        .build();
                }
                for key in keys {
                    let kx = x(key.frame as f32);
                    let selected = active
                        && self
                            .selection
                            .is_some_and(|s| s.range().contains(&key.frame));
                    let marker = if selected {
                        theme::WIDGET_FOCUS
                    } else {
                        colour
                    };
                    dl.add_rect([kx - 2.5, y - 2.5], [kx + 2.5, y + 2.5], marker)
                        .filled(true)
                        .build();
                }
            }
        }
        if let Some(read) = reduction {
            let at = origin[1] + ROW * (doc.nodes().len() + 1) as f32;
            dl.add_text(
                [origin[0] + 6.0, at + 2.0],
                theme::DIM,
                &format!(
                    "Events · {} frame, {} sound, {} effect",
                    read.events[0], read.events[1], read.events[2]
                ),
            );
        }
        let scrub = x(transport.frame.min(last));
        dl.add_line(
            [scrub, origin[1]],
            [scrub, origin[1] + height],
            theme::WIDGET_FOCUS,
        )
        .thickness(2.0)
        .build();
        ui.text_colored(AXES[0], "Rotation");
        ui.same_line();
        ui.text_colored(AXES[1], "Translation");
        ui.same_line();
        ui.text_colored(AXES[2], "Scale");
    }

    fn timeline_tools(&mut self, ui: &Ui, doc: &mut AnimDoc, transport: &mut Transport) {
        let (node, kind) = (self.node, self.channel);
        let selected = self.selection.map(|s| s.range());
        let keys = doc.keys(node, kind);
        let has_selected = selected
            .as_ref()
            .is_some_and(|span| keys.iter().any(|k| span.contains(&k.frame)));
        let disabled = ui.begin_disabled_with_cond(!has_selected);
        if ui.button("Copy##anim-keys")
            && let Some(span) = &selected
        {
            self.clipboard = Some((
                kind,
                keys.iter()
                    .filter(|k| span.contains(&k.frame))
                    .copied()
                    .collect(),
            ));
        }
        drop(disabled);

        ui.same_line();
        let disabled = ui.begin_disabled_with_cond(
            !self
                .clipboard
                .as_ref()
                .is_some_and(|(held, _)| *held == kind),
        );
        if ui.button("Paste at head##anim-keys")
            && let Some((held, copied)) = &self.clipboard
            && *held == kind
            && let Some(first) = copied.first()
        {
            let at = transport.frame.round() as usize;
            let mut keys = doc.keys(node, kind).to_vec();
            let end = at + copied.last().expect("nonempty clipboard").frame - first.frame;
            for key in copied {
                let mut key = *key;
                key.frame = at + key.frame - first.frame;
                match keys.binary_search_by_key(&key.frame, |k| k.frame) {
                    Ok(index) => keys[index] = key,
                    Err(index) => keys.insert(index, key),
                }
            }
            let result = doc.replace_keys(node, kind, keys, None);
            if result.is_ok() {
                self.selection = Some(Span {
                    anchor: at,
                    head: end,
                });
            }
            self.record_result(result);
        }
        drop(disabled);

        ui.same_line();
        let disabled = ui.begin_disabled_with_cond(!has_selected);
        if ui.button("Delete##anim-keys")
            && let Some(span) = &selected
        {
            let keys = doc
                .keys(node, kind)
                .iter()
                .filter(|k| !span.contains(&k.frame))
                .copied()
                .collect();
            self.record_result(doc.replace_keys(node, kind, keys, None));
            self.selection = None;
        }
        ui.set_next_item_width(90.0);
        ui.slider_f32("##anim-time-factor", &mut self.time_scale, 0.1, 4.0);
        ui.same_line();
        if ui.button("Scale time##anim-keys")
            && let Some(span) = &selected
        {
            let start = *span.start();
            let factor = self.time_scale;
            let scaled = |f| start + (((f - start) as f32) * factor).round() as usize;
            let mut keys = doc.keys(node, kind).to_vec();
            for key in &mut keys {
                if span.contains(&key.frame) {
                    key.frame = scaled(key.frame);
                    key.tangent = key.tangent.map(|v| v / factor);
                }
            }
            keys.sort_unstable_by_key(|k| k.frame);
            let result = doc.replace_keys(node, kind, keys, None);
            if result.is_ok() {
                self.selection = self.selection.map(|s| s.map(scaled));
            }
            self.record_result(result);
        }
        drop(disabled);
    }

    fn interpolation(&mut self, ui: &Ui, doc: &mut AnimDoc, transport: &Transport) {
        let (node, kind) = (self.node, self.channel);
        let frame = transport.frame.round() as usize;
        let Some(key) = doc.key(node, kind, frame) else {
            return;
        };
        if let Some(_combo) = ui.begin_combo("Interpolation##anim-interp", key.interp.name()) {
            for mode in Interp::ALL {
                if ui
                    .selectable_config(mode.name())
                    .selected(key.interp == mode)
                    .build()
                {
                    self.record_result(doc.set_key(node, kind, frame, key.value, Some(mode)));
                }
            }
        }
    }

    fn curves(&mut self, ui: &Ui, doc: &mut AnimDoc, transport: &mut Transport) {
        let node_label = doc.nodes()[self.node]
            .name
            .as_deref()
            .unwrap_or("Unnamed node");
        if let Some(_combo) = ui.begin_combo("Node##anim-curve-node", node_label) {
            for (i, node) in doc.nodes().iter().enumerate() {
                let label = node.name.clone().unwrap_or_else(|| format!("Node {i}"));
                if ui
                    .selectable_config(format!("{label}##curve-node-{i}"))
                    .selected(self.node == i)
                    .build()
                {
                    self.select_track(i, self.channel);
                }
            }
        }
        let kind = self.channel;
        let node = self.node;
        let axis_labels: &[&str] = match kind {
            Kind::Rotation => &["Yaw (°)", "Pitch (°)", "Roll (°)"],
            Kind::Translation => &["X", "Y", "Z"],
            Kind::Scale => &["Scale"],
        };
        for (axis, label) in axis_labels.iter().enumerate() {
            if axis > 0 {
                ui.same_line();
            }
            if self.axis == axis {
                ui.text_colored(AXES[axis], label);
            } else if ui.small_button(format!("{label}##anim-curve-axis")) {
                self.axis = axis;
            }
        }

        let keys = doc.keys(node, kind);
        if keys.is_empty() {
            ui.text_disabled("Use the viewport gizmo to set the first key on this channel");
            return;
        }
        let points = curve_points(doc, node, kind);
        let handles = curve_keys(doc, node, kind, &points);
        let last = doc.frames().saturating_sub(1).max(1) as f32;
        let plot = Plot {
            origin: ui.cursor_screen_pos(),
            size: [ui.content_region_avail()[0].max(72.0), 190.0],
            x: [0.0, last],
            y: self
                .curve_drag
                .as_ref()
                .map(|d| d.plot.y)
                .unwrap_or_else(|| plot_range(&points, axis_labels.len(), kind)),
        };
        ui.invisible_button("##anim-curve", plot.size);
        plot.background(ui);
        for (axis, &colour) in AXES[..axis_labels.len()].iter().enumerate() {
            plot.line(ui, points.iter().map(|(f, v)| [*f, v[axis]]), colour);
        }
        let dl = ui.get_window_draw_list();
        for frame in (0..doc.frames()).step_by(ruler_step(doc.frames(), plot.size[0])) {
            let [x, y] = plot.point(frame as f32, plot.y[0]);
            dl.add_text([x + 2.0, y - 15.0], theme::DIM, &frame.to_string());
        }
        let x = plot.point(transport.frame, plot.y[0])[0];
        dl.add_line(
            [x, plot.origin[1]],
            [x, plot.origin[1] + plot.size[1]],
            theme::WIDGET_FOCUS,
        )
        .build();

        let cursor = ui.io().mouse_pos();
        let mut hovered = None;
        let mut distance = 64.0;
        let axis = self.axis;
        let colour = AXES[axis];
        for (index, key) in handles.iter().enumerate() {
            let selected = self
                .selection
                .is_some_and(|s| s.range().contains(&key.key.frame));
            let point = plot.point(key.key.frame as f32, key.value[axis]);
            let mut hit = |point: [f32; 2], handle| {
                let d = (cursor[0] - point[0]).powi(2) + (cursor[1] - point[1]).powi(2);
                if d < distance {
                    distance = d;
                    hovered = Some((*key, axis, handle));
                }
            };
            hit(point, Handle::Value);
            let [x, y] = point;
            dl.add_rect(
                [x - 3.0, y - 3.0],
                [x + 3.0, y + 3.0],
                if selected {
                    theme::WIDGET_FOCUS
                } else {
                    colour
                },
            )
            .filled(true)
            .build();
            let smooth_out = keys
                .get(index + 1)
                .filter(|_| key.key.interp == Interp::Smooth);
            let smooth_in = index
                .checked_sub(1)
                .and_then(|i| keys.get(i))
                .filter(|k| k.interp == Interp::Smooth);
            for neighbour in smooth_out.into_iter().chain(smooth_in).filter(|_| selected) {
                let length = (neighbour.frame as f32 - key.key.frame as f32) / 3.0;
                let end = plot.point(
                    key.key.frame as f32 + length,
                    key.value[axis] + key.slope[axis] * length,
                );
                dl.add_line(point, end, colour).build();
                dl.add_rect(
                    [end[0] - 3.0, end[1] - 3.0],
                    [end[0] + 3.0, end[1] + 3.0],
                    colour,
                )
                .build();
                hit(end, Handle::Tangent(length));
            }
        }
        drop(dl);
        if ui.is_item_clicked() {
            if let Some((key, axis, handle)) = hovered {
                self.select_key(node, kind, key.key.frame, false);
                self.curve_drag = Some(CurveDrag {
                    gesture: doc.begin_gesture(),
                    revision: doc.revision(),
                    key,
                    axis,
                    handle,
                    start_y: cursor[1],
                    plot,
                });
                transport.scrub(key.key.frame as f32, doc.frames());
            } else {
                let frame = ((cursor[0] - plot.origin[0]) / plot.size[0] * last).round();
                transport.scrub(frame, doc.frames());
                self.selection = None;
            }
        } else if ui.is_item_active()
            && ui.is_mouse_down(MouseButton::Left)
            && let Some(drag) = &mut self.curve_drag
            && drag.revision == doc.revision()
        {
            let result = drag.apply(doc, node, kind, cursor[1]);
            drag.revision = doc.revision();
            self.record_result(result);
        }
    }
}

impl CurveDrag {
    fn apply(
        &self,
        doc: &mut AnimDoc,
        node: usize,
        kind: Kind,
        mouse_y: f32,
    ) -> Result<bool, String> {
        let frame = self.key.key.frame;
        let axis = self.axis;
        let gesture = Some(self.gesture);
        if mouse_y == self.start_y {
            // Return to the exact source value if the pointer returns.
            return match self.handle {
                Handle::Value => {
                    doc.set_key_during_gesture(node, kind, frame, self.key.key.value, None, gesture)
                }
                Handle::Tangent(_) => {
                    doc.set_tangent(node, kind, frame, self.key.key.tangent, gesture)
                }
            };
        }
        let value = self.key.value;
        let change = self.plot.delta_y(self.start_y - mouse_y);
        match self.handle {
            Handle::Value => {
                let mut next = value;
                next[axis] += change;
                let stored = if kind == Kind::Rotation {
                    angles_to_rotation(next)
                } else {
                    [next[0], next[1], next[2], 0.0]
                };
                doc.set_key_during_gesture(node, kind, frame, stored, None, gesture)
            }
            Handle::Tangent(length) => {
                let mut next = self.key.slope;
                next[axis] += change / length;
                let tangent = if kind == Kind::Rotation {
                    tangent_from_angles(self.key.key.value, value, next)
                } else {
                    [next[0], next[1], next[2], 0.0]
                };
                doc.set_tangent(node, kind, frame, tangent, gesture)
            }
        }
    }
}

fn frame_values(kind: Kind, value: [f32; 4], previous: Option<[f32; 3]>) -> [f32; 3] {
    if kind == Kind::Rotation {
        previous.map_or_else(
            || rotation_to_angles(value),
            |prev| angles_near(value, prev),
        )
    } else {
        [value[0], value[1], value[2]]
    }
}

/// Sample the actual interpolation, including exact keys and samples
/// immediately to the left of step boundaries.
fn curve_points(doc: &AnimDoc, node: usize, kind: Kind) -> Vec<(f32, [f32; 3])> {
    let last = (doc.frames() - 1) as f32;
    let mut frames: Vec<_> = (0..=192).map(|f| last * f as f32 / 192.0).collect();
    let keys = doc.keys(node, kind);
    frames.extend(keys.iter().map(|k| k.frame as f32));
    for pair in keys.windows(2) {
        let [a, b] = [pair[0].frame as f32, pair[1].frame as f32];
        match pair[0].interp {
            Interp::Step => frames.push(b - 1.0 / 64.0),
            Interp::Smooth => frames.extend([a + (b - a) / 3.0, b - (b - a) / 3.0]),
            Interp::Linear => {}
        }
    }
    frames.sort_unstable_by(f32::total_cmp);
    frames.dedup();
    let mut points = Vec::with_capacity(frames.len());
    let mut previous = None;
    for frame in frames {
        if let Some(value) = doc.value_at(node, kind, frame) {
            let value = frame_values(kind, value, previous);
            points.push((frame, value));
            previous = Some(value);
        }
    }
    points
}

fn curve_keys(doc: &AnimDoc, node: usize, kind: Kind, points: &[(f32, [f32; 3])]) -> Vec<CurveKey> {
    doc.keys(node, kind)
        .iter()
        .filter_map(|&key| {
            let i = points
                .binary_search_by(|p| p.0.total_cmp(&(key.frame as f32)))
                .ok()?;
            let value = points[i].1;
            let slope = if kind == Kind::Rotation {
                tangent_to_angles(key.value, value, key.tangent)
            } else {
                [key.tangent[0], key.tangent[1], key.tangent[2]]
            };
            Some(CurveKey { key, value, slope })
        })
        .collect()
}

fn plot_range(points: &[(f32, [f32; 3])], axes: usize, kind: Kind) -> [f32; 2] {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(_, values) in points {
        for &v in &values[..axes] {
            lo = lo.min(f64::from(v));
            hi = hi.max(f64::from(v));
        }
    }
    let minimum = match kind {
        Kind::Rotation => 15.0,
        Kind::Translation => 0.25,
        Kind::Scale => 0.1,
    };
    let pad = ((hi - lo) * 0.12)
        .max(lo.abs().max(hi.abs()) * 1e-6)
        .max(minimum);
    let limit = f64::from(f32::MAX);
    [(lo - pad).max(-limit) as f32, (hi + pad).min(limit) as f32]
}

/// The first 1, 2 or 5 times a power of ten which leaves room for its label.
fn ruler_step(frames: usize, width: f32) -> usize {
    let per_frame = width / frames.saturating_sub(1).max(1) as f32;
    for scale in [1, 10, 100, 1000, 10000] {
        for count in [1, 2, 5] {
            if (count * scale) as f32 * per_frame >= 40.0 {
                return count * scale;
            }
        }
    }
    frames.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anim_edit::Node;

    #[test]
    fn drawing_a_rotation_curve_preserves_the_stored_quaternions() {
        let mut doc = AnimDoc::new(
            "invented".into(),
            5,
            vec![Node {
                name: None,
                parent: -1,
            }],
        )
        .unwrap();
        let kind = Kind::Rotation;
        for (frame, yaw) in [(0, 175.0), (4, 185.0)] {
            doc.set_key(0, kind, frame, angles_to_rotation([yaw, 40.0, 18.0]), None)
                .unwrap();
        }
        let original = doc.keys(0, kind).to_vec();
        let revision = doc.revision();
        let plot = curve_points(&doc, 0, kind);
        let handles = curve_keys(&doc, 0, kind, &plot);
        assert!((handles[1].value[0] - 185.0).abs() < 0.01);
        assert_eq!(doc.keys(0, kind), original);
        assert_eq!(doc.revision(), revision);
    }
}
