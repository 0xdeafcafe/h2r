//! What game runs found when they tried the scanned spots: a map's
//! verified file (docs/game/superbounce.md, "Verifying spots").
//!
//! `superbounce.verify` (the game's verifier) writes one line per spot it
//! has tried; the scan's consumers read it back and mark each spot
//! [`Status::Proven`] or [`Status::Failed`] ([`apply`]). The file is text,
//! one spot a line, so a person can read and edit it:
//!
//! ```text
//! # superbounce verified v1
//! crouch proven -10.910 0.720 -1.160 dx=0.060 dy=-0.030 yaw=-121.0 trials=9 ok=4
//! bounce failed -10.620 3.880 -3.190 dx=0.000 dy=0.050 height=2.430 trials=15 ok=0 best=1.71
//! ```
//!
//! The kind, the verdict and the spot's `stand` as the scan gave it (what
//! a spot is matched by, to [`MATCH`]), then `key=value` fields in any
//! order: `dx`, `dy` (the working offset from `stand`, world units: where
//! a crouch trial started, or where a drop landed), `yaw` (degrees, the
//! working approach of a crouch trial), `height` (the working drop height
//! over the strip), `trials` and `ok` (how many were tried and worked),
//! `best` (the best launch seen, units/s). Unknown keys are kept out of the
//! way: a reader ignores them. Blank lines and `#` lines are comments.

use crate::geom::segment_xy;
use crate::{Kind, Spot, Status, Vec3};

/// The first line [`to_text`] writes.
pub const HEADER: &str = "# superbounce verified v1";

/// A verified entry matches a spot of its kind whose `stand` is within this
/// (world units, horizontally) and [`MATCH_Z`] vertically.
pub const MATCH: f32 = 0.3;
/// See [`MATCH`].
pub const MATCH_Z: f32 = 0.5;

/// What a game run made of one spot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    /// [`Status::Proven`] or [`Status::Failed`]; a [`Status::Likely`]
    /// entry says nothing and is not written.
    pub status: Status,
    /// The spot's `stand`, as the scan gave it.
    pub at: Vec3,
    /// The working offset from `at` (X and Y): a crouch trial's start, a
    /// drop's landing point. Zero when none worked.
    pub offset: [f32; 2],
    /// A crouch trial's working approach yaw, degrees.
    pub yaw: Option<f32>,
    /// A drop's working height over the strip, world units.
    pub height: Option<f32>,
    pub trials: u32,
    pub ok: u32,
    /// The best launch seen (units/s up); 0 when none.
    pub best: f32,
}

impl Entry {
    /// Where the working offset puts it: `at` moved by `offset`.
    pub fn working(&self) -> Vec3 {
        [
            self.at[0] + self.offset[0],
            self.at[1] + self.offset[1],
            self.at[2],
        ]
    }

    /// Whether this entry is about `spot`.
    pub fn matches(&self, spot: &Spot) -> bool {
        self.kind == spot.kind && near(self.at, spot.stand)
    }
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= MATCH && (a[2] - b[2]).abs() <= MATCH_Z
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Bounce => "bounce",
        Kind::Crouch => "crouch",
    }
}

/// The text of a verified file: [`HEADER`], then one line an entry, in
/// order. Likely entries are left out.
pub fn to_text(entries: &[Entry]) -> String {
    let mut o = String::from(HEADER);
    o.push('\n');
    for e in entries.iter().filter(|e| e.status != Status::Likely) {
        o.push_str(&format!(
            "{} {} {:.3} {:.3} {:.3} dx={:.3} dy={:.3}",
            kind_name(e.kind),
            e.status.name(),
            e.at[0],
            e.at[1],
            e.at[2],
            e.offset[0],
            e.offset[1],
        ));
        if let Some(y) = e.yaw {
            o.push_str(&format!(" yaw={y:.1}"));
        }
        if let Some(h) = e.height {
            o.push_str(&format!(" height={h:.3}"));
        }
        o.push_str(&format!(" trials={} ok={}", e.trials, e.ok));
        if e.best > 0.0 {
            o.push_str(&format!(" best={:.2}", e.best));
        }
        o.push('\n');
    }
    o
}

/// The entries of a verified file. An error names the line (from 1) and
/// what is wrong with it.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?);
    }
    Ok(out)
}

fn parse_line(line: &str) -> Result<Entry, String> {
    let mut t = line.split_whitespace();
    let kind = match t.next() {
        Some("bounce") => Kind::Bounce,
        Some("crouch") => Kind::Crouch,
        other => return Err(format!("kind {other:?}: bounce or crouch")),
    };
    let status = match t.next() {
        Some("proven") => Status::Proven,
        Some("failed") => Status::Failed,
        other => return Err(format!("status {other:?}: proven or failed")),
    };
    let mut num = |what: &str| -> Result<f32, String> {
        let s = t.next().ok_or_else(|| format!("no {what}"))?;
        s.parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{what} {s:?} is not a number"))
    };
    let at = [num("x")?, num("y")?, num("z")?];
    let mut e = Entry {
        kind,
        status,
        at,
        ..Entry::default()
    };
    for kv in t {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(format!("{kv:?} is not key=value"));
        };
        let f = || {
            v.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{k}={v:?} is not a number"))
        };
        let n = || {
            v.parse::<u32>()
                .map_err(|_| format!("{k}={v:?} is not a count"))
        };
        match k {
            "dx" => e.offset[0] = f()?,
            "dy" => e.offset[1] = f()?,
            "yaw" => e.yaw = Some(f()?),
            "height" => e.height = Some(f()?),
            "trials" => e.trials = n()?,
            "ok" => e.ok = n()?,
            "best" => e.best = f()?,
            _ => {}
        }
    }
    Ok(e)
}

/// The entry about `spot`, the nearest when several match.
pub fn entry_for<'a>(entries: &'a [Entry], spot: &Spot) -> Option<&'a Entry> {
    entries.iter().filter(|e| e.matches(spot)).min_by(|a, b| {
        let d = |e: &Entry| (e.at[0] - spot.stand[0]).hypot(e.at[1] - spot.stand[1]);
        d(a).total_cmp(&d(b))
    })
}

/// Marks each spot an entry is about with the entry's status. A proven
/// spot scores 1. Spots no entry is about keep the scan's status.
///
/// A bounce entry belongs to one spot: of the bounce spots it is about,
/// the one whose target line passes nearest the entry's working point.
/// Ascension's entry (standing at y 3.88, landing at y 3.976) is within
/// reach of both the slot on its seam (y 4.000) and the crease candidate
/// (y 3.800); its landing is on the seam, so it proves the slot alone.
pub fn apply(spots: &mut [Spot], entries: &[Entry]) {
    if entries.is_empty() {
        return;
    }
    let before = spots.to_vec();
    let off = |e: &Entry, s: &Spot| {
        let w = e.working();
        segment_xy(s.edge, [w[0], w[1]])
    };
    for (i, s) in spots.iter_mut().enumerate() {
        if let Some(e) = entry_for(entries, s) {
            let taken = s.kind == Kind::Bounce
                && before.iter().enumerate().any(|(k, o)| {
                    k != i && e.matches(o) && off(e, o) < off(e, &before[i])
                });
            if taken {
                continue;
            }
            s.status = e.status;
            if e.status == Status::Proven {
                s.score = 1.0;
            }
        }
    }
}

/// `old` with `new` merged in: an entry of `new` replaces every entry of
/// `old` of its kind within [`MATCH`] of it; the rest of `old` is kept, in
/// order, then the new entries, in order.
pub fn merge(old: &[Entry], new: &[Entry]) -> Vec<Entry> {
    let mut out: Vec<Entry> = old
        .iter()
        .filter(|o| !new.iter().any(|n| n.kind == o.kind && near(n.at, o.at)))
        .copied()
        .collect();
    out.extend(new.iter().filter(|e| e.status != Status::Likely).copied());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(kind: Kind, stand: Vec3) -> Spot {
        Spot {
            kind,
            stand,
            score: 0.5,
            ..Spot::default()
        }
    }

    fn crouch_entry() -> Entry {
        Entry {
            kind: Kind::Crouch,
            status: Status::Proven,
            at: [-10.91, 0.72, -1.16],
            offset: [0.06, -0.03],
            yaw: Some(-121.0),
            height: None,
            trials: 9,
            ok: 4,
            best: 0.0,
        }
    }

    fn bounce_entry() -> Entry {
        Entry {
            kind: Kind::Bounce,
            status: Status::Failed,
            at: [-10.62, 3.88, -3.19],
            offset: [0.0, 0.05],
            yaw: None,
            height: Some(2.43),
            trials: 15,
            ok: 0,
            best: 1.71,
        }
    }

    #[test]
    fn the_file_round_trips() {
        let entries = vec![crouch_entry(), bounce_entry()];
        let text = to_text(&entries);
        assert!(text.starts_with(HEADER));
        assert!(text.contains(
            "crouch proven -10.910 0.720 -1.160 dx=0.060 dy=-0.030 yaw=-121.0 trials=9 ok=4\n"
        ));
        assert!(text.contains("bounce failed -10.620 3.880 -3.190 dx=0.000 dy=0.050 height=2.430 trials=15 ok=0 best=1.71\n"));
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in back.iter().zip(&entries) {
            assert_eq!(
                (
                    a.kind,
                    a.status,
                    a.trials,
                    a.ok,
                    a.yaw.is_some(),
                    a.height.is_some()
                ),
                (
                    b.kind,
                    b.status,
                    b.trials,
                    b.ok,
                    b.yaw.is_some(),
                    b.height.is_some()
                )
            );
            for k in 0..3 {
                assert!((a.at[k] - b.at[k]).abs() < 1e-3);
            }
            assert!((a.offset[1] - b.offset[1]).abs() < 1e-3);
            assert!((a.best - b.best).abs() < 1e-2);
        }
    }

    #[test]
    fn a_reader_skips_comments_and_unknown_keys_and_names_a_bad_line() {
        let text = "# hi\n\ncrouch proven 1 2 3 colour=blue trials=2 ok=1\n";
        let e = parse(text).unwrap();
        assert_eq!(
            (e.len(), e[0].trials, e[0].ok, e[0].offset),
            (1, 2, 1, [0.0, 0.0])
        );
        let err = parse("# x\nbounce maybe 1 2 3\n").unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
        assert!(parse("crouch proven 1 2\n").unwrap_err().contains("no z"));
        assert!(
            parse("crouch proven 1 2 3 dx\n")
                .unwrap_err()
                .contains("key=value")
        );
        assert!(
            parse("crouch proven 1 2 3 ok=-1\n")
                .unwrap_err()
                .contains("count")
        );
        assert!(parse("crouch proven 1 2 nan\n").is_err());
    }

    #[test]
    fn likely_entries_are_not_written() {
        let mut e = crouch_entry();
        e.status = Status::Likely;
        assert_eq!(to_text(&[e]), format!("{HEADER}\n"));
    }

    #[test]
    fn apply_marks_the_spots_entries_are_about_and_nothing_else() {
        let mut spots = vec![
            spot(Kind::Crouch, [-10.85, 0.70, -1.16]),
            // The right place, the wrong kind.
            spot(Kind::Bounce, [-10.91, 0.72, -1.16]),
            spot(Kind::Bounce, [-10.62, 3.90, -3.19]),
            // Too far.
            spot(Kind::Bounce, [-10.62, 4.50, -3.19]),
            // Right above, a floor too high.
            spot(Kind::Crouch, [-10.91, 0.72, 0.5]),
        ];
        apply(&mut spots, &[crouch_entry(), bounce_entry()]);
        let st: Vec<_> = spots.iter().map(|s| s.status).collect();
        assert_eq!(
            st,
            [
                Status::Proven,
                Status::Likely,
                Status::Failed,
                Status::Likely,
                Status::Likely
            ]
        );
        assert_eq!(spots[0].score, 1.0);
        assert_eq!(spots[2].score, 0.5);
        assert_eq!(
            entry_for(&[crouch_entry()], &spots[0]).unwrap().yaw,
            Some(-121.0)
        );
        let w = crouch_entry().working();
        assert!((w[0] + 10.85).abs() < 1e-4 && (w[1] - 0.69).abs() < 1e-4);
    }

    #[test]
    fn a_bounce_entry_proves_the_spot_its_landing_is_on_and_no_other() {
        // Ascension's file: standing at y 3.88, the drop that worked landed
        // 0.096 on, at y 3.976.
        let e = Entry {
            kind: Kind::Bounce,
            status: Status::Proven,
            at: [-10.538, 3.88, -3.19],
            offset: [0.081, 0.096],
            ..Entry::default()
        };
        let line = |y: f32| [[-11.10, y, -3.2], [-9.98, y, -3.2]];
        let slot = Spot {
            edge: line(4.0),
            ..spot(Kind::Bounce, [-10.54, 4.0, -3.19])
        };
        let crease = Spot {
            edge: line(3.8),
            shape: crate::Shape::Crease,
            ..spot(Kind::Bounce, [-10.54, 3.8, -3.19])
        };
        let mut spots = vec![slot, crease];
        apply(&mut spots, &[e]);
        assert_eq!(
            (spots[0].status, spots[1].status),
            (Status::Proven, Status::Likely)
        );
        assert!(spots[1].candidate());
    }

    #[test]
    fn merge_replaces_what_is_tried_again_and_keeps_the_rest() {
        let old = vec![crouch_entry(), bounce_entry()];
        let mut again = bounce_entry();
        again.status = Status::Proven;
        again.at[1] += 0.1;
        let other = Entry {
            at: [5.0, 5.0, 0.0],
            ..crouch_entry()
        };
        let m = merge(&old, &[again, other]);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], crouch_entry());
        assert_eq!(m[1].status, Status::Proven);
        assert_eq!(m[2].at, [5.0, 5.0, 0.0]);
    }
}
