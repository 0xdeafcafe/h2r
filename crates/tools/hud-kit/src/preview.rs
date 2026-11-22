//! The preview (docs/game/hud-kit.md, "The preview and the checks"): on
//! `hud_edit::render`'s 640 by 480 canvas, the player's retail `nhdt` at a
//! `HudState`, then the compiled kit widgets through the same draw with
//! the values answered as the runtime's hooks answer them, then the
//! channels: the prompt line and a notification at their points, and the
//! training box around its text. Each picture comes with where every
//! element landed, for the JSON and the checks.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use hud_edit::Mode;
use hud_edit::model::Widget;
use hud_edit::place::{self, Frame, Rect};
use hud_edit::render::{self, Answers, Canvas, Cover, FALLBACK_CHAR, Fonts, Look, Placed, Sprite};
use hud_edit::state::{Grenade, HudState};
use serde_json::{Value as Json, json};

use crate::compile::{AsHudEdit, Compiled};
use crate::model::{Element, Hud, Kind, NAVPOINT_FONT, Style, Value, Values};
use crate::rects::{MESSAGE_LINE, PROMPT_DOWN, TRAINING_PAD};
use crate::source::{Colours, Source};

/// The metadata address the preview gives the kit's widget block (the
/// runtime's comes from `Builder::guest_alloc`).
pub const KIT_BASE: u32 = 0xf000_0000;
/// The kit's HUD's index in the list `hud_edit::render` draws (0 is the
/// retail player's).
pub const KIT: usize = 1;
/// A `Dim` text's alpha, as a fraction of the game's (Design; slice 4
/// sets it against the game's own alpha at `HUD_TEXT_SETUP`).
pub const DIM_ALPHA: f32 = 0.5;
/// An `Inactive` text's or sprite's alpha, as a fraction of the game's
/// (greyed as well).
pub const INACTIVE_ALPHA: f32 = 0.7;
/// The preview's navpoint: its point is a sample, this fraction of the
/// frame's width and height from the frame's centre (the runtime's is the
/// strip's stand point projected, slice 6).
pub const NAVPOINT_SAMPLE: [f32; 2] = [0.2, -0.25];
/// The samples of a HUD's navpoints, by their order among its elements:
/// the first is [`NAVPOINT_SAMPLE`], and the others are apart from it and
/// from each other, so two navpoints shown at once do not draw their
/// numbers over each other. A navpoint past the last takes the last.
pub const NAVPOINT_SAMPLES: [[f32; 2]; 4] = [
    NAVPOINT_SAMPLE,
    [-0.22, -0.25],
    [0.0, -0.3],
    [0.34, -0.12],
];

/// The sample of a HUD's `ordinal`th navpoint ([`NAVPOINT_SAMPLES`]).
pub fn navpoint_sample(ordinal: usize) -> [f32; 2] {
    NAVPOINT_SAMPLES[ordinal.min(NAVPOINT_SAMPLES.len() - 1)]
}
/// Pixels between a navpoint's arrow and its marker: the marker is put
/// out from the point by the arrow's length and one more (E `0x0024ecae`
/// with `0x0024e9c7`'s return; S: the rest of its sum).
pub const NAVPOINT_GAP: f32 = 1.0;
/// Pixels between a navpoint's marker and the kit's number above it
/// (Design: the game draws no number).
pub const NAVPOINT_NUMBER_GAP: f32 = 2.0;
/// The message and training font: font id 6 fullscreen, 5 in split
/// screen (E `0x0024cfaf`..`0x0024cfc9`, `0x001e71d7`).
pub const MESSAGE_FONT: [i16; 2] = [6, 5];
/// The retail HUD's state words the sheet starts from: word A `0x1000`,
/// the retail shield meter's show word. Each source adds the word the
/// widget on the motion tracker's rectangle shows on
/// (`source::retail_words`; S: that the game sets it whenever it draws the
/// tracker); the disc test checks the tracker and the shield meter draw
/// at the result.
pub const RETAIL_WORDS: [u16; 4] = [0x1000, 0, 0, 0];

/// A screen mode's name in file names, the JSON and the command line.
pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Full => "full",
        Mode::Half => "half",
        Mode::Quarter => "quarter",
    }
}

pub fn parse_mode(s: &str) -> Option<Mode> {
    Mode::ALL.into_iter().find(|m| mode_name(*m) == s)
}

/// The retail HUD states of the sheet at the state words `words`
/// (`Source::words`), the first the one pictures are drawn at: plain,
/// overshield, plasma grenades selected, zoomed.
pub fn retail_states(mode: Mode, words: [u16; 4]) -> Vec<HudState> {
    let base = HudState {
        mode,
        gate: true,
        words,
        ..HudState::default()
    };
    vec![
        base.clone(),
        HudState {
            shield: 2.0,
            ..base.clone()
        },
        HudState {
            grenade: Grenade::Plasma,
            ..base.clone()
        },
        HudState {
            zoomed: true,
            ..base
        },
    ]
}

/// One element as drawn.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub id: &'static str,
    pub kind: &'static str,
    pub channel: bool,
    /// Where it landed in this state (`None`: hidden in this mode, or
    /// nothing to place).
    pub rect: Option<Rect>,
    /// The area it may take in any state: a text's at its `longest`, else
    /// its rectangle.
    pub reserved: Option<Rect>,
    pub text: Option<String>,
    pub style: Style,
    pub fill: Option<f32>,
    pub angle: Option<f32>,
    /// Its value shows it.
    pub shown: bool,
    pub drawn: bool,
    /// Why it is not drawn.
    pub hidden: Option<String>,
    /// Its opaque pixels.
    pub cover: Option<Cover>,
    /// The engine font its text is in.
    pub font: Option<i16>,
    pub notes: Vec<String>,
}

/// One picture: a HUD at a state and a screen mode.
pub struct Shot {
    pub hud: &'static str,
    pub state: String,
    pub mode: Mode,
    pub canvas: Canvas,
    pub frame: Frame,
    /// The retail widgets.
    pub retail: Vec<Placed>,
    pub elements: Vec<Drawn>,
}

impl Shot {
    pub fn element(&self, id: &str) -> Option<&Drawn> {
        self.elements.iter().find(|d| d.id == id)
    }
    /// `<hud>-<state>-<mode>`.
    pub fn stem(&self) -> String {
        format!("{}-{}-{}", self.hud, self.state, mode_name(self.mode))
    }
}

/// A text's colour for its style on top of `base`.
pub fn style_colour(base: [u8; 4], style: Style, colours: &Colours) -> [u8; 4] {
    match style {
        Style::Normal => base,
        Style::Dim => [
            base[0],
            base[1],
            base[2],
            (f32::from(base[3]) * DIM_ALPHA).round() as u8,
        ],
        Style::Inactive => Look {
            alpha: INACTIVE_ALPHA,
            grey: true,
        }
        .apply(base),
        // The record's colour at its start (`hudg::Flash::start`); the
        // curve over `since_tick` is slice 4's.
        Style::Flash(p) => colours.flash(p),
    }
}

/// How a bitmap element's texels are drawn for its style: as they are
/// (`Normal`, `Flash`), faded (`Dim`), greyed (`Inactive`).
pub fn style_look(style: Style) -> Option<Look> {
    match style {
        Style::Normal | Style::Flash(_) => None,
        Style::Dim => Some(Look {
            alpha: DIM_ALPHA,
            grey: false,
        }),
        Style::Inactive => Some(Look {
            alpha: INACTIVE_ALPHA,
            grey: true,
        }),
    }
}

/// What the runtime's hooks answer for the kit's widgets.
struct KitAnswers<'a> {
    hud: &'a Hud,
    compiled: &'a Compiled,
    values: &'a Values,
    mode: Mode,
    colours: &'a Colours,
}

impl KitAnswers<'_> {
    fn of(&self, h: usize, w: &Widget) -> Option<(&Element, Value)> {
        if h != KIT {
            return None;
        }
        let e = self.hud.element(&w.name)?;
        Some((e, self.values.get(e.id).cloned().unwrap_or_default()))
    }
}

impl Answers for KitAnswers<'_> {
    fn shown(&self, h: usize, w: &Widget) -> Option<bool> {
        let (e, v) = self.of(h, w)?;
        Some(v.shown && e.at.offset(self.mode).is_some())
    }
    /// `hud_input1` as the runtime's input hook gives it: the fill, or a
    /// cropped meter's ramp at the fill (`hud_kit_core::records::Widget::input`).
    fn fill(&self, h: usize, w: &Widget) -> Option<f32> {
        let (e, v) = self.of(h, w)?;
        if !matches!(e.kind, Kind::Meter { .. }) {
            return None;
        }
        Some(match self.compiled.widget(e.id) {
            Some(c) => c.input(v.fill),
            None => v.fill.clamp(0.0, 1.0),
        })
    }
    fn text(&self, h: usize, w: &Widget) -> Option<String> {
        let (e, v) = self.of(h, w)?;
        matches!(e.kind, Kind::Text { .. }).then_some(v.text)
    }
    fn colour(&self, h: usize, w: &Widget) -> Option<[u8; 4]> {
        let (e, v) = self.of(h, w)?;
        matches!(e.kind, Kind::Text { .. })
            .then(|| style_colour(self.colours.widget, v.style, self.colours))
    }
    fn angle(&self, h: usize, w: &Widget) -> Option<f32> {
        let (e, v) = self.of(h, w)?;
        matches!(e.kind, Kind::Pointer { .. }).then_some(v.angle)
    }
    fn look(&self, h: usize, w: &Widget) -> Option<Look> {
        let (e, v) = self.of(h, w)?;
        e.kind.art().and_then(|_| style_look(v.style))
    }
    fn crop(&self, h: usize, w: &Widget) -> Option<[f32; 4]> {
        let (e, _) = self.of(h, w)?;
        self.compiled.widget(e.id)?.crop
    }
}

/// The size of each line of `text` in font `font` (none: one empty line).
pub fn line_sizes(fonts: Option<&Fonts>, font: i16, text: &str) -> Vec<[f32; 2]> {
    text.split('\n')
        .map(|l| render::text_size(fonts, font, l))
        .collect()
}

/// A block of lines: its width (the widest) and height (the sum).
fn block_size(lines: &[[f32; 2]]) -> [f32; 2] {
    [
        lines.iter().map(|s| s[0]).fold(0.0, f32::max),
        lines.iter().map(|s| s[1]).sum(),
    ]
}

/// Draws one line at `at` (its top left), into `cover`.
fn blit(
    canvas: &mut Canvas,
    cover: &mut Cover,
    fonts: Option<&Fonts>,
    font: i16,
    text: &str,
    colour: [u8; 4],
    at: [f32; 2],
) {
    let [x0, y0] = [at[0].round() as i32, at[1].round() as i32];
    match fonts.and_then(|f| f.render(font, text, colour)) {
        Some((b, _)) => {
            for y in 0..b.height {
                for x in 0..b.width {
                    let i = ((y * b.width + x) * 4) as usize;
                    let c = [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2], b.rgba[i + 3]];
                    let (cx, cy) = (x0 + x as i32, y0 + y as i32);
                    if c[3] > 64 {
                        cover.set(cx, cy);
                    }
                    canvas.blend(cx, cy, c);
                }
            }
        }
        // hud_edit's stand-in: a bar per character.
        None => {
            let [rw, rh] = render::text_size(None, font, text).map(|v| v as i32);
            for y in 2..rh - 2 {
                for x in 0..rw {
                    if x % FALLBACK_CHAR[0] as i32 != 6 {
                        if colour[3] > 64 {
                            cover.set(x0 + x, y0 + y);
                        }
                        canvas.blend(x0 + x, y0 + y, colour);
                    }
                }
            }
        }
    }
}

/// Where a channel's text block sits for a block of `size`: its
/// rectangle (a training box's includes its margin).
fn channel_rect(kind: &Kind, frame: &Frame, mode: Mode, size: [f32; 2]) -> Rect {
    let split = usize::from(mode != Mode::Full);
    let start = [
        f32::from(frame.left),
        f32::from(frame.top) + PROMPT_DOWN[split],
    ];
    match kind {
        Kind::Prompt { .. } => Rect {
            x0: start[0],
            y0: start[1],
            x1: start[0] + size[0],
            y1: start[1] + size[1],
        },
        // The notifications start below the first line's box (S: the
        // order of 0x0024d33a's list against the first line).
        Kind::Message { .. } => {
            let y = start[1] + 5.0 * MESSAGE_LINE;
            Rect {
                x0: start[0],
                y0: y,
                x1: start[0] + size[0],
                y1: y + size[1],
            }
        }
        // Centred on the frame's centre, half the text plus the margin
        // each way (E 0x001e72e9..0x001e7373).
        _ => {
            let c = frame.anchor_point(4).expect("the centre");
            let (hw, hh) = (
                (size[0] / 2.0).floor() + TRAINING_PAD[0],
                (size[1] / 2.0).floor() + TRAINING_PAD[1],
            );
            Rect {
                x0: c[0] - hw,
                y0: c[1] - hh,
                x1: c[0] + hw,
                y1: c[1] + hh,
            }
        }
    }
}

fn channel(
    canvas: &mut Canvas,
    src: &mut Source,
    frame: &Frame,
    mode: Mode,
    e: &Element,
    v: &Value,
    navpoint_ordinal: usize,
) -> Drawn {
    if let Kind::Navpoint { .. } = e.kind {
        return navpoint(canvas, src, frame, mode, e, v, navpoint_ordinal);
    }
    let fonts = src.fonts.as_ref();
    let font = MESSAGE_FONT[usize::from(mode != Mode::Full)];
    let longest = e.kind.longest().unwrap_or("");
    let reserved = channel_rect(
        &e.kind,
        frame,
        mode,
        block_size(&line_sizes(fonts, font, longest)),
    );
    // A channel, like a widget, shows only in the modes its place has
    // (fullscreen unless it says otherwise).
    let in_mode = e.at.offset(mode).is_some();
    let hidden = if !in_mode {
        Some(format!("no {} offset", mode.label()))
    } else if !v.shown {
        Some("not shown in this state".to_owned())
    } else {
        None
    };
    let mut d = Drawn {
        id: e.id,
        kind: e.kind.name(),
        channel: true,
        rect: None,
        reserved: in_mode.then_some(reserved),
        text: v.shown.then(|| v.text.clone()),
        style: v.style,
        fill: None,
        angle: None,
        shown: v.shown,
        drawn: false,
        hidden,
        cover: None,
        font: Some(font),
        notes: Vec::new(),
    };
    if fonts.is_none() {
        d.notes.push("no fonts: the fixed-advance stand-in".into());
    }
    if d.hidden.is_some() {
        return d;
    }
    let lines = line_sizes(fonts, font, &v.text);
    let rect = channel_rect(&e.kind, frame, mode, block_size(&lines));
    let base = match e.kind {
        Kind::Prompt { .. } => src.colours.prompt,
        Kind::Message { .. } => src.colours.message,
        _ => src.colours.training,
    };
    let colour = style_colour(base, v.style, &src.colours);
    let mut cover = Cover::new();
    if let Kind::Training { .. } = e.kind {
        // The box: a stand-in for the frame art (decoding 3).
        let [x0, y0, x1, y1] = rect.pixels();
        for y in y0..y1 {
            for x in x0..x1 {
                let edge = x == x0 || y == y0 || x == x1 - 1 || y == y1 - 1;
                let c = if edge {
                    [colour[0], colour[1], colour[2], 200]
                } else {
                    [0, 0, 0, 120]
                };
                canvas.blend(x, y, c);
            }
        }
        cover.add(&Cover::rect(&rect));
        d.notes
            .push("the training box's frame art is a stand-in (decoding 3)".into());
    }
    let mut y = match e.kind {
        Kind::Training { .. } => rect.y0 + TRAINING_PAD[1],
        _ => rect.y0,
    };
    for (line, size) in v.text.split('\n').zip(&lines) {
        let x = match e.kind {
            Kind::Training { .. } => (rect.x0 + rect.x1) / 2.0 - (size[0] / 2.0).floor(),
            _ => rect.x0,
        };
        blit(canvas, &mut cover, fonts, font, line, colour, [x, y]);
        y += size[1];
    }
    d.rect = Some(rect);
    d.drawn = true;
    d.cover = Some(cover);
    d
}

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => Rect {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        },
    }
}

/// A rectangle of `size` centred across `cx` with its top at `top`.
fn centred(cx: f32, top: f32, size: [f32; 2]) -> Rect {
    let x0 = (cx - size[0] / 2.0).round();
    Rect {
        x0,
        y0: top.round(),
        x1: x0 + size[0],
        y1: top.round() + size[1],
    }
}

/// Draws `sprite` into `at`, each texel multiplied by `tint` (the HUD
/// shader's colour), into `cover`; without texels, a hatched stand-in.
fn draw_sprite(canvas: &mut Canvas, cover: &mut Cover, sprite: &Sprite, at: &Rect, tint: [u8; 4]) {
    let [pw, ph] = sprite.px;
    let [x0, y0] = [at.x0 as i32, at.y0 as i32];
    let mul = |a: u8, b: u8| ((u16::from(a) * u16::from(b) + 127) / 255) as u8;
    for y in 0..ph {
        for x in 0..pw {
            let t = match &sprite.rgba {
                Some(rgba) => {
                    let i = ((y * pw + x) * 4) as usize;
                    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
                }
                None if (x + y) % 6 < 2 => [255, 80, 200, 160],
                None => [255, 80, 200, 40],
            };
            let c = [
                mul(t[0], tint[0]),
                mul(t[1], tint[1]),
                mul(t[2], tint[2]),
                mul(t[3], tint[3]),
            ];
            let (cx, cy) = (x0 + x as i32, y0 + y as i32);
            if c[3] > 64 {
                cover.set(cx, cy);
            }
            canvas.blend(cx, cy, c);
        }
    }
}

/// A navpoint as the game draws one on screen (E `0x0024e59f`): the
/// navpoint arrow (`hudg::NAVPOINT_ARROW`, `0x0024e9c7`) with its tip on
/// the point, pointing down at it, and the `hudg` waypoint marker's
/// in-view sequence (`0x0024ecae`) above the arrow, [`NAVPOINT_GAP`] up;
/// both through the HUD shader, texels times the user's HUD colour (the
/// four colours `0x0024da7d` writes are all `0x0013927e`'s, the
/// notifications' colour here; S: the shader's combine) at the style's
/// alpha. Then the value's number, centred above the marker in
/// [`NAVPOINT_FONT`] (the game draws no number: the kit's). The point is
/// the sample of the HUD's `ordinal`th navpoint ([`navpoint_sample`]). The outline
/// `0x0024f0fc` draws from `arrow_assets +0x1c` is not drawn (decoding 8).
fn navpoint(
    canvas: &mut Canvas,
    src: &mut Source,
    frame: &Frame,
    mode: Mode,
    e: &Element,
    v: &Value,
    ordinal: usize,
) -> Drawn {
    let font = NAVPOINT_FONT.engine();
    let in_mode = e.at.offset(mode).is_some();
    let mut d = Drawn {
        id: e.id,
        kind: e.kind.name(),
        channel: true,
        rect: None,
        reserved: None,
        text: v.shown.then(|| v.text.clone()),
        style: v.style,
        fill: None,
        angle: None,
        shown: v.shown,
        drawn: false,
        hidden: if !in_mode {
            Some(format!("no {} offset", mode.label()))
        } else if !v.shown {
            Some("not shown in this state".to_owned())
        } else {
            None
        },
        cover: None,
        font: Some(font),
        notes: vec![
            "the point is the preview's sample".into(),
            "the arrow's outline (0x0024f0fc) is not drawn".into(),
        ],
    };
    if !in_mode {
        return d;
    }
    let c = frame.anchor_point(4).expect("the centre");
    let sample = navpoint_sample(ordinal);
    let mut point = [
        c[0] + sample[0] * f32::from(frame.right - frame.left),
        c[1] + sample[1] * f32::from(frame.bottom - frame.top),
    ];
    let hudg = src.hudg.clone();
    let mut sprite = |image, seq: i16, what: &str, notes: &mut Vec<String>| {
        let seq = i8::try_from(seq).unwrap_or(-1);
        match src.art.sprite(&mut src.map, image, seq) {
            Ok(s) => {
                notes.extend(s.note.clone());
                Some(s)
            }
            Err(why) => {
                notes.push(format!("the {what}: {why}"));
                None
            }
        }
    };
    let marker = match hudg.as_ref().and_then(|h| h.marker) {
        Some(m) => sprite(m.image, m.visible, "marker", &mut d.notes),
        None => {
            d.notes
                .push("the hudg has no waypoint_markers[0]: no marker".into());
            None
        }
    };
    let arrow = match hudg.as_ref().and_then(|h| h.navpoint_arrow) {
        Some(a) => sprite(a.image, a.sequence, "arrow", &mut d.notes),
        None => None,
    };
    let fonts = src.fonts.as_ref();
    let longest = e.kind.longest().unwrap_or("");
    let lsize = render::text_size(fonts, font, longest);
    // Bottom to top: the arrow's tip on the point, the marker, the number.
    let stack = |point: [f32; 2]| {
        let arect = arrow
            .as_ref()
            .map(|a| centred(point[0], point[1] - a.size[1], a.size));
        let mrect = marker.as_ref().map(|m| {
            let bottom = arect.map_or(point[1], |a| a.y0 - NAVPOINT_GAP);
            centred(point[0], bottom - m.size[1], m.size)
        });
        let top = mrect
            .or(arect)
            .map_or(point[1], |r| r.y0 - NAVPOINT_NUMBER_GAP);
        (arect, mrect, top, centred(point[0], top - lsize[1], lsize))
    };
    // A navpoint is drawn inside the waypoint area, the safe bounds less
    // `hudg +0xec`..`+0xf8` (A: the waypoint insets; S: the game keeps a
    // navpoint's stack in it). The sample is the preview's own, so one
    // whose stack would leave the frame's top (a split-screen frame is
    // 204 high) is moved down until it fits.
    let lift = f32::from(frame.top) - stack(point).3.y0;
    if lift > 0.0 {
        point[1] += lift.ceil();
        d.notes.push(format!(
            "the sample moved {} down to keep it in the frame",
            lift.ceil()
        ));
    }
    let (arect, mrect, top, lrect) = stack(point);
    let mut reserved = None;
    for r in [arect, mrect].into_iter().flatten() {
        reserved = Some(union(reserved, r));
    }
    d.reserved = Some(union(reserved, lrect));
    if d.hidden.is_some() {
        return d;
    }
    let colour = style_colour(src.colours.message, v.style, &src.colours);
    let mut cover = Cover::new();
    let mut rect = reserved;
    for (s, r) in [(&arrow, arect), (&marker, mrect)] {
        if let (Some(s), Some(r)) = (s, r) {
            draw_sprite(canvas, &mut cover, s, &r, colour);
        }
    }
    let tsize = render::text_size(fonts, font, &v.text);
    let trect = centred(point[0], top - tsize[1], tsize);
    blit(
        canvas,
        &mut cover,
        fonts,
        font,
        &v.text,
        colour,
        [trect.x0, trect.y0],
    );
    rect = Some(union(rect, trect));
    d.rect = rect;
    d.drawn = true;
    d.cover = Some(cover);
    d
}

/// `hud` (compiled as `compiled`) at `values` over the retail player HUD
/// at `st`.
pub fn shot(
    src: &mut Source,
    hud: &Hud,
    compiled: &Compiled,
    state: &str,
    values: &Values,
    st: &HudState,
) -> Result<Shot> {
    let retail = hud_edit::model::read(&mut src.map, src.player)?;
    let kit = compiled.as_hud_edit(KIT_BASE);
    let mode = st.mode;
    let answers = KitAnswers {
        hud,
        compiled,
        values,
        mode,
        colours: &src.colours,
    };
    let pic = render::draw_answered(
        &mut src.map,
        &mut src.art,
        src.fonts.as_ref(),
        &[&retail, &kit],
        st,
        &answers,
    );
    let frame = pic.frame;
    let mut canvas = pic.canvas;
    let mut elements = Vec::new();
    for e in &hud.elements {
        let v = values.get(e.id).cloned().unwrap_or_default();
        if e.kind.is_channel() {
            // Which of the HUD's navpoints this is: its own sample.
            let ordinal = hud
                .elements
                .iter()
                .filter(|x| matches!(x.kind, Kind::Navpoint { .. }))
                .position(|x| x.id == e.id)
                .unwrap_or(0);
            elements.push(channel(&mut canvas, src, &frame, mode, e, &v, ordinal));
            continue;
        }
        let p = pic
            .placed
            .iter()
            .find(|p| p.hud == KIT && p.name == e.id)
            .with_context(|| format!("{} was not placed", e.id))?;
        let in_mode = e.at.offset(mode).is_some();
        let (font, reserved) = match &e.kind {
            Kind::Text {
                font,
                justify,
                longest,
                ..
            } => {
                let size = render::text_size(src.fonts.as_ref(), font.engine(), longest);
                (
                    Some(font.engine()),
                    p.point
                        .map(|pt| place::text_rect(pt, size, *justify as u16)),
                )
            }
            _ => (None, p.rect),
        };
        let hidden = if !in_mode {
            Some(format!("no {} offset", mode.label()))
        } else if !v.shown {
            Some("not shown in this state".to_owned())
        } else {
            None
        };
        elements.push(Drawn {
            id: e.id,
            kind: e.kind.name(),
            channel: false,
            rect: if in_mode { p.rect } else { None },
            reserved: if in_mode { reserved } else { None },
            text: match e.kind {
                Kind::Text { .. } if v.shown => Some(v.text.clone()),
                _ => None,
            },
            style: v.style,
            fill: matches!(e.kind, Kind::Meter { .. }).then_some(v.fill),
            angle: matches!(e.kind, Kind::Pointer { .. }).then_some(v.angle),
            shown: v.shown,
            drawn: p.drawn,
            hidden,
            cover: p.cover.clone(),
            font,
            notes: p.notes.clone(),
        });
    }
    Ok(Shot {
        hud: hud.name,
        state: state.to_owned(),
        mode,
        canvas,
        frame,
        retail: pic.placed.into_iter().filter(|p| p.hud == 0).collect(),
        elements,
    })
}

fn rect_json(r: &Option<Rect>) -> Json {
    match r {
        Some(r) => json!(r.pixels()),
        None => Json::Null,
    }
}

/// A shot's JSON: every element's id, kind, rectangle, text and style.
pub fn shot_json(s: &Shot, src: &Source, findings: &[crate::check::Finding]) -> Json {
    let f = s.frame;
    json!({
        "hud": s.hud,
        "state": s.state,
        "mode": mode_name(s.mode),
        "source": if src.synthetic { "synthetic" } else { "disc" },
        "map": src.name,
        "frame": [f.left, f.top, f.right, f.bottom],
        "elements": s.elements.iter().map(|d| json!({
            "id": d.id,
            "kind": d.kind,
            "rect": rect_json(&d.rect),
            "reserved": rect_json(&d.reserved),
            "text": d.text,
            "style": d.style.name(),
            "fill": d.fill,
            "angle": d.angle,
            "font": d.font,
            "shown": d.shown,
            "drawn": d.drawn,
            "hidden": d.hidden,
            "notes": d.notes,
        })).collect::<Vec<_>>(),
        "colours": src.colours.notes,
        "findings": findings
            .iter()
            .filter(|x| x.hud == s.hud && x.state.as_deref().is_none_or(|st| st == s.state) && x.mode.is_none_or(|m| m == s.mode))
            .map(crate::check::Finding::json)
            .collect::<Vec<_>>(),
    })
}

/// Refuses a directory inside a Git checkout: the pictures are the disc's
/// art (`hud_edit::session::HudSession::export_png` refuses the same).
pub fn guard(dir: &Path) -> Result<()> {
    let abs = std::path::absolute(dir)?;
    if let Some(repo) = abs.ancestors().find(|a| a.join(".git").exists()) {
        bail!(
            "{} is inside the Git checkout {}; pick a folder outside it",
            dir.display(),
            repo.display()
        );
    }
    Ok(())
}

/// Writes `<stem>.png` and `<stem>.json` into `dir`.
pub fn write(dir: &Path, s: &Shot, json: &Json) -> Result<[PathBuf; 2]> {
    guard(dir)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let png = dir.join(format!("{}.png", s.stem()));
    s.canvas.write_png(&png)?;
    let js = dir.join(format!("{}.json", s.stem()));
    std::fs::write(&js, serde_json::to_vec_pretty(json)?)?;
    Ok([png, js])
}

/// The tiles of a sheet: half size, `cols` across.
pub const TILE: [u32; 2] = [render::W / 2, render::H / 2];

/// The sheet: every shot at half size, `cols` across (a row per HUD and
/// state, a column per mode), and where each tile is.
pub fn sheet(shots: &[Shot], cols: usize) -> (Canvas, Json) {
    let cols = cols.max(1);
    let rows = shots.len().div_ceil(cols).max(1);
    let (w, h) = (TILE[0] * cols as u32, TILE[1] * rows as u32);
    let mut c = Canvas {
        width: w,
        height: h,
        rgba: vec![0; (w * h * 4) as usize],
    };
    let mut tiles = Vec::new();
    for (i, s) in shots.iter().enumerate() {
        let (tx, ty) = ((i % cols) as u32 * TILE[0], (i / cols) as u32 * TILE[1]);
        for y in 0..TILE[1] {
            for x in 0..TILE[0] {
                let mut sum = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let si = (((2 * y + dy) * s.canvas.width + 2 * x + dx) * 4) as usize;
                    for k in 0..4 {
                        sum[k] += u32::from(s.canvas.rgba[si + k]);
                    }
                }
                let di = (((ty + y) * w + tx + x) * 4) as usize;
                for k in 0..4 {
                    c.rgba[di + k] = (sum[k] / 4) as u8;
                }
            }
        }
        tiles.push(json!({"hud": s.hud, "state": s.state, "mode": mode_name(s.mode), "x": tx, "y": ty, "w": TILE[0], "h": TILE[1]}));
    }
    (c, json!({ "tiles": tiles }))
}
