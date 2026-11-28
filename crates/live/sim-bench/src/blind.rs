//! The render-blind gate's comparison (docs/engine/sim-render-split.md,
//! "The proof"): one tape run in several arms, the render skipped in one and
//! drawn in the rest in different windows, lenses and frame rates, must
//! leave the same `state_hashes_v1` series, tick for tick.
//!
//! An arm's series is read from a sim report (`state_hashes_v1`) or from a
//! running game's `sim.hashes` reply (`rows`). [`compare`] holds every arm
//! against the first and names the first tick, the arm and the digest that
//! differ. Pure: nothing here reads the game.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::metrics::TickHash;

/// One arm's name and its series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm {
    pub name: String,
    pub rows: Vec<TickHash>,
}

/// What the arms' series say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every arm has the reference's ticks and digests.
    Identical { arms: usize, ticks: usize },
    /// The first difference, by tick.
    Differs {
        tick: u32,
        arm: String,
        field: &'static str,
        reference: String,
        got: String,
    },
    /// The gate did not compare what it needs to: too few ticks, an arm
    /// without the arena hash, or a tick one arm lacks.
    Incomplete(String),
}

impl Verdict {
    pub fn passed(&self) -> bool {
        matches!(self, Verdict::Identical { .. })
    }

    pub fn render(&self) -> String {
        match self {
            Verdict::Identical { arms, ticks } => {
                format!("render-blind: {arms} arms identical over {ticks} ticks\n")
            }
            Verdict::Differs { tick, arm, field, reference, got } => {
                let mut out = String::new();
                let _ = writeln!(
                    out,
                    "render-blind: first difference at tick {tick}, arm {arm}: {field} {got} against the reference's {reference}"
                );
                out
            }
            Verdict::Incomplete(why) => format!("render-blind: not compared: {why}\n"),
        }
    }
}

/// What the gate asks of the series besides equality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// At least this many ticks in every arm.
    pub ticks: usize,
    /// Every row carries the arena hash.
    pub arena: bool,
}

/// The rows of a sim report (`state_hashes_v1`) or a `sim.hashes` reply
/// (`rows`).
pub fn rows(json: &str) -> Result<Vec<TickHash>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v
        .get("state_hashes_v1")
        .or_else(|| v.get("rows"))
        .ok_or("no state_hashes_v1 (a sim report) or rows (a sim.hashes reply)")?;
    serde_json::from_value(list.clone()).map_err(|e| e.to_string())
}

/// Every arm against the first.
pub fn compare(arms: &[Arm], needs: Needs) -> Verdict {
    let Some((reference, rest)) = arms.split_first() else {
        return Verdict::Incomplete("no arms".into());
    };
    if rest.is_empty() {
        return Verdict::Incomplete("one arm: nothing to compare it with".into());
    }
    fn by_tick(a: &Arm) -> BTreeMap<u32, &TickHash> {
        a.rows.iter().map(|r| (r.tick, r)).collect()
    }
    let base = by_tick(reference);
    for arm in arms {
        let have = by_tick(arm);
        if have.len() != arm.rows.len() {
            return Verdict::Incomplete(format!("arm {} repeats a tick", arm.name));
        }
        if have.len() < needs.ticks {
            return Verdict::Incomplete(format!("arm {} has {} ticks, the gate needs {}", arm.name, have.len(), needs.ticks));
        }
        if needs.arena {
            if let Some(r) = arm.rows.iter().find(|r| r.arena.is_none()) {
                return Verdict::Incomplete(format!(
                    "arm {} has no arena hash at tick {} (run with impl.sim.arena-hash)",
                    arm.name, r.tick
                ));
            }
        }
    }
    for arm in rest {
        let have = by_tick(arm);
        for (tick, want) in &base {
            let Some(got) = have.get(tick) else {
                return Verdict::Incomplete(format!("arm {} has no tick {tick}", arm.name));
            };
            if got.gameplay != want.gameplay {
                return Verdict::Differs {
                    tick: *tick,
                    arm: arm.name.clone(),
                    field: "gameplay",
                    reference: want.gameplay.clone(),
                    got: got.gameplay.clone(),
                };
            }
            if got.arena != want.arena {
                return Verdict::Differs {
                    tick: *tick,
                    arm: arm.name.clone(),
                    field: "arena",
                    reference: want.arena.clone().unwrap_or_default(),
                    got: got.arena.clone().unwrap_or_default(),
                };
            }
        }
        if have.len() != base.len() {
            let extra = have.keys().find(|t| !base.contains_key(t)).copied().unwrap_or_default();
            return Verdict::Incomplete(format!("arm {} has tick {extra}, the reference does not", arm.name));
        }
    }
    Verdict::Identical { arms: arms.len(), ticks: base.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(name: &str, rows: &[(u32, &str, Option<&str>)]) -> Arm {
        Arm {
            name: name.into(),
            rows: rows
                .iter()
                .map(|&(tick, g, a)| TickHash { tick, gameplay: g.into(), arena: a.map(Into::into) })
                .collect(),
        }
    }

    const SERIES: [(u32, &str, Option<&str>); 3] = [(1, "aa", Some("a1")), (2, "bb", Some("b1")), (3, "cc", Some("c1"))];

    #[test]
    fn equal_series_pass_in_every_arm() {
        let arms: Vec<Arm> = ["skipped", "640x480", "32:9x4", "free-camera", "120fps"]
            .into_iter()
            .map(|n| arm(n, &SERIES))
            .collect();
        let v = compare(&arms, Needs { ticks: 3, arena: true });
        assert_eq!(v, Verdict::Identical { arms: 5, ticks: 3 });
        assert!(v.passed());
    }

    #[test]
    fn the_first_differing_tick_and_arm_are_named() {
        let mut bad = SERIES;
        bad[1].1 = "xx";
        let mut late = SERIES;
        late[2].2 = Some("zz");
        let arms = [arm("skipped", &SERIES), arm("window", &late), arm("camera", &bad)];
        let v = compare(&arms, Needs::default());
        assert_eq!(
            v,
            Verdict::Differs { tick: 3, arm: "window".into(), field: "arena", reference: "c1".into(), got: "zz".into() }
        );
        assert!(v.render().contains("tick 3, arm window: arena zz"));
        let v = compare(&[arms[0].clone(), arms[2].clone()], Needs::default());
        assert!(matches!(v, Verdict::Differs { tick: 2, field: "gameplay", .. }));
    }

    #[test]
    fn a_short_arm_or_a_missing_arena_is_not_a_pass() {
        let two = &SERIES[..2];
        let v = compare(&[arm("a", &SERIES), arm("b", two)], Needs::default());
        assert!(matches!(&v, Verdict::Incomplete(m) if m.contains("no tick 3")), "{v:?}");
        let v = compare(&[arm("a", &SERIES), arm("b", &SERIES)], Needs { ticks: 1800, arena: false });
        assert!(matches!(&v, Verdict::Incomplete(m) if m.contains("needs 1800")), "{v:?}");
        let bare = [(1, "aa", None), (2, "bb", None), (3, "cc", None)];
        let v = compare(&[arm("a", &bare), arm("b", &bare)], Needs { ticks: 3, arena: true });
        assert!(matches!(&v, Verdict::Incomplete(m) if m.contains("arena")), "{v:?}");
        assert!(compare(&[arm("a", &bare), arm("b", &bare)], Needs { ticks: 3, arena: false }).passed());
        assert!(!compare(&[arm("a", &SERIES)], Needs::default()).passed());
        let v = compare(&[arm("a", two), arm("b", &SERIES)], Needs::default());
        assert!(matches!(&v, Verdict::Incomplete(m) if m.contains("reference does not")), "{v:?}");
    }

    #[test]
    fn reports_and_hash_replies_both_read() {
        let report = r#"{"format":"x","state_hashes_v1":[{"tick":1,"gameplay":"aa","arena":"a1"}]}"#;
        let reply = r#"{"rows":[{"tick":1,"gameplay":"aa","arena":"a1"}],"dropped":0,"resets":0}"#;
        assert_eq!(rows(report).unwrap(), rows(reply).unwrap());
        assert!(rows("{}").is_err());
        assert!(rows("nope").is_err());
    }
}
