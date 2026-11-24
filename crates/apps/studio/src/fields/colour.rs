//! The colour picker of a colour field (docs/editor/rich-editors.md,
//! "Colours"): the row's swatch and numbers, and the dialog with the
//! saturation and value square, the hue bar, the alpha bar for a form
//! with alpha, RGB, HSV and hex entry, the value in the file beside the
//! new one, the recent colours and the saved palette.
//!
//! A colour field is one with a colour `view=` word, or a three-float
//! field whose name says colour (`registry::Hint::Colour`, shown as
//! `rgb`). The words name the stored form ([`form`]); [`decode`] and
//! [`encode`] go between the stored bytes and the picker's [`Colour`].
//! A byte form shows the value after quantisation, so the picker never
//! shows a colour the field cannot hold, and the packed forms without
//! alpha keep their top byte as it is. The picker shows stored values
//! and does no colour-space conversion.
//!
//! Every write is a same-length byte write through `TagDoc` with the
//! field panel's merge key, so a drag is one undo step ([`pick`], then
//! [`finish`]); `tag.set` takes `"#aarrggbb"` or `{rgba, intensity?}`
//! ([`from_json`]); `tag.fields` rows carry [`row_json`]; `tag.colours`
//! lists them.

use dear_imgui_rs::{ColorButtonFlags, ColorPickerFlags, Drag, Key, MouseButton, Ui, WindowFlags};
use map::TagId;
use serde_json::{Value, json};
use tags::registry::{Field, FieldView, Hint, Kind};

use super::{Cx, PanelState, Row, Settings, fmt_f, write_action};
use crate::app::{Action, TagAction, TagDoc};
use crate::theme;

/// Recent colours kept, newest first.
pub const RECENT: usize = 12;
/// Saved colours kept, in the order they were saved.
pub const SAVED: usize = 24;
/// The row's swatch, in points.
const SWATCH: [f32; 2] = [36.0, 0.0];
/// A recent or saved colour's swatch in the dialog, in points.
const CHIP: [f32; 2] = [20.0, 20.0];
/// The largest intensity the dialog's drag reaches.
const MAX_INTENSITY: f32 = 1000.0;

/// A colour as the picker holds it: red, green, blue and alpha in 0 to 1
/// (alpha 1 for a form without it), and for `hdr_rgb` the intensity the
/// colour is multiplied by when stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub rgba: [f32; 4],
    pub intensity: Option<f32>,
}

impl Colour {
    /// The colour at intensity `intensity` (`None` for every form but
    /// `hdr_rgb`).
    pub fn new(rgba: [f32; 4], intensity: Option<f32>) -> Colour {
        Colour { rgba, intensity }
    }
}

/// The form a field's colour is stored in: its colour `view=` word, else
/// `rgb` for a three-float field whose name says colour (the name match
/// kept from before the words). `None` for a field that is not a colour.
pub fn form(f: &Field) -> Option<FieldView> {
    match (f.view, &f.kind) {
        (Some(v), _) if v.is_colour() => Some(v),
        (
            None,
            Kind::Float {
                count: 3,
                hint: Hint::Colour,
            },
        ) => Some(FieldView::Rgb),
        _ => None,
    }
}

/// How the field got its colour form: `view` (a definition word) or
/// `name` (the name match).
pub fn source(f: &Field) -> &'static str {
    if f.view.is_some() { "view" } else { "name" }
}

/// Whether the form stores alpha.
pub fn has_alpha(form: FieldView) -> bool {
    matches!(form, FieldView::Argb | FieldView::Argb8)
}

/// The bytes a form takes.
pub fn width(form: FieldView) -> usize {
    match form {
        FieldView::Rgb | FieldView::HdrRgb => 12,
        FieldView::Argb => 16,
        _ => 4,
    }
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

fn u32_at(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// A channel as a byte: rounded, clamped to 0 to 1.
fn byte(x: f32) -> u32 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    (x * 255.0).round() as u32
}

/// The colour `bytes` hold in `form`. An `hdr_rgb` value is split into a
/// colour at intensity 1 and the intensity: its largest channel when that
/// is above 1, else 1.
pub fn decode(form: FieldView, bytes: &[u8]) -> Colour {
    match form {
        FieldView::Rgb => Colour::new(
            [f32_at(bytes, 0), f32_at(bytes, 1), f32_at(bytes, 2), 1.0],
            None,
        ),
        FieldView::HdrRgb => {
            let [r, g, b] = [f32_at(bytes, 0), f32_at(bytes, 1), f32_at(bytes, 2)];
            let top = r.max(g).max(b);
            let i = if top > 1.0 { top } else { 1.0 };
            Colour::new([r / i, g / i, b / i, 1.0], Some(i))
        }
        FieldView::Argb => Colour::new(
            [
                f32_at(bytes, 1),
                f32_at(bytes, 2),
                f32_at(bytes, 3),
                f32_at(bytes, 0),
            ],
            None,
        ),
        _ => {
            let v = u32_at(bytes);
            let ch = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
            let a = if form == FieldView::Argb8 {
                ch(24)
            } else {
                1.0
            };
            Colour::new([ch(16), ch(8), ch(0), a], None)
        }
    }
}

/// The bytes `c` is stored as in `form`. `old` is the field's bytes now:
/// a packed form without alpha keeps its top byte. Float forms store the
/// channels as given (`hdr_rgb` times the intensity); byte forms round.
pub fn encode(form: FieldView, c: Colour, old: &[u8]) -> Vec<u8> {
    let [r, g, b, a] = c.rgba;
    let floats = |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    match form {
        FieldView::Rgb => floats(&[r, g, b]),
        FieldView::HdrRgb => {
            let i = c.intensity.unwrap_or(1.0);
            floats(&[r * i, g * i, b * i])
        }
        FieldView::Argb => floats(&[a, r, g, b]),
        _ => {
            let top = match form {
                FieldView::Argb8 => byte(a),
                _ => old.get(3).copied().map_or(0, u32::from),
            };
            (top << 24 | byte(r) << 16 | byte(g) << 8 | byte(b))
                .to_le_bytes()
                .to_vec()
        }
    }
}

/// `c` as `form` holds it: after quantisation for a byte form, alpha 1
/// for a form without alpha, intensity only for `hdr_rgb`.
pub fn held(form: FieldView, c: Colour) -> Colour {
    let mut c = c;
    if !has_alpha(form) {
        c.rgba[3] = 1.0;
    }
    c.intensity = match form {
        FieldView::HdrRgb => Some(c.intensity.unwrap_or(1.0)),
        _ => None,
    };
    match form {
        FieldView::Rgb8 | FieldView::Argb8 | FieldView::Xrgb8 => {
            decode(form, &encode(form, c, &[0; 4]))
        }
        _ => c,
    }
}

/// `#aarrggbb` of a colour (alpha `ff` for a form without it; an
/// `hdr_rgb` colour at intensity 1).
pub fn hex(c: Colour) -> String {
    let [r, g, b, a] = c.rgba;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(a),
        byte(r),
        byte(g),
        byte(b)
    )
}

/// The red, green, blue and alpha of `#rrggbb` or `#aarrggbb` (the `#`
/// optional; alpha 1 when left out).
pub fn parse_hex(text: &str) -> Option<[f32; 4]> {
    let t = text.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    let a = match t.len() {
        6 => 0xff,
        8 => v >> 24,
        _ => return None,
    };
    let ch = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
    Some([ch(16), ch(8), ch(0), a as f32 / 255.0])
}

/// A pasted hex colour in `form`, over the field's colour now: alpha is
/// dropped for a form without it, and an `hdr_rgb` field keeps its
/// intensity.
pub fn paste(form: FieldView, text: &str, now: Colour) -> Option<Colour> {
    let rgba = parse_hex(text)?;
    Some(held(form, Colour::new(rgba, now.intensity)))
}

/// The `colour` key of a `tag.fields` row: `{form, by, hex, rgba,
/// intensity?}`.
pub fn colour_json(form: FieldView, by: &str, c: Colour) -> Value {
    let mut out = json!({
        "form": form.word(),
        "by": by,
        "hex": hex(c),
        "rgba": c.rgba,
    });
    if let Some(i) = c.intensity {
        out["intensity"] = json!(i);
    }
    out
}

/// The `colour` key of the row of `f` at `address`, or `None` when `f` is
/// not a colour field or its bytes do not read.
pub fn row_json(doc: &TagDoc, tag: TagId, address: u32, f: &Field) -> Option<Value> {
    let form = form(f)?;
    let bytes = doc.read(tag, address, width(form) as u32)?;
    Some(colour_json(form, source(f), decode(form, &bytes)))
}

/// The bytes a `tag.set` value writes to a colour field: `"#aarrggbb"`
/// (or `#rrggbb`) or `{rgba: [r, g, b, a], intensity?}`. `Ok(None)` for
/// any other value (numbers go the plain way). Alpha is ignored by a form
/// without it; channels outside 0 to 1 and an intensity on a form
/// without one are refused by name.
pub fn from_json(form: FieldView, v: &Value, old: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let now = decode(form, old);
    let c = match v {
        Value::String(s) => paste(form, s, now)
            .ok_or_else(|| format!("{s:?} is not a hex colour (#aarrggbb or #rrggbb)"))?,
        Value::Object(o) => {
            let list = o
                .get("rgba")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 4)
                .ok_or("rgba: four numbers, red, green, blue and alpha")?;
            let mut rgba = [0f32; 4];
            for (k, x) in list.iter().enumerate() {
                let x = x
                    .as_f64()
                    .filter(|x| x.is_finite())
                    .ok_or("rgba: four numbers, red, green, blue and alpha")?
                    as f32;
                if !(0.0..=1.0).contains(&x) {
                    return Err(format!("rgba: {x} is outside 0 to 1"));
                }
                rgba[k] = x;
            }
            let intensity = match o.get("intensity") {
                None => now.intensity,
                Some(_) if form != FieldView::HdrRgb => {
                    return Err(format!(
                        "intensity: only an hdr_rgb colour has one, this is {}",
                        form.word()
                    ));
                }
                Some(x) => Some(
                    x.as_f64()
                        .filter(|x| x.is_finite() && *x >= 0.0)
                        .ok_or("intensity: a number, 0 or more")? as f32,
                ),
            };
            held(form, Colour::new(rgba, intensity))
        }
        _ => return Ok(None),
    };
    Ok(Some(encode(form, c, old)))
}

/// Remember `hex` as the newest recent colour (once, at most [`RECENT`]).
pub fn remember(recent: &mut Vec<String>, hex: &str) {
    recent.retain(|h| h != hex);
    recent.insert(0, hex.to_owned());
    recent.truncate(RECENT);
}

/// Save `hex` to the palette (once, at most [`SAVED`]; the oldest goes).
pub fn save(saved: &mut Vec<String>, hex: &str) {
    if saved.iter().any(|h| h == hex) {
        return;
    }
    saved.push(hex.to_owned());
    if saved.len() > SAVED {
        saved.remove(0);
    }
}

/// The open colour dialog.
#[derive(Clone, Debug)]
pub struct Open {
    pub tag: TagId,
    /// The field's address.
    pub address: u32,
    pub form: FieldView,
    /// The field, for the undo step's label.
    pub field: Field,
    /// The row's label, the dialog's title.
    pub title: String,
    /// What the picker shows: the colour as the field holds it.
    pub colour: Colour,
    /// The colour in the loaded file, beside the new one.
    pub file: Colour,
    /// A drag is under way: its writes merge into one undo step, which
    /// [`finish`] ends.
    pub dragging: bool,
    opening: bool,
}

impl Open {
    /// The dialog for the colour field `f` at `address`, or `None` when
    /// `f` is not a colour or its bytes do not read.
    pub fn new(doc: &TagDoc, tag: TagId, address: u32, f: &Field, title: &str) -> Option<Open> {
        let form = form(f)?;
        let n = width(form) as u32;
        let now = doc.read(tag, address, n)?;
        let file = doc
            .read_source(tag, address, n)
            .map_or(decode(form, &now), |b| decode(form, b));
        Some(Open {
            tag,
            address,
            form,
            field: f.clone(),
            title: title.to_owned(),
            colour: decode(form, &now),
            file,
            dragging: false,
            opening: true,
        })
    }
}

/// Pick `c` in the dialog: the picker shows it as the field holds it, and
/// the write (with the panel's merge key, so a drag's picks are one undo
/// step) when the bytes change. `None` when the bytes do not read or do
/// not change.
pub fn pick(doc: &TagDoc, open: &mut Open, c: Colour) -> Option<Action> {
    let c = held(open.form, c);
    open.colour = c;
    let old = doc.read(open.tag, open.address, width(open.form) as u32)?;
    let new = encode(open.form, c, &old);
    if new == old {
        return None;
    }
    open.dragging = true;
    Some(write_action(open.tag, open.address, new, &open.field.name))
}

/// End a drag: the seal that makes its writes one undo step, and the
/// colour joins the recent ones. `None` when no drag is under way.
pub fn finish(open: &mut Open, settings: &mut Settings) -> Option<Action> {
    if !std::mem::take(&mut open.dragging) {
        return None;
    }
    remember(&mut settings.recent_colours, &hex(open.colour));
    Some(Action::Tag(TagAction::Seal))
}

/// Follow a change made elsewhere (undo, the row's numbers, `tag.set`)
/// while no drag is under way: the picker shows the field's bytes.
fn follow(doc: &TagDoc, open: &mut Open) {
    if open.dragging {
        return;
    }
    if let Some(now) = doc.read(open.tag, open.address, width(open.form) as u32)
        && encode(open.form, open.colour, &now) != now
    {
        open.colour = decode(open.form, &now);
    }
}

/// The swatch colour of `c`: at intensity 1, opaque for a form without
/// alpha.
fn swatch(c: Colour) -> [f32; 4] {
    let [r, g, b, a] = c.rgba;
    [
        r.clamp(0.0, 1.0),
        g.clamp(0.0, 1.0),
        b.clamp(0.0, 1.0),
        a.clamp(0.0, 1.0),
    ]
}

fn button_flags(form: FieldView) -> ColorButtonFlags {
    if has_alpha(form) {
        ColorButtonFlags::ALPHA_PREVIEW_HALF
    } else {
        ColorButtonFlags::NO_ALPHA
    }
}

/// The row's own widget for a colour field: the swatch (a click opens the
/// dialog; its context menu copies and pastes a hex colour), the stored
/// numbers, and an `hdr_rgb` colour's intensity mark.
pub(super) fn row(cx: &mut Cx<'_, '_>, r: &Row, b: &[u8]) {
    let ui = cx.ui;
    let f = r.field;
    let Some(form) = form(f) else {
        return;
    };
    let c = decode(form, b);
    if ui
        .color_button_config("##swatch", swatch(c))
        .flags(button_flags(form) | ColorButtonFlags::NO_TOOLTIP)
        .size(SWATCH)
        .build()
    {
        cx.st.colour = Open::new(cx.doc, cx.tag, r.address, f, &r.label);
    }
    if ui.is_item_hovered() {
        ui.set_item_tooltip(format!(
            "{} ({}). Click to pick; right-click to copy or paste.",
            hex(c),
            super::rich::stored_form(form)
        ));
    }
    if let Some(_menu) = ui.begin_popup_context_item() {
        if ui.menu_item(format!("Copy {}", hex(c))) {
            let mut s = cx.st.settings.clone();
            s.colour_clip = hex(c);
            cx.st.set_settings(s);
        }
        let clip = cx.st.settings.colour_clip.clone();
        let pasted = paste(form, &clip, c);
        let _off = ui.begin_disabled_with_cond(pasted.is_none() || r.read_only.is_some());
        let label = match &pasted {
            Some(_) => format!("Paste {clip}"),
            None => "Paste (copy a colour first)".into(),
        };
        if ui.menu_item(label)
            && let Some(p) = pasted
        {
            cx.write(r.address, encode(form, p, b), &f.name);
            cx.actions.push(Action::Tag(TagAction::Seal));
            let mut s = cx.st.settings.clone();
            remember(&mut s.recent_colours, &hex(p));
            cx.st.set_settings(s);
        }
    }
    ui.same_line();
    let width = (ui.content_region_avail()[0] - cx.trailing).max(80.0);
    ui.set_next_item_width(width);
    match form {
        FieldView::Rgb | FieldView::HdrRgb | FieldView::Argb => {
            let mut v: Vec<f32> = (0..width_floats(form)).map(|i| f32_at(b, i)).collect();
            if Drag::<f32, _>::new("##v")
                .speed(0.005)
                .display_format(fmt_f("%.3f"))
                .build_array(ui, &mut v)
            {
                let out = v.iter().flat_map(|x| x.to_le_bytes()).collect();
                cx.write(r.address, out, &f.name);
            }
        }
        _ => {
            let mut v = u64::from(u32_at(b));
            if Drag::<u64, _>::new("##v")
                .range(0, 0xffff_ffff)
                .display_format(super::fmt_hex("0x%08llx"))
                .build(ui, &mut v)
            {
                cx.write(r.address, (v as u32).to_le_bytes().to_vec(), &f.name);
            }
        }
    }
    cx.after();
    if let Some(i) = c.intensity.filter(|i| *i != 1.0) {
        ui.same_line();
        ui.text_disabled(format!("\u{d7}{i:.2}"));
    }
}

fn width_floats(form: FieldView) -> usize {
    width(form) / 4
}

/// Draw the open dialog (call once per panel, after its rows, beside the
/// pick and curve dialogs).
pub(super) fn draw(ui: &Ui, doc: &TagDoc, st: &mut PanelState, actions: &mut Vec<Action>) {
    let Some(mut open) = st.colour.take() else {
        return;
    };
    let opening = std::mem::take(&mut open.opening);
    let title = format!("{}###colour", open.title);
    if opening {
        ui.open_popup(&title);
    }
    let Some(_modal) = ui
        .begin_modal_popup_config(&title)
        .flags(WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS)
        .begin()
    else {
        let mut s = st.settings.clone();
        if let Some(a) = finish(&mut open, &mut s) {
            actions.push(a);
            st.set_settings(s);
        }
        return;
    };
    let mut settings = st.settings.clone();
    let done = editor(ui, doc, &mut open, &mut settings, actions);
    st.set_settings(settings);
    if done {
        ui.close_current_popup();
    } else {
        st.colour = Some(open);
    }
}

/// The dialog's body. Returns true when the dialog closes.
fn editor(
    ui: &Ui,
    doc: &TagDoc,
    open: &mut Open,
    settings: &mut Settings,
    actions: &mut Vec<Action>,
) -> bool {
    follow(doc, open);
    let read_only = doc.refusal(open.tag);
    if let Some(why) = read_only {
        ui.text_colored(theme::WARN, why);
    }
    ui.text_disabled(format!(
        "{}: {}",
        open.form.word(),
        super::rich::stored_form(open.form)
    ));
    let off = ui.begin_disabled_with_cond(read_only.is_some());

    // The picker: square, hue bar, alpha bar for a form with alpha, the
    // RGB, HSV and hex entries, and the file's value beside the new one.
    let mut flags = ColorPickerFlags::PICKER_NO_ROTATE;
    flags |= if has_alpha(open.form) {
        ColorPickerFlags::ALPHA_BAR | ColorPickerFlags::ALPHA_PREVIEW_HALF
    } else {
        ColorPickerFlags::NO_ALPHA
    };
    let mut rgba = open.colour.rgba;
    if ui
        .color_picker4_config("##picker", &mut rgba)
        .flags(flags)
        .reference_color(swatch(open.file))
        .build()
    {
        let c = Colour::new(rgba, open.colour.intensity);
        actions.extend(pick(doc, open, c));
    }
    ui.text_disabled(format!(
        "In the file {}    now {}",
        hex(open.file),
        hex(open.colour)
    ));

    // An hdr colour's intensity.
    if let Some(mut i) = open.colour.intensity {
        ui.text_disabled("Intensity");
        ui.same_line();
        ui.set_next_item_width(120.0);
        if Drag::<f32, _>::new("##intensity")
            .speed(0.01)
            .range(0.0, MAX_INTENSITY)
            .display_format(fmt_f("%.3f"))
            .build(ui, &mut i)
        {
            let c = Colour::new(open.colour.rgba, Some(i));
            actions.extend(pick(doc, open, c));
        }
        if let Some(f) = open.file.intensity {
            ui.same_line();
            ui.text_disabled(format!("in the file {f:.3}"));
        }
    }

    // A drag ends when the mouse is let go.
    if open.dragging && !ui.is_mouse_down(MouseButton::Left) && !ui.is_any_item_active() {
        actions.extend(finish(open, settings));
    }

    // Recent and saved colours: a click applies one (one undo step).
    let mut chosen = None;
    chips(
        ui,
        "Recent",
        "recent",
        &settings.recent_colours,
        &mut chosen,
    );
    chips(ui, "Saved", "saved", &settings.saved_colours, &mut chosen);
    if let Some(text) = chosen
        && let Some(c) = paste(open.form, &text, open.colour)
    {
        actions.extend(pick(doc, open, c));
        actions.extend(finish(open, settings));
    }
    let now = hex(open.colour);
    if ui.small_button("Save colour") {
        save(&mut settings.saved_colours, &now);
    }
    if settings.saved_colours.contains(&now) {
        ui.same_line();
        if ui.small_button("Remove from saved") {
            settings.saved_colours.retain(|h| *h != now);
        }
    }
    drop(off);

    ui.separator();
    let close = ui.button("Done") || ui.is_key_pressed(Key::Escape);
    if close {
        actions.extend(finish(open, settings));
    }
    close
}

/// A row of colour chips with its words; a click sets `chosen`.
fn chips(ui: &Ui, words: &str, id: &str, list: &[String], chosen: &mut Option<String>) {
    ui.text_disabled(words);
    if list.is_empty() {
        ui.same_line();
        ui.text_disabled("none yet");
        return;
    }
    for (i, h) in list.iter().enumerate() {
        let Some(rgba) = parse_hex(h) else {
            continue;
        };
        ui.same_line();
        if ui
            .color_button_config(format!("{h}##{id}{i}"), rgba)
            .flags(ColorButtonFlags::ALPHA_PREVIEW_HALF)
            .size(CHIP)
            .build()
        {
            *chosen = Some(h.clone());
        }
    }
}

#[cfg(test)]
mod tests;
