//! The superbounce step chain (docs/game/superbounce.md, "The step
//! chain"): a row of small panels low in the middle of the HUD, one a step
//! of a bounce. The first is the charge; primed, the next slides in beside
//! it with the live vertical speed against the speed the strip needs; in a
//! fall the last says whether the landing is on the strip, then what the
//! landing came to. The step to do is at full strength, a done one is
//! ticked and dimmed, a failed one is red.
//!
//! The panels are host marks ([`crate::superbounce_view::Shape`], drawn by
//! the window as `gfx::world_lines::HudMark`s) in the HUD's colour and
//! font, in the shape of the game's training box: a dark panel of the HUD
//! colour with cut corners, a border and a bevel. The HUD kit cannot draw
//! them: the game has one training box.
//!
//! [`chain`] is the rule (what each block says for a state), [`place`] and
//! [`shapes`] the layout, [`Track`] what runs on the wall clock (a block's
//! slide in, how long a landing's result stays), so the picture is the
//! same at any frame rate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::superbounce_charge::{GRAVITY, STANDING_JUMP_SPEED};
use crate::superbounce_mode::{
    ChargePhase, Course, LandingResult, METRES_PER_UNIT, NavKind, State, Verb,
};
use crate::superbounce_prompts::{FLAT_FALL, whole_metres};
use crate::superbounce_view::Shape;
use crate::superbounce_world::RED;

/// The feature's name (`H2_FEATURES` switches it off).
pub const FEATURE: &str = "superbounce-steps";

/// Seconds a block takes to slide in.
pub const SLIDE: f32 = 0.25;
/// How far left of its place a block starts, HUD pixels.
pub const SLIDE_FROM: f32 = 24.0;
/// Seconds a landing's result stays.
pub const RESULT_SECONDS: f32 = 2.5;
/// The chain's top, HUD pixels below the view's centre: under the reticle
/// and over the arming bar.
pub const TOP: f32 = 46.0;
/// A block's padding, the gap between blocks, the least width of a block,
/// its corners' cut and the room a tick or a cross takes, HUD pixels.
pub const PAD_X: f32 = 8.0;
pub const PAD_Y: f32 = 4.0;
pub const GAP: f32 = 12.0;
pub const MIN_WIDTH: f32 = 56.0;
pub const CUT: f32 = 4.0;
pub const MARK: f32 = 14.0;
/// The speed strip along a block's foot: its height and the room it takes.
pub const STRIP: f32 = 3.0;
pub const STRIP_ROOM: f32 = 6.0;
/// A done block's strength, and the panel's own under its text.
pub const DONE_ALPHA: f32 = 0.55;
pub const PANEL_ALPHA: f32 = 0.6;
/// The chevron between two blocks.
pub const CHEVRON_ALPHA: f32 = 0.7;
/// The panel is the HUD colour this dark.
pub const PANEL_SHADE: f32 = 0.25;

/// A step of a bounce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// Get the charge: crouch under a lip, let go, walk out.
    Prime,
    /// Fall fast enough for the strip.
    Speed,
    /// Land on the strip.
    Land,
}

/// How a block stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// The step to do: full strength.
    Current,
    /// Done: ticked and dimmed.
    Done,
    /// It went wrong: red, with a cross.
    Failed,
}

/// One block of the chain.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub step: Step,
    pub status: Status,
    /// The first line.
    pub title: String,
    /// The second line, what the step needs; empty for none.
    pub detail: String,
    /// The strip along the foot, 0 to 1; `None` for none.
    pub fill: Option<f32>,
}

impl Block {
    fn new(step: Step, status: Status, title: impl Into<String>) -> Block {
        Block {
            step,
            status,
            title: title.into(),
            detail: String::new(),
            fill: None,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Block {
        self.detail = detail.into();
        self
    }

    /// The text the block's width is measured from: the title, or for the
    /// live numbers their widest form, so the block does not change width
    /// as the digits do.
    pub fn widest_title(&self) -> &str {
        match (self.step, self.status) {
            (Step::Speed, _) => "Speed 00.0",
            (Step::Land, Status::Current) => "Steer 0.0 m",
            _ => &self.title,
        }
    }
}

/// Metres a second, one decimal, from world units a second.
fn metres(units: f32) -> String {
    format!("{:.1}", units * METRES_PER_UNIT)
}

/// The touchdown speed the strip ahead needs (units/s): the predicted
/// landing's strip's, else the navpoint's strip's worked from its fall.
/// `None` with neither.
pub fn need(s: &State) -> Option<f32> {
    if let Some(l) = &s.landing {
        return Some(l.need);
    }
    let n = s.nav_target.as_ref().filter(|n| n.kind == NavKind::Strip)?;
    let fall = if n.fall.is_finite() {
        n.fall.max(0.0)
    } else {
        0.0
    };
    Some((STANDING_JUMP_SPEED * STANDING_JUMP_SPEED + 2.0 * GRAVITY * fall).sqrt())
}

/// The player's speed downward now (units/s): 0 on the ground or rising.
pub fn falling_speed(s: &State) -> f32 {
    s.vz.filter(|v| s.airborne && v.is_finite())
        .map_or(0.0, |v| (-v).max(0.0))
}

fn prime_block(s: &State) -> Block {
    if s.primed() {
        let title = if s.verb == Some(Verb::KeepMoving) {
            "Primed: keep moving"
        } else {
            "Primed"
        };
        return Block::new(Step::Prime, Status::Done, title);
    }
    let charge = s.charge.as_ref();
    let detail = match charge.map(|c| c.phase) {
        Some(ChargePhase::Near) => "Hold crouch, walk under",
        Some(ChargePhase::Under) => "Let go of crouch",
        Some(ChargePhase::Held) => "Walk out to stand up",
        Some(ChargePhase::Lost) => "Charge lost. Crouch again",
        Some(ChargePhase::Primed) | None => "Crouch under a lip",
    };
    Block {
        fill: charge
            .filter(|c| c.phase != ChargePhase::Lost)
            .map(|c| c.fill.clamp(0.0, 1.0)),
        ..Block::new(Step::Prime, Status::Current, "Prime").detail(detail)
    }
}

fn speed_block(s: &State) -> Option<Block> {
    let need = need(s).filter(|n| n.is_finite() && *n > 0.0)?;
    let live = falling_speed(s);
    // In a fall, what counts is the speed at the strip.
    let at_strip = s.landing.as_ref().map_or(live, |l| l.speed);
    let fast = at_strip >= need;
    let fall = s.nav_target.as_ref().map_or(0.0, |n| n.fall);
    let detail = if fast {
        "Fast enough".to_owned()
    } else if s.airborne || s.landing.is_some() {
        format!("Need {} m/s", metres(need))
    } else if fall > FLAT_FALL {
        format!("Fall {} m for {}", whole_metres(fall), metres(need))
    } else {
        "A jump is enough".to_owned()
    };
    Some(Block {
        fill: Some((at_strip / need).clamp(0.0, 1.0)),
        ..Block::new(
            Step::Speed,
            if fast { Status::Done } else { Status::Current },
            format!("Speed {}", metres(live)),
        )
        .detail(detail)
    })
}

fn land_block(s: &State, result: Option<LandingResult>) -> Option<Block> {
    if let Some(r) = result {
        let (status, title) = match r {
            LandingResult::Bounce => (Status::Done, "Bounce!"),
            LandingResult::NotPrimed => (Status::Failed, "Not primed"),
            LandingResult::Missed => (Status::Failed, "Missed the strip"),
            LandingResult::TooSlow => (Status::Failed, "Too slow"),
            LandingResult::NoBounce => (Status::Failed, "No bounce"),
        };
        return Some(Block::new(Step::Land, status, title));
    }
    let l = s.landing.as_ref().filter(|_| s.primed())?;
    Some(match l.verdict {
        Course::OnSlot => Block::new(Step::Land, Status::Done, "On the strip"),
        Course::Fixable => Block::new(
            Step::Land,
            Status::Current,
            format!("Steer {:.1} m", l.miss() * METRES_PER_UNIT),
        )
        .detail("Onto the strip"),
        Course::Miss => Block::new(
            Step::Land,
            Status::Current,
            format!("Steer {:.1} m", l.miss() * METRES_PER_UNIT),
        )
        .detail("Too far to reach"),
    })
}

/// The chain for `s`, with a landing's `result` while it stays
/// ([`Track::result`]): the charge; primed, the speed; in a primed fall
/// toward a strip, or with a result, the landing. Empty with the mode off
/// or no player.
pub fn chain(s: &State, result: Option<LandingResult>) -> Vec<Block> {
    if !s.active || s.position.is_none() {
        return Vec::new();
    }
    let mut out = vec![prime_block(s)];
    if s.primed()
        && let Some(b) = speed_block(s)
    {
        out.push(b);
    }
    out.extend(land_block(s, result));
    out
}

/// The step to do: the first block that is not done.
pub fn current(blocks: &[Block]) -> Option<Step> {
    blocks
        .iter()
        .find(|b| b.status != Status::Done)
        .map(|b| b.step)
}

// ---- the wall clock ----

/// What the chain keeps between ticks, on the wall clock: when each block
/// came, and the last landing's result while it stays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// Each block shown and the instant it came.
    since: Vec<(Step, Instant)>,
    /// The last landing seen, by its tick.
    seen: Option<u32>,
    /// The state has been seen once: the landing it held then is not news.
    started: bool,
    result: Option<(LandingResult, Instant)>,
}

impl Track {
    /// The landing result to show at `now`: the last landing's, from the
    /// tick it is filed, for [`RESULT_SECONDS`] or until the next fall
    /// toward a strip begins.
    pub fn result(&mut self, s: &State, now: Instant) -> Option<LandingResult> {
        let landing = s.last_landing.as_ref();
        let tick = landing.map(|l| l.tick);
        if tick != self.seen {
            self.seen = tick;
            self.result = landing.filter(|_| self.started).map(|l| (l.result, now));
        }
        self.started = true;
        let stays = |at: Instant| {
            s.landing.is_none() && now.saturating_duration_since(at).as_secs_f32() < RESULT_SECONDS
        };
        self.result = self.result.filter(|(_, at)| stays(*at));
        self.result.map(|(r, _)| r)
    }

    /// Notes which `blocks` show at `now`: a block that was not shown
    /// starts its slide.
    pub fn show(&mut self, blocks: &[Block], now: Instant) {
        self.since
            .retain(|(s, _)| blocks.iter().any(|b| b.step == *s));
        for b in blocks {
            if !self.since.iter().any(|(s, _)| *s == b.step) {
                self.since.push((b.step, now));
            }
        }
    }

    /// When `step`'s block came; `None` when it is not shown.
    pub fn since(&self, step: Step) -> Option<Instant> {
        self.since.iter().find(|(s, _)| *s == step).map(|(_, t)| *t)
    }

    /// Forgets everything: the mode went off.
    pub fn clear(&mut self) {
        *self = Track::default();
    }
}

/// How far in a block that came at `since` is at `now`: 0 as it comes, 1
/// in place, smoothstep over [`SLIDE`] seconds.
pub fn level(since: Instant, now: Instant) -> f32 {
    let t = (now.saturating_duration_since(since).as_secs_f32() / SLIDE).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---- the layout ----

/// How text measures: a line's width and the height of a line, HUD
/// pixels.
#[derive(Clone, Copy)]
pub struct Metrics<'a> {
    pub width: &'a dyn Fn(&str) -> f32,
    pub line: f32,
}

/// A block's box, HUD pixels: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Whether a block has a tick or a cross before its title.
fn marked(b: &Block) -> bool {
    b.status != Status::Current
}

/// A block's size: its lines, padded, with room for its mark and its
/// strip.
pub fn size(b: &Block, m: &Metrics<'_>) -> [f32; 2] {
    let mark = if marked(b) { MARK } else { 0.0 };
    let title = (m.width)(b.widest_title()) + mark;
    let detail = if b.detail.is_empty() {
        0.0
    } else {
        (m.width)(&b.detail)
    };
    let lines = if b.detail.is_empty() { 1.0 } else { 2.0 };
    let strip = if b.fill.is_some() { STRIP_ROOM } else { 0.0 };
    [
        (title.max(detail) + 2.0 * PAD_X).max(MIN_WIDTH),
        lines * m.line + 2.0 * PAD_Y + strip,
    ]
}

/// Where each block goes: one row, [`GAP`] apart, the row centred under
/// `centre` with its top [`TOP`] below it, each block centred on the
/// row's middle line.
pub fn place(blocks: &[Block], m: &Metrics<'_>, centre: [f32; 2]) -> Vec<Rect> {
    let sizes: Vec<[f32; 2]> = blocks.iter().map(|b| size(b, m)).collect();
    let total: f32 =
        sizes.iter().map(|s| s[0]).sum::<f32>() + GAP * sizes.len().saturating_sub(1) as f32;
    let tallest = sizes.iter().map(|s| s[1]).fold(0.0, f32::max);
    let mut x = centre[0] - total / 2.0;
    sizes
        .iter()
        .map(|&[w, h]| {
            let r = Rect {
                x,
                y: centre[1] + TOP + (tallest - h) / 2.0,
                w,
                h,
            };
            x += w + GAP;
            r
        })
        .collect()
}

fn scaled(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| (v * k).clamp(0.0, 1.0))
}

/// `c` moved `k` of the way to white.
fn lightened(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| (v + (1.0 - v) * k).clamp(0.0, 1.0))
}

/// A box with its corners cut by [`CUT`], clockwise from the top left.
pub fn panel(r: &Rect) -> Vec<[f32; 2]> {
    let c = CUT.min(r.w / 2.0).min(r.h / 2.0);
    vec![
        [r.x + c, r.y],
        [r.right() - c, r.y],
        [r.right(), r.y + c],
        [r.right(), r.bottom() - c],
        [r.right() - c, r.bottom()],
        [r.x + c, r.bottom()],
        [r.x, r.bottom() - c],
        [r.x, r.y + c],
    ]
}

/// One block at `r`, `level` in (0 to 1), in the HUD's `colour`.
fn block_shapes(
    b: &Block,
    r: &Rect,
    level: f32,
    colour: [f32; 3],
    line: f32,
    out: &mut Vec<Shape>,
) {
    let level = level.clamp(0.0, 1.0);
    if level <= 0.0 {
        return;
    }
    let r = Rect {
        x: r.x - (1.0 - level) * SLIDE_FROM,
        ..*r
    };
    let strength = match b.status {
        Status::Current => 1.0,
        Status::Done => DONE_ALPHA,
        Status::Failed => 0.9,
    } * level;
    let ink = if b.status == Status::Failed {
        RED
    } else {
        colour
    };
    let path = |points: Vec<[f32; 2]>, closed: bool, width: f32, color: [f32; 3], alpha: f32| {
        Shape::Path {
            points,
            closed,
            width,
            dashed: false,
            color,
            alpha,
        }
    };
    // The panel, its bevel (light along the top, dark along the foot) and
    // its border.
    let c = CUT.min(r.w / 2.0).min(r.h / 2.0);
    out.push(Shape::Fill {
        points: panel(&r),
        color: scaled(colour, PANEL_SHADE),
        alpha: PANEL_ALPHA * level,
    });
    out.push(path(
        vec![[r.x + c, r.y + 1.5], [r.right() - c, r.y + 1.5]],
        false,
        1.0,
        lightened(ink, 0.5),
        0.5 * strength,
    ));
    out.push(path(
        vec![
            [r.x + c, r.bottom() - 1.5],
            [r.right() - c, r.bottom() - 1.5],
        ],
        false,
        1.0,
        scaled(ink, 0.4),
        0.8 * strength,
    ));
    let border = if b.status == Status::Current {
        1.5
    } else {
        1.0
    };
    out.push(path(panel(&r), true, border, ink, strength));
    // The title, after its tick or cross.
    let mark = if marked(b) { MARK } else { 0.0 };
    let title_y = r.y + PAD_Y + line / 2.0;
    let middle = r.x + r.w / 2.0;
    if marked(b) {
        let (x, y) = (r.x + PAD_X, title_y);
        if b.status == Status::Done {
            out.push(path(
                vec![[x, y], [x + 3.0, y + 3.5], [x + 9.0, y - 4.0]],
                false,
                1.5,
                ink,
                strength,
            ));
        } else {
            for (y0, y1) in [(y - 4.0, y + 4.0), (y + 4.0, y - 4.0)] {
                out.push(path(
                    vec![[x, y0], [x + 8.0, y1]],
                    false,
                    1.5,
                    ink,
                    strength,
                ));
            }
        }
    }
    out.push(Shape::Text {
        at: [middle + mark / 2.0, title_y],
        text: b.title.clone(),
        color: ink,
        alpha: strength,
        pixel: 1.0,
    });
    if !b.detail.is_empty() {
        out.push(Shape::Text {
            at: [middle, title_y + line],
            text: b.detail.clone(),
            color: ink,
            alpha: 0.8 * strength,
            pixel: 1.0,
        });
    }
    // The strip along the foot: its track, then what is filled.
    if let Some(fill) = b.fill {
        let (x0, x1) = (r.x + PAD_X, r.right() - PAD_X);
        let y1 = r.bottom() - PAD_Y;
        let bar = |to: f32| vec![[x0, y1 - STRIP], [to, y1 - STRIP], [to, y1], [x0, y1]];
        out.push(Shape::Fill {
            points: bar(x1),
            color: ink,
            alpha: 0.25 * strength,
        });
        let to = x0 + (x1 - x0) * fill.clamp(0.0, 1.0);
        if to > x0 {
            out.push(Shape::Fill {
                points: bar(to),
                color: ink,
                alpha: strength,
            });
        }
    }
}

/// The chain as shapes, before the host's HUD fit: each block at its
/// place and level, and a chevron in each gap pointing at the next block.
pub fn shapes(
    blocks: &[Block],
    levels: &[f32],
    m: &Metrics<'_>,
    centre: [f32; 2],
    colour: [f32; 3],
) -> Vec<Shape> {
    let rects = place(blocks, m, centre);
    let mut out = Vec::new();
    let level = |i: usize| levels.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
    for (i, (b, r)) in blocks.iter().zip(&rects).enumerate() {
        block_shapes(b, r, level(i), colour, m.line, &mut out);
        if i + 1 < rects.len() && level(i + 1) > 0.0 {
            let x = r.right() + GAP / 2.0;
            let y = r.y + r.h / 2.0;
            out.push(Shape::Path {
                points: vec![[x - 2.0, y - 4.0], [x + 2.0, y], [x - 2.0, y + 4.0]],
                closed: false,
                width: 1.5,
                dashed: false,
                color: colour,
                alpha: CHEVRON_ALPHA * level(i + 1),
            });
        }
    }
    out
}

// ---- the feature ----

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether the chain is drawn: the `superbounce-steps` feature is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The feature: a switch for the chain. The drawing is the window's
/// ([`Source`]); it has no guest hooks.
pub struct SuperbounceSteps;

impl hooks::Feature for SuperbounceSteps {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce step chain on the HUD")
            .doc("docs/game/superbounce.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Rebuilt from the mode's state at each capture.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature to `registry`; `H2_FEATURES=-superbounce-steps` leaves
/// the chain out.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let on = registry
        .add(&SuperbounceSteps)
        .map_err(|e| anyhow::anyhow!("superbounce steps: {e}"))?;
    ENABLED.store(on, Ordering::Relaxed);
    Ok(())
}

/// The window's source for the chain (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::Source;

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::{Arc, Mutex};

    use gfx::world_lines::{HudMark, ImageMarks, LabelFont};

    use super::*;

    /// The 8x8 debug font's cell, for a host with no HUD font.
    const DEBUG_CELL: f32 = 8.0;

    static TRACK: Mutex<Track> = Mutex::new(Track {
        since: Vec::new(),
        seen: None,
        started: false,
        result: None,
    });

    /// The step chain while the mode is on.
    pub struct Source;

    impl crate::world_lines::WorldLineSource for Source {
        fn active(&self) -> bool {
            enabled() && crate::superbounce_mode::active()
        }

        fn capture(&self, guest: &dyn engine::GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let now = Instant::now();
            let mut track = TRACK.lock().unwrap_or_else(|e| e.into_inner());
            let Some(s) = crate::superbounce_mode::hud_state().filter(|_| enabled()) else {
                track.clear();
                return None;
            };
            let result = track.result(&s, now);
            let blocks = chain(&s, result);
            track.show(&blocks, now);
            if blocks.is_empty() {
                return None;
            }
            let since = blocks
                .iter()
                .map(|b| track.since(b.step).unwrap_or(now))
                .collect();
            let (centre, fit) = crate::render_aspect::hud_centre_fit();
            Some(Arc::new(Sample {
                blocks,
                since,
                centre,
                fit,
                colour: crate::superbounce_font::hud_colour(guest).unwrap_or([1.0; 3]),
                font: gfx::world_lines::hud_font(),
            }))
        }
    }

    #[derive(Debug)]
    struct Sample {
        blocks: Vec<Block>,
        /// When each block came.
        since: Vec<Instant>,
        centre: [f32; 2],
        fit: [f32; 4],
        colour: [f32; 3],
        font: Option<Arc<LabelFont>>,
    }

    impl ImageMarks for Sample {
        fn lines(&self, _visitor: &mut dyn FnMut(gfx::world_lines::WorldLine<'_>)) {}

        fn hud(&self, now: &dyn Fn() -> Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
            let now = now();
            let levels: Vec<f32> = self.since.iter().map(|&t| level(t, now)).collect();
            let width = |text: &str| match &self.font {
                Some(f) => f.layout(text).width as f32,
                None => DEBUG_CELL * text.chars().count() as f32,
            };
            let metrics = Metrics {
                width: &width,
                line: self
                    .font
                    .as_ref()
                    .map_or(DEBUG_CELL + 2.0, |f| f.line_height() as f32),
            };
            for shape in shapes(&self.blocks, &levels, &metrics, self.centre, self.colour) {
                crate::superbounce_view::emit(&shape.fitted(self.fit), visitor);
            }
        }
    }
}

#[cfg(test)]
#[path = "superbounce_steps_tests.rs"]
mod tests;
