//! A batch's trials summed up by spot: the verdicts, one line a spot and
//! the verified file's entries (docs/game/superbounce.md, "Verifying
//! spots"). Pure.

use std::collections::BTreeMap;

use agent_proto::superbounce::{VerifyBest, VerifySpotResult, VerifyTrial};
use superbounce::Status;
use superbounce::verified::Entry;

use super::plan::{Target, TrialKind};
use super::trial::REACH;

/// Why a trial that did not work failed, by name; `None` for one that
/// worked.
pub fn reason(kind: TrialKind, t: &VerifyTrial) -> Option<String> {
    if t.ok {
        return None;
    }
    if let Some(n) = &t.note
        && (n.starts_with("teleport") || n.starts_with("the seat"))
    {
        return Some("setup".into());
    }
    Some(match kind {
        TrialKind::Crouch => if t.closest.is_some_and(|c| c > REACH) {
            "not_under"
        } else if t.crouched_first == Some(false) {
            "not_crouched"
        } else if t.forced != Some(true) {
            "not_forced"
        } else {
            "forced_no_word"
        }
        .to_string(),
        TrialKind::Drop => t.landing.clone().unwrap_or_else(|| "no_touchdown".into()),
        TrialKind::Route => t.route.clone().unwrap_or_else(|| "no_bounce".into()),
    })
}

/// Each target's verdict over `outcomes` (the trials run so far, in plan
/// order); `planned` counts each target's trials in the plan.
pub fn results(
    kind: TrialKind,
    targets: &[Target],
    planned: &[usize],
    outcomes: &[VerifyTrial],
) -> Vec<VerifySpotResult> {
    (0..targets.len())
        .map(|i| {
            let mine: Vec<&VerifyTrial> = outcomes.iter().filter(|t| t.spot == i).collect();
            let ok = mine.iter().filter(|t| t.ok).count() as u32;
            let mut why = BTreeMap::new();
            for t in &mine {
                if let Some(r) = reason(kind, t) {
                    *why.entry(r).or_insert(0) += 1;
                }
            }
            let all_run = mine.len() >= planned.get(i).copied().unwrap_or(0) && !mine.is_empty();
            // Setup failures say nothing about the spot.
            let real = mine
                .iter()
                .filter(|t| reason(kind, t).as_deref() != Some("setup"))
                .count();
            let status = if ok > 0 {
                "proven"
            } else if all_run && real > 0 {
                "failed"
            } else {
                "untried"
            };
            VerifySpotResult {
                spot: i,
                trials: mine.len() as u32,
                ok,
                status: status.into(),
                best: best(kind, &mine),
                why,
            }
        })
        .collect()
}

/// The best trial of a spot: a crouch's quickest prime, a drop's biggest
/// launch, a route's first that worked. A failed drop still names the
/// biggest launch it saw.
fn best(kind: TrialKind, mine: &[&VerifyTrial]) -> Option<VerifyBest> {
    let pick = match kind {
        TrialKind::Crouch => mine
            .iter()
            .filter(|t| t.ok)
            .min_by_key(|t| t.primed_tick.unwrap_or(u32::MAX)),
        TrialKind::Drop => {
            let worked = mine.iter().filter(|t| t.ok);
            let any = mine.iter().filter(|t| t.vz_out.is_some());
            let by_launch = |a: &&&VerifyTrial, b: &&&VerifyTrial| {
                a.vz_out
                    .unwrap_or(f32::MIN)
                    .total_cmp(&b.vz_out.unwrap_or(f32::MIN))
            };
            worked.max_by(by_launch).or_else(|| any.max_by(by_launch))
        }
        TrialKind::Route => mine.iter().find(|t| t.ok),
    }?;
    Some(VerifyBest {
        trial: pick.index,
        offset: pick.offset,
        yaw: pick.yaw.filter(|_| kind == TrialKind::Crouch),
        distance: pick.distance,
        height: pick.height,
        launch: pick.vz_out,
    })
}

fn fmt_xyz(v: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])
}

/// One line a spot, e.g. `crouch #3 (-10.91, 0.72, -1.16): primed 4/9,
/// best offset (0.06, -0.03) yaw -121 from 0.3 out`.
pub fn lines(kind: TrialKind, targets: &[Target], results: &[VerifySpotResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| {
            let t = &targets[r.spot];
            let head = format!(
                "{} {} {}",
                kind.name(),
                t.label(r.spot),
                fmt_xyz(t.spot.stand)
            );
            let verb = match kind {
                TrialKind::Crouch => "primed",
                TrialKind::Drop => "launched",
                TrialKind::Route => "bounced",
            };
            let mut line = format!("{head}: {verb} {}/{}", r.ok, r.trials);
            if r.status != "proven" {
                line.push_str(&format!(" ({})", r.status));
            }
            if let Some(b) = &r.best {
                match kind {
                    TrialKind::Crouch => {
                        line.push_str(&format!(
                            ", best offset ({:.2}, {:.2})",
                            b.offset[0], b.offset[1]
                        ));
                        if let Some(y) = b.yaw {
                            line.push_str(&format!(" yaw {y:.0}"));
                        }
                        if let Some(d) = b.distance {
                            line.push_str(&format!(" from {d:.2} out"));
                        }
                    }
                    TrialKind::Drop => {
                        line.push_str(&format!(
                            ", best across {:.2} along {:.2}",
                            b.offset[0], b.offset[1]
                        ));
                        if let Some(h) = b.height {
                            line.push_str(&format!(" height {h:.2}"));
                        }
                        if let Some(l) = b.launch {
                            line.push_str(&format!(", launch {l:.1}"));
                        }
                    }
                    TrialKind::Route => {}
                }
            }
            if !r.why.is_empty() {
                let why: Vec<String> = r.why.iter().map(|(k, n)| format!("{k} {n}")).collect();
                line.push_str(&format!("; failed: {}", why.join(", ")));
            }
            line
        })
        .collect()
}

/// The verified file's entries for the scan's spots a batch proved or
/// failed (explicit points and routes write none).
pub fn entries(
    kind: TrialKind,
    targets: &[Target],
    results: &[VerifySpotResult],
    outcomes: &[VerifyTrial],
) -> Vec<Entry> {
    if kind == TrialKind::Route {
        return Vec::new();
    }
    results
        .iter()
        .filter_map(|r| {
            let t = targets.get(r.spot)?;
            t.index?;
            let status = match r.status.as_str() {
                "proven" => Status::Proven,
                "failed" => Status::Failed,
                _ => return None,
            };
            let stand = t.spot.stand;
            let launch = outcomes
                .iter()
                .filter(|o| o.spot == r.spot)
                .filter_map(|o| o.vz_out)
                .fold(0.0f32, f32::max);
            let mut e = Entry {
                kind: t.spot.kind,
                status,
                at: stand,
                trials: r.trials,
                ok: r.ok,
                best: if kind == TrialKind::Drop { launch } else { 0.0 },
                ..Entry::default()
            };
            if status == Status::Proven
                && let Some(b) = &r.best
                && let Some(o) = outcomes.iter().find(|o| o.index == b.trial)
            {
                match kind {
                    // Where the working trial started, and its way in.
                    TrialKind::Crouch => {
                        e.offset = [o.start[0] - stand[0], o.start[1] - stand[1]];
                        e.yaw = o.yaw;
                    }
                    // Where it landed (the origin the tick before), and
                    // the height it fell.
                    TrialKind::Drop => {
                        let at = o.landed_at.unwrap_or(o.start);
                        e.offset = [at[0] - stand[0], at[1] - stand[1]];
                        e.height = o.height;
                    }
                    TrialKind::Route => {}
                }
            }
            Some(e)
        })
        .collect()
}
