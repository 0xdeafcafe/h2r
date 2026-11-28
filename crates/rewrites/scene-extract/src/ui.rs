//! The menus, the loading screen, the fade and the bars as the overlay
//! stage draws them (docs/graphics/retire-d3d.md, slice R7;
//! `frame_scene::overlay`): what the guest's UI draws after a view's HUD,
//! as typed state ([`UiFrame`]), turned into ordered overlay items by
//! [`draw_list`]. The UI's logic, input and state stay the guest's; this
//! replaces only its drawing.
//!
//! A frame is the guest's draws in its call order: the fade `0x00040890`,
//! the solid rectangles of `0x00052ce7` (menu fade rectangles, cinematic
//! bars, split-screen dividers), the loading draw's bitmaps
//! (`0x000289e0`) and the screens `0x00147af1` draws. A screen's widgets
//! are drawn in the order the guest's list `0x002353a5` gives them: a
//! post-order walk of the shown widgets ([`collect`]), sorted by the C
//! runtime's `qsort` (`0x00321460`, [`crt_qsort`]) with the comparator
//! `0x002356d8` ([`compare`]), drawn when their depth is inside the
//! window ([`in_window`]). A dialog's screen first fills the target with
//! the backdrop colour and blurs it (`0x0023029a`, docs/formats/ui.md,
//! "What a screen, pane and widget are").
//!
//! The values a frame holds (colours, blends, the blur's steps and tap)
//! are read from the running image by whoever fills it, never committed.
//! [`read_widget`] reads one widget's header and, by the draw its vtable
//! names, what it draws, or why that is not read yet ([`Unread`]).

use crate::GuestRead;
use frame_scene::{
    ClipRect, Combiner, Overlay, OverlayBlend, OverlayBlur, OverlayError, OverlayGlyph,
    OverlayMaterial, OverlayModelView, OverlayTexture, OverlayVertex, Program, Sampler,
};

/// The UI's canvas, guest pixels: its rectangles are a 640 by 480 frame
/// about the centre (docs/formats/ui.md, "Coordinates").
pub const CANVAS: [u32; 2] = [640, 480];

/// How a text block sits in its rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Left,
    Right,
    Centre,
}

/// What the line renderer `0x0013f0e0` lays a string out by besides its
/// rectangle and justification: the advance scale (`[esp+94h]`), the tab
/// stops (`0x004e73de`, their count at `0x004e73dc`) and the two indents
/// (`0x004e73fe` for the first row group, `0x004e7400` for the rest).
///
/// Every metric is times the scale: advances, offsets, kerning, line
/// heights and each glyph's quad, and the stops and indents too. A stop
/// is in the canvas's pixels, not the rectangle's: column `n` of a row
/// runs from stop `n - 1` (the rectangle's left plus the indent for
/// column 0) to stop `n` (the rectangle's right past the last stop).
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    pub scale: f32,
    pub stops: Vec<i16>,
    pub indents: [i16; 2],
}

impl Default for TextLayout {
    fn default() -> Self {
        TextLayout { scale: 1.0, stops: Vec::new(), indents: [0; 2] }
    }
}

impl TextLayout {
    /// The layout a font that lays out only plain lines draws: scale 1,
    /// no stops and no indents.
    pub fn plain(&self) -> bool {
        self.scale.to_bits() == 1f32.to_bits() && self.stops.is_empty() && self.indents == [0; 2]
    }
}

/// A textured rectangle outside a screen (the loading draw's background,
/// sweep and spinner) or a bitmap widget's art.
#[derive(Clone, Debug, PartialEq)]
pub struct UiBitmap {
    pub texture: OverlayTexture,
    /// `[x0, y0, x1, y1]`, guest pixels (a widget's own is its rectangle).
    pub rect: [f32; 4],
    /// `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub combiner: Combiner,
    pub sampler: Sampler,
    pub blend: OverlayBlend,
}

/// What a widget draws.
#[derive(Clone, Debug, PartialEq)]
pub enum UiContent {
    /// A container (screen, list, list item, page selector, table): no
    /// pixels of its own.
    None,
    /// A bitmap widget (type 8): the quad its draw makes. Its `rect` is
    /// where the draw puts it (the header's rectangle projected by its
    /// depth and moved by the view's offset), and its `color` the draw's
    /// tint, the widget's alpha in it; neither is the header's again.
    Bitmap(UiBitmap),
    /// A shader bitmap (type 9): the shader's own program over `uv`.
    Shader {
        tag: u32,
        uv: [f32; 4],
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A text widget or a button's label, the string as the widget holds
    /// it after substitution (UTF-16).
    Text {
        font: u16,
        text: Vec<u16>,
        color: [f32; 4],
        justify: Justify,
        shadow: bool,
        layout: TextLayout,
    },
    /// A model scene (type 7): its view into the viewport its camera
    /// call names ([`UiModel`]).
    Model(UiModel),
}

/// What a model scene (type 7, `0x002b0baa`) draws: one view of the
/// scenario objects its names select. The draw builds the camera's axes
/// (forward +Y, up +Z, normalised by `0x00030bf0`) and calls the view
/// `0x0002c560` at `0x002b0e12` with the camera's centre, the viewport and
/// the field of view (`engine::ui_model::camera` makes those words in
/// Rust). The viewport is the camera's, about the parent view's centre,
/// not the widget's rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct UiModel {
    /// `[left, top, right, bottom]`, canvas pixels, as the camera call
    /// takes them.
    pub viewport: [u16; 4],
    pub view: OverlayModelView,
}

/// The camera axes the model draw builds before its view call.
pub const MODEL_FORWARD: [f32; 3] = [0.0, 1.0, 0.0];
pub const MODEL_UP: [f32; 3] = [0.0, 0.0, 1.0];

impl UiModel {
    /// The view of `objects` (live handles, in the widget's name order)
    /// from the camera words of `engine::ui_model::Camera`: the centre,
    /// field of view and planes as `f32` bits, the viewport.
    pub fn new(objects: Vec<u32>, center: [u32; 3], viewport: [u16; 4], fov: u32, planes: [u32; 2]) -> Self {
        UiModel {
            viewport,
            view: OverlayModelView {
                camera: frame_scene::OverlayCamera {
                    position: center.map(f32::from_bits),
                    forward: MODEL_FORWARD,
                    up: MODEL_UP,
                    fov_y: f32::from_bits(fov),
                    planes: planes.map(f32::from_bits),
                    projection: None,
                },
                objects,
            },
        }
    }

    /// The view with the projection its call `0x0002c560` builds at
    /// `0x004b9e14` through `0x0002fd90`: `0x0002fb70`'s scales and
    /// offsets for its FOV and its record's viewport (top, left, bottom,
    /// right) under the lens inputs `k`
    /// ([`crate::visibility::view::projection_terms`]).
    pub fn with_lens(mut self, k: &crate::visibility::view::LensInputs) -> Self {
        let [left, top, right, bottom] = self.viewport.map(|w| w as i16);
        let c = &mut self.view.camera;
        c.projection = Some(crate::visibility::view::projection_terms(c.fov_y, [top, left, bottom, right], k));
        self
    }

    /// The viewport as the overlay's clip: `None` unless it lies wholly in
    /// the canvas, since clamping it would change the projection.
    pub fn clip(&self) -> Option<ClipRect> {
        let [left, top, right, bottom] = self.viewport.map(|w| i32::from(w as i16));
        let [w, h] = CANVAS.map(|v| v as i32);
        if left < 0 || top < 0 || right <= left || bottom <= top || right > w || bottom > h {
            return None;
        }
        Some(ClipRect {
            x: left as u32,
            y: top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }
}

/// One widget as the guest holds it (the common header `0x0022e27b`).
#[derive(Clone, Debug, PartialEq)]
pub struct UiWidget {
    /// The widget type at `+4`.
    pub kind: u32,
    /// The sort depth at `+0x60` (the Z of the animated offset).
    pub depth: f32,
    /// The render depth bias at `+0x6a`.
    pub bias: i16,
    /// The rectangle at `+0x20`: top, left, bottom, right, about the
    /// centre with Y up.
    pub rect: [i16; 4],
    /// The animated offset's X and Y at `+0x58`, Y up.
    pub offset: [f32; 2],
    /// The alpha at `+0x64`: multiplies the content's.
    pub alpha: f32,
    pub content: UiContent,
}

/// The fill and blur a dialog's screen draws under its widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct UiBackdrop {
    /// The shared globals' colour at `+0x6c`, RGBA, 0 to 1.
    pub color: [f32; 4],
    /// The screen's transition fraction: the fill's alpha is the colour's
    /// times it.
    pub fraction: f32,
    /// The blur's steps at that fraction (`0x00235756`).
    pub blur: OverlayBlur,
}

/// One screen of a window stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiScreen {
    pub backdrop: Option<UiBackdrop>,
    /// In the order the guest collects them ([`collect`]): [`draw_list`]
    /// sorts them as the guest does, ties included.
    pub widgets: Vec<UiWidget>,
}

/// One of the guest's UI draws, in its call order.
#[derive(Clone, Debug, PartialEq)]
pub enum UiDraw {
    /// The full-screen fade `0x00040890`: its colour (`0x00485874`) and the
    /// blend its mode picks.
    Fade {
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A solid rectangle (`0x00052ce7`), guest pixels `[x0, y0, x1, y1]`.
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
        blend: OverlayBlend,
    },
    /// A loading screen's bitmap.
    Bitmap(UiBitmap),
    Screen(UiScreen),
}

/// What the UI draws over a frame's views and HUD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFrame {
    pub draws: Vec<UiDraw>,
}

/// Lays out a text block in the renderer's fonts.
pub trait UiText {
    /// `text` in engine font `font`, wrapped and cut to `rect` (`[x0, y0,
    /// x1, y1]`, guest pixels) as the game does and justified in it: each
    /// glyph's index and rectangle, guest pixels. `None` when the font
    /// does not lay it out.
    fn shape(
        &self,
        font: u16,
        text: &[u16],
        rect: [f32; 4],
        justify: Justify,
    ) -> Option<Vec<OverlayGlyph>>;

    /// As [`UiText::shape`], with the fonts' button glyphs
    /// ([`BUTTON_GLYPHS`]) laid out as characters. The caller checks the
    /// glyphs against the guest's before it draws them. Fonts that do not
    /// lay a button out give `None` for a text with one.
    fn shape_buttons(
        &self,
        font: u16,
        text: &[u16],
        rect: [f32; 4],
        justify: Justify,
    ) -> Option<Vec<OverlayGlyph>> {
        if text.iter().any(|unit| BUTTON_GLYPHS.contains(unit)) {
            return None;
        }
        self.shape(font, text, rect, justify)
    }

    /// As [`UiText::shape_buttons`], laid out by `layout`'s scale, tab
    /// stops, indents and the scanner's alignment units (U+E405 to
    /// U+E407). Fonts that lay out only plain lines give `None` for a
    /// layout that is not [`TextLayout::plain`] or a text with a tab or
    /// an alignment unit.
    fn shape_layout(
        &self,
        font: u16,
        text: &[u16],
        rect: [f32; 4],
        justify: Justify,
        layout: &TextLayout,
    ) -> Option<Vec<OverlayGlyph>> {
        let laid = |u: &u16| *u == 9 || (0xe405..=0xe407).contains(u);
        if !layout.plain() || text.iter().any(laid) {
            return None;
        }
        self.shape_buttons(font, text, rect, justify)
    }

    /// The glyph index engine font `font` gives the character `unit`.
    /// `None` when the fonts do not say.
    fn glyph(&self, font: u16, unit: u16) -> Option<u32> {
        let _ = (font, unit);
        None
    }
}

/// The button glyphs a string holds as characters: the entries of the
/// string draw's table (`0x00470204`) with no handler, to `0xe12b`, and
/// the two sticks' three frames a control's handler writes (`0x0022e1ed`).
pub const BUTTON_GLYPHS: std::ops::RangeInclusive<u16> = 0xe100..=0xe131;

/// No fonts: no text is listed.
pub struct NoText;

impl UiText for NoText {
    fn shape(&self, _: u16, _: &[u16], _: [f32; 4], _: Justify) -> Option<Vec<OverlayGlyph>> {
        None
    }
}

/// One draw or widget of the walk: the overlay item it added, or why none.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// Its index in [`UiFrame::draws`].
    pub draw: usize,
    /// A screen's widget: its index in [`UiScreen::widgets`].
    pub widget: Option<usize>,
    /// Its index in the overlay's items; a backdrop lists its fill.
    pub item: Option<u32>,
    pub note: Option<String>,
}

/// What the guest's comparator reads of a list entry: the depth and the
/// bias the entry holds (`0x0023566a` copies them from `+0x60` and
/// `+0x6a`) and the widget's type at `+4`, signed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SortKey {
    pub depth: f32,
    pub bias: i16,
    pub kind: i32,
}

impl SortKey {
    pub fn of(w: &UiWidget) -> SortKey {
        SortKey {
            depth: w.depth,
            bias: w.bias,
            kind: w.kind as i32,
        }
    }
}

/// Depths whose difference is at most this tie (`[0x0045dc70]`, 0.001).
pub const DEPTH_TIE: f32 = 0.001;

/// The guest's comparator `0x002356d8`, as `qsort` takes it (negative:
/// `a` first). Depths more than [`DEPTH_TIE`] apart sort the deeper
/// first; tied depths sort by bias, smaller first, then by type, larger
/// first. A NaN depth compares equal to everything.
pub fn compare(a: &SortKey, b: &SortKey) -> i32 {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let d = a.depth - b.depth;
    let apart = if d >= 0.0 { d } else { 0.0 - d };
    // `comiss eps, |d|; jb`: an unordered difference is not a tie.
    if apart <= DEPTH_TIE {
        return match (a.bias.cmp(&b.bias), a.kind.cmp(&b.kind)) {
            (Greater, _) => 1,
            (Less, _) => -1,
            (Equal, Less) => 1,
            (Equal, Equal) => 0,
            (Equal, Greater) => -1,
        };
    }
    if b.depth > a.depth {
        1
    } else if a.depth > b.depth {
        -1
    } else {
        0
    }
}

/// The C runtime's `qsort` (`0x00321460`: the median-of-three quicksort
/// with an explicit stack, partitions of at most 8 left to a selection
/// sort that moves the largest to the end): the same comparisons and
/// swaps, so the same order where `cmp` ties. Not stable.
pub fn crt_qsort<T>(v: &mut [T], cmp: &dyn Fn(&T, &T) -> i32) {
    const CUTOFF: usize = 8;
    if v.len() < 2 {
        return;
    }
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let (mut lo, mut hi) = (0usize, v.len() - 1);
    loop {
        let size = hi - lo + 1;
        if size <= CUTOFF {
            short_sort(v, lo, hi, cmp);
        } else {
            let mut mid = lo + size / 2;
            if cmp(&v[lo], &v[mid]) > 0 {
                v.swap(lo, mid);
            }
            if cmp(&v[lo], &v[hi]) > 0 {
                v.swap(lo, hi);
            }
            if cmp(&v[mid], &v[hi]) > 0 {
                v.swap(mid, hi);
            }
            let mut loguy = lo;
            // Signed: the guest's pointer can step one entry below `lo`.
            let mut higuy = hi as isize;
            loop {
                if mid > loguy {
                    loop {
                        loguy += 1;
                        if !(loguy < mid && cmp(&v[loguy], &v[mid]) <= 0) {
                            break;
                        }
                    }
                }
                if mid <= loguy {
                    loop {
                        loguy += 1;
                        if !(loguy <= hi && cmp(&v[loguy], &v[mid]) <= 0) {
                            break;
                        }
                    }
                }
                loop {
                    higuy -= 1;
                    if !(higuy > mid as isize && cmp(&v[higuy as usize], &v[mid]) > 0) {
                        break;
                    }
                }
                if higuy < loguy as isize {
                    break;
                }
                v.swap(loguy, higuy as usize);
                if mid as isize == higuy {
                    mid = loguy;
                }
            }
            higuy += 1;
            if (mid as isize) < higuy {
                loop {
                    higuy -= 1;
                    if !(higuy > mid as isize && cmp(&v[higuy as usize], &v[mid]) == 0) {
                        break;
                    }
                }
            }
            if mid as isize >= higuy {
                loop {
                    higuy -= 1;
                    if !(higuy > lo as isize && cmp(&v[higuy as usize], &v[mid]) == 0) {
                        break;
                    }
                }
            }
            if higuy - lo as isize >= hi as isize - loguy as isize {
                if (lo as isize) < higuy {
                    stack.push((lo, higuy as usize));
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            } else {
                if loguy < hi {
                    stack.push((loguy, hi));
                }
                if (lo as isize) < higuy {
                    hi = higuy as usize;
                    continue;
                }
            }
        }
        match stack.pop() {
            Some((l, h)) => (lo, hi) = (l, h),
            None => return,
        }
    }
}

/// `qsort`'s selection sort over `lo..=hi`: the last largest (greater by
/// `cmp` than the largest so far) swapped to the end, the end moved in.
fn short_sort<T>(v: &mut [T], lo: usize, mut hi: usize, cmp: &dyn Fn(&T, &T) -> i32) {
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if cmp(&v[p], &v[max]) > 0 {
                max = p;
            }
        }
        v.swap(max, hi);
        hi -= 1;
    }
}

/// The most widgets one list holds (`0x0023566a` stops adding at 256).
pub const MAX_LISTED: usize = 256;

/// The widgets' indices in draw order: the first [`MAX_LISTED`], given
/// in the order the guest collects them ([`collect`]), sorted as the
/// guest sorts its list.
pub fn sort_widgets(widgets: &[UiWidget]) -> Vec<usize> {
    let mut order: Vec<(usize, SortKey)> = widgets
        .iter()
        .take(MAX_LISTED)
        .map(SortKey::of)
        .enumerate()
        .collect();
    crt_qsort(&mut order, &|a: &(usize, SortKey), b: &(usize, SortKey)| {
        compare(&a.1, &b.1)
    });
    order.into_iter().map(|(i, _)| i).collect()
}

/// One widget of a screen's tree as the collector walks it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiNode {
    /// Its vtable's `+0x40` answer: a hidden widget is skipped with its
    /// children.
    pub shown: bool,
    /// `+0x14`.
    pub first_child: Option<usize>,
    /// `+0x18`.
    pub next_sibling: Option<usize>,
}

/// The guest's list before the sort (`0x0023566a` from the root): each
/// shown widget after its shown children, the children in sibling
/// order; at most [`MAX_LISTED`], the children walked even when it is
/// full. A sibling chain that comes back to a widget walked stops there.
pub fn collect(nodes: &[UiNode], root: usize) -> Vec<usize> {
    fn visit(nodes: &[UiNode], at: usize, seen: &mut [bool], out: &mut Vec<usize>) {
        let Some(node) = nodes.get(at) else { return };
        if std::mem::replace(&mut seen[at], true) || !node.shown {
            return;
        }
        let mut child = node.first_child;
        while let Some(c) = child.filter(|&c| !seen.get(c).copied().unwrap_or(true)) {
            visit(nodes, c, seen, out);
            child = nodes[c].next_sibling;
        }
        if out.len() < MAX_LISTED {
            out.push(at);
        }
    }
    let mut seen = vec![false; nodes.len()];
    let mut out = Vec::new();
    visit(nodes, root, &mut seen, &mut out);
    out
}

/// Whether a listed widget at `depth` is drawn: inside `[near, far]`, the
/// window `0x002353a5` takes from `0x0054d5ac` and `0x0054d5b4` less
/// `0x0054d5b0` (`comiss`: a NaN depth is not).
pub fn in_window(depth: f32, [near, far]: [f32; 2]) -> bool {
    depth >= near && far >= depth
}

/// The widget draws a vtable's `+0x10` slot names (Code, `h2 disasm`).
pub mod draws {
    /// `ret 4`: the base widget's, and the screen's, list item's, page
    /// selector's and table's (vtables `0x00458788`, `0x00458840`,
    /// `0x00459f10`, `0x0045b570`, `0x0045ad18`).
    pub const NONE: u32 = 0x0033_3105;
    /// The bitmap widget's (`0x0045ad60`): a meter (`0x002b07e2`) or its
    /// bitmap (`0x002b0565`) through the screen quads, tinted by `+0x28`
    /// to `+0x30` and the alpha, each times 255 and rounded by `fistp`.
    pub const BITMAP: u32 = 0x002b_0540;
    /// The shader bitmap's (`0x0045adf0`): the HUD shader quad.
    pub const SHADER_BITMAP: u32 = 0x002b_11c9;
    /// The text widget's (`0x0045a5e0`): its colour times the text
    /// interface's (the `+0x3c` slot), written into the interface for
    /// the interface's draw.
    pub const TEXT: u32 = 0x0025_38f5;
    /// The model scene's (`0x0045ada8`).
    pub const MODEL: u32 = 0x002b_0baa;
    /// The list's (`0x0045a030`) and the button's (`0x0045a628`).
    pub const LIST: u32 = 0x0024_bda2;
    pub const BUTTON: u32 = 0x0025_3ea8;
}

/// The widget header (`0x0022e27b`, docs/formats/ui.md).
mod header {
    pub const KIND: u32 = 0x04;
    pub const RECT: u32 = 0x20;
    pub const OFFSET: u32 = 0x58;
    pub const ALPHA: u32 = 0x64;
    /// The `i16` at `+0x6a`, the high half of the word at `+0x68`.
    pub const BIAS_WORD: u32 = 0x68;
    pub const DRAW_SLOT: u32 = 0x10;
}

/// Why [`read_widget`] does not list a widget: its draw is not read into
/// a [`UiContent`] yet, or its header did not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// A bitmap widget: its draw reads more than the guest's widget
    /// (the view's rectangle, the bitmap tag's images, the blend tables),
    /// so the caller reads it (`render_bridge::native_hud::ui_overlay::
    /// bitmaps`) over this header ([`read_header`]).
    Bitmap,
    /// A shader bitmap: its combiner is set at its quad, not held at the
    /// list, so its quad is taken there (`render_bridge::native_hud::
    /// hud_overlay::menu_shader_quad`).
    Shader,
    /// A text widget: the string, its font and the interface's colour.
    /// Its reader names the check that refused it
    /// (`render_bridge::native_hud::ui_overlay::Gap`).
    Text,
    /// A model scene whose view the caller does not read: lights, more
    /// than 32 names, or no stage to draw the view.
    Model,
    /// Another draw (a list's, a button's, a screen's own): not read.
    Draw,
    /// The widget, its vtable or its header did not read.
    Unreadable,
}

/// The widget at `widget` as a list entry holds it (`depth` is the
/// entry's): its header and what its draw makes. A widget whose draw is
/// the base's `ret 4` is a container with no pixels ([`UiContent::None`]).
pub fn read_widget(mem: &dyn GuestRead, widget: u32, depth: f32) -> Result<UiWidget, Unread> {
    let draw = widget_draw(mem, widget)?;
    let content = match draw {
        draws::NONE => UiContent::None,
        draws::BITMAP => return Err(Unread::Bitmap),
        draws::SHADER_BITMAP => return Err(Unread::Shader),
        draws::TEXT => return Err(Unread::Text),
        draws::MODEL => return Err(Unread::Model),
        _ => return Err(Unread::Draw),
    };
    read_widget_header(mem, widget, depth, content)
}

/// The virtual draw invoked by the sorted item call.
pub fn widget_draw(mem: &dyn GuestRead, widget: u32) -> Result<u32, Unread> {
    draw_of(mem, widget).ok_or(Unread::Unreadable)
}

/// Combines the common widget header with its class-specific draw content.
pub fn read_widget_header(mem: &dyn GuestRead, widget: u32, depth: f32, content: UiContent) -> Result<UiWidget, Unread> {
    Ok(UiWidget {
        content,
        ..read_header(mem, widget, depth)?
    })
}

/// The draw the vtable of the widget at `widget` names (its `+0x10`
/// slot, [`draws`]).
pub fn draw_of(mem: &dyn GuestRead, widget: u32) -> Option<u32> {
    mem.read_u32(mem.read_u32(widget)?.checked_add(header::DRAW_SLOT)?)
}

/// The header of the widget at `widget` (`depth` is the list entry's),
/// with no content.
pub fn read_header(mem: &dyn GuestRead, widget: u32, depth: f32) -> Result<UiWidget, Unread> {
    let word = |offset: u32| {
        widget
            .checked_add(offset)
            .and_then(|a| mem.read_u32(a))
            .ok_or(Unread::Unreadable)
    };
    let halves = |w: u32| [w as u16 as i16, (w >> 16) as u16 as i16];
    let [top, left] = halves(word(header::RECT)?);
    let [bottom, right] = halves(word(header::RECT + 4)?);
    Ok(UiWidget {
        kind: word(header::KIND)?,
        depth,
        bias: halves(word(header::BIAS_WORD)?)[1],
        rect: [top, left, bottom, right],
        offset: [
            f32::from_bits(word(header::OFFSET)?),
            f32::from_bits(word(header::OFFSET + 4)?),
        ],
        alpha: f32::from_bits(word(header::ALPHA)?),
        content: UiContent::None,
    })
}

/// A widget's rectangle in guest pixels, `[x0, y0, x1, y1]`, moved by its
/// animated offset.
pub fn widget_rect(w: &UiWidget) -> [f32; 4] {
    let [top, left, bottom, right] = w.rect.map(f32::from);
    let (cx, cy) = (CANVAS[0] as f32 / 2.0, CANVAS[1] as f32 / 2.0);
    let [dx, dy] = w.offset;
    [
        left + cx + dx,
        cy - top - dy,
        right + cx + dx,
        cy - bottom - dy,
    ]
}

/// The pixels a rectangle touches, as a clip.
fn clip_of([x0, y0, x1, y1]: [f32; 4]) -> ClipRect {
    let (x0, y0) = (x0.floor().max(0.0), y0.floor().max(0.0));
    let (x1, y1) = (x1.ceil().max(x0), y1.ceil().max(y0));
    ClipRect {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    }
}

fn corners(
    [x0, y0, x1, y1]: [f32; 4],
    [u0, v0, u1, v1]: [f32; 4],
    color: [f32; 4],
) -> [OverlayVertex; 4] {
    let c = |x, y, u, v| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color,
    };
    [
        c(x0, y0, u0, v0),
        c(x1, y0, u1, v0),
        c(x1, y1, u1, v1),
        c(x0, y1, u0, v1),
    ]
}

fn faded([r, g, b, a]: [f32; 4], alpha: f32) -> [f32; 4] {
    [r, g, b, a * alpha]
}

fn refused(e: OverlayError) -> String {
    format!("the overlay refused it: {e:?}")
}

/// `b` as one quad of `overlay`, at its own rectangle.
pub fn push_bitmap(overlay: &mut Overlay, b: &UiBitmap) -> Result<u32, OverlayError> {
    bitmap(overlay, b, b.rect, 1.0)
}

fn bitmap(
    overlay: &mut Overlay,
    b: &UiBitmap,
    rect: [f32; 4],
    alpha: f32,
) -> Result<u32, OverlayError> {
    let material = OverlayMaterial {
        program: Program::Fixed(b.combiner),
        texture: b.texture,
        sampler: b.sampler,
        blend: b.blend,
    };
    overlay.push_quad(
        material,
        ClipRect::full(overlay.size),
        corners(rect, b.uv, faded(b.color, alpha)),
    )
}

fn widget(overlay: &mut Overlay, text: &dyn UiText, w: &UiWidget) -> Result<Option<u32>, String> {
    let rect = widget_rect(w);
    let all = ClipRect::full(overlay.size);
    match &w.content {
        UiContent::None => Ok(None),
        UiContent::Bitmap(b) => bitmap(overlay, b, b.rect, 1.0).map(Some).map_err(refused),
        UiContent::Shader {
            tag,
            uv,
            color,
            blend,
        } => {
            let material = OverlayMaterial {
                program: Program::Shader { tag: *tag },
                texture: OverlayTexture::None,
                sampler: Sampler::default(),
                blend: *blend,
            };
            overlay
                .push_quad(material, all, corners(rect, *uv, faded(*color, w.alpha)))
                .map(Some)
                .map_err(refused)
        }
        UiContent::Text {
            font,
            text: string,
            color,
            justify,
            shadow,
            layout,
        } => {
            if string.is_empty() {
                return Ok(None);
            }
            // A button glyph left by the substitution pass is a character
            // to the line scanner (`0x0013f5a0`, type 4).
            let glyphs = text
                .shape_layout(*font, string, rect, *justify, layout)
                .ok_or_else(|| format!("font {font} does not lay out the text"))?;
            let glyphs: Vec<_> = glyphs
                .into_iter()
                .filter(|g| g.rect[2] > g.rect[0] && g.rect[3] > g.rect[1])
                .collect();
            if glyphs.is_empty() {
                return Ok(None);
            }
            if *shadow && w.alpha * color[3] > 0.0 {
                // The glyph callback clips a glyph first, then draws its
                // shadow moved by the advance scale (`0x0001f530`)
                // followed immediately by its face.
                let lift = layout.scale;
                let [x0, y0, x1, y1] = rect;
                let shadow_clip = clip_of([x0.max(0.0) + lift, y0.max(0.0) + lift,
                    x1.min(overlay.size[0] as f32) + lift, y1.min(overlay.size[1] as f32) + lift]);
                let clip = clip_of(rect);
                let material = OverlayMaterial::text(*font);
                let face = faded(*color, w.alpha);
                let mut first = None;
                for glyph in glyphs {
                    let shifted = OverlayGlyph { rect: glyph.rect.map(|v| v + lift), ..glyph };
                    let item = overlay.push_text_run(material, shadow_clip,
                        [0.0, 0.0, 0.0, face[3]], &[shifted]).map_err(refused)?;
                    first.get_or_insert(item);
                    overlay.push_text_run(material, clip, face, &[glyph]).map_err(refused)?;
                }
                return Ok(first);
            }
            overlay
                .push_text_run(
                    OverlayMaterial::text(*font),
                    clip_of(rect),
                    faded(*color, w.alpha),
                    &glyphs,
                )
                .map(Some)
                .map_err(refused)
        }
        UiContent::Model(model) => {
            let clip = model.clip().ok_or_else(|| refused(OverlayError::Clip))?;
            overlay
                .push_model_view(clip, model.view.clone())
                .map(Some)
                .map_err(refused)
        }
    }
}

/// Adds `frame`'s draws to `overlay` in the guest's order, each screen's
/// widgets sorted. The overlay's target is the UI's canvas; one with no
/// size is given it. A draw or widget the overlay refuses is listed with
/// the reason and adds nothing.
pub fn draw_list(frame: &UiFrame, text: &dyn UiText, overlay: &mut Overlay) -> Vec<Listed> {
    if overlay.size == [0, 0] {
        overlay.set_size(CANVAS);
    }
    let all = ClipRect::full(overlay.size);
    let whole = [0.0, 0.0, overlay.size[0] as f32, overlay.size[1] as f32];
    let mut listed = Vec::new();
    let mut list = |draw, widget, r: Result<Option<u32>, String>| {
        let (item, note) = match r {
            Ok(item) => (item, None),
            Err(note) => (None, Some(note)),
        };
        listed.push(Listed {
            draw,
            widget,
            item,
            note,
        });
    };
    for (n, draw) in frame.draws.iter().enumerate() {
        match draw {
            UiDraw::Fade { color, blend } => {
                list(
                    n,
                    None,
                    overlay
                        .push_fill(all, whole, *color, *blend)
                        .map(Some)
                        .map_err(refused),
                );
            }
            UiDraw::Rect { rect, color, blend } => {
                list(
                    n,
                    None,
                    overlay
                        .push_fill(all, *rect, *color, *blend)
                        .map(Some)
                        .map_err(refused),
                );
            }
            UiDraw::Bitmap(b) => list(
                n,
                None,
                bitmap(overlay, b, b.rect, 1.0).map(Some).map_err(refused),
            ),
            UiDraw::Screen(screen) => {
                if let Some(b) = &screen.backdrop {
                    let fill = faded(b.color, b.fraction.clamp(0.0, 1.0));
                    let r = overlay
                        .push_fill(all, whole, fill, OverlayBlend::ALPHA)
                        .and_then(|item| overlay.push_blur(all, b.blur.clone()).map(|_| item));
                    list(n, None, r.map(Some).map_err(refused));
                }
                for i in sort_widgets(&screen.widgets) {
                    list(n, Some(i), widget(overlay, text, &screen.widgets[i]));
                }
                if screen.widgets.len() > 256 {
                    let r = Err(format!(
                        "{} widgets past the 256 the guest collects",
                        screen.widgets.len() - 256
                    ));
                    list(n, None, r);
                }
            }
        }
    }
    listed
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{OverlayCamera, OverlayKind};

    fn w(kind: u32, depth: f32, bias: i16) -> UiWidget {
        UiWidget {
            kind,
            depth,
            bias,
            rect: [10, -10, -10, 10],
            offset: [0.0; 2],
            alpha: 1.0,
            content: UiContent::None,
        }
    }

    fn art(tag: u32) -> UiBitmap {
        UiBitmap {
            texture: OverlayTexture::Bitmap { tag, image: 0 },
            rect: [0.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            combiner: Combiner::Texture,
            sampler: Sampler::default(),
            blend: OverlayBlend::ALPHA,
        }
    }

    /// Lays each unit out as an 8 by 10 cell from the rectangle's top left.
    struct Cells;

    impl UiText for Cells {
        fn shape(
            &self,
            font: u16,
            text: &[u16],
            rect: [f32; 4],
            _: Justify,
        ) -> Option<Vec<OverlayGlyph>> {
            (font == 1).then(|| {
                text.iter()
                    .enumerate()
                    .map(|(i, u)| {
                        let x = rect[0] + 8.0 * i as f32;
                        OverlayGlyph {
                            index: u32::from(*u),
                            rect: [x, rect[1], x + 8.0, rect[1] + 10.0],
                        }
                    })
                    .collect()
            })
        }
    }

    #[test]
    fn widgets_sort_deepest_first_then_by_bias_then_by_type_descending() {
        let widgets = [
            w(8, 0.5, 0),
            w(6, 0.1, 0),
            w(8, 0.1, 0),
            w(3, 0.1, 2),
            w(9, 0.1005, -1),
            w(6, -1.0, 9),
        ];
        // 0.5 first; depths 0.1 and 0.1005 tie: bias -1 first, then bias
        // 0 by type, 8 before 6, then bias 2; -1.0 last.
        assert_eq!(sort_widgets(&widgets), [0, 4, 2, 1, 3, 5]);
        let same = [w(6, 0.0, 0), w(6, 0.0, 0), w(6, 0.0, 0)];
        assert_eq!(
            sort_widgets(&same),
            [1, 2, 0],
            "qsort's selection sort moves equal widgets"
        );
        let many: Vec<_> = (0..300).map(|i| w(6, i as f32, 0)).collect();
        let order = sort_widgets(&many);
        assert_eq!(order.len(), 256);
        assert_eq!((order[0], order[255]), (255, 0));
    }

    #[test]
    fn the_comparator_is_the_guests() {
        let k = |depth, bias, kind| SortKey { depth, bias, kind };
        assert_eq!(compare(&k(1.0, 0, 6), &k(0.0, 0, 6)), -1, "deeper first");
        assert_eq!(compare(&k(0.0, 0, 6), &k(1.0, 0, 6)), 1);
        assert_eq!(
            compare(&k(0.0, 0, 6), &k(0.001, 5, 6)),
            -1,
            "0.001 apart ties"
        );
        assert_eq!(
            compare(&k(0.0, 1, 6), &k(0.0, 0, 6)),
            1,
            "larger bias after"
        );
        assert_eq!(
            compare(&k(0.0, 0, 9), &k(0.0, 0, 6)),
            -1,
            "larger type first"
        );
        assert_eq!(
            compare(&k(0.0, 0, -1), &k(0.0, 0, 6)),
            1,
            "the type is signed"
        );
        assert_eq!(compare(&k(0.0, 0, 6), &k(0.0, 0, 6)), 0);
        assert_eq!(
            compare(&k(f32::NAN, 0, 6), &k(0.0, 0, 9)),
            0,
            "NaN is equal"
        );
    }

    /// Past 8 entries the partitioning runs: equal entries land where the
    /// guest's `qsort` puts them, which a stable sort would not.
    #[test]
    fn a_long_list_is_partitioned_as_the_guest_does() {
        let mut keys = [(0.0, 0, 6); 12];
        keys[5] = (1.0, 0, 8);
        keys[7] = (2.0, 0, 6);
        keys[9] = (0.0, -1, 6);
        keys[11] = (1.0, 0, 8);
        let widgets: Vec<_> = keys.iter().map(|&(d, b, k)| w(k, d, b)).collect();
        assert_eq!(
            sort_widgets(&widgets),
            [7, 11, 5, 9, 2, 6, 1, 8, 4, 3, 10, 0]
        );
        let mut falling: Vec<u32> = (0..10).collect();
        crt_qsort(&mut falling, &|a: &u32, b: &u32| (*b as i32) - (*a as i32));
        assert_eq!(falling, [9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    }

    /// Children before their parent, siblings in order, a hidden widget
    /// with its children left out, a sibling chain that comes back walked
    /// once.
    #[test]
    fn the_list_is_collected_after_the_children() {
        let n = |shown, first_child, next_sibling| UiNode {
            shown,
            first_child,
            next_sibling,
        };
        let nodes = [
            n(true, Some(1), None),     // 0: the screen
            n(true, Some(3), Some(2)),  // 1: a list
            n(false, Some(4), Some(5)), // 2: hidden, with a child
            n(true, None, None),        // 3: an item of 1
            n(true, None, None),        // 4: the hidden one's
            n(true, None, Some(1)),     // 5: back to 1
        ];
        assert_eq!(collect(&nodes, 0), [3, 1, 5, 0]);
        let wide: Vec<_> = (0..300)
            .map(|i| {
                n(
                    true,
                    (i == 0).then_some(1),
                    (i > 0 && i < 299).then_some(i + 1),
                )
            })
            .collect();
        let listed = collect(&wide, 0);
        assert_eq!(listed.len(), 256);
        assert!(!listed.contains(&0), "the root, past 256, is not listed");
    }

    #[test]
    fn only_depths_inside_the_window_draw() {
        assert!(in_window(0.0, [-1.0, 1.0]));
        assert!(in_window(1.0, [-1.0, 1.0]) && in_window(-1.0, [-1.0, 1.0]));
        assert!(!in_window(1.5, [-1.0, 1.0]));
        assert!(!in_window(f32::NAN, [-1.0, 1.0]));
    }

    /// A container's header reads into a widget with no content; any
    /// other draw is named.
    #[test]
    fn a_widget_reads_its_header_and_its_draw() {
        use crate::testing::Mem;
        let (widget, vtable) = (0x1000, 0x2000);
        let mut mem = Mem::default();
        mem.w(widget, vtable);
        mem.w(widget + 4, 10);
        mem.w(widget + 0x20, ((-320i16 as u16 as u32) << 16) | 240);
        mem.w(widget + 0x24, (320 << 16) | (-240i16 as u16 as u32));
        mem.f(widget + 0x58, 2.0);
        mem.f(widget + 0x5c, -3.0);
        mem.f(widget + 0x64, 0.5);
        mem.w(widget + 0x68, (-2i16 as u16 as u32) << 16);
        mem.w(vtable + 0x10, draws::NONE);
        let read = read_widget(&mem, widget, 0.25).unwrap();
        assert_eq!(
            read,
            UiWidget {
                kind: 10,
                depth: 0.25,
                bias: -2,
                rect: [240, -320, -240, 320],
                offset: [2.0, -3.0],
                alpha: 0.5,
                content: UiContent::None,
            }
        );
        for (draw, why) in [
            (draws::BITMAP, Unread::Bitmap),
            (draws::SHADER_BITMAP, Unread::Shader),
            (draws::TEXT, Unread::Text),
            (draws::MODEL, Unread::Model),
            (draws::LIST, Unread::Draw),
            (0x0030_0000, Unread::Draw),
        ] {
            mem.w(vtable + 0x10, draw);
            assert_eq!(read_widget(&mem, widget, 0.0), Err(why));
        }
        assert_eq!(read_widget(&mem, 0x9000, 0.0), Err(Unread::Unreadable));
    }

    #[test]
    fn rectangles_are_about_the_centre_with_y_up_and_move_by_the_offset() {
        let mut x = w(8, 0.0, 0);
        x.rect = [240, -320, -240, 320];
        assert_eq!(widget_rect(&x), [0.0, 0.0, 640.0, 480.0]);
        x.rect = [100, -50, 60, 30];
        x.offset = [5.0, 10.0];
        assert_eq!(widget_rect(&x), [275.0, 130.0, 355.0, 170.0]);
    }

    #[test]
    fn each_glyph_draws_its_clipped_shadow_before_its_face() {
        let mut label = w(6, 0.0, 0);
        label.content = UiContent::Text {
            font: 1,
            text: vec![61, 62],
            justify: Justify::Left,
            color: [1.0; 4],
            shadow: true,
            layout: TextLayout::default(),
        };
        let frame = UiFrame { draws: vec![UiDraw::Screen(UiScreen {
            backdrop: None,
            widgets: vec![label],
        })] };
        let mut o = Overlay::default();
        assert!(draw_list(&frame, &Cells, &mut o).iter().all(|l| l.note.is_none()));
        assert_eq!(o.items.len(), 4);
        for (i, pair) in o.items.chunks_exact(2).enumerate() {
            let shadow = o.glyphs_of(&pair[0])[0];
            let face = o.glyphs_of(&pair[1])[0];
            assert_eq!(face.index, 61 + i as u32);
            assert_eq!(shadow.index, face.index);
            assert_eq!(shadow.rect, face.rect.map(|edge| edge + 1.0));
            assert_eq!(pair[0].clip.x, pair[1].clip.x + 1);
            assert_eq!(pair[0].color, [0.0, 0.0, 0.0, 1.0]);
            assert_eq!(pair[1].color, [1.0; 4]);
        }
    }

    /// Cells that lay a button glyph out as any other unit.
    struct Buttons;

    impl UiText for Buttons {
        fn shape(&self, font: u16, text: &[u16], rect: [f32; 4], j: Justify) -> Option<Vec<OverlayGlyph>> {
            Cells.shape(font, text, rect, j)
        }

        fn shape_buttons(&self, font: u16, text: &[u16], rect: [f32; 4], j: Justify) -> Option<Vec<OverlayGlyph>> {
            self.shape(font, text, rect, j)
        }
    }

    /// A label holding a button glyph lists when its fonts lay buttons
    /// out, and is refused, with a note, when they do not.
    #[test]
    fn a_labels_button_glyph_is_a_character_to_fonts_that_lay_buttons_out() {
        let mut label = w(6, 0.0, 0);
        label.content = UiContent::Text {
            font: 1,
            text: vec![0x41, *BUTTON_GLYPHS.start()],
            justify: Justify::Left,
            color: [1.0; 4],
            shadow: false,
            layout: TextLayout::default(),
        };
        let frame = UiFrame { draws: vec![UiDraw::Screen(UiScreen {
            backdrop: None,
            widgets: vec![label],
        })] };
        let mut o = Overlay::default();
        assert!(draw_list(&frame, &Buttons, &mut o).iter().all(|l| l.note.is_none()));
        let glyphs = o.glyphs_of(&o.items[0]);
        assert_eq!(glyphs[1].index, u32::from(*BUTTON_GLYPHS.start()));
        let mut o = Overlay::default();
        assert!(draw_list(&frame, &Cells, &mut o).iter().any(|l| l.note.is_some()));
    }

    /// The UI order test (retire-d3d.md, R7's proof) on a synthetic
    /// frame: the fade, a bar, a loading bitmap, then a dialog's screen
    /// (its backdrop's fill and blur, then its widgets sorted), each
    /// becoming its items in that order.
    #[test]
    fn the_frame_draws_in_the_guests_order() {
        let mut title = w(6, 0.2, 0);
        title.content = UiContent::Text {
            font: 1,
            text: "OK".encode_utf16().collect(),
            color: [1.0, 1.0, 0.5, 1.0],
            justify: Justify::Left,
            shadow: false,
            layout: TextLayout::default(),
        };
        title.alpha = 0.5;
        let mut panel = w(8, 0.0, 0);
        let mut panel_art = art(0xe001_0001);
        panel_art.rect = [300.0, 220.0, 340.0, 260.0];
        panel_art.color = [1.0, 1.0, 1.0, 0.5];
        panel.alpha = 0.25;
        panel.content = UiContent::Bitmap(panel_art);
        let mut picture = w(7, 0.1, 0);
        let camera = OverlayCamera {
            position: [0.0, -1.0, 0.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: 1.0,
            planes: [0.0625, 200.0],
            projection: None,
        };
        picture.content = UiContent::Model(UiModel {
            viewport: [310, 230, 330, 250],
            view: OverlayModelView {
                camera,
                objects: vec![0xe100_0004],
            },
        });
        let mut meter = w(9, 0.3, 0);
        meter.content = UiContent::Shader {
            tag: 0xe200_0005,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            blend: OverlayBlend::ALPHA,
        };
        let list = w(1, 0.05, 0);
        let screen = UiScreen {
            backdrop: Some(UiBackdrop {
                color: [0.0, 0.0, 0.0, 0.5],
                fraction: 0.5,
                blur: OverlayBlur {
                    tap: 0.5,
                    spreads: vec![1.0, 0.875],
                    pick: 0.25,
                },
            }),
            widgets: vec![title, meter, list, picture, panel],
        };
        let mut loading = art(0xe003_0003);
        loading.rect = [0.0, 0.0, 640.0, 480.0];
        let frame = UiFrame {
            draws: vec![
                UiDraw::Fade {
                    color: [0.0, 0.0, 0.0, 0.25],
                    blend: OverlayBlend::ALPHA,
                },
                UiDraw::Rect {
                    rect: [0.0, 0.0, 640.0, 60.0],
                    color: [0.0, 0.0, 0.0, 1.0],
                    blend: OverlayBlend::Off,
                },
                UiDraw::Bitmap(loading),
                UiDraw::Screen(screen),
            ],
        };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert_eq!(o.size, CANVAS);
        let kinds: Vec<_> = o.items.iter().map(|i| i.kind).collect();
        use OverlayKind::*;
        assert_eq!(
            kinds,
            [Fill, Fill, Quad, Fill, Blur, Quad, TextRun, ModelView, Quad]
        );
        let widgets: Vec<_> = listed.iter().filter_map(|l| l.widget).collect();
        assert_eq!(
            widgets,
            [1, 0, 3, 2, 4],
            "deepest first: meter, title, picture, list, panel"
        );
        let list_row = listed.iter().find(|l| l.widget == Some(2)).unwrap();
        assert_eq!(
            (list_row.item, &list_row.note),
            (None, &None),
            "a container draws nothing"
        );
        assert_eq!(
            o.vertices_of(&o.items[3])[0].color,
            [0.0, 0.0, 0.0, 0.25],
            "the backdrop's alpha times its fraction"
        );
        assert_eq!(o.blur_of(&o.items[4]).unwrap().spreads, [1.0, 0.875]);
        let text = &o.items[6];
        assert_eq!(text.color, [1.0, 1.0, 0.5, 0.5], "the widget's alpha");
        assert_eq!(
            text.clip,
            ClipRect {
                x: 310,
                y: 230,
                width: 20,
                height: 20
            }
        );
        assert_eq!(o.glyphs_of(text)[1].index, u32::from(b'K'));
        assert_eq!(
            o.items[5].material.program,
            Program::Shader { tag: 0xe200_0005 }
        );
        assert_eq!(
            o.items[7].clip,
            ClipRect {
                x: 310,
                y: 230,
                width: 20,
                height: 20
            }
        );
        let quad = o.vertices_of(&o.items[8]);
        assert_eq!(
            (quad[0].position, quad[2].position, quad[0].color[3]),
            ([300.0, 220.0], [340.0, 260.0], 0.5),
            "a bitmap widget's quad and tint are its draw's, not the header's"
        );
        assert_eq!(o.check(), Ok(()));
    }

    #[test]
    fn a_widget_the_overlay_or_the_fonts_refuse_is_listed_with_why() {
        let mut text = w(6, 0.0, 0);
        text.content = UiContent::Text {
            font: 2,
            text: vec![0x41],
            color: [1.0; 4],
            justify: Justify::Centre,
            shadow: false,
            layout: TextLayout::default(),
        };
        let mut off = w(8, 0.1, 0);
        off.rect = [10, 400, -10, 420];
        off.content = UiContent::Model(UiModel {
            viewport: [600, 400, 660, 420],
            view: OverlayModelView {
                camera: OverlayCamera::default(),
                objects: vec![],
            },
        });
        let frame = UiFrame {
            draws: vec![UiDraw::Screen(UiScreen {
                backdrop: None,
                widgets: vec![text, off],
            })],
        };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert!(o.is_empty());
        // Deeper first: the off-canvas widget at depth 0.1 lists before
        // the text at 0.0.
        assert!(
            listed[0].note.as_deref().unwrap().contains("Clip"),
            "a viewport off the canvas"
        );
        assert!(listed[1].note.as_deref().unwrap().contains("font 2"));
        let none = draw_list(&frame, &NoText, &mut o);
        assert!(
            none[1].item.is_none() && none[1].note.is_some(),
            "no fonts at all refuses the text too"
        );
    }

    /// A model scene draws one view, into its camera's viewport (not its
    /// widget's rectangle), of its objects in name order, with the draw's
    /// own axes; a viewport past the canvas lists nothing.
    #[test]
    fn a_model_widget_is_one_model_view_item() {
        let center = [0.5f32, -2.0, 1.25].map(f32::to_bits);
        let model = UiModel::new(vec![0xe100_0004, 0xe101_0009], center, [200, 120, 440, 360], 0.75f32.to_bits(), [0.0625f32, 200.0].map(f32::to_bits));
        let mut picture = w(7, 0.0, 0);
        picture.rect = [100, -300, -100, 300];
        picture.content = UiContent::Model(model.clone());
        let frame = UiFrame {
            draws: vec![UiDraw::Screen(UiScreen { backdrop: None, widgets: vec![picture.clone()] })],
        };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert_eq!(o.items.len(), 1);
        assert_eq!((o.items[0].kind, listed[0].item, &listed[0].note), (OverlayKind::ModelView, Some(0), &None));
        assert_eq!(o.items[0].clip, ClipRect { x: 200, y: 120, width: 240, height: 240 });
        let view = o.model_view_of(&o.items[0]).unwrap();
        assert_eq!(view.objects, [0xe100_0004, 0xe101_0009]);
        assert_eq!(view.camera.position, [0.5, -2.0, 1.25]);
        assert_eq!((view.camera.forward, view.camera.up, view.camera.fov_y), (MODEL_FORWARD, MODEL_UP, 0.75));
        assert!(o.vertices_of(&o.items[0]).is_empty() && o.glyphs_of(&o.items[0]).is_empty());
        assert_eq!(o.check(), Ok(()));

        let mut off = picture;
        off.content = UiContent::Model(UiModel { viewport: [600, 120, 700, 360], ..model });
        let frame = UiFrame { draws: vec![UiDraw::Screen(UiScreen { backdrop: None, widgets: vec![off] })] };
        let mut o = Overlay::default();
        let listed = draw_list(&frame, &Cells, &mut o);
        assert!(o.is_empty());
        assert!(listed[0].note.as_deref().unwrap().contains("Clip"));
    }

    /// A model view's lens is the one its call builds at `0x004b9e14`: the
    /// record's viewport (top, left, bottom, right) from the call's left,
    /// top, right, bottom, and the widescreen factor in the x scale.
    #[test]
    fn a_model_views_lens_is_the_projection_its_call_builds() {
        use crate::visibility::view::{LensInputs, projection_terms};
        let k = LensInputs {
            extent_min: -1.0,
            one: 1.0,
            half: 0.5,
            offset_scale: 1.0,
            anamorphic: 4.0 / 3.0,
            anamorphic_tan: 2.0,
            widescreen: true,
            display_flag: false,
            override_lens: false,
        };
        let fov = std::f32::consts::FRAC_PI_2;
        let model = UiModel::new(vec![1], [0; 3], [200, 120, 440, 300], fov.to_bits(), [0.0625f32, 200.0].map(f32::to_bits));
        assert_eq!(model.view.camera.projection, None);
        let lensed = model.with_lens(&k);
        let p = lensed.view.camera.projection.unwrap();
        assert_eq!(p, projection_terms(fov, [120, 200, 300, 440], &k));
        // 240 by 180: aspect 4/3, times the anamorphic factor.
        assert!((p[0] - 1.0 / (4.0 / 3.0 * 4.0 / 3.0)).abs() < 1e-5 && (p[1] - 1.0).abs() < 1e-5, "{p:?}");
        assert_eq!(lensed.viewport, [200, 120, 440, 300], "the clip stays the call's");
    }
}
