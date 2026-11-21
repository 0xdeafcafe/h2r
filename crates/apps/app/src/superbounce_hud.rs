//! The superbounce game type's HUD (docs/game/superbounce.md, "The
//! superbounce game type"): a small glass panel at the right of the game
//! image that says whether the player is primed, how hard the nearest
//! strip launches, where it is and how far to drop onto it, and, the
//! first time, four tutorial cards under it that teach the trick.
//!
//! Everything is drawn on the CPU with [`crate::hud_shapes`] into one
//! straight-alpha image, uploaded and drawn as one quad (`overlay::Sprite`).
//! The image is redrawn only when what it shows changes: the inputs are
//! rounded to what is printed ([`Frame`]), and the two animations (the
//! primed pulse and the scan spinner) are stepped, so a still HUD costs a
//! comparison a present and an animating one at most [`ANIM_HZ`] redraws a
//! second.
//!
//! The game side is `game::superbounce_mode`; `h2 play` turns its state
//! into a [`View`] and the seen-flag into [`Source::with_tutorial`]. This
//! module knows nothing of the game.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::hud_shapes::{Canvas, Pt, Rgba, fade, text_width};
use crate::scale::Viewport;

// ---- what the HUD shows ----

/// The spot scan, as the HUD tells it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Scan {
    #[default]
    Scanning,
    Ready,
    Failed(String),
}

/// The bounce spot nearest the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nearest {
    /// Metres to its edge.
    pub distance_m: f32,
    /// Estimated launch, world units a second.
    pub launch: f32,
    /// How far to fall onto it, world units (0 for a standing jump).
    pub fall: f32,
}

/// One tick's state, as the HUD needs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub primed: bool,
    pub scan: Scan,
    pub nearest: Option<Nearest>,
    /// Where the nearest spot is, radians clockwise from where the camera
    /// looks; `None` hides the arrow.
    pub bearing: Option<f32>,
    pub airborne: bool,
    /// Vertical velocity, world units a second; a bounce is a jump in it.
    pub vz: Option<f32>,
}

// ---- the tutorial ----

/// Tutorial cards.
pub const STEPS: u8 = 4;
/// A strip this close (metres) shows card 3.
pub const NEAR_M: f32 = 8.0;
/// An upward speed (world units a second) past which the player is
/// bouncing: a standing jump leaves at about 3.
pub const BOUNCE_VZ: f32 = 8.0;
/// Card 3 gives way to card 4 after this long without a bounce.
pub const CARD3_SECS: f64 = 20.0;
/// Card 4 closes after this long without a bounce.
pub const CARD4_SECS: f64 = 15.0;

/// Card titles and bodies, one a step.
pub const CARDS: [(&str, &str); 4] = [
    (
        "CROUCH, THEN STAND",
        "Find a CYAN line: a forced-crouch spot. Walk in crouched, then stand up.",
    ),
    (
        "PRIMED",
        "The HUD lights up PRIMED: your collision is in its give-up state (-3). \
         Don't crouch again, or you lose it.",
    ),
    (
        "FIND A STRIP",
        "Find a glowing strip. Green: a standing jump is enough. Red: you need a big drop.",
    ),
    (
        "BOUNCE",
        "Jump or fall onto it. Hold X while falling to guide yourself on.",
    ),
];

/// The tutorial's step machine: which card shows, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Tutorial {
    /// Still to show this run.
    pending: bool,
    /// The card showing, 1..=4.
    step: Option<u8>,
    /// When the card showing came up, seconds on the caller's clock.
    since: f64,
    /// The last vertical speed seen, for the bounce edge.
    last_vz: f32,
}

/// What [`Tutorial::update`] or [`Tutorial::dismiss`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Now showing this card.
    Card(u8),
    /// The last card closed: the tutorial is done.
    Done,
}

impl Tutorial {
    /// `show`: the cards are to be shown the first time the mode is on.
    pub fn new(show: bool) -> Self {
        Self {
            pending: show,
            step: None,
            since: 0.0,
            last_vz: 0.0,
        }
    }

    /// The card showing.
    pub fn step(&self) -> Option<u8> {
        self.step
    }

    fn go(&mut self, step: u8, now: f64) -> Change {
        if step > STEPS {
            self.step = None;
            self.pending = false;
            return Change::Done;
        }
        self.step = Some(step);
        self.since = now;
        Change::Card(step)
    }

    /// One present with the mode on (`view`) or off (`None`), at `now`
    /// seconds. At most one card change a call, so each card shows at
    /// least once.
    pub fn update(&mut self, view: Option<&View>, now: f64) -> Change {
        let Some(v) = view else {
            return Change::None;
        };
        let vz = v.vz.unwrap_or(0.0);
        let bounce = vz >= BOUNCE_VZ && self.last_vz < BOUNCE_VZ;
        self.last_vz = vz;
        if !self.pending {
            return Change::None;
        }
        let age = now - self.since;
        match self.step {
            None => self.go(1, now),
            Some(1) if v.primed => self.go(2, now),
            Some(2) if v.nearest.is_some_and(|n| n.distance_m <= NEAR_M) => self.go(3, now),
            Some(3) if bounce || age >= CARD3_SECS => self.go(4, now),
            Some(4) if bounce || age >= CARD4_SECS => self.go(5, now),
            _ => Change::None,
        }
    }

    /// The dismiss key: the next card, or done after the last. Nothing
    /// (and the key is not taken) while no card shows.
    pub fn dismiss(&mut self, now: f64) -> Change {
        match self.step {
            Some(s) => self.go(s + 1, now),
            None => Change::None,
        }
    }
}

// ---- layout ----

/// The panel, in design units (pixels at 720 lines).
pub const PANEL_W: f32 = 240.0;
pub const PANEL_H: f32 = 80.0;
/// Added for the hold-X hint row.
pub const HINT_H: f32 = 16.0;
pub const CARD_GAP: f32 = 6.0;
pub const CARD_H: f32 = 118.0;
/// Room for the glow around everything.
pub const GLOW: f32 = 8.0;
/// From the image's right edge to the panel's.
pub const MARGIN: f32 = 20.0;
/// The panel's top, as a fraction of the image's height down.
pub const TOP: f32 = 0.22;
/// The clipped corners.
const CHAMFER: f32 = 10.0;

/// Pixels a design unit for `area`: its height over 720, rounded to a
/// whole number so the letters stay sharp (1 at 720p, 2 at 1080p and
/// 1440p, 3 at 4K), at least 1, and small enough that the panel fits the
/// width.
pub fn unit_for(area: Viewport) -> f32 {
    let tall = (area.height as f32 / 720.0).round();
    let wide = (area.width as f32 / (PANEL_W + 2.0 * GLOW + MARGIN)).floor();
    tall.min(wide).max(1.0)
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Where the HUD's image goes and what is in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub unit: f32,
    /// The image's top-left in the target, and its size.
    pub at: (u32, u32),
    pub size: (u32, u32),
    /// The panel and the card, in the image's pixels.
    pub panel: Rect,
    pub card: Option<Rect>,
}

/// The layout in `area` (the game image) with the hint row (`hint`) and a
/// card (`card`).
pub fn layout(area: Viewport, hint: bool, card: bool) -> Layout {
    let u = unit_for(area);
    let panel_h = PANEL_H + if hint { HINT_H } else { 0.0 };
    let total_h = panel_h + if card { CARD_GAP + CARD_H } else { 0.0 };
    let size = (
        ((PANEL_W + 2.0 * GLOW) * u).ceil() as u32,
        ((total_h + 2.0 * GLOW) * u).ceil() as u32,
    );
    let right = (area.x + area.width) as f32 - MARGIN * u;
    let x = (right - (PANEL_W + GLOW) * u).round().max(area.x as f32) as u32;
    let y = (area.y as f32 + area.height as f32 * TOP - GLOW * u)
        .round()
        .max(area.y as f32) as u32;
    let panel = Rect {
        x: GLOW * u,
        y: GLOW * u,
        w: PANEL_W * u,
        h: panel_h * u,
    };
    Layout {
        unit: u,
        at: (x, y),
        size,
        panel,
        card: card.then_some(Rect {
            x: GLOW * u,
            y: (GLOW + panel_h + CARD_GAP) * u,
            w: PANEL_W * u,
            h: CARD_H * u,
        }),
    }
}

/// The launch gauge's fill for a launch of `launch` world units a second:
/// 0 at 0, full at 30, clamped; 0 for a NaN.
pub fn gauge_fill(launch: f32) -> f32 {
    if launch.is_nan() {
        0.0
    } else {
        (launch / 30.0).clamp(0.0, 1.0)
    }
}

/// `text` broken into lines of at most `cols` characters at spaces (a word
/// longer than a line is cut).
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > cols {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(cols).collect();
            word = word.chars().skip(cols).collect();
            lines.push(head);
        }
        let need = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
        if need > cols && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ---- one drawn frame ----

/// The animations' redraw rate.
pub const ANIM_HZ: f32 = 15.0;
/// The primed pulse's period, seconds.
const PULSE_SECS: f32 = 1.2;
/// The spinner's turn, seconds.
const SPIN_SECS: f32 = 1.0;

/// Everything one HUD image shows, rounded to what is drawn: two equal
/// frames draw the same pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub layout: Layout,
    pub primed: bool,
    /// Pulse phase step (0 when not primed).
    pub pulse: u8,
    /// Launch, to a tenth; `None` without a spot.
    pub launch: Option<f32>,
    /// `6.1 m`.
    pub distance: Option<String>,
    /// The arrow's turn, whole degrees in steps of 5.
    pub arrow: Option<i32>,
    /// `drop 1.1`.
    pub drop: Option<String>,
    /// Said instead of the spot row: scanning, a failure, no spots.
    pub status: Option<String>,
    /// Spinner step while scanning.
    pub spinner: Option<u8>,
    pub hint: bool,
    pub card: Option<u8>,
}

fn steps(t: f32, period: f32) -> u8 {
    let n = (ANIM_HZ * period).round().max(1.0);
    ((t / period).fract() * n).floor() as u8
}

impl Frame {
    /// The frame for `view` with card `card` showing, at `t` seconds on
    /// the animation clock, in `area`.
    pub fn new(view: &View, card: Option<u8>, t: f32, area: Viewport) -> Self {
        let hint = view.primed && view.airborne;
        let tenth = |v: f32| (v * 10.0).round() / 10.0;
        let (status, spinner) = match &view.scan {
            Scan::Scanning => (
                Some("scanning map...".to_string()),
                Some(steps(t, SPIN_SECS)),
            ),
            Scan::Failed(why) => (Some(fit(&format!("no spots: {why}"), 23)), None),
            Scan::Ready if view.nearest.is_none() => (Some("no bounce spots here".into()), None),
            Scan::Ready => (None, None),
        };
        let near = view.nearest.filter(|_| status.is_none());
        Self {
            layout: layout(area, hint, card.is_some()),
            primed: view.primed,
            pulse: if view.primed { steps(t, PULSE_SECS) } else { 0 },
            launch: view.nearest.map(|n| tenth(n.launch)),
            distance: near.map(|n| format!("{:.1} m", n.distance_m)),
            arrow: near
                .and(view.bearing)
                .filter(|b| b.is_finite())
                .map(|b| ((b.to_degrees() / 5.0).round() as i32 * 5).rem_euclid(360)),
            drop: near.map(|n| format!("drop {:.1}", n.fall.max(0.0))),
            status,
            spinner,
            hint,
            card,
        }
    }
}

fn fit(text: &str, max: usize) -> String {
    crate::watermark::fit(text, max)
}

// ---- drawing ----

const GLASS: Rgba = [0.02, 0.09, 0.16, 0.66];
const GLASS_TOP: Rgba = [0.55, 0.85, 1.0, 0.07];
const EDGE: Rgba = [0.40, 0.88, 1.0, 0.95];
const GLOW_RGB: Rgba = [0.15, 0.65, 1.0, 0.30];
const LIT: Rgba = [0.55, 0.96, 1.0, 1.0];
const TEXT: Rgba = [0.86, 0.95, 1.0, 1.0];
const DIM_TEXT: Rgba = [0.55, 0.72, 0.80, 0.85];
const GREY: Rgba = [0.52, 0.56, 0.60, 0.80];
const TRACK: Rgba = [0.08, 0.20, 0.30, 0.80];
const X_BLUE: Rgba = [0.12, 0.42, 1.0, 1.0];
const GREEN: Rgba = [0.25, 1.0, 0.35, 1.0];
const RED: Rgba = [1.0, 0.25, 0.20, 1.0];
const INSET: Rgba = [0.0, 0.04, 0.09, 0.65];

/// Draws in design units: `o` is the origin in pixels, `u` pixels a unit.
struct Pen<'a> {
    c: &'a mut Canvas,
    o: Pt,
    u: f32,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> Pt {
        (self.o.0 + x * self.u, self.o.1 + y * self.u)
    }

    fn pts(&self, pts: &[Pt]) -> Vec<Pt> {
        pts.iter().map(|&(x, y)| self.p(x, y)).collect()
    }

    fn fill(&mut self, pts: &[Pt], col: Rgba) {
        let pts = self.pts(pts);
        self.c.fill_polygon(&pts, col);
    }

    fn glow(&mut self, pts: &[Pt], radius: f32, col: Rgba) {
        let pts = self.pts(pts);
        self.c.glow_polygon(&pts, radius * self.u, col);
    }

    fn line(&mut self, pts: &[Pt], width: f32, closed: bool, col: Rgba) {
        let pts = self.pts(pts);
        self.c.stroke(&pts, width * self.u, closed, col);
    }

    fn ring(&mut self, x: f32, y: f32, outer: f32, inner: f32, col: Rgba) {
        let at = self.p(x, y);
        self.c.ring(at, outer * self.u, inner * self.u, col);
    }

    fn disc(&mut self, x: f32, y: f32, r: f32, col: Rgba) {
        let at = self.p(x, y);
        self.c.disc(at, r * self.u, col);
    }

    fn glow_disc(&mut self, x: f32, y: f32, r: f32, radius: f32, col: Rgba) {
        let at = self.p(x, y);
        self.c.glow_disc(at, r * self.u, radius * self.u, col);
    }

    #[allow(clippy::too_many_arguments)]
    fn arc(&mut self, x: f32, y: f32, outer: f32, inner: f32, a0: f32, a1: f32, col: Rgba) {
        let at = self.p(x, y);
        self.c.arc(at, outer * self.u, inner * self.u, a0, a1, col);
    }

    /// Text with its top-left at `(x, y)`, `size` units a font pixel.
    fn text(&mut self, x: f32, y: f32, size: f32, s: &str, col: Rgba) {
        let at = self.p(x, y);
        // A soft shadow one font pixel down-right, then the letters.
        let px = size * self.u;
        self.c.text(
            (at.0 + px * 0.5, at.1 + px * 0.5),
            px,
            s,
            [0.0, 0.0, 0.0, 0.45 * col[3]],
        );
        self.c.text(at, px, s, col);
    }

    /// Text ending at `right`.
    fn text_right(&mut self, right: f32, y: f32, size: f32, s: &str, col: Rgba) {
        let x = right - text_width(s, size);
        self.text(x, y, size, s, col);
    }

    /// An arrow from `a` to `b`, `width` thick, its head `head` long.
    fn arrow(&mut self, a: Pt, b: Pt, width: f32, head: f32, col: Rgba) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-3);
        let (ux, uy) = (dx / len, dy / len);
        let base = (b.0 - ux * head, b.1 - uy * head);
        self.line(&[a, base], width, false, col);
        let half = head * 0.7;
        self.fill(
            &[
                b,
                (base.0 - uy * half, base.1 + ux * half),
                (base.0 + uy * half, base.1 - ux * half),
            ],
            col,
        );
    }

    /// A biped as a capsule from its feet to its head.
    fn figure(&mut self, feet: Pt, head: Pt, col: Rgba) {
        self.line(&[feet, head], 7.0, false, col);
    }
}

/// The clipped-corner outline of a `w` x `h` plate.
fn plate(w: f32, h: f32) -> [Pt; 6] {
    let c = CHAMFER;
    [
        (c, 0.0),
        (w, 0.0),
        (w, h - c),
        (w - c, h),
        (0.0, h),
        (0.0, c),
    ]
}

/// An upward chevron centred at `(x, y)`, `s` across.
fn chevron(x: f32, y: f32, s: f32) -> [Pt; 6] {
    let h = s / 2.0;
    [
        (x, y - h * 0.9),
        (x + h, y + h * 0.15),
        (x + h, y + h * 0.75),
        (x, y - h * 0.2),
        (x - h, y + h * 0.75),
        (x - h, y + h * 0.15),
    ]
}

/// Glass, glow and edges of a plate at `r`.
fn glass(c: &mut Canvas, r: Rect, u: f32, glow: f32) {
    let mut pen = Pen {
        c,
        o: (r.x, r.y),
        u,
    };
    let (w, h) = (r.w / u, r.h / u);
    let shape = plate(w, h);
    pen.glow(&shape, GLOW * 0.9, fade(GLOW_RGB, glow));
    pen.fill(&shape, GLASS);
    // A faint sheen along the top.
    pen.fill(
        &[
            (CHAMFER, 0.0),
            (w, 0.0),
            (w, 12.0),
            (0.0, 12.0),
            (0.0, CHAMFER),
        ],
        GLASS_TOP,
    );
    pen.line(&shape, 1.0, true, fade(EDGE, 0.75));
    // Bright accents: along the top from the cut corner, and the far cut.
    pen.line(
        &[(CHAMFER + 2.0, 0.0), (CHAMFER + 56.0, 0.0)],
        2.5,
        false,
        EDGE,
    );
    pen.line(&[(w, h - CHAMFER), (w - CHAMFER, h)], 2.0, false, EDGE);
    pen.line(&[(w - 30.0, h), (w - CHAMFER - 2.0, h)], 1.5, false, EDGE);
}

/// The panel's contents.
fn panel(c: &mut Canvas, f: &Frame) {
    let r = f.layout.panel;
    let u = f.layout.unit;
    let pulse = if f.primed {
        let n = (ANIM_HZ * PULSE_SECS).round();
        0.5 - 0.5 * (f32::from(f.pulse) / n * TAU).cos()
    } else {
        0.0
    };
    let mut pen = Pen {
        c,
        o: (r.x, r.y),
        u,
    };
    let w = PANEL_W;

    // The primed ring and chevron.
    let (rx, ry) = (26.0, 26.0);
    if f.primed {
        pen.glow_disc(
            rx,
            ry,
            13.0,
            7.0 + 5.0 * pulse,
            fade(LIT, 0.25 + 0.25 * pulse),
        );
        pen.ring(rx, ry, 15.0, 12.0, fade(LIT, 0.75 + 0.25 * pulse));
        pen.fill(&chevron(rx, ry, 13.0), LIT);
    } else {
        pen.ring(rx, ry, 15.0, 12.5, fade(GREY, 0.6));
        pen.fill(&chevron(rx, ry, 13.0), fade(GREY, 0.55));
    }
    let tick = if f.primed {
        fade(LIT, 0.8)
    } else {
        fade(GREY, 0.4)
    };
    for k in 0..4 {
        let a = PI / 4.0 + k as f32 * PI / 2.0;
        pen.arc(rx, ry, 19.0, 17.5, a - 0.25, a + 0.25, tick);
    }
    pen.text(50.0, 8.0, 1.0, "SUPERBOUNCE", DIM_TEXT);
    let (label, col) = if f.primed {
        ("PRIMED -3", LIT)
    } else {
        ("NOT PRIMED", GREY)
    };
    pen.text(50.0, 20.0, 2.0, label, col);
    pen.line(
        &[(8.0, 45.0), (w - 8.0, 45.0)],
        1.0,
        false,
        fade(EDGE, 0.25),
    );

    // The launch gauge: a slanted bar, ticks every 5 units/s.
    let y = 50.0;
    pen.text(10.0, y, 1.0, "LAUNCH", DIM_TEXT);
    let (x0, x1, skew) = (64.0, 184.0, 3.0);
    let len = x1 - x0 - skew;
    pen.fill(
        &[(x0 + skew, y), (x1, y), (x1 - skew, y + 8.0), (x0, y + 8.0)],
        TRACK,
    );
    let fill = f.launch.map_or(0.0, gauge_fill);
    if fill > 0.0 {
        let l = len * fill;
        let col = if f.primed { LIT } else { fade(EDGE, 0.5) };
        let bar = [
            (x0 + skew, y),
            (x0 + skew + l, y),
            (x0 + l, y + 8.0),
            (x0, y + 8.0),
        ];
        if f.primed {
            pen.glow(&bar, 4.0, fade(LIT, 0.35));
        }
        pen.fill(&bar, col);
    }
    for k in 0..=6 {
        let x = x0 + skew / 2.0 + len * k as f32 / 6.0;
        pen.line(&[(x, y + 10.0), (x, y + 12.0)], 1.0, false, fade(EDGE, 0.5));
    }
    let number = f.launch.map_or("--".to_string(), |l| format!("{l:.1}"));
    pen.text_right(
        w - 10.0,
        y,
        1.0,
        &number,
        if f.primed { TEXT } else { GREY },
    );

    // The spot row, or what the scan says.
    let y = 66.0;
    let (cx, cy) = (16.0, 70.0);
    if let Some(status) = &f.status {
        if let Some(step) = f.spinner {
            let n = (ANIM_HZ * SPIN_SECS).round();
            let a = f32::from(step) / n * TAU;
            pen.ring(cx, cy, 6.0, 4.5, fade(EDGE, 0.2));
            pen.arc(cx, cy, 6.0, 4.5, a, a + 1.9, EDGE);
        } else {
            pen.ring(cx, cy, 6.0, 4.5, fade(GREY, 0.5));
        }
        pen.text(28.0, y, 1.0, status, DIM_TEXT);
    } else {
        let mut x = 10.0;
        if let Some(deg) = f.arrow {
            let b = (deg as f32).to_radians();
            let (s, c) = b.sin_cos();
            let rot = |px: f32, py: f32| (cx + px * c - py * s, cy + px * s + py * c);
            pen.ring(cx, cy, 7.0, 5.8, fade(EDGE, 0.45));
            pen.fill(
                &[rot(0.0, -4.5), rot(3.4, 3.2), rot(0.0, 1.4), rot(-3.4, 3.2)],
                TEXT,
            );
            x = 28.0;
        }
        if let Some(d) = &f.distance {
            pen.text(x, y, 1.0, d, TEXT);
        }
        if let Some(d) = &f.drop {
            pen.text_right(w - 10.0, y, 1.0, d, TEXT);
        }
    }

    // The hold-X hint while primed in the air.
    if f.hint {
        let y = 82.0;
        pen.disc(16.0, y + 4.0, 6.0, X_BLUE);
        pen.text(12.0, y, 1.0, "X", [1.0; 4]);
        pen.text(28.0, y, 1.0, "HOLD X - guide onto strip", LIT);
    }
}

/// A tutorial card.
fn card(c: &mut Canvas, f: &Frame, r: Rect, step: u8) {
    let u = f.layout.unit;
    let mut pen = Pen {
        c,
        o: (r.x, r.y),
        u,
    };
    let i = usize::from(step.clamp(1, STEPS) - 1);
    let (title, body) = CARDS[i];
    pen.text(10.0, 7.0, 1.0, &format!("{step}/{STEPS}"), LIT);
    pen.text(42.0, 7.0, 1.0, title, TEXT);
    pen.line(
        &[(8.0, 19.0), (PANEL_W - 8.0, 19.0)],
        1.0,
        false,
        fade(EDGE, 0.25),
    );
    // The diagram's box.
    let (bx, by, bw, bh) = (10.0, 24.0, 64.0, 56.0);
    let inset = [(bx, by), (bx + bw, by), (bx + bw, by + bh), (bx, by + bh)];
    pen.fill(&inset, INSET);
    pen.line(&inset, 1.0, true, fade(EDGE, 0.35));
    let o = pen.p(bx, by);
    diagram(
        &mut Pen {
            c: &mut *pen.c,
            o,
            u,
        },
        step,
    );
    // Where in the four.
    for k in 0..STEPS {
        let (x, y) = (bx + 8.0 + f32::from(k) * 16.0, 92.0);
        let d = [(x, y - 3.5), (x + 3.5, y), (x, y + 3.5), (x - 3.5, y)];
        let col = if k < step { LIT } else { fade(GREY, 0.5) };
        pen.fill(&d, col);
    }
    let cols = ((PANEL_W - 10.0 - 82.0) / 8.0) as usize;
    for (n, line) in wrap(body, cols).iter().take(8).enumerate() {
        pen.text(82.0, 24.0 + n as f32 * 10.0, 1.0, line, TEXT);
    }
    let foot = if step >= STEPS { "F4 done" } else { "F4 next" };
    pen.text_right(PANEL_W - 10.0, 106.0, 1.0, foot, DIM_TEXT);
}

/// Card `step`'s little picture, in a 64 x 56 box at the pen's origin.
fn diagram(pen: &mut Pen<'_>, step: u8) {
    let floor = fade(GREY, 0.9);
    let cyan = [0.0, 0.85, 1.0, 1.0];
    match step {
        1 => {
            pen.line(&[(4.0, 48.0), (60.0, 48.0)], 1.5, false, floor);
            // A ledge overhanging the floor, with a lip under it.
            pen.fill(
                &[
                    (36.0, 8.0),
                    (60.0, 8.0),
                    (60.0, 22.0),
                    (40.0, 22.0),
                    (36.0, 18.0),
                ],
                [0.40, 0.50, 0.60, 0.9],
            );
            let mut x = 34.0;
            while x < 58.0 {
                pen.line(&[(x, 46.0), (x + 3.5, 46.0)], 2.0, false, cyan);
                x += 6.0;
            }
            pen.figure((46.0, 42.0), (46.0, 32.0), fade(TEXT, 0.9));
            pen.arrow((20.0, 44.0), (20.0, 18.0), 2.0, 6.0, TEXT);
        }
        2 => {
            pen.glow_disc(32.0, 24.0, 13.0, 8.0, fade(LIT, 0.4));
            pen.ring(32.0, 24.0, 15.0, 12.0, LIT);
            pen.fill(&chevron(32.0, 24.0, 13.0), LIT);
            pen.text(24.0, 44.0, 1.0, "-3", LIT);
        }
        3 => {
            pen.line(&[(2.0, 48.0), (62.0, 48.0)], 1.5, false, floor);
            pen.line(&[(6.0, 48.0), (24.0, 48.0)], 3.0, false, GREEN);
            pen.arc(15.0, 46.0, 9.0, 7.5, -1.1, 1.1, fade(GREEN, 0.8));
            pen.line(&[(40.0, 48.0), (58.0, 48.0)], 3.0, false, RED);
            pen.arrow((49.0, 6.0), (49.0, 42.0), 2.0, 6.0, fade(RED, 0.9));
        }
        _ => {
            pen.line(&[(2.0, 50.0), (62.0, 50.0)], 1.5, false, floor);
            pen.line(&[(22.0, 50.0), (42.0, 50.0)], 3.0, false, GREEN);
            pen.figure((20.0, 18.0), (20.0, 8.0), fade(TEXT, 0.8));
            pen.arrow((22.0, 24.0), (28.0, 44.0), 2.0, 6.0, TEXT);
            pen.arrow((36.0, 44.0), (42.0, 6.0), 2.5, 7.0, LIT);
            pen.disc(54.0, 14.0, 7.0, X_BLUE);
            pen.text(50.0, 10.0, 1.0, "X", [1.0; 4]);
        }
    }
}

/// `frame` drawn: (width, height, straight-alpha RGBA) at its layout's
/// size.
pub fn render(frame: &Frame) -> (u32, u32, Vec<u8>) {
    let key = Base::of(frame);
    render_on(key.draw(), frame)
}

/// What the glass under a frame depends on: drawn once while it stays the
/// same, so a changed number or an animation step redraws only the
/// contents.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Base {
    layout: Layout,
    primed: bool,
}

impl Base {
    fn of(frame: &Frame) -> Self {
        Self {
            layout: frame.layout,
            primed: frame.primed,
        }
    }

    /// The panel's glass (brighter primed), and the card's.
    fn draw(&self) -> Canvas {
        let l = &self.layout;
        let mut c = Canvas::new(l.size.0, l.size.1);
        glass(&mut c, l.panel, l.unit, if self.primed { 0.9 } else { 0.45 });
        if let Some(r) = l.card {
            glass(&mut c, r, l.unit, 0.6);
        }
        c
    }
}

/// `frame`'s contents over `base`, its glass.
fn render_on(mut c: Canvas, frame: &Frame) -> (u32, u32, Vec<u8>) {
    let (w, h) = frame.layout.size;
    panel(&mut c, frame);
    if let (Some(r), Some(step)) = (frame.layout.card, frame.card) {
        card(&mut c, frame, r, step);
    }
    (w, h, c.into_rgba8())
}

/// A whole `size` image with the HUD for `view` (and card `card`) over a
/// dark blue-grey gradient standing in for the game: what
/// `h2 superbounce hud-preview` writes. Straight-alpha RGBA, opaque.
pub fn preview(view: &View, card: Option<u8>, t: f32, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = size;
    let area = Viewport {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    let frame = Frame::new(view, card, t, area);
    let (hw, hh, hud) = render(&frame);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = y as f32 / h.max(1) as f32;
            let s = x as f32 / w.max(1) as f32;
            let g = 0.30 + 0.22 * (1.0 - t) + 0.05 * s;
            out.extend_from_slice(&[
                (g * 0.86 * 255.0) as u8,
                (g * 0.92 * 255.0) as u8,
                (g * 255.0) as u8,
                255,
            ]);
        }
    }
    let (ax, ay) = frame.layout.at;
    for y in 0..hh {
        for x in 0..hw {
            let (tx, ty) = (ax + x, ay + y);
            if tx >= w || ty >= h {
                continue;
            }
            let s = ((y * hw + x) * 4) as usize;
            let d = ((ty * w + tx) * 4) as usize;
            let a = f32::from(hud[s + 3]) / 255.0;
            for k in 0..3 {
                let v = f32::from(hud[s + k]) * a + f32::from(out[d + k]) * (1.0 - a);
                out[d + k] = v.round() as u8;
            }
        }
    }
    out
}

// ---- the window's handle ----

type ViewFn = Arc<dyn Fn() -> Option<View> + Send + Sync>;
type SeenFn = Arc<dyn Fn() + Send + Sync>;

/// The superbounce HUD on `AppConfig::superbounce_hud`: where its state
/// comes from and the tutorial. Cloning shares the tutorial.
#[derive(Clone)]
pub struct Source {
    view: ViewFn,
    on_seen: Option<SeenFn>,
    tutorial: Arc<Mutex<Tutorial>>,
    start: Instant,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("superbounce_hud::Source")
            .field(
                "tutorial",
                &*self.tutorial.lock().unwrap_or_else(|e| e.into_inner()),
            )
            .finish()
    }
}

impl Source {
    /// `view` is asked each present (it must be cheap); `None` while the
    /// mode is off draws nothing. No tutorial.
    pub fn new(view: impl Fn() -> Option<View> + Send + Sync + 'static) -> Self {
        Self {
            view: Arc::new(view),
            on_seen: None,
            tutorial: Arc::new(Mutex::new(Tutorial::new(false))),
            start: Instant::now(),
        }
    }

    /// Shows the tutorial cards (`show`) the first time the mode is on;
    /// `on_seen` runs once when the last card closes, to save that.
    pub fn with_tutorial(mut self, show: bool, on_seen: impl Fn() + Send + Sync + 'static) -> Self {
        self.tutorial = Arc::new(Mutex::new(Tutorial::new(show)));
        self.on_seen = Some(Arc::new(on_seen));
        self
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn tutorial(&self) -> std::sync::MutexGuard<'_, Tutorial> {
        self.tutorial.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, change: Change) {
        if change == Change::Done
            && let Some(f) = &self.on_seen
        {
            f();
        }
    }

    /// This present's frame in `area`, or `None` while the mode is off.
    /// Steps the tutorial.
    pub fn frame(&self, area: Viewport) -> Option<Frame> {
        let view = (self.view)();
        let now = self.now();
        let (change, card) = {
            let mut t = self.tutorial();
            let change = t.update(view.as_ref(), now);
            (change, t.step())
        };
        self.changed(change);
        let view = view?;
        Some(Frame::new(&view, card, now as f32, area))
    }

    /// The dismiss key (F4): true when a card was showing and moved on.
    pub fn dismiss(&self) -> bool {
        let change = {
            let mut t = self.tutorial();
            t.dismiss(self.now())
        };
        self.changed(change);
        change != Change::None
    }
}

/// Draws the HUD over an already composited target.
pub struct Renderer {
    sprite: crate::overlay::Sprite,
    drawn: Option<Frame>,
    /// The glass last drawn, reused while its layout and brightness hold.
    base: Option<(Base, Canvas)>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: crate::overlay::Sprite::new(device, "app superbounce hud"),
            drawn: None,
            base: None,
        }
    }

    /// Records the pass drawing `frame` into `dst`. Redraws the image only
    /// when the frame differs from the last.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
    ) {
        if self.drawn.as_ref() != Some(frame) {
            let key = Base::of(frame);
            let base = match &self.base {
                Some((k, c)) if *k == key => c.clone(),
                _ => {
                    let c = key.draw();
                    self.base = Some((key, c.clone()));
                    c
                }
            };
            let (w, h, px) = render_on(base, frame);
            self.sprite.upload(device, queue, (w, h), &px);
            self.drawn = Some(frame.clone());
        }
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, frame.layout.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: u32, height: u32) -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn primed_view() -> View {
        View {
            primed: true,
            scan: Scan::Ready,
            nearest: Some(Nearest {
                distance_m: 6.2,
                launch: 17.85,
                fall: 1.1,
            }),
            bearing: Some(0.5),
            airborne: false,
            vz: Some(0.0),
        }
    }

    #[test]
    fn the_unit_is_whole_and_follows_the_height() {
        assert_eq!(unit_for(area(1280, 720)), 1.0);
        assert_eq!(unit_for(area(1920, 1080)), 2.0);
        assert_eq!(unit_for(area(2560, 1440)), 2.0);
        assert_eq!(unit_for(area(3840, 2160)), 3.0);
        assert_eq!(unit_for(area(640, 480)), 1.0);
        // A tall, narrow image: the width decides.
        assert_eq!(unit_for(area(600, 2160)), 2.0);
        assert_eq!(unit_for(area(100, 100)), 1.0);
    }

    #[test]
    fn the_panel_sits_at_the_right_a_fifth_down_and_scales() {
        for (w, h, u) in [(1280, 720, 1.0), (1920, 1080, 2.0), (3840, 2160, 3.0)] {
            let l = layout(area(w, h), false, false);
            assert_eq!(l.unit, u);
            assert_eq!(
                l.size,
                (
                    ((PANEL_W + 2.0 * GLOW) * u) as u32,
                    ((PANEL_H + 2.0 * GLOW) * u) as u32
                )
            );
            // The panel's right edge MARGIN units in from the image's.
            let right = l.at.0 as f32 + l.panel.x + l.panel.w;
            assert!(
                (right - (w as f32 - MARGIN * u)).abs() <= 1.0,
                "{w}: {right}"
            );
            let top = l.at.1 as f32 + l.panel.y;
            assert!((top - h as f32 * TOP).abs() <= 1.0, "{h}: {top}");
            assert!(l.at.0 + l.size.0 <= w && l.at.1 + l.size.1 <= h);
            assert!(l.card.is_none());
        }
        // The hint and a card make it taller, the card under the panel.
        let l = layout(area(1280, 720), true, true);
        assert_eq!(l.panel.h, PANEL_H + HINT_H);
        let card = l.card.unwrap();
        assert_eq!(card.y, l.panel.y + l.panel.h + CARD_GAP);
        assert_eq!(
            l.size.1 as f32,
            PANEL_H + HINT_H + CARD_GAP + CARD_H + 2.0 * GLOW
        );
        // Inside a pillarboxed image, not the window.
        let boxed = Viewport {
            x: 240,
            y: 0,
            width: 1440,
            height: 1080,
        };
        let l = layout(boxed, false, false);
        assert!(l.at.0 + l.size.0 <= 240 + 1440 && l.at.0 >= 240);
    }

    #[test]
    fn the_gauge_fills_from_0_to_30() {
        assert_eq!(gauge_fill(0.0), 0.0);
        assert_eq!(gauge_fill(15.0), 0.5);
        assert_eq!(gauge_fill(30.0), 1.0);
        assert_eq!(gauge_fill(45.0), 1.0);
        assert_eq!(gauge_fill(-3.0), 0.0);
        assert_eq!(gauge_fill(f32::NAN), 0.0);
        assert!((gauge_fill(17.85) - 0.595).abs() < 1e-6);
    }

    #[test]
    fn text_wraps_at_spaces() {
        assert_eq!(wrap("a bb ccc dddd", 6), ["a bb", "ccc", "dddd"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert!(wrap("", 4).is_empty());
        // Every card fits its eight lines.
        for (title, body) in CARDS {
            assert!(title.len() <= 22, "{title}");
            assert!(wrap(body, 18).len() <= 8, "{body}");
        }
    }

    fn at(t: &mut Tutorial, v: &View, now: f64) -> Change {
        t.update(Some(v), now)
    }

    #[test]
    fn the_tutorial_follows_what_the_player_does() {
        let mut t = Tutorial::new(true);
        assert_eq!(t.update(None, 0.0), Change::None, "waits for the mode");
        let mut v = View {
            primed: false,
            nearest: None,
            ..primed_view()
        };
        assert_eq!(at(&mut t, &v, 1.0), Change::Card(1));
        assert_eq!(at(&mut t, &v, 2.0), Change::None);
        v.primed = true;
        assert_eq!(at(&mut t, &v, 3.0), Change::Card(2));
        // Far from a strip: card 2 stays. Near one: card 3.
        v.nearest = Some(Nearest {
            distance_m: 30.0,
            launch: 17.9,
            fall: 1.1,
        });
        assert_eq!(at(&mut t, &v, 4.0), Change::None);
        v.nearest = primed_view().nearest;
        assert_eq!(at(&mut t, &v, 5.0), Change::Card(3));
        // A bounce (vertical speed jumping past BOUNCE_VZ) shows card 4;
        // the same bounce does not also close it.
        v.vz = Some(17.0);
        assert_eq!(at(&mut t, &v, 6.0), Change::Card(4));
        assert_eq!(at(&mut t, &v, 7.0), Change::None);
        v.vz = Some(-2.0);
        assert_eq!(at(&mut t, &v, 8.0), Change::None);
        v.vz = Some(12.0);
        assert_eq!(at(&mut t, &v, 9.0), Change::Done);
        assert_eq!(t.step(), None);
        // Done stays done.
        assert_eq!(at(&mut t, &v, 10.0), Change::None);
    }

    #[test]
    fn cards_three_and_four_time_out_and_the_key_advances() {
        let mut t = Tutorial::new(true);
        let v = primed_view();
        assert_eq!(at(&mut t, &v, 0.0), Change::Card(1));
        assert_eq!(t.dismiss(0.5), Change::Card(2));
        assert_eq!(t.dismiss(0.6), Change::Card(3));
        assert_eq!(at(&mut t, &v, 10.0), Change::None);
        let four = 0.6 + CARD3_SECS + 0.01;
        assert_eq!(at(&mut t, &v, four), Change::Card(4));
        assert_eq!(at(&mut t, &v, four + 1.0), Change::None);
        assert_eq!(at(&mut t, &v, four + CARD4_SECS + 0.01), Change::Done);
        // Dismissing walks to the end too.
        let mut t = Tutorial::new(true);
        at(&mut t, &v, 0.0);
        for s in 2..=4 {
            assert_eq!(t.dismiss(1.0), Change::Card(s));
        }
        assert_eq!(t.dismiss(1.0), Change::Done);
        assert_eq!(t.dismiss(1.0), Change::None, "no card, key not taken");
        // Not to be shown: never shows.
        let mut off = Tutorial::new(false);
        assert_eq!(at(&mut off, &v, 0.0), Change::None);
        assert_eq!(off.dismiss(0.0), Change::None);
    }

    #[test]
    fn the_source_saves_once_when_the_last_card_closes() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let saved = Arc::new(AtomicU32::new(0));
        let s2 = Arc::clone(&saved);
        let src = Source::new(|| Some(primed_view())).with_tutorial(true, move || {
            s2.fetch_add(1, Ordering::Relaxed);
        });
        let f = src.frame(area(1280, 720)).unwrap();
        assert_eq!(f.card, Some(1));
        assert!(f.layout.card.is_some());
        for _ in 0..4 {
            assert!(src.dismiss());
        }
        assert!(!src.dismiss());
        assert_eq!(saved.load(Ordering::Relaxed), 1);
        assert_eq!(src.frame(area(1280, 720)).unwrap().card, None);
        assert!(Source::new(|| None).frame(area(1280, 720)).is_none());
    }

    #[test]
    fn frames_round_to_what_is_printed() {
        let a = area(1280, 720);
        let f = Frame::new(&primed_view(), None, 0.0, a);
        assert_eq!(f.launch, Some(17.9));
        assert_eq!(f.distance.as_deref(), Some("6.2 m"));
        assert_eq!(f.drop.as_deref(), Some("drop 1.1"));
        assert_eq!(f.arrow, Some(30));
        assert!(f.status.is_none() && !f.hint);
        // A hair's change draws the same image.
        let mut v = primed_view();
        v.nearest.as_mut().unwrap().launch = 17.86;
        v.bearing = Some(0.51);
        assert_eq!(Frame::new(&v, None, 0.0, a), f);
        // Primed in the air: the hint row.
        let air = View {
            airborne: true,
            ..primed_view()
        };
        assert!(Frame::new(&air, None, 0.0, a).hint);
        let grounded_unprimed = View {
            primed: false,
            airborne: true,
            ..primed_view()
        };
        assert!(!Frame::new(&grounded_unprimed, None, 0.0, a).hint);
        // Scanning spins; the pulse only runs primed; both step at most
        // ANIM_HZ times a second.
        let scan = View {
            scan: Scan::Scanning,
            ..primed_view()
        };
        let s0 = Frame::new(&scan, None, 0.0, a);
        assert_eq!(s0.status.as_deref(), Some("scanning map..."));
        assert!(s0.distance.is_none());
        assert_ne!(Frame::new(&scan, None, 0.5, a).spinner, s0.spinner);
        assert_eq!(Frame::new(&scan, None, 0.01, a).spinner, s0.spinner);
        let idle = View {
            primed: false,
            ..primed_view()
        };
        assert_eq!(Frame::new(&idle, None, 0.6, a).pulse, 0);
        let changes = (0..300)
            .map(|i| Frame::new(&primed_view(), None, i as f32 / 300.0, a).pulse)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count();
        assert!(changes as f32 <= ANIM_HZ + 1.0, "{changes} in a second");
        // No arrow without a bearing; failures say why.
        let blind = View {
            bearing: None,
            ..primed_view()
        };
        assert_eq!(Frame::new(&blind, None, 0.0, a).arrow, None);
        let failed = View {
            scan: Scan::Failed("no collision".into()),
            ..primed_view()
        };
        assert_eq!(
            Frame::new(&failed, None, 0.0, a).status.as_deref(),
            Some("no spots: no collision")
        );
    }

    #[test]
    fn the_image_is_glass_with_a_bright_edge_and_clear_corners() {
        let f = Frame::new(&primed_view(), Some(2), 0.0, area(1280, 720));
        let (w, h, px) = render(&f);
        assert_eq!((w, h), f.layout.size);
        let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        // The clipped top-left corner is outside the glass: only glow.
        let g = GLOW as u32;
        assert!(alpha(g + 1, g + 1) < 120, "{}", alpha(g + 1, g + 1));
        // Inside the panel, away from everything drawn: the glass.
        let glass = alpha(g + 120, g + 60 + 13);
        assert!(glass > 100, "{glass}");
        // The top edge is brighter than the glass under it.
        let blue = |x: u32, y: u32| px[((y * w + x) * 4 + 2) as usize];
        assert!(blue(g + 150, g) > blue(g + 150, g + 40));
        // The far corners of the image are clear.
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(w - 1, h - 1), 0);
    }

    #[test]
    fn the_preview_is_the_whole_frame_with_the_hud_over_it() {
        let size = (1280, 720);
        let px = preview(&primed_view(), None, 0.0, size);
        assert_eq!(px.len(), 1280 * 720 * 4);
        assert!(px.chunks(4).all(|p| p[3] == 255));
        let l = layout(area(1280, 720), false, false);
        // The left half is backdrop only: blue-grey, red below blue.
        let p = &px[((360 * 1280 + 100) * 4) as usize..][..4];
        assert!(p[0] < p[2]);
        // Inside the panel it is darker than the backdrop (the glass).
        let (x, y) = (l.at.0 + 140, l.at.1 + 8 + 74);
        let inside = &px[((y * 1280 + x) * 4) as usize..][..4];
        let backdrop = &px[((y * 1280 + 100) * 4) as usize..][..4];
        assert!(inside[0] < backdrop[0], "{inside:?} {backdrop:?}");
    }
}
