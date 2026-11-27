//! The Firefight HUD's layout (docs/game/firefight.md, "The HUD"): where
//! each element sits in a viewer's viewport, from the mode blob alone, and
//! the checks that no element sits on one of Halo 2's own.
//!
//! Halo 2 keeps the weapon top right, the grenades top left, the shield and
//! tracker bottom left and the game type's score plate bottom right. The
//! Firefight block sits above the plate at the bottom right, right-justified.
//! What it shows follows Halo 3: ODST's Firefight HUD: a lives count, the
//! set's numeral, a circle a round and a bar a wave, lit from the left, the
//! skulls on, and a centred line for what begins.
//! [`layout`] is a pure function of a [`Mode`], the viewer's roster slot,
//! the [`Scope`] and the [`Screen`]; the HUD kit's `firefight` HUD draws
//! these elements and its checks call [`check`].
//!
//! Every number here that the retail HUD decides (the rectangles, the plate's
//! height, the character width) is assumed from the census, marked **V**
//! (to verify), and held in one constant so a measurement changes one line.

use ::firefight::Skull;

use super::{Mode, Scope, lives_of};

/// A text line's height.
pub const LINE: i16 = 14;
/// A text character's advance in the HUD font (**V**).
pub const CHAR_W: i16 = 8;
/// The retail score plate's top edge above the frame's bottom (**V**).
pub const PLATE_TOP: i16 = 44;
/// The retail plate already shows the team's total, so the HUD draws no
/// score line of its own (**V**).
pub const PLATE_SHOWS_TOTAL: bool = false;
/// The enemies meter's size.
/// The battery strip the kit draws the meter with is 92 by 8
/// (`hud_kit_core::views::firefight`, docs/game/hud-kit.md, "The Firefight HUD").
pub const METER_W: i16 = 92;
pub const METER_H: i16 = 8;
/// The lives line at one life: it blinks as it appears.
pub const LAST_LIFE: &str = "LAST LIFE";
/// The largest points and lives the lines show.
pub const SCORE_MAX: u32 = 9_999_999;
pub const LIVES_MAX: u16 = 999;
/// The widest banner, in characters: the longest skull name and " ON".
pub const BANNER_CHARS: usize = 23;
/// The skull slots: one per skull, by slot, fixed for the HUD's life.
/// Slot 6 is Iron, which has no session skull yet.
pub const SKULL_SLOTS: [Option<Skull>; 7] = [
    Some(Skull::Ghost),
    Some(Skull::Catch),
    Some(Skull::BlackEye),
    Some(Skull::Thunderstorm),
    Some(Skull::Famine),
    Some(Skull::Mythic),
    None,
];
/// The elements' ids: the lines, the meter, the seven skull slots, the banner.
pub const SCORE: &str = "score";
pub const LIVES: &str = "lives";
pub const PROGRESS: &str = "progress";
pub const ENEMIES: &str = "enemies";
pub const BANNER: &str = "banner";
pub const SKULL_IDS: [&str; 7] = [
    "skull_0", "skull_1", "skull_2", "skull_3", "skull_4", "skull_5", "skull_6",
];
/// The set's numeral, the round pips (a set's three) and the wave pips (a
/// round's five), left to right.
pub const SET: &str = "set";
pub const ROUND_IDS: [&str; 3] = ["round_0", "round_1", "round_2"];
pub const WAVE_IDS: [&str; 5] = ["wave_0", "wave_1", "wave_2", "wave_3", "wave_4"];
/// A pip's height, a round's width and pitch, a wave's width and pitch,
/// and the gap between the numeral, the rounds and the waves
/// (`font::skulls::Pip`, `content/modes/huds/firefight.hud.yaml`).
pub const PIP_H: i16 = 10;
pub const ROUND_PIP: (i16, i16) = (8, 9);
pub const WAVE_PIP: (i16, i16) = (6, 7);
pub const PIP_GAP: i16 = 6;
/// How far above the frame's bottom the game draws its timed centred
/// line, the banner (`hud_kit::preview::BANNER_UP`; E `0x0024d554`).
pub const BANNER_UP: i16 = 40;

/// A rectangle in viewport pixels, from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
}

impl Rect {
    pub const fn new(x: i16, y: i16, w: i16, h: i16) -> Rect {
        Rect { x, y, w, h }
    }
    pub const fn right(&self) -> i16 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i16 {
        self.y + self.h
    }
    /// The two share area; touching edges do not.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.w > 0
            && self.h > 0
            && o.w > 0
            && o.h > 0
            && self.x < o.right()
            && o.x < self.right()
            && self.y < o.bottom()
            && o.y < self.bottom()
    }
    /// The whole of `self` is inside `outer`.
    pub fn inside(&self, outer: &Rect) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom()
    }
}

/// The viewport a viewer has: the whole screen, a split-screen half or a
/// quarter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// 640x480.
    Full,
    /// 640x240: two players.
    Half,
    /// 320x240: three or four players.
    Quarter,
}

impl Screen {
    pub const ALL: [Screen; 3] = [Screen::Full, Screen::Half, Screen::Quarter];

    /// The retail frame (the safe area the HUD draws in): x 48..592,
    /// y 36..444 at full size, scaled with the viewport.
    pub fn frame(self) -> Rect {
        match self {
            Screen::Full => Rect::new(48, 36, 544, 408),
            Screen::Half => Rect::new(48, 18, 544, 204),
            Screen::Quarter => Rect::new(24, 18, 272, 204),
        }
    }

    /// Divisors of the full-size viewport's x and y.
    fn shrink(self) -> (i16, i16) {
        match self {
            Screen::Full => (1, 1),
            Screen::Half => (1, 2),
            Screen::Quarter => (2, 2),
        }
    }

    /// Halo 2's own HUD pieces the Firefight block stays off, by name
    /// (**V**: the census's rectangles at full size, scaled with the
    /// viewport; the plate is [`PLATE_TOP`] high at every size).
    pub fn retail(self) -> [(&'static str, Rect); 4] {
        let (sx, sy) = self.shrink();
        let f = self.frame();
        let scaled = |x: i16, y: i16, w: i16, h: i16| Rect::new(x / sx, y / sy, w / sx, h / sy);
        let plate_w = 200 / sx;
        [
            ("weapon", scaled(424, 36, 168, 64)),
            ("grenades", scaled(48, 36, 168, 44)),
            ("tracker", scaled(48, 344, 152, 100)),
            (
                "plate",
                Rect::new(f.right() - plate_w, f.bottom() - PLATE_TOP, plate_w, PLATE_TOP),
            ),
        ]
    }

    /// The screen of `users` local players: one has the whole screen, two
    /// a half each, three or four a quarter each.
    pub fn for_users(users: u32) -> Screen {
        match users {
            0 | 1 => Screen::Full,
            2 => Screen::Half,
            _ => Screen::Quarter,
        }
    }

    /// Whether the whole block is drawn: a quarter drops the score, the
    /// meter, the skulls and the banner, and keeps the lines and the pips.
    fn small(self) -> bool {
        self == Screen::Quarter
    }
}

/// The slot of skull slot `i`: fixed for the screen, whatever the state.
/// None in a quarter, which draws no skulls.
pub fn skull_slot(i: usize, screen: Screen) -> Option<Rect> {
    let f = screen.frame();
    let (size, pitch, up) = match screen {
        Screen::Full => (16, 18, 82),
        Screen::Half => (14, 14, 80),
        Screen::Quarter => return None,
    };
    let right = f.right() - pitch * i as i16;
    Some(Rect::new(right - size, f.bottom() - PLATE_TOP - up, size, size))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Meter,
    Gauge,
    /// The game's own timed centred line: the game places it, so it has a
    /// point and no extent here.
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Normal,
    /// The warning style: no lives left.
    Warning,
}

/// One element for one viewer. A hidden element keeps its rectangle: the
/// skull slots and the meter never move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: &'static str,
    pub kind: Kind,
    pub rect: Rect,
    pub shown: bool,
    /// A text element's words.
    pub text: String,
    /// A meter's value and full length.
    pub fill: (u16, u16),
    pub look: Look,
}

/// A line of `text` right-justified above the plate, `up` pixels (to its
/// top) above the plate's top.
fn line(id: &'static str, text: String, up: i16, f: Rect, shown: bool, look: Look) -> Element {
    let w = text.chars().count() as i16 * CHAR_W;
    Element {
        id,
        kind: Kind::Text,
        rect: Rect::new(f.right() - w, f.bottom() - PLATE_TOP - up, w, LINE),
        shown,
        text,
        fill: (0, 0),
        look,
    }
}

/// `n` capped to what a line shows.
fn capped(n: u32, max: u32) -> u32 {
    n.min(max)
}

/// The score line's words: the team's points, or each team's.
pub fn score_text(m: &Mode) -> String {
    match &m.teams {
        None => format!("SCORE {}", capped(m.score, SCORE_MAX)),
        Some(t) => format!(
            "RED {} BLUE {}",
            capped(t.scores[0], SCORE_MAX),
            capped(t.scores[1], SCORE_MAX)
        ),
    }
}

/// The lives line's words and look for `n` lives.
pub fn lives_text(n: u16) -> (String, Look) {
    match n {
        0 => ("NO LIVES".into(), Look::Warning),
        1 => (LAST_LIFE.into(), Look::Normal),
        n => (format!("LIVES {}", n.min(LIVES_MAX)), Look::Normal),
    }
}

/// The most rounds the round pips show: a set's three. A definition with
/// more names its round on the progress line.
pub const ROUND_PIPS: u8 = ROUND_IDS.len() as u8;

/// The progress line's words, for what the pips cannot say: the bonus
/// round's clock, or the round of a definition with more rounds than
/// there are round pips. None otherwise.
pub fn progress_text(m: &Mode) -> Option<String> {
    if m.bonus_left > 0 {
        let secs = u32::from(m.bonus_left).div_ceil(super::banners::TICKS_PER_SECOND.into());
        return Some(format!("BONUS {}:{:02}", secs / 60, secs % 60));
    }
    (m.rounds > ROUND_PIPS).then(|| format!("ROUND {} OF {}", u16::from(m.round) + 1, m.rounds))
}

/// A pip `w` wide with its left edge `left` of the frame's right edge,
/// `up` above the plate: shown, and lit for the one on and those before.
fn pip(id: &'static str, f: Rect, left: i16, w: i16, up: i16, shown: bool, lit: bool) -> Element {
    Element {
        id,
        kind: Kind::Gauge,
        rect: Rect::new(f.right() - left, f.bottom() - PLATE_TOP - up, w, PIP_H),
        shown,
        text: String::new(),
        fill: (u16::from(lit), 1),
        look: Look::Normal,
    }
}

/// The elements for `m` as the player at roster slot `local` sees them in
/// `scope` on `screen`, with the banner's words if one is up. Always the
/// same elements, in [`IDS`] order; a hidden one keeps its place.
pub fn layout(
    m: &Mode,
    local: u8,
    scope: Scope,
    screen: Screen,
    banner: Option<&str>,
) -> Vec<Element> {
    let f = screen.frame();
    let small = screen.small();
    let block = scope != Scope::Monitor;
    let on = !m.stage.over();
    let (lives_up, progress_up) = if small { (14, 28) } else { (28, 42) };
    let mut out = Vec::with_capacity(IDS.len());

    out.push(line(
        SCORE,
        score_text(m),
        14,
        f,
        block && !small && !PLATE_SHOWS_TOTAL,
        Look::Normal,
    ));
    let own = lives_of(m, local).filter(|_| block && scope == Scope::Match && on);
    let (words, look) = lives_text(own.unwrap_or(0));
    out.push(line(LIVES, words, lives_up, f, own.is_some(), look));
    let words = progress_text(m);
    out.push(line(
        PROGRESS,
        words.clone().unwrap_or_default(),
        progress_up,
        f,
        block && on && words.is_some(),
        Look::Normal,
    ));

    let bonus = m.bonus_left > 0;
    let wave_on = m.stage.wave_on();
    out.push(Element {
        id: ENEMIES,
        kind: Kind::Meter,
        rect: Rect::new(
            f.right() - METER_W,
            f.bottom() - PLATE_TOP - 50,
            METER_W,
            METER_H,
        ),
        shown: block && !small && (wave_on || bonus),
        text: String::new(),
        fill: if bonus {
            (m.bonus_left.min(m.total), m.total)
        } else if wave_on {
            (m.left.min(m.total), m.total)
        } else {
            (0, m.total)
        },
        look: Look::Normal,
    });

    for (i, slot) in SKULL_SLOTS.iter().enumerate() {
        let Some(rect) = skull_slot(i, screen) else {
            continue;
        };
        out.push(Element {
            id: SKULL_IDS[i],
            kind: Kind::Gauge,
            rect,
            shown: block && slot.is_some_and(|s| m.skulls & (1 << s as u8) != 0),
            text: String::new(),
            fill: (0, 0),
            look: Look::Normal,
        });
    }

    // The game's timed line: centred across the frame, the game's own
    // distance above its bottom.
    out.push(Element {
        id: BANNER,
        kind: Kind::Line,
        rect: Rect::new(f.x + f.w / 2, f.bottom() - BANNER_UP, 0, 0),
        shown: banner.is_some() && !small,
        text: banner.unwrap_or("").to_owned(),
        fill: (0, 0),
        look: Look::Normal,
    });

    // The set's numeral, the rounds and the waves, between the meter and
    // the skulls (one line lower in a quarter). A bonus round has no wave.
    let pips = block && on;
    let up = if small { 40 } else { 64 };
    let waves_left = WAVE_PIP.0 + WAVE_PIP.1 * (WAVE_IDS.len() as i16 - 1);
    let rounds_left = waves_left + PIP_GAP + ROUND_PIP.0 + ROUND_PIP.1 * (ROUND_IDS.len() as i16 - 1);
    let numeral = (u16::from(m.set) + 1).to_string();
    let w = numeral.chars().count() as i16 * CHAR_W;
    out.push(Element {
        id: SET,
        kind: Kind::Text,
        rect: Rect::new(
            f.right() - rounds_left - PIP_GAP - w,
            f.bottom() - PLATE_TOP - up - 2,
            w,
            LINE,
        ),
        shown: pips && m.endless,
        text: numeral,
        fill: (0, 0),
        look: Look::Normal,
    });
    for (i, id) in ROUND_IDS.into_iter().enumerate() {
        let i = i as u8;
        let shown = pips && m.rounds <= ROUND_PIPS && i < m.rounds;
        let left = rounds_left - ROUND_PIP.1 * i16::from(i);
        out.push(pip(id, f, left, ROUND_PIP.0, up, shown, i <= m.round));
    }
    for (i, id) in WAVE_IDS.into_iter().enumerate() {
        let i = i as u8;
        let shown = pips && !bonus && i < m.waves;
        let left = waves_left - WAVE_PIP.1 * i16::from(i);
        out.push(pip(id, f, left, WAVE_PIP.0, up, shown, i <= m.wave));
    }
    out
}

/// What is wrong with `elements` on `screen`: an element outside the
/// frame, on one of Halo 2's own pieces or on another, a repeated id, a
/// skull off its slot. Empty when the layout is sound.
pub fn check(screen: Screen, elements: &[Element]) -> Vec<String> {
    let mut bad = Vec::new();
    let f = screen.frame();
    for (i, e) in elements.iter().enumerate() {
        if elements[..i].iter().any(|o| o.id == e.id) {
            bad.push(format!("{}: id repeated", e.id));
        }
        // A hidden line has no extent; a hidden meter or skull keeps its slot.
        let placed = e.shown || e.kind != Kind::Text;
        if !placed {
            continue;
        }
        if !e.rect.inside(&f) {
            bad.push(format!("{}: {:?} outside the frame {f:?}", e.id, e.rect));
        }
        for (name, r) in screen.retail() {
            if e.rect.overlaps(&r) {
                bad.push(format!("{}: {:?} overlaps the retail {name} {r:?}", e.id, e.rect));
            }
        }
        for o in &elements[..i] {
            if (o.shown || o.kind != Kind::Text) && e.rect.overlaps(&o.rect) {
                bad.push(format!("{} overlaps {}", e.id, o.id));
            }
        }
        if let Some(n) = SKULL_IDS.iter().position(|id| *id == e.id) {
            if skull_slot(n, screen) != Some(e.rect) {
                bad.push(format!("{}: off its slot", e.id));
            }
        }
    }
    bad
}

/// The ids, in the order [`layout`] makes them; a quarter has no skull
/// slots.
pub const IDS: [&str; 21] = [
    SCORE,
    LIVES,
    PROGRESS,
    ENEMIES,
    SKULL_IDS[0],
    SKULL_IDS[1],
    SKULL_IDS[2],
    SKULL_IDS[3],
    SKULL_IDS[4],
    SKULL_IDS[5],
    SKULL_IDS[6],
    BANNER,
    SET,
    ROUND_IDS[0],
    ROUND_IDS[1],
    ROUND_IDS[2],
    WAVE_IDS[0],
    WAVE_IDS[1],
    WAVE_IDS[2],
    WAVE_IDS[3],
    WAVE_IDS[4],
];
