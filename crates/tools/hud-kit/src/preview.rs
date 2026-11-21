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
use hud_edit::render::{self, Answers, Canvas, Cover, FALLBACK_CHAR, Fonts, Placed};
use hud_edit::state::{Grenade, HudState};
use serde_json::{Value as Json, json};

use crate::compile::Compiled;
use crate::model::{Element, Hud, Kind, Style, Value, Values};
use crate::rects::{self, MESSAGE_LINE, PROMPT_DOWN, TRAINING_PAD};
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
/// The message and training font: font id 6 fullscreen, 5 in split
/// screen (E `0x0024cfaf`..`0x0024cfc9`, `0x001e71d7`).
pub const MESSAGE_FONT: [i16; 2] = [6, 5];
/// The retail HUD's state words for the sheet: word A `0x1000`, the
/// retail shield meter's show word (S: what else the game sets; the disc
/// test checks the tracker and the shield meter draw at it).
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

/// The retail HUD states of the sheet, the first the one pictures are
/// drawn at: plain, overshield, plasma grenades selected, zoomed.
pub fn retail_states(mode: Mode) -> Vec<HudState> {
    let base = HudState {
        mode,
        gate: true,
        words: RETAIL_WORDS,
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
        // The record's colour at its start (`hudg::Flash::start`); the
        // curve over `since_tick` is slice 4's.
        Style::Flash(p) => colours.flash(p),
    }
}

/// What the runtime's hooks answer for the kit's widgets.
struct KitAnswers<'a> {
    hud: &'a Hud,
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
    fn fill(&self, h: usize, w: &Widget) -> Option<f32> {
        let (e, v) = self.of(h, w)?;
        matches!(e.kind, Kind::Meter { .. }).then_some(v.fill.clamp(0.0, 1.0))
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
    src: &Source,
    frame: &Frame,
    mode: Mode,
    e: &Element,
    v: &Value,
) -> Drawn {
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
            elements.push(channel(&mut canvas, src, &frame, mode, e, &v));
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
