//! `superbounce.reload` and `superbounce.tune` (docs/runtime/agent-api.md,
//! "superbounce"): iterate on a map's spots and the model's constants in a
//! running game, with no restart. Both are acts, answered on the game
//! thread at a tick boundary like the rest of the agent's methods.
//!
//! - Reload re-reads the map's `.spots`, `.learned`, `.verified` and
//!   `.ghosts` from the bots directory (`bots::spots::reload_in`, the
//!   ghosts' store) and republishes them the way a map load does: the
//!   bots' table and its generation (the brains read their spots again),
//!   and the mode's own copy ([`crate::superbounce_mode::refresh`]) that
//!   the marks, HUD, assist and tracker read.
//! - Tune sets or clears the overrides in `superbounce::tune`. Nothing is
//!   saved.

use agent_proto::superbounce as wire;
use superbounce::{Kind, Shape, Spot, Status};

/// The tally of `all` (the scan with the files applied) and how many of
/// them `marked` holds.
pub fn counts(all: &[Spot], marked: usize) -> wire::SpotCounts {
    let mut c = wire::SpotCounts {
        total: all.len(),
        marked,
        ..Default::default()
    };
    for s in all {
        match (s.kind, s.shape) {
            (Kind::Crouch, _) => c.crouch += 1,
            (Kind::Bounce, Shape::Slot) => c.slot += 1,
            (Kind::Bounce, Shape::Crease) => c.crease += 1,
            (Kind::Bounce, Shape::Vee) => c.vee += 1,
            (Kind::Bounce, Shape::Busy) => c.busy += 1,
        }
        match s.status {
            Status::Likely => c.likely += 1,
            Status::Proven => c.proven += 1,
            Status::Failed => c.failed += 1,
        }
    }
    c
}

fn tally(map: &str) -> wire::SpotCounts {
    let (all, shown) = crate::superbounce_mode::applied(map);
    counts(&all, shown.len())
}

/// `superbounce.reload`.
pub fn reload(p: &wire::ReloadParams) -> Result<wire::Reload, String> {
    let map = match &p.map {
        Some(m) if !m.trim().is_empty() => m.trim().to_ascii_lowercase(),
        Some(_) => return Err("map is empty".into()),
        None => crate::superbounce_mode::current_map()
            .map(|m| m.to_ascii_lowercase())
            .ok_or("no map loaded: give map")?,
    };
    let Some(dir) = bots::spots::verified_dir() else {
        return Err("no bots directory is set: the game keeps no spot files".into());
    };
    let before = tally(&map);
    let (ghosts_before, ghosts_after) = crate::superbounce_ghost::reload(&map);
    let r = bots::spots::reload_in(&dir, &map);
    let mode_updated = crate::superbounce_mode::refresh(&map);
    Ok(wire::Reload {
        after: tally(&map),
        before,
        map,
        dir: dir.display().to_string(),
        spots_file: r.spots_file,
        verified_entries: r.verified,
        learned_entries: r.learned,
        ghosts_before,
        ghosts_after,
        mode_updated,
    })
}

/// What `superbounce.tune` will do, checked: `None` leaves a constant
/// alone, `Some(None)` clears its override, `Some(Some(v))` sets it.
pub fn plan(p: &wire::TuneParams) -> Result<wire::TuneParams, String> {
    if let Some(Some(v)) = p.bounce_speed
        && !(v.is_finite() && v > 0.0)
    {
        return Err(format!("bounce_speed must be greater than 0, got {v}"));
    }
    if let Some(Some(v)) = p.catch
        && !(v.is_finite() && (0.0..1.0).contains(&v))
    {
        return Err(format!("catch must be in 0..1, got {v}"));
    }
    if let Some(Some(v)) = p.launch_gain
        && !(v.is_finite() && v >= 0.0)
    {
        return Err(format!("launch_gain must be 0 or more, got {v}"));
    }
    Ok(p.clone())
}

fn apply_one(t: &superbounce::tune::Tunable, change: Option<Option<f32>>) {
    match change {
        Some(Some(v)) => t.set(v),
        Some(None) => t.clear(),
        None => {}
    }
}

fn tuned(t: &superbounce::tune::Tunable) -> wire::Tuned {
    wire::Tuned {
        value: t.get(),
        source: t.source().into(),
    }
}

/// The values in effect.
pub fn in_effect() -> wire::Tune {
    use superbounce::tune;
    wire::Tune {
        bounce_speed: tuned(&tune::BOUNCE_SPEED),
        catch: tuned(&tune::CATCH),
        launch_gain: tuned(&tune::EXCESS_GAIN),
    }
}

/// `superbounce.tune`: validates every key first, then applies them, so a
/// refusal changes nothing.
pub fn tune(p: &wire::TuneParams) -> Result<wire::Tune, String> {
    let p = plan(p)?;
    apply_one(&superbounce::tune::BOUNCE_SPEED, p.bounce_speed);
    apply_one(&superbounce::tune::CATCH, p.catch);
    apply_one(&superbounce::tune::EXCESS_GAIN, p.launch_gain);
    Ok(in_effect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<wire::TuneParams, serde_json::Error> {
        serde_json::from_value(json)
    }

    #[test]
    fn tune_params_tell_absent_from_null_and_refuse_unknown_keys() {
        let p = params(serde_json::json!({"bounce_speed": 6.0, "catch": null})).unwrap();
        assert_eq!(p.bounce_speed, Some(Some(6.0)));
        assert_eq!(p.catch, Some(None));
        assert_eq!(p.launch_gain, None);
        assert!(params(serde_json::json!({"gravity": 4.0})).is_err());
        assert_eq!(params(serde_json::json!({})).unwrap(), wire::TuneParams::default());
    }

    #[test]
    fn the_values_are_checked() {
        let bad = |j: serde_json::Value| plan(&params(j).unwrap()).unwrap_err();
        assert!(bad(serde_json::json!({"bounce_speed": 0.0})).contains("greater than 0"));
        assert!(bad(serde_json::json!({"bounce_speed": -1.0})).contains("bounce_speed"));
        assert!(bad(serde_json::json!({"catch": 1.0})).contains("0..1"));
        assert!(bad(serde_json::json!({"catch": -0.1})).contains("0..1"));
        assert!(bad(serde_json::json!({"launch_gain": -2.0})).contains("launch_gain"));
        let ok = |j: serde_json::Value| plan(&params(j).unwrap()).is_ok();
        assert!(ok(serde_json::json!({"bounce_speed": 0.01, "catch": 0.0, "launch_gain": 0.0})));
        assert!(ok(serde_json::json!({"bounce_speed": null, "catch": null, "launch_gain": null})));
        assert!(ok(serde_json::json!({"catch": 0.999})));
    }

    #[test]
    fn counts_split_by_shape_and_status() {
        let spot = |kind, shape, status| Spot {
            kind,
            shape,
            status,
            ..Spot::default()
        };
        let all = [
            spot(Kind::Crouch, Shape::Slot, Status::Likely),
            spot(Kind::Bounce, Shape::Slot, Status::Proven),
            spot(Kind::Bounce, Shape::Vee, Status::Failed),
            spot(Kind::Bounce, Shape::Crease, Status::Likely),
            spot(Kind::Bounce, Shape::Busy, Status::Likely),
        ];
        let c = counts(&all, 3);
        assert_eq!(
            (c.total, c.crouch, c.slot, c.crease, c.vee, c.busy, c.marked),
            (5, 1, 1, 1, 1, 1, 3)
        );
        assert_eq!((c.likely, c.proven, c.failed), (3, 1, 1));
    }
}
