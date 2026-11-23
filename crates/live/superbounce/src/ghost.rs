//! Ghost runs: a recorded run that bounced off a spot, kept to be shown
//! to a player (docs/game/superbounce.md, "Ghost runs").
//!
//! A run is one player's ticks from the start of priming (the crouch that
//! primed the word, with a second before it) to the top of the bounce:
//! per tick the origin, the facing, and what the player did ([`Frame`]).
//! The game records them from the bots' practice and the verifier's trials
//! (`game::superbounce_ghost`) and keeps the best run of each bounce spot
//! in the map's ghosts file, `<map>.ghosts` beside the spots and verified
//! files ([`to_text`], [`parse`]). The file is text, versioned by its
//! first line:
//!
//! ```text
//! # superbounce ghosts v2
//! ghost -10.620 3.880 -3.190 source=bot launch=12.31 touchdown=212 frames=230
//! f -10.910 0.720 -1.160 -121.0 c
//! f -10.905 0.718 -1.160 -120.5 ca
//! ```
//!
//! A `ghost` line gives the spot's `stand` (what a run is matched to a
//! spot by, as a verified entry is: [`crate::verified::MATCH`]), then
//! `key=value` fields: `source` (`bot`, `verify` or `player`), `launch`
//! (units/s up after the bounce), `touchdown` (the frame of the bounce
//! tick) and `frames` (how many `f` lines follow). Each `f` line is one
//! tick: x, y, z, the facing in degrees, and its flags as letters, `-`
//! for none: `c` crouch held (the unit's crouch request), `j` the jump's
//! take-off, `a` airborne, `p` primed (the level pair's word at -3).
//! Blank lines and other `#` lines are comments; unknown keys are
//! skipped. Every position, the spot's and each frame's, is the unit's
//! origin (object `+0x64`). A file of another version is refused, not
//! read; one of an older version ([`older`]) is not converted, and the
//! next run kept replaces it.

use crate::Vec3;
use crate::verified::{MATCH, MATCH_Z};

/// The file format's version; [`parse`] refuses any other. It rises when
/// what a line means changes (v2: every position is the origin).
pub const VERSION: u32 = 2;
/// The first line, before the version number.
pub const HEADER_PREFIX: &str = "# superbounce ghosts v";
/// Game ticks a second: the guest tick is 30 Hz at every display rate
/// (docs/game/high-framerate.md).
pub const TICK_HZ: f32 = 30.0;
/// The most frames a run may have: a minute.
pub const MAX_FRAMES: usize = 1_800;

/// [`Frame::flags`]: the unit's crouch request (unit flags bit 23) is set.
pub const CROUCH: u8 = 1;
/// The tick the run left the ground in a jump.
pub const JUMP: u8 = 2;
/// The biped is off the ground.
pub const AIRBORNE: u8 = 4;
/// The level pair's word is -3.
pub const PRIMED: u8 = 8;

/// The first line of a file of this version.
pub fn header() -> String {
    format!("{HEADER_PREFIX}{VERSION}")
}

/// One tick of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// The unit's origin, world units.
    pub at: Vec3,
    /// Facing: the aim's yaw, radians from +X toward +Y.
    pub yaw: f32,
    pub flags: u8,
}

impl Frame {
    pub fn crouch(&self) -> bool {
        self.flags & CROUCH != 0
    }
    pub fn jump(&self) -> bool {
        self.flags & JUMP != 0
    }
    pub fn airborne(&self) -> bool {
        self.flags & AIRBORNE != 0
    }
    pub fn primed(&self) -> bool {
        self.flags & PRIMED != 0
    }
}

/// Who made a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Source {
    /// A bot practising (docs/game/bots.md, "Practising bounces").
    #[default]
    Bot,
    /// A `superbounce.verify` trial.
    Verify,
    /// A player.
    Player,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Bot => "bot",
            Source::Verify => "verify",
            Source::Player => "player",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "bot" => Some(Source::Bot),
            "verify" => Some(Source::Verify),
            "player" => Some(Source::Player),
            _ => None,
        }
    }
}

/// A button the run pressed, for the glyph shown where it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    /// The crouch request went on.
    Crouch,
    /// The jump's take-off.
    Jump,
}

/// A press: the frame and the button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub frame: usize,
    pub button: Button,
}

/// One recorded run that bounced off a spot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ghost {
    /// The bounce spot's `stand`, as the scan gave it.
    pub spot: Vec3,
    pub source: Source,
    /// The vertical speed out of the bounce, units/s.
    pub launch: f32,
    /// The frame of the bounce tick: the one whose vertical speed is the
    /// launch. The frame before it is the touchdown's origin.
    pub touchdown: usize,
    pub frames: Vec<Frame>,
}

/// Whether `a` and `b` are the same spot: within [`MATCH`] across and
/// [`MATCH_Z`] up, as verified entries match.
pub fn same_spot(a: Vec3, b: Vec3) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= MATCH && (a[2] - b[2]).abs() <= MATCH_Z
}

impl Ghost {
    /// Game ticks from the first frame to the last.
    pub fn ticks(&self) -> u32 {
        self.frames.len().saturating_sub(1) as u32
    }

    /// Seconds from the first frame to the last.
    pub fn seconds(&self) -> f32 {
        self.ticks() as f32 / TICK_HZ
    }

    /// The run primed itself: a crouch before the touchdown. A run whose
    /// word was set for it (a verifier's drop) has none.
    pub fn crouch_primed(&self) -> bool {
        self.frames.iter().take(self.touchdown).any(Frame::crouch)
    }

    /// Where it touched down: the origin on the tick before the bounce.
    pub fn landing(&self) -> Vec3 {
        let i = self.touchdown.saturating_sub(1);
        self.frames.get(i).map_or(self.spot, |f| f.at)
    }

    /// Whether this run is of the spot standing at `stand`.
    pub fn is_of(&self, stand: Vec3) -> bool {
        same_spot(self.spot, stand)
    }

    /// The buttons it pressed: each tick the crouch request went on, and
    /// each jump's take-off, in order.
    pub fn presses(&self) -> Vec<Press> {
        let mut out = Vec::new();
        let mut was_crouched = false;
        for (i, f) in self.frames.iter().enumerate() {
            if f.crouch() && !was_crouched {
                out.push(Press {
                    frame: i,
                    button: Button::Crouch,
                });
            }
            was_crouched = f.crouch();
            if f.jump() {
                out.push(Press {
                    frame: i,
                    button: Button::Jump,
                });
            }
        }
        out
    }

    /// Why it cannot be kept, if it cannot: no frames, too many, the bounce
    /// outside them or first, or a position that is not a number.
    pub fn fault(&self) -> Option<&'static str> {
        if self.frames.is_empty() {
            return Some("no frames");
        }
        if self.frames.len() > MAX_FRAMES {
            return Some("longer than a minute");
        }
        if self.touchdown == 0 || self.touchdown >= self.frames.len() {
            return Some("the bounce is not inside the run");
        }
        let finite = self
            .frames
            .iter()
            .all(|f| f.at.iter().all(|v| v.is_finite()) && f.yaw.is_finite());
        if !finite || !self.spot.iter().all(|v| v.is_finite()) || !self.launch.is_finite() {
            return Some("a number that is not finite");
        }
        None
    }

    /// Whether this run beats `other` (of the same spot): a run that
    /// primed itself beats one whose word was set for it, as it shows the
    /// whole recipe; then the shorter wins; then the higher launch.
    pub fn better_than(&self, other: &Ghost) -> bool {
        let rank = |g: &Ghost| (!g.crouch_primed(), g.ticks());
        match rank(self).cmp(&rank(other)) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            std::cmp::Ordering::Equal => self.launch > other.launch,
        }
    }
}

/// The run of the spot standing at `stand`, if any (the nearest when
/// several match).
pub fn for_spot(ghosts: &[Ghost], stand: Vec3) -> Option<(usize, &Ghost)> {
    ghosts
        .iter()
        .enumerate()
        .filter(|(_, g)| g.is_of(stand))
        .min_by(|a, b| {
            let d = |g: &Ghost| (g.spot[0] - stand[0]).hypot(g.spot[1] - stand[1]);
            d(a.1).total_cmp(&d(b.1))
        })
}

/// Keeps `g` in `ghosts` if it is clean and beats the run already kept for
/// its spot (or there is none): it replaces that one in place, or is added
/// at the end. Whether it was kept.
pub fn offer(ghosts: &mut Vec<Ghost>, g: Ghost) -> bool {
    if g.fault().is_some() {
        return false;
    }
    match ghosts.iter().position(|o| same_spot(o.spot, g.spot)) {
        Some(i) if g.better_than(&ghosts[i]) => {
            ghosts[i] = g;
            true
        }
        Some(_) => false,
        None => {
            ghosts.push(g);
            true
        }
    }
}

fn flag_letters(flags: u8) -> String {
    let mut s = String::new();
    for (bit, c) in [(CROUCH, 'c'), (JUMP, 'j'), (AIRBORNE, 'a'), (PRIMED, 'p')] {
        if flags & bit != 0 {
            s.push(c);
        }
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// The text of a ghosts file: [`header`], then each run's `ghost` line and
/// its frames.
pub fn to_text(ghosts: &[Ghost]) -> String {
    let mut o = header();
    o.push('\n');
    for g in ghosts {
        o.push_str(&format!(
            "ghost {:.3} {:.3} {:.3} source={} launch={:.2} touchdown={} frames={}\n",
            g.spot[0],
            g.spot[1],
            g.spot[2],
            g.source.name(),
            g.launch,
            g.touchdown,
            g.frames.len(),
        ));
        for f in &g.frames {
            o.push_str(&format!(
                "f {:.3} {:.3} {:.3} {:.1} {}\n",
                f.at[0],
                f.at[1],
                f.at[2],
                f.yaw.to_degrees(),
                flag_letters(f.flags),
            ));
        }
    }
    o
}

fn number(t: &mut std::str::SplitWhitespace<'_>, what: &str) -> Result<f32, String> {
    let s = t.next().ok_or_else(|| format!("no {what}"))?;
    s.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{what} {s:?} is not a number"))
}

/// The version of a ghosts file older than [`VERSION`], named by its
/// header. Such a file is not read and not converted: the next run kept
/// starts a fresh file in its place (`game::superbounce_ghost`,
/// `bots::harvest`). `None` for a file of this version, a newer one, a
/// damaged header or an empty file.
pub fn older(text: &str) -> Option<u32> {
    crate::verified::header_version(text, HEADER_PREFIX).filter(|&v| v < VERSION)
}

/// The runs of a ghosts file. The first line that is not blank must be
/// [`header`]; a file of another version is an error that names it. An
/// error names the line (from 1) and what is wrong with it.
pub fn parse(text: &str) -> Result<Vec<Ghost>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty());
    let Some((_, first)) = lines.next() else {
        return Ok(Vec::new());
    };
    let Some(v) = first.strip_prefix(HEADER_PREFIX) else {
        return Err(format!("line 1: not a ghosts file (want {:?})", header()));
    };
    match v.trim().parse::<u32>() {
        Ok(VERSION) => {}
        Ok(other) => {
            return Err(format!(
                "a ghosts file of version {other}; this build reads version {VERSION}"
            ));
        }
        Err(_) => return Err(format!("line 1: version {v:?} is not a number")),
    }
    let mut out: Vec<Ghost> = Vec::new();
    // How many frames the last ghost line still expects.
    let mut want = 0usize;
    for (n, line) in lines {
        if line.starts_with('#') {
            continue;
        }
        let at = |e: String| format!("line {n}: {e}");
        let mut t = line.split_whitespace();
        match t.next() {
            Some("ghost") => {
                if want > 0 {
                    return Err(at(format!("{want} frames missing before it")));
                }
                let spot = [
                    number(&mut t, "x").map_err(at)?,
                    number(&mut t, "y").map_err(at)?,
                    number(&mut t, "z").map_err(at)?,
                ];
                let mut g = Ghost {
                    spot,
                    ..Ghost::default()
                };
                let mut frames = None;
                for kv in t {
                    let Some((k, v)) = kv.split_once('=') else {
                        return Err(at(format!("{kv:?} is not key=value")));
                    };
                    let count = || {
                        v.parse::<usize>()
                            .map_err(|_| at(format!("{k}={v:?} is not a count")))
                    };
                    match k {
                        "source" => {
                            g.source = Source::parse(v)
                                .ok_or_else(|| at(format!("source {v:?}: bot, verify or player")))?
                        }
                        "launch" => {
                            g.launch = v
                                .parse::<f32>()
                                .ok()
                                .filter(|v| v.is_finite())
                                .ok_or_else(|| at(format!("launch={v:?} is not a number")))?
                        }
                        "touchdown" => g.touchdown = count()?,
                        "frames" => frames = Some(count()?),
                        _ => {}
                    }
                }
                let frames = frames.ok_or_else(|| at("no frames=".into()))?;
                if frames > MAX_FRAMES {
                    return Err(at(format!("{frames} frames, more than {MAX_FRAMES}")));
                }
                g.frames.reserve(frames);
                want = frames;
                out.push(g);
            }
            Some("f") => {
                let Some(g) = out.last_mut().filter(|_| want > 0) else {
                    return Err(at("a frame with no ghost line expecting it".into()));
                };
                let f = Frame {
                    at: [
                        number(&mut t, "x").map_err(at)?,
                        number(&mut t, "y").map_err(at)?,
                        number(&mut t, "z").map_err(at)?,
                    ],
                    yaw: number(&mut t, "yaw").map_err(at)?.to_radians(),
                    flags: {
                        let s = t.next().ok_or_else(|| at("no flags".into()))?;
                        let mut flags = 0u8;
                        for c in s.chars() {
                            flags |= match c {
                                'c' => CROUCH,
                                'j' => JUMP,
                                'a' => AIRBORNE,
                                'p' => PRIMED,
                                '-' => 0,
                                _ => return Err(at(format!("flag {c:?}: c, j, a, p or -"))),
                            };
                        }
                        flags
                    },
                };
                g.frames.push(f);
                want -= 1;
            }
            other => return Err(at(format!("{other:?}: ghost or f"))),
        }
    }
    if want > 0 {
        return Err(format!("the last ghost is {want} frames short"));
    }
    for (i, g) in out.iter().enumerate() {
        if let Some(why) = g.fault() {
            return Err(format!("ghost {i}: {why}"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, flags: u8) -> Frame {
        Frame {
            at: [x, 1.0, -3.0],
            yaw: 0.5,
            flags,
        }
    }

    /// A run of `len` frames that crouches on frames 2 and 3 (when
    /// `crouch`), jumps at 6 and bounces at `len - 2`.
    fn run(len: usize, crouch: bool, launch: f32) -> Ghost {
        let frames = (0..len)
            .map(|i| {
                let mut flags = 0;
                if crouch && (2..4).contains(&i) {
                    flags |= CROUCH;
                }
                if i == 6 {
                    flags |= JUMP | AIRBORNE;
                }
                if i >= 4 {
                    flags |= PRIMED;
                }
                frame(i as f32 * 0.1, flags)
            })
            .collect();
        Ghost {
            spot: [-10.62, 3.88, -3.19],
            source: Source::Bot,
            launch,
            touchdown: len - 2,
            frames,
        }
    }

    #[test]
    fn the_file_round_trips_with_its_version() {
        let ghosts = vec![run(12, true, 12.31), {
            let mut g = run(9, false, 9.5);
            g.spot = [5.0, -26.7, 2.1];
            g.source = Source::Verify;
            g
        }];
        let text = to_text(&ghosts);
        assert!(text.starts_with("# superbounce ghosts v2\n"), "{text}");
        assert!(text.contains(
            "ghost -10.620 3.880 -3.190 source=bot launch=12.31 touchdown=10 frames=12\n"
        ));
        assert!(text.contains("f 0.200 1.000 -3.000 28.6 c\n"), "{text}");
        assert!(text.contains("f 0.600 1.000 -3.000 28.6 jap\n"), "{text}");
        assert!(text.contains("f 0.000 1.000 -3.000 28.6 -\n"), "{text}");
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in back.iter().zip(&ghosts) {
            assert_eq!(
                (a.source, a.touchdown, a.frames.len()),
                (b.source, b.touchdown, b.frames.len())
            );
            assert!((a.launch - b.launch).abs() < 1e-2);
            for (fa, fb) in a.frames.iter().zip(&b.frames) {
                assert_eq!(fa.flags, fb.flags);
                assert!((fa.yaw - fb.yaw).abs() < 1e-3);
                for k in 0..3 {
                    assert!((fa.at[k] - fb.at[k]).abs() < 1e-3);
                }
            }
        }
        // An empty file is no ghosts, and the header alone too.
        assert_eq!(parse("").unwrap(), Vec::new());
        assert_eq!(parse(&to_text(&[])).unwrap(), Vec::new());
    }

    #[test]
    fn another_version_or_no_header_is_refused_by_name() {
        let current = to_text(&[run(12, true, 12.0)]);
        assert_eq!(older(&current), None);
        // v1 runs were recorded at the bounding sphere's centre, about 0.4
        // over the origin: refused by name, and older, so replaced.
        let v1 = current.replace("ghosts v2", "ghosts v1");
        let e = parse(&v1).unwrap_err();
        assert!(
            e.contains("version 1") && e.contains("reads version 2"),
            "{e}"
        );
        assert_eq!(older(&v1), Some(1));
        // A newer version is refused and is not older: not written over.
        let v3 = current.replace("ghosts v2", "ghosts v3");
        let e = parse(&v3).unwrap_err();
        assert!(
            e.contains("version 3") && e.contains("reads version 2"),
            "{e}"
        );
        assert_eq!(older(&v3), None);
        // A damaged file of this version is not older.
        let damaged = format!("{}\nf 0 0 0 0 -\n", header());
        assert!(parse(&damaged).is_err());
        assert_eq!(older(&damaged), None);
        assert_eq!(older(""), None);
        let e = parse("ghost 1 2 3 frames=0\n").unwrap_err();
        assert!(e.contains("not a ghosts file"), "{e}");
        assert!(
            parse("# superbounce ghosts vx\n")
                .unwrap_err()
                .contains("not a number")
        );
    }

    #[test]
    fn a_bad_file_names_its_line() {
        let h = header();
        let short = format!("{h}\nghost 1 2 3 touchdown=1 frames=2\nf 0 0 0 0 -\n");
        assert!(parse(&short).unwrap_err().contains("1 frames short"));
        let orphan = format!("{h}\nf 0 0 0 0 -\n");
        assert!(parse(&orphan).unwrap_err().starts_with("line 2:"));
        let flag = format!("{h}\nghost 1 2 3 touchdown=1 frames=2\nf 0 0 0 0 x\nf 0 0 0 0 -\n");
        assert!(parse(&flag).unwrap_err().contains("line 3: flag 'x'"));
        let outside = format!("{h}\nghost 1 2 3 touchdown=5 frames=1\nf 0 0 0 0 -\n");
        assert!(parse(&outside).unwrap_err().contains("not inside"));
        let source = format!("{h}\nghost 1 2 3 source=ghost frames=0\n");
        assert!(
            parse(&source)
                .unwrap_err()
                .contains("bot, verify or player")
        );
        // Comments and unknown keys are fine.
        let ok = format!(
            "{h}\n# hi\nghost 1 2 3 colour=blue touchdown=1 frames=2\nf 0 0 0 0 -\nf 0 0 1 0 a\n"
        );
        assert_eq!(parse(&ok).unwrap()[0].frames.len(), 2);
    }

    #[test]
    fn the_shortest_self_primed_run_wins() {
        let mut kept = Vec::new();
        assert!(offer(&mut kept, run(20, true, 10.0)));
        // Longer: kept out.
        assert!(!offer(&mut kept, run(25, true, 14.0)));
        // Shorter: replaces it in place.
        assert!(offer(&mut kept, run(15, true, 9.0)));
        assert_eq!((kept.len(), kept[0].frames.len()), (1, 15));
        // Same length, higher launch: replaces it.
        assert!(offer(&mut kept, run(15, true, 11.0)));
        assert_eq!(kept[0].launch, 11.0);
        // A shorter run whose word was set for it loses to one that crouched.
        assert!(!offer(&mut kept, run(10, false, 13.0)));
        // Another spot is added.
        let mut other = run(30, true, 8.0);
        other.spot = [50.0, 0.0, 0.0];
        assert!(offer(&mut kept, other));
        assert_eq!(kept.len(), 2);
        // A run with no bounce in it is never kept.
        let mut broken = run(5, true, 20.0);
        broken.touchdown = 9;
        assert!(!offer(&mut kept, broken));
        assert_eq!(for_spot(&kept, [-10.7, 3.9, -3.2]).map(|(i, _)| i), Some(0));
        assert_eq!(for_spot(&kept, [-10.62, 3.88, -1.0]), None);
    }

    #[test]
    fn presses_are_the_crouch_going_on_and_each_take_off() {
        let g = run(12, true, 12.0);
        assert_eq!(
            g.presses(),
            [
                Press {
                    frame: 2,
                    button: Button::Crouch
                },
                Press {
                    frame: 6,
                    button: Button::Jump
                }
            ]
        );
        assert!(g.crouch_primed() && !run(12, false, 1.0).crouch_primed());
        assert_eq!(g.landing(), g.frames[9].at);
        assert_eq!((g.ticks(), g.seconds()), (11, 11.0 / 30.0));
    }
}
