//! The curve editor of a `view=function` data field
//! (docs/editor/rich-editors.md, "Curves"): the row's small plot of the
//! curve, and the dialog that shows it large with the type's handles.
//! Both read and write the function blob through `TagDoc` alone: every
//! write is a byte write of the same length at the data's start, except
//! a type change, which rebuilds the blob at the new type's length with
//! its defaults (`Function::with_kind`) and resizes the data
//! (`TagAction::ResizeData`); either way it is one undo step, and save
//! and `tag.fields` see it as an ordinary edit.
//!
//! The plot is `Function::evaluate` itself (the engine's evaluator,
//! ported), sampled over the input's `[0, 1]`; the agent API's
//! `function.sample` (api/curves.rs) reads the same values.

use std::sync::Arc;

use dear_imgui_rs::{Drag, Key, Ui, WindowFlags};
use map::TagId;
use serde_json::{Value, json};
use tags::effects::function::{Function, FunctionType, Set, Tables};
use tags::registry::{Field, Layout};

use super::{Cx, PanelState, Row, fmt_f, write_action};
use crate::app::{Action, TagAction, TagDoc};
use crate::theme;

/// Why a function of a type the engine does not evaluate (6, 8, unknown)
/// is shown only.
pub const EVALUATED_AS_ZERO: &str = "The game evaluates this type as 0.";
/// Why a change to `kind` is not written: the game evaluates it as 0 (or
/// has no layout for it), so there is nothing to rebuild the blob as.
pub fn change_to_unevaluated(kind: FunctionType) -> String {
    format!(
        "The game evaluates the {} type as 0, so a curve is not changed to it.",
        kind.word()
    )
}
/// The second half of a wave's engine-tables sentence.
const ENGINE_WAVE: &str = "needs the engine's tables; it evaluates as 0 without them";
/// The plot's samples, in the row and in the dialog.
const ROW_COLUMNS: usize = 49;
const PLOT_COLUMNS: usize = 193;
/// The row's plot and the dialog's, in points.
const ROW_PLOT: [f32; 2] = [96.0, 26.0];
const PLOT: [f32; 2] = [440.0, 190.0];
/// The curve types the type picker offers (the handles' list; identity
/// has no handles and the multi types no evaluator, so none of the three
/// is offered).
const TYPES: [FunctionType; 8] = [
    FunctionType::Constant,
    FunctionType::Transition,
    FunctionType::Periodic,
    FunctionType::Linear,
    FunctionType::LinearKey,
    FunctionType::Spline,
    FunctionType::Spline2,
    FunctionType::Exponent,
];

/// The function blob a `view=function` data field's pair at `address`
/// points at: its byte length, where the bytes start, and the parsed
/// function.
pub fn function(doc: &TagDoc, tag: TagId, address: u32) -> Result<(u32, u32, Function), String> {
    let (n, start) = doc
        .pair(tag, address)
        .ok_or("the data reference is unreadable")?;
    let bytes = doc
        .read(tag, start, n)
        .ok_or("the function's bytes are outside the tag's address space")?;
    Function::parse(&bytes).map(|f| (n, start, f)).map_err(why)
}

/// A parse error in words, without the record-level prefix.
fn why(e: tags::Error) -> String {
    match e {
        tags::Error::Invalid(m) => m.to_owned(),
        e => e.to_string(),
    }
}

/// Why a `view=function` field's data is not shown as a curve: empty,
/// unreadable, or not a function blob.
pub(super) fn not_a_function(doc: &TagDoc, tag: TagId, n: u32, start: u32) -> String {
    if n == 0 {
        return "The data reference is empty.".into();
    }
    match doc.read(tag, start, n) {
        None => "The function's bytes are outside the tag's address space.".into(),
        Some(bytes) => Function::parse(&bytes)
            .err()
            .map_or_else(|| "The data is a function curve.".into(), why),
    }
}

/// The `function` key of a `tag.fields` row: `{type, ranged, range,
/// length}`, or null when the data is not a function blob.
pub fn row_json(doc: &TagDoc, tag: TagId, address: u32) -> Value {
    match function(doc, tag, address) {
        Ok((n, _, f)) => json!({
            "type": f.kind.word(),
            "ranged": f.is_ranged(),
            "range": f.range,
            "length": n,
        }),
        Err(_) => Value::Null,
    }
}

/// Screen coordinates shared by function and animation curves.
#[derive(Clone, Copy)]
pub(crate) struct Plot {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub x: [f32; 2],
    pub y: [f32; 2],
}

impl Plot {
    pub(crate) fn point(self, x: f32, y: f32) -> [f32; 2] {
        let fraction = |v: f32, [lo, hi]: [f32; 2]| {
            let (v, lo, hi) = (f64::from(v), f64::from(lo), f64::from(hi));
            ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32
        };
        let u = fraction(x, self.x);
        let v = fraction(y, self.y);
        [
            self.origin[0] + self.size[0] * u,
            self.origin[1] + self.size[1] * (1.0 - v),
        ]
    }

    pub(crate) fn delta_y(self, pixels_up: f32) -> f32 {
        let [lo, hi] = self.y.map(f64::from);
        (f64::from(pixels_up / self.size[1]) * (hi - lo)) as f32
    }

    pub(crate) fn background(self, ui: &Ui) {
        ui.get_window_draw_list()
            .add_rect(
                self.origin,
                [self.origin[0] + self.size[0], self.origin[1] + self.size[1]],
                theme::RAISED,
            )
            .filled(true)
            .build();
    }

    pub(crate) fn line(
        self,
        ui: &Ui,
        values: impl IntoIterator<Item = [f32; 2]>,
        colour: [f32; 4],
    ) {
        let dl = ui.get_window_draw_list();
        let mut last = None;
        for [x, y] in values {
            let point = (x.is_finite() && y.is_finite()).then(|| self.point(x, y));
            if let (Some(a), Some(b)) = (last, point) {
                dl.add_line(a, b, colour).thickness(1.5).build();
            }
            last = point;
        }
    }
}

/// The curve of `f` over the input's `[0, 1]` at the range input `r`,
/// with the two sets faint when the function is ranged. A NaN output
/// (which the engine passes through) leaves a gap.
fn plot(ui: &Ui, origin: [f32; 2], size: [f32; 2], f: &Function, r: f32, columns: usize) {
    let plot = Plot {
        origin,
        size,
        x: [0.0, 1.0],
        y: [0.0, 1.0],
    };
    plot.background(ui);
    let curve = |blend, colour| {
        plot.line(
            ui,
            (0..columns).map(|i| {
                let t = i as f32 / (columns as f32 - 1.0);
                [t, f.evaluate(t, blend)]
            }),
            colour,
        );
    };
    if f.is_ranged() {
        curve(0.0, theme::DIM);
        curve(1.0, theme::DIM);
    }
    curve(r, theme::ACCENT);
}

/// The type's display name (the picker's words).
fn type_name(kind: FunctionType) -> String {
    match kind {
        FunctionType::Identity => "Identity".into(),
        FunctionType::Constant => "Constant".into(),
        FunctionType::Transition => "Transition".into(),
        FunctionType::Periodic => "Periodic".into(),
        FunctionType::Linear => "Linear".into(),
        FunctionType::LinearKey => "Linear key".into(),
        FunctionType::MultiLinearKey => "Multi linear key".into(),
        FunctionType::Spline => "Spline".into(),
        FunctionType::MultiSpline => "Multi spline".into(),
        FunctionType::Exponent => "Exponent".into(),
        FunctionType::Spline2 => "Spline 2".into(),
        FunctionType::Unknown(n) => format!("Unknown ({n})"),
    }
}

/// A shape's name (type 2; names **S**, docs/formats/effects.md).
fn shape_name(sub: u8) -> String {
    [
        "Linear",
        "Square root",
        "Fourth root",
        "Square",
        "Fourth power",
        "Cosine ease",
    ]
    .get(usize::from(sub))
    .map_or_else(|| format!("Shape {sub}"), |s| (*s).into())
}

/// A wave's name (type 3; names **S**); the ones only the engine's own
/// tables hold are named by number.
fn wave_name(sub: u8) -> String {
    match sub {
        0 => "One".into(),
        1 => "Zero".into(),
        2 => "Cosine".into(),
        4 => "Triangle".into(),
        6 => "Sawtooth".into(),
        n => format!("Wave {n}"),
    }
}

/// The open curve dialog.
#[derive(Clone, Debug)]
pub struct Open {
    pub tag: TagId,
    /// The field's address (its data pair).
    pub address: u32,
    /// The field, for the undo step's label.
    pub field: Field,
    /// The struct the field is in (its address and layout), for a type
    /// change's resize.
    element: u32,
    layout: Arc<Layout>,
    /// The row's label, the dialog's title.
    pub title: String,
    /// The range-input slider's place (what the dialog is showing).
    pub blend: f32,
    /// The choice list a row opened (None: the editor itself).
    pick: Option<Pick>,
    /// Why the last choice was not taken.
    problem: Option<String>,
    opening: bool,
}

/// What a choice row in the dialog is listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    /// The curve's type.
    Type,
    /// Set `i`'s wave (periodic) or shape (transition).
    Selector { set: usize },
}

impl Open {
    pub(super) fn new(
        tag: TagId,
        address: u32,
        field: &Field,
        layout: &Arc<Layout>,
        title: &str,
    ) -> Self {
        Self {
            tag,
            address,
            field: field.clone(),
            element: address - field.offset,
            layout: layout.clone(),
            title: title.to_owned(),
            blend: 0.5,
            pick: None,
            problem: None,
            opening: true,
        }
    }
}

/// The row's own widget for a function field: the small plot (the curve
/// blended at the middle of the range input) and **Edit curve...**, which
/// opens the dialog.
pub(super) fn row(cx: &mut Cx<'_, '_>, at: u32, layout: &Arc<Layout>, r: &Row, f: Function) {
    let ui = cx.ui;
    cx.label(at, layout, r);
    let origin = ui.cursor_screen_pos();
    ui.dummy(ROW_PLOT);
    let blend = if f.is_ranged() { 0.5 } else { 0.0 };
    plot(ui, origin, ROW_PLOT, &f, blend, ROW_COLUMNS);
    cx.hover(at, layout, r);
    ui.same_line();
    if ui.small_button("Edit curve...##curve") {
        cx.st.curve = Some(Open::new(cx.tag, r.address, r.field, layout, &r.label));
    }
    cx.tail(r);
    if !f.kind.is_evaluated() {
        ui.text_disabled(EVALUATED_AS_ZERO);
    }
}

/// The small plot of `f` as the Shader tab's animation rows draw it (the
/// row's size, the curve over the input's `[0, 1]`), with a marker at
/// input `marker` (the preview clock's place in the cycle) when given.
pub(super) fn animation_plot(ui: &Ui, f: &Function, marker: Option<f32>) {
    let origin = ui.cursor_screen_pos();
    ui.dummy(ROW_PLOT);
    plot(ui, origin, ROW_PLOT, f, 0.0, ROW_COLUMNS);
    if let Some(x) = marker.filter(|x| x.is_finite()) {
        let at = Plot {
            origin,
            size: ROW_PLOT,
            x: [0.0, 1.0],
            y: [0.0, 1.0],
        };
        ui.get_window_draw_list()
            .add_line(at.point(x, 0.0), at.point(x, 1.0), theme::WARN)
            .thickness(1.0)
            .build();
    }
}

/// Draw the open dialog (call once per panel, after its rows, beside the
/// pick dialog: it opens at the panel's ID level). A write is pushed as
/// a same-length byte write at the data's start; a drag is one undo step.
pub(super) fn draw(ui: &Ui, doc: &TagDoc, st: &mut PanelState, actions: &mut Vec<Action>) {
    let Some(mut open) = st.curve.take() else {
        return;
    };
    let opening = std::mem::take(&mut open.opening);
    let title = format!("{}###curve", open.title);
    if opening {
        ui.open_popup(&title);
    }
    let Some(_modal) = ui
        .begin_modal_popup_config(&title)
        .flags(WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS)
        .begin()
    else {
        return;
    };
    if editor(ui, doc, &mut open, actions) {
        ui.close_current_popup();
    } else {
        st.curve = Some(open);
    }
}

/// The dialog's body: the curve as it stands, or the choice list a row
/// opened. Returns true when the dialog closes.
fn editor(ui: &Ui, doc: &TagDoc, open: &mut Open, actions: &mut Vec<Action>) -> bool {
    let mut done = ui.is_key_pressed(Key::Escape);
    let Some((n, start)) = doc.pair(open.tag, open.address) else {
        ui.text_disabled("The data reference is unreadable.");
        return true;
    };
    let Some(bytes) = doc.read(open.tag, start, n) else {
        ui.text_disabled("The function's bytes are outside the tag's address space.");
        return true;
    };
    let f = match Function::parse(&bytes) {
        Ok(f) => f,
        Err(e) => {
            ui.text_disabled(why(e));
            return true;
        }
    };
    if let Some(pick) = open.pick {
        return choices(ui, open, actions, start, &f, pick) || done;
    }
    if let Some(problem) = &open.problem {
        ui.text_colored(theme::WARN, problem);
    }
    let read_only = doc.refusal(open.tag).is_some() || !f.kind.is_evaluated();
    if let Some(reason) = doc
        .refusal(open.tag)
        .or((!f.kind.is_evaluated()).then_some(EVALUATED_AS_ZERO))
    {
        ui.text_colored(theme::WARN, reason);
    }

    // The plot, the input along the bottom, the output range at the side.
    let origin = ui.cursor_screen_pos();
    ui.dummy(PLOT);
    plot(ui, origin, PLOT, &f, open.blend, PLOT_COLUMNS);
    let dl = ui.get_window_draw_list();
    let [lo, hi] = f.range;
    let (top, bottom) = if f.is_raw_output() {
        (1.0, 0.0)
    } else {
        (hi, lo)
    };
    dl.add_text(
        [origin[0] + PLOT[0] + 6.0, origin[1]],
        theme::DIM,
        format!("{top:.2}"),
    );
    dl.add_text(
        [
            origin[0] + PLOT[0] + 6.0,
            origin[1] + PLOT[1] - ui.text_line_height(),
        ],
        theme::DIM,
        format!("{bottom:.2}"),
    );
    dl.add_text([origin[0], origin[1] + PLOT[1] + 2.0], theme::DIM, "0");
    dl.add_text(
        [origin[0] + PLOT[0] - 10.0, origin[1] + PLOT[1] + 2.0],
        theme::DIM,
        "1",
    );

    // The range input, when the two sets blend.
    if f.is_ranged() {
        ui.text_disabled("Range input");
        ui.same_line();
        ui.set_next_item_width(PLOT[0] * 0.5);
        ui.slider_config("##blend", 0.0, 1.0)
            .display_format(fmt_f("%.2f"))
            .build(&mut open.blend);
    }

    // The type row and the ranged switch.
    ui.text_disabled("Type");
    ui.same_line();
    if ui.button_with_size(format!("{}##type", type_name(f.kind)), [150.0, 0.0]) {
        open.pick = Some(Pick::Type);
        open.problem = None;
    }
    ui.same_line();
    let off = ui.begin_disabled_with_cond(read_only);
    let mut ranged = f.is_ranged();
    if ui.checkbox("Ranged##ranged", &mut ranged) {
        let mut next = f.clone();
        next.flags = (next.flags & !1) | u8::from(ranged);
        write(actions, open, start, &next);
        actions.push(Action::Tag(TagAction::Seal));
    }
    drop(off);

    // The output range, unless the output is used as it is.
    ui.text_disabled("Output range");
    ui.same_line();
    ui.set_next_item_width(90.0);
    header(
        ui,
        open,
        actions,
        start,
        read_only,
        &f,
        "##lo",
        lo,
        |f, v| f.range[0] = v,
    );
    ui.same_line();
    ui.set_next_item_width(90.0);
    header(
        ui,
        open,
        actions,
        start,
        read_only,
        &f,
        "##hi",
        hi,
        |f, v| f.range[1] = v,
    );
    if f.is_raw_output() {
        ui.text_disabled("The output is used as it is, not mapped onto the range.");
    }

    handles(ui, open, actions, start, read_only, &f, 0);
    if f.is_ranged() {
        ui.separator();
        handles(ui, open, actions, start, read_only, &f, 1);
    }
    ui.separator();
    done |= ui.button("Close##curve");
    done
}

/// A type change: the blob rebuilt at the new type's length, written as
/// one undo step through `TagDoc::resize_data` (in place when it is no
/// longer than the data's room, else grown; growth off is refused by
/// name and shown as the error).
fn retype(actions: &mut Vec<Action>, open: &Open, f: &Function) {
    actions.push(Action::Tag(TagAction::ResizeData {
        tag: open.tag,
        element: open.element,
        layout: open.layout.clone(),
        field: open.field.offset,
        bytes: f.to_bytes(),
        label: open.field.name.clone(),
    }));
    actions.push(Action::Tag(TagAction::Seal));
}

/// One write of the new function bytes at the data's start, merged by
/// the data's address so one drag is one undo step.
fn write(actions: &mut Vec<Action>, open: &Open, start: u32, f: &Function) {
    actions.push(write_action(
        open.tag,
        start,
        f.to_bytes(),
        &open.field.name,
    ));
}

/// One handle: a number drag that writes set `set`'s typed value with
/// `put` applied to the dragged number. A drag is one undo step.
fn handle(
    ui: &Ui,
    open: &Open,
    actions: &mut Vec<Action>,
    start: u32,
    read_only: bool,
    f: &Function,
    set: usize,
    id: &str,
    v: f32,
    put: &dyn Fn(&mut Set, f32),
) {
    let mut x = v;
    let off = ui.begin_disabled_with_cond(read_only);
    if Drag::<f32, _>::new(id)
        .speed(0.01)
        .display_format(fmt_f("%.4f"))
        .build(ui, &mut x)
    {
        let mut next = f.clone();
        if let Some(mut values) = next.set_values(set) {
            put(&mut values, x);
            let _ = next.write_set(set, &values);
            write(actions, open, start, &next);
        }
    }
    drop(off);
    if ui.is_item_deactivated_after_edit() {
        actions.push(Action::Tag(TagAction::Seal));
    }
}

/// One header number (the output range): as [`handle`], on the function
/// itself.
fn header(
    ui: &Ui,
    open: &Open,
    actions: &mut Vec<Action>,
    start: u32,
    read_only: bool,
    f: &Function,
    id: &str,
    v: f32,
    put: impl Fn(&mut Function, f32),
) {
    let mut x = v;
    let off = ui.begin_disabled_with_cond(read_only);
    if Drag::<f32, _>::new(id)
        .speed(0.01)
        .display_format(fmt_f("%.3f"))
        .build(ui, &mut x)
    {
        let mut next = f.clone();
        put(&mut next, x);
        write(actions, open, start, &next);
    }
    drop(off);
    if ui.is_item_deactivated_after_edit() {
        actions.push(Action::Tag(TagAction::Seal));
    }
}

/// The handles of set `set` (0, or 1 under a separator): the wave or
/// shape selector where the type uses one, then the type's own numbers.
fn handles(
    ui: &Ui,
    open: &mut Open,
    actions: &mut Vec<Action>,
    start: u32,
    read_only: bool,
    f: &Function,
    set: usize,
) {
    let _id = ui.push_id(set);
    ui.text_disabled(if set == 0 { "Set 0" } else { "Set 1" });
    match f.kind {
        FunctionType::Transition => selector(ui, open, f, set, read_only, "Shape", shape_name),
        FunctionType::Periodic => selector(ui, open, f, set, read_only, "Wave", wave_name),
        _ => {}
    }
    if f.kind == FunctionType::Periodic
        && !Tables::procedural().has_wave(f.sub_types[set])
        && f.sub_types[set] != 0
    {
        ui.text_disabled(format!("{} {}", wave_name(f.sub_types[set]), ENGINE_WAVE));
    }
    let Some(values) = f.set_values(set) else {
        return;
    };
    let put = |ui: &Ui,
               open: &Open,
               actions: &mut Vec<Action>,
               id: &str,
               v: f32,
               set_word: &dyn Fn(&mut Set, f32)| {
        handle(ui, open, actions, start, read_only, f, set, id, v, set_word)
    };
    match values {
        Set::Identity => ui.text_disabled("The output is the input."),
        Set::Words(_) => {
            if f.kind == FunctionType::Constant {
                ui.text_disabled("The engine reads no word of a constant.");
            }
        }
        Set::Transition { from, to } => {
            put(ui, open, actions, "##from", from, &|s, v| {
                if let Set::Transition { from, .. } = s {
                    *from = v;
                }
            });
            put(ui, open, actions, "##to", to, &|s, v| {
                if let Set::Transition { to, .. } = s {
                    *to = v;
                }
            });
        }
        Set::Periodic {
            frequency,
            phase,
            minimum,
            maximum,
        } => {
            put(ui, open, actions, "##freq", frequency, &|s, v| {
                if let Set::Periodic { frequency, .. } = s {
                    *frequency = v;
                }
            });
            put(ui, open, actions, "##phase", phase, &|s, v| {
                if let Set::Periodic { phase, .. } = s {
                    *phase = v;
                }
            });
            put(ui, open, actions, "##min", minimum, &|s, v| {
                if let Set::Periodic { minimum, .. } = s {
                    *minimum = v;
                }
            });
            put(ui, open, actions, "##max", maximum, &|s, v| {
                if let Set::Periodic { maximum, .. } = s {
                    *maximum = v;
                }
            });
        }
        Set::Linear { slope, intercept } => {
            put(ui, open, actions, "##slope", slope, &|s, v| {
                if let Set::Linear { slope, .. } = s {
                    *slope = v;
                }
            });
            put(ui, open, actions, "##intercept", intercept, &|s, v| {
                if let Set::Linear { intercept, .. } = s {
                    *intercept = v;
                }
            });
        }
        Set::LinearKey { base, keys } => {
            put(ui, open, actions, "##base", base, &|s, v| {
                if let Set::LinearKey { base, .. } = s {
                    *base = v;
                }
            });
            for (k, key) in keys.iter().enumerate() {
                ui.text_disabled(format!("Key {}", k + 1));
                put(
                    ui,
                    open,
                    actions,
                    &format!("##kv{k}"),
                    key.value,
                    &|s, v| {
                        if let Set::LinearKey { keys, .. } = s {
                            keys[k].value = v;
                        }
                    },
                );
                put(
                    ui,
                    open,
                    actions,
                    &format!("##kp{k}"),
                    key.position,
                    &|s, v| {
                        if let Set::LinearKey { keys, .. } = s {
                            keys[k].position = v;
                        }
                    },
                );
                put(
                    ui,
                    open,
                    actions,
                    &format!("##ks{k}"),
                    key.inverse_span,
                    &|s, v| {
                        if let Set::LinearKey { keys, .. } = s {
                            keys[k].inverse_span = v;
                        }
                    },
                );
            }
        }
        Set::Spline {
            points,
            coefficients,
        } => {
            let names = ["t³", "t²", "t", "1"];
            for (k, c) in coefficients.iter().enumerate() {
                put(ui, open, actions, &format!("##c{k}"), *c, &|s, v| {
                    if let Set::Spline { coefficients, .. } = s {
                        coefficients[k] = v;
                    }
                });
                ui.same_line();
                ui.text_disabled(names[k]);
            }
            if f.kind == FunctionType::Spline2 {
                for (k, p) in points.iter().enumerate() {
                    ui.text_disabled(format!("Point {}", k + 1));
                    put(ui, open, actions, &format!("##px{k}"), p[0], &|s, v| {
                        if let Set::Spline { points, .. } = s {
                            points[k][0] = v;
                        }
                    });
                    put(ui, open, actions, &format!("##py{k}"), p[1], &|s, v| {
                        if let Set::Spline { points, .. } = s {
                            points[k][1] = v;
                        }
                    });
                }
            }
        }
        Set::Exponent {
            minimum,
            maximum,
            power,
        } => {
            put(ui, open, actions, "##min", minimum, &|s, v| {
                if let Set::Exponent { minimum, .. } = s {
                    *minimum = v;
                }
            });
            put(ui, open, actions, "##max", maximum, &|s, v| {
                if let Set::Exponent { maximum, .. } = s {
                    *maximum = v;
                }
            });
            put(ui, open, actions, "##power", power, &|s, v| {
                if let Set::Exponent { power, .. } = s {
                    *power = v;
                }
            });
        }
    }
}

/// The wave or shape selector row for `set`: the current name as a
/// button that opens the choice list.
fn selector(
    ui: &Ui,
    open: &mut Open,
    f: &Function,
    set: usize,
    read_only: bool,
    label: &str,
    name: fn(u8) -> String,
) {
    ui.text_disabled(label);
    ui.same_line();
    let off = ui.begin_disabled_with_cond(read_only);
    if ui.button_with_size(
        format!("{}##{label}-{set}", name(f.sub_types[set])),
        [150.0, 0.0],
    ) {
        open.pick = Some(Pick::Selector { set });
        open.problem = None;
    }
    drop(off);
}

/// A choice list the dialog opened: the type (a pick rebuilds the blob
/// at the new type's length), or set `set`'s wave or shape (a header byte, written in place). Returns
/// true when the dialog closes.
fn choices(
    ui: &Ui,
    open: &mut Open,
    actions: &mut Vec<Action>,
    start: u32,
    f: &Function,
    pick: Pick,
) -> bool {
    let done = ui.is_key_pressed(Key::Escape);
    match pick {
        Pick::Type => {
            let mut list: Vec<FunctionType> = Vec::new();
            if !TYPES.contains(&f.kind) {
                list.push(f.kind);
            }
            list.extend(TYPES);
            for t in list {
                if ui
                    .selectable_config(format!("{}##type", type_name(t)))
                    .selected(t == f.kind)
                    .build()
                {
                    open.pick = None;
                    open.problem = None;
                    if t != f.kind {
                        match f.with_kind(t) {
                            Ok(next) => retype(actions, open, &next),
                            Err(_) => open.problem = Some(change_to_unevaluated(t)),
                        }
                    }
                }
            }
        }
        Pick::Selector { set } => {
            let (label, count, name, waves) = match f.kind {
                FunctionType::Transition => ("Shape", 6, shape_name as fn(u8) -> String, false),
                _ => ("Wave", 12, wave_name as fn(u8) -> String, true),
            };
            ui.text_disabled(label);
            for sub in 0..count {
                if ui
                    .selectable_config(format!("{}##{label}-{sub}", name(sub)))
                    .selected(f.sub_types[set] == sub)
                    .build()
                {
                    open.pick = None;
                    open.problem = None;
                    let mut next = f.clone();
                    next.sub_types[set] = sub;
                    write(actions, open, start, &next);
                    actions.push(Action::Tag(TagAction::Seal));
                    return done;
                }
                if waves && !Tables::procedural().has_wave(sub) {
                    ui.same_line();
                    ui.text_disabled("(engine tables)");
                }
            }
        }
    }
    ui.separator();
    if ui.button("Cancel##pick-cancel") {
        open.pick = None;
        open.problem = None;
    }
    done
}
