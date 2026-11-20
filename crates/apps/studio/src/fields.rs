//! The field panel: every field of a tag element, labelled, with a widget
//! per type, reusable by any tool that edits tags (the Tags tool; a HUD
//! editor showing one widget's element).
//!
//! [`panel`] draws the element at a path from a tag's root and pushes
//! [`Action::Tag`] commands; it never writes itself. Numbers drag; enums
//! are combos; flags are checkboxes; colours pick; angles show degrees;
//! bounds drag as a pair; string IDs edit as text (any string the map
//! has); tag references pick from the map's tags of the allowed groups
//! and click through; blocks list their elements with add, remove and
//! duplicate; data shows (and edits) as hex. Counts, addresses and
//! resource pointers are shown, never edited. Fields nothing names are
//! labelled by offset and shown as their value suggests (a float, an
//! integer or hex), with a context menu to view them otherwise.
use std::collections::HashMap;
use std::sync::Arc;

use dear_imgui_rs::{Drag, DragRange, NumericFormat, Ui};
use map::{FourCC, TagId};
use tags::registry::{Field, Hint, Kind, Layout};

use crate::app::tag_doc::STRUCTURAL;
use crate::app::{Action, BlockOp, TagAction, TagDoc, TagWrite};
use crate::theme::icon;

/// How an unnamed span is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Float,
    Int,
    Hex,
}

/// The field under the mouse, for the hex pane and the status line.
#[derive(Clone, Debug, PartialEq)]
pub struct Hovered {
    pub address: u32,
    pub len: u32,
    /// Offset in its element.
    pub offset: u32,
    pub name: String,
    pub kind: String,
    pub named: bool,
    pub meaning: String,
}

/// The panel's UI state (one per place the panel is drawn).
#[derive(Default)]
pub struct PanelState {
    /// The element shown per block, by the address of its `(count,
    /// address)` pair.
    pub index: HashMap<u32, u32>,
    /// The field under the mouse this frame.
    pub hovered: Option<Hovered>,
    /// The element the mouse was last in: its address and layout (the hex
    /// pane shows it).
    pub focus: Option<(u32, Arc<Layout>)>,
    /// How unnamed spans are shown, by address (else guessed).
    pub views: HashMap<u32, View>,
    /// Text being typed into string ID and data fields, by address.
    text: HashMap<u32, String>,
    /// The tag picker's filter.
    picker: String,
    /// Show padding fields.
    pub show_pad: bool,
    /// Why the last typed value was not taken.
    pub problem: Option<String>,
}

/// Width of the label column, in points.
const LABEL: f32 = 210.0;
/// Bytes of a data reference shown and editable as hex.
const DATA_SHOWN: u32 = 4096;
/// Tags a picker lists at most.
const PICKS: usize = 400;

/// Draw the element at `path` (per step, a block field's offset and the
/// element index) of `tag`'s root: every field, editable where the tag
/// may be written. The empty path is the root.
pub fn panel(
    ui: &Ui,
    doc: &TagDoc,
    tag: TagId,
    path: &[(u32, u32)],
    st: &mut PanelState,
    actions: &mut Vec<Action>,
) {
    st.hovered = None;
    let Some((at, layout)) = doc.resolve(tag, path) else {
        ui.text_disabled("Nothing to show: the element is not in the map's metadata.");
        return;
    };
    let editable = doc.refusal(tag).is_none();
    let mut cx = Cx {
        ui,
        doc,
        tag,
        editable,
        st,
        actions,
    };
    cx.element(at, &layout);
}

struct Cx<'a> {
    ui: &'a Ui,
    doc: &'a TagDoc,
    tag: TagId,
    editable: bool,
    st: &'a mut PanelState,
    actions: &'a mut Vec<Action>,
}

fn key(address: u32) -> u64 {
    0x7461_6700_0000_0000 | u64::from(address)
}

/// The field panel's byte write, with the drag's merge key.
pub(crate) fn write_action(tag: TagId, address: u32, bytes: Vec<u8>, label: &str) -> Action {
    Action::Tag(TagAction::Write(TagWrite {
        tag,
        address,
        bytes,
        label: label.to_owned(),
        merge: Some(key(address)),
    }))
}

fn le(b: &[u8], at: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for k in (0..n).rev() {
        v = v << 8 | u64::from(b[at + k]);
    }
    v
}

fn sign(v: u64, bytes: usize) -> i64 {
    let shift = 64 - 8 * bytes as u32;
    ((v << shift) as i64) >> shift
}

fn group_text(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

/// The guessed view of an unnamed word: a float when its bits read as an
/// ordinary float (or zero), an integer when small, else hex.
pub fn guess(v: u32) -> View {
    let f = f32::from_bits(v);
    if v == 0 {
        View::Int
    } else if f.is_finite() && (1e-4..1e7).contains(&f.abs()) {
        View::Float
    } else if v < 0x10000 || (v as i32) > -0x10000 && (v as i32) < 0 {
        View::Int
    } else {
        View::Hex
    }
}

impl Cx<'_> {
    fn write(&mut self, address: u32, bytes: Vec<u8>, label: &str) {
        self.actions.push(write_action(self.tag, address, bytes, label));
    }

    /// After a widget: seal the drag when it ends.
    fn after(&mut self) {
        if self.ui.is_item_deactivated_after_edit() {
            self.actions.push(Action::Tag(TagAction::Seal));
        }
    }

    fn element(&mut self, at: u32, layout: &Arc<Layout>) {
        let ui = self.ui;
        let mut section = String::new();
        for f in &layout.fields {
            if matches!(f.kind, Kind::Pad { .. }) && !self.st.show_pad {
                continue;
            }
            if f.section != section {
                section = f.section.clone();
                if !section.is_empty() {
                    ui.separator_with_text(&section);
                }
            }
            let _id = ui.push_id(f.offset as usize);
            self.field(at, layout, f);
        }
    }

    fn label(&mut self, at: u32, layout: &Arc<Layout>, f: &Field) {
        let ui = self.ui;
        if f.named {
            ui.text(&f.name);
        } else {
            ui.text_disabled(&f.name);
        }
        self.hover(at, layout, f);
        ui.same_line_with_pos(LABEL);
        ui.set_next_item_width(-1.0);
    }

    /// Record the field as hovered when the last item is.
    fn hover(&mut self, at: u32, layout: &Arc<Layout>, f: &Field) {
        let ui = self.ui;
        if !ui.is_item_hovered() {
            return;
        }
        let tip = format!(
            "{} at +{:#x}{}{}",
            f.kind.label(),
            f.offset,
            if f.named {
                ""
            } else {
                "\nunnamed: labelled by its offset"
            },
            if f.meaning.is_empty() {
                String::new()
            } else {
                format!("\n{}", f.meaning)
            }
        );
        ui.set_item_tooltip(tip);
        self.st.hovered = Some(Hovered {
            address: at + f.offset,
            len: f.kind.width(),
            offset: f.offset,
            name: f.name.clone(),
            kind: f.kind.label(),
            named: f.named,
            meaning: f.meaning.clone(),
        });
        self.st.focus = Some((at, layout.clone()));
    }

    fn field(&mut self, at: u32, layout: &Arc<Layout>, f: &Field) {
        let ui = self.ui;
        let a = at + f.offset;
        let w = f.kind.width();
        let Some(b) = self.doc.read(a, w) else {
            ui.text_disabled(format!("{}: outside the metadata", f.name));
            return;
        };
        match &f.kind {
            Kind::Block(child) => return self.block(at, layout, f, child),
            Kind::Data => return self.data(at, layout, f, &b),
            _ => {}
        }
        self.label(at, layout, f);
        let structural = f.kind.is_structural();
        let _off = ui.begin_disabled_with_cond(!self.editable || structural);
        match &f.kind {
            Kind::Int {
                bytes,
                signed,
                count,
            } => {
                let n = *bytes as usize;
                let (lo, hi) = match (n, signed) {
                    (1, false) => (0i64, 0xff),
                    (1, true) => (-0x80, 0x7f),
                    (2, false) => (0, 0xffff),
                    (2, true) => (-0x8000, 0x7fff),
                    (_, false) => (0, 0xffff_ffff),
                    (_, true) => (i64::from(i32::MIN), i64::from(i32::MAX)),
                };
                let mut v: Vec<i64> = (0..*count as usize)
                    .map(|i| {
                        let u = le(&b, i * n, n);
                        if *signed { sign(u, n) } else { u as i64 }
                    })
                    .collect();
                if Drag::<i64, _>::new("##v")
                    .range(lo, hi)
                    .speed(0.2)
                    .build_array(ui, &mut v)
                {
                    let mut out = Vec::with_capacity(w as usize);
                    for x in v {
                        out.extend_from_slice(&x.to_le_bytes()[..n]);
                    }
                    self.write(a, out, &f.name);
                }
                self.after();
            }
            Kind::Float { count, hint } => self.float(a, f, &b, *count, *hint),
            Kind::Enum { bytes, values } => {
                let n = *bytes as usize;
                let v = le(&b, 0, n);
                let name = |i: u64| match values.get(i as usize) {
                    Some(s) => format!("{i}: {s}"),
                    None => format!("{i}"),
                };
                if let Some(_c) = ui.begin_combo("##v", name(v)) {
                    for i in 0..values.len().max(v as usize + 1) as u64 {
                        if ui.selectable_config(name(i)).selected(i == v).build() && i != v {
                            self.write(a, i.to_le_bytes()[..n].to_vec(), &f.name);
                        }
                    }
                }
            }
            Kind::Flags { bytes, bits } => {
                let n = *bytes as usize;
                let v = le(&b, 0, n);
                ui.text_disabled(format!("{v:#0w$x}", w = 2 + 2 * n));
                for bit in 0..(8 * n) {
                    let named = bits.get(bit).filter(|s| s.as_str() != "_");
                    let on = v >> bit & 1 == 1;
                    if named.is_none() && !on {
                        continue;
                    }
                    let text = match named {
                        Some(s) => format!("{s}##b{bit}"),
                        None => format!("bit {bit}##b{bit}"),
                    };
                    ui.dummy([LABEL - ui.cursor_pos()[0] + 8.0, 0.0]);
                    ui.same_line_with_pos(LABEL + 8.0);
                    let mut on2 = on;
                    if ui.checkbox(text, &mut on2) {
                        let nv = v ^ (1 << bit);
                        self.write(a, nv.to_le_bytes()[..n].to_vec(), &f.name);
                    }
                    self.hover(at, layout, f);
                }
            }
            Kind::StringId => self.string_id(a, f, &b),
            Kind::TagRef { groups } => {
                let group = FourCC::from_u32(le(&b, 0, 4) as u32);
                let id = le(&b, 4, 4) as u32;
                if let Some((g, chosen)) = self.picker(f, groups, id) {
                    let g = g.or((group.as_u32() != u32::MAX).then_some(group));
                    let mut out = g.map_or(u32::MAX, FourCC::as_u32).to_le_bytes().to_vec();
                    out.extend_from_slice(&chosen.to_le_bytes());
                    self.write(a, out, &f.name);
                }
            }
            Kind::Datum { groups } => {
                let id = le(&b, 0, 4) as u32;
                if let Some((_, chosen)) = self.picker(f, groups, id) {
                    self.write(a, chosen.to_le_bytes().to_vec(), &f.name);
                }
            }
            Kind::Word => {
                let mut v = le(&b, 0, 4) as i64;
                if Drag::<i64, _>::new("##v")
                    .range(0, 0xffff_ffff)
                    .display_format(fmt_i("0x%08llx"))
                    .build(ui, &mut v)
                {
                    self.write(a, (v as u32).to_le_bytes().to_vec(), &f.name);
                }
                self.after();
            }
            Kind::Raw | Kind::RawSize { .. } | Kind::Bsp => {
                let words: Vec<String> = (0..w as usize / 4)
                    .map(|i| format!("{:#010x}", le(&b, 4 * i, 4)))
                    .collect();
                ui.text(words.join(" "));
                if ui.is_item_hovered() {
                    ui.set_item_tooltip(STRUCTURAL);
                }
            }
            Kind::Pad { .. } => {
                ui.text_disabled(hex(&b));
            }
            Kind::Unknown { len } => self.unknown(a, f, &b, *len),
            Kind::Block(_) | Kind::Data => unreachable!(),
        }
        self.hover(at, layout, f);
    }

    fn float(&mut self, a: u32, f: &Field, b: &[u8], count: u32, hint: Hint) {
        let ui = self.ui;
        let mut v: Vec<f32> = (0..count as usize)
            .map(|i| f32::from_bits(le(b, 4 * i, 4) as u32))
            .collect();
        let deg = 180.0 / std::f32::consts::PI;
        let changed = match hint {
            Hint::Colour if count == 3 => {
                let mut c = [v[0], v[1], v[2]];
                let ch = ui.color_edit3("##v", &mut c);
                v.copy_from_slice(&c);
                ch
            }
            Hint::Bounds if count == 2 => {
                let (mut lo, mut hi) = (v[0], v[1]);
                let ch = DragRange::<f32, _>::new("##v")
                    .speed(0.01)
                    .build(ui, &mut lo, &mut hi);
                v = vec![lo, hi];
                ch
            }
            Hint::Angle => {
                let mut d: Vec<f32> = v.iter().map(|x| x * deg).collect();
                let ch = Drag::<f32, _>::new("##v")
                    .speed(0.5)
                    .display_format(fmt_f("%.2f deg"))
                    .build_array(ui, &mut d);
                v = d.iter().map(|x| x / deg).collect();
                ch
            }
            _ => Drag::<f32, _>::new("##v")
                .speed(0.01)
                .display_format(fmt_f("%.4f"))
                .build_array(ui, &mut v),
        };
        if changed {
            let out = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.write(a, out, &f.name);
        }
        self.after();
    }

    fn unknown(&mut self, a: u32, f: &Field, b: &[u8], len: u32) {
        let ui = self.ui;
        let v = le(b, 0, len as usize) as u32;
        let view =
            self.st
                .views
                .get(&a)
                .copied()
                .unwrap_or(if len == 4 { guess(v) } else { View::Int });
        let n = len as usize;
        match view {
            View::Float if len == 4 => {
                let mut x = f32::from_bits(v);
                if Drag::<f32, _>::new("##v")
                    .speed(0.01)
                    .display_format(fmt_f("%.4f"))
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes().to_vec(), &f.name);
                }
            }
            View::Hex => {
                let mut x = i64::from(v);
                let hexfmt = format!("0x%0{}llx", 2 * n);
                if Drag::<i64, _>::new("##v")
                    .range(0, (1i64 << (8 * n)) - 1)
                    .display_format(fmt_i(hexfmt.as_str()))
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes()[..n].to_vec(), &f.name);
                }
            }
            _ => {
                let mut x = sign(u64::from(v), n);
                let lim = 1i64 << (8 * n - 1);
                if Drag::<i64, _>::new("##v")
                    .range(-lim, lim - 1)
                    .speed(0.2)
                    .build(ui, &mut x)
                {
                    self.write(a, x.to_le_bytes()[..n].to_vec(), &f.name);
                }
            }
        }
        self.after();
        if let Some(_p) = ui.begin_popup_context_item() {
            ui.text_disabled("Show as");
            for (label, to) in [
                ("float", View::Float),
                ("integer", View::Int),
                ("hex", View::Hex),
            ] {
                if (to != View::Float || len == 4)
                    && ui.selectable_config(label).selected(view == to).build()
                {
                    self.st.views.insert(a, to);
                }
            }
        }
    }

    fn string_id(&mut self, a: u32, f: &Field, b: &[u8]) {
        let ui = self.ui;
        let v = le(b, 0, 4) as u32;
        let current = match self.doc.string(v) {
            Some(s) => s.to_owned(),
            None if v == u32::MAX => String::new(),
            None => format!("#{v:#010x}"),
        };
        let text = self.st.text.entry(a).or_insert_with(|| current.clone());
        let entered = ui.input_text("##v", text).enter_returns_true(true).build();
        let active = ui.is_item_active();
        let typed = text.clone();
        if entered || ui.is_item_deactivated_after_edit() {
            if typed == current {
            } else if typed.is_empty() {
                self.write(a, u32::MAX.to_le_bytes().to_vec(), &f.name);
            } else if let Some(id) = self.doc.string_id(&typed) {
                self.write(a, id.to_le_bytes().to_vec(), &f.name);
            } else {
                // A string the map lacks: added to the map with the write.
                self.actions.push(Action::Tag(TagAction::SetString {
                    tag: self.tag,
                    address: a,
                    text: typed,
                    label: f.name.clone(),
                }));
            }
            self.st.text.remove(&a);
        } else if !active {
            // Follow the field (undo, another element) while not typing.
            self.st.text.insert(a, current);
        }
    }

    /// A tag picker: the current tag, a list of the map's tags of the
    /// allowed groups, and a button to show the tag. Returns the picked
    /// tag's group (none for "none") and datum.
    fn picker(&mut self, f: &Field, groups: &[FourCC], id: u32) -> Option<(Option<FourCC>, u32)> {
        let ui = self.ui;
        let doc = self.doc;
        let mut out = None;
        let width = ui.content_region_avail()[0] - 32.0;
        ui.set_next_item_width(width.max(60.0));
        if let Some(_c) = ui.begin_combo("##v", doc.tag_label(id)) {
            ui.set_next_item_width(-1.0);
            ui.input_text("##filter", &mut self.st.picker).build();
            let filter = self.st.picker.to_ascii_lowercase();
            if ui
                .selectable_config("none")
                .selected(id == u32::MAX)
                .build()
            {
                out = Some((None, u32::MAX));
            }
            let mut shown = 0;
            for t in doc.tags() {
                if !doc.fits(t.group, groups) {
                    continue;
                }
                let label = format!("{}: {}", group_text(t.group), t.name);
                if !filter.is_empty() && !label.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                shown += 1;
                if shown > PICKS {
                    ui.text_disabled("... type to narrow the list");
                    break;
                }
                if ui.selectable_config(&label).selected(t.id.0 == id).build() {
                    out = Some((Some(t.group), t.id.0));
                }
            }
        }
        if !groups.is_empty() && ui.is_item_hovered() {
            let g: Vec<String> = groups.iter().map(|g| group_text(*g)).collect();
            ui.set_item_tooltip(format!("{}: {}", f.name, g.join(", ")));
        }
        ui.same_line();
        let target = doc.tag(TagId(id)).map(|t| t.id);
        let _go = ui.begin_disabled_with_cond(target.is_none());
        if ui.small_button(format!("{}##go", icon::FOCUS)) {
            if let Some(t) = target {
                self.actions.push(Action::Tag(TagAction::Select(t)));
            }
        }
        if ui.is_item_hovered() {
            ui.set_item_tooltip("Show this tag");
        }
        out
    }

    fn block(&mut self, at: u32, layout: &Arc<Layout>, f: &Field, child: &Arc<Layout>) {
        let ui = self.ui;
        let pair = at + f.offset;
        let Some((n, a)) = self.doc.block(pair, child) else {
            ui.text_disabled(format!(
                "{}: its elements are outside the map's metadata",
                f.name
            ));
            return;
        };
        let cap = self.doc.capacity(pair);
        let header = format!(
            "{}  [{n}]{}",
            f.name,
            if cap > n {
                format!(" of {cap}")
            } else {
                String::new()
            }
        );
        let node = ui
            .tree_node_config(format!("blk{pair:x}"))
            .label(header)
            .span_avail_width(true)
            .push();
        self.hover(at, layout, f);
        let Some(_node) = node else { return };
        let i = self
            .st
            .index
            .get(&pair)
            .copied()
            .unwrap_or(0)
            .min(n.saturating_sub(1));
        // Element picker and the block's commands.
        let mut shown = i as i32;
        ui.set_next_item_width(110.0);
        if n > 0 && ui.input_int("element##idx", &mut shown) {
            let shown = shown.clamp(0, n as i32 - 1) as u32;
            self.st.index.insert(pair, shown);
        }
        let tag = self.tag;
        let op_button = |cx: &mut Cx, label: &str, op: BlockOp| {
            let why = cx.doc.block_refusal(tag, pair, child, &op);
            ui.same_line();
            {
                let _d = ui.begin_disabled_with_cond(why.is_some());
                if ui.small_button(label) {
                    cx.actions.push(Action::Tag(TagAction::Block {
                        tag,
                        pair,
                        layout: child.clone(),
                        op: op.clone(),
                    }));
                    if let BlockOp::Add | BlockOp::Duplicate(_) = op {
                        cx.st.index.insert(
                            pair,
                            match op {
                                BlockOp::Duplicate(i) => i + 1,
                                _ => n,
                            },
                        );
                    }
                }
            }
            if let Some(why) = why {
                if ui.is_item_hovered_with_flags(
                    dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED,
                ) {
                    ui.set_item_tooltip(why);
                }
            }
        };
        op_button(self, &format!("{} Add##add", icon::PLUS), BlockOp::Add);
        if n > 0 {
            op_button(
                self,
                &format!("{} Duplicate##dup", icon::COPY),
                BlockOp::Duplicate(i),
            );
            op_button(
                self,
                &format!("{} Remove##rm", icon::TRASH),
                BlockOp::Remove(i),
            );
        }
        if n == 0 {
            ui.text_disabled("empty");
            return;
        }
        let _id = ui.push_id(i as usize);
        self.element(a + i * child.size, child);
    }

    fn data(&mut self, at: u32, layout: &Arc<Layout>, f: &Field, b: &[u8]) {
        let ui = self.ui;
        let (n, a) = (le(b, 0, 4) as u32, le(b, 4, 4) as u32);
        let node = ui
            .tree_node_config(format!("dat{:x}", at + f.offset))
            .label(format!("{}  ({n} bytes)", f.name))
            .span_avail_width(true)
            .push();
        self.hover(at, layout, f);
        let Some(_node) = node else { return };
        if n == 0 {
            ui.text_disabled("empty");
            return;
        }
        let shown = n.min(DATA_SHOWN);
        let Some(bytes) = self.doc.read(a, shown) else {
            ui.text_disabled("its bytes are outside the map's metadata");
            return;
        };
        let editable = self.editable && n <= DATA_SHOWN;
        let current = hex_lines(&bytes);
        let text = self.st.text.entry(a).or_insert_with(|| current.clone());
        let lines = (shown as f32 / 16.0).ceil().clamp(2.0, 16.0);
        ui.input_text_multiline(
            "##hex",
            text,
            [-1.0, lines * ui.text_line_height_with_spacing()],
        )
        .read_only(!editable)
        .build();
        let done = ui.is_item_deactivated_after_edit();
        let active = ui.is_item_active();
        if n > DATA_SHOWN {
            ui.text_disabled(format!(
                "first {DATA_SHOWN} of {n} bytes shown; edit larger data elsewhere"
            ));
        }
        if done && editable {
            let typed = self.st.text.remove(&a).unwrap_or_default();
            match parse_hex(&typed) {
                Some(new) if new.len() == bytes.len() => {
                    if new != bytes {
                        self.write(a, new, &f.name);
                        self.actions.push(Action::Tag(TagAction::Seal));
                    }
                }
                _ => {
                    self.st.problem = Some(format!(
                        "{}: type exactly {} hex bytes (data keeps its length)",
                        f.name,
                        bytes.len()
                    ))
                }
            }
        } else if !active {
            self.st.text.insert(a, current);
        }
    }
}

/// Display formats (fixed here, so always valid).
fn fmt_f(s: &str) -> NumericFormat<'_, f32> {
    NumericFormat::new(s).expect("a valid float format")
}
fn fmt_i(s: &str) -> NumericFormat<'_, i64> {
    NumericFormat::new(s).expect("a valid integer format")
}

/// Bytes as space-separated hex.
pub fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bytes as hex, 16 to a line.
pub fn hex_lines(b: &[u8]) -> String {
    b.chunks(16).map(hex).collect::<Vec<_>>().join("\n")
}

/// Hex digits (any whitespace between bytes) as bytes.
pub fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let digits: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() % 2 != 0 {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unnamed_words_show_as_their_value_suggests() {
        assert_eq!(guess(0), View::Int);
        assert_eq!(guess(1.5f32.to_bits()), View::Float);
        assert_eq!(guess(7), View::Int);
        assert_eq!(guess((-3i32) as u32), View::Int);
        assert_eq!(guess(0x8123_4567), View::Hex);
    }

    #[test]
    fn hex_round_trips() {
        let b: Vec<u8> = (0..40).collect();
        assert_eq!(parse_hex(&hex_lines(&b)).unwrap(), b);
        assert_eq!(parse_hex("0a 0"), None);
        assert_eq!(parse_hex("zz"), None);
    }
}
