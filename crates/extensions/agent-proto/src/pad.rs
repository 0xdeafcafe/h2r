//! `input.pad`: timed pad steps for a player (docs/runtime/agent-api.md,
//! "Input"). Plain data: the agent's input tap turns them into what the
//! game's controller reads return.

use serde::{Deserialize, Serialize};

/// Button names a step may hold.
pub const PAD_BUTTONS: &[&str] = &[
    "a", "b", "x", "y", "black", "white", "lt", "rt", "up", "down", "left", "right", "start",
    "back", "ls", "rs",
];

/// One pad state, held for a while.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PadStep {
    /// Held buttons ([`PAD_BUTTONS`]); `lt`/`rt` pull a trigger fully.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<String>,
    /// Left stick, x and y from -1 to 1; +y is up.
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub left: [f32; 2],
    /// Right stick (look), as `left`.
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub right: [f32; 2],
    /// Trigger pulls, 0 to 1 (overriding `lt`/`rt` in `buttons`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lt: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt: Option<f32>,
    /// How long to hold it, in wall-clock ms. With neither `ms` nor
    /// `ticks`: until the next `input.pad` or `input.release` (a step after
    /// it never starts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// How long to hold it, in game ticks (the stamp's `tick`): the same
    /// on every run, where `ms` depends on the frame rate. With `ms` too,
    /// both must pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    /// And until the game has read the pad at least this many times
    /// (default 1), so a short press is never lost at a low frame rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reads: Option<u32>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero2(v: &[f32; 2]) -> bool {
    *v == [0.0, 0.0]
}

impl PadStep {
    /// Why the step can't be held, if it can't.
    pub fn check(&self) -> Result<(), String> {
        if let Some(b) = self
            .buttons
            .iter()
            .find(|b| !PAD_BUTTONS.contains(&b.to_ascii_lowercase().as_str()))
        {
            return Err(format!(
                "no button {b}; buttons are {}",
                PAD_BUTTONS.join(" ")
            ));
        }
        let unit = |v: f32| v.is_finite() && (-1.0..=1.0).contains(&v);
        if !self.left.iter().chain(&self.right).all(|v| unit(*v)) {
            return Err("stick axes go from -1 to 1".into());
        }
        if !self
            .lt
            .iter()
            .chain(&self.rt)
            .all(|v| unit(*v) && *v >= 0.0)
        {
            return Err("triggers go from 0 to 1".into());
        }
        Ok(())
    }
}

/// When the first step starts: a game tick, or `"now"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AtTick {
    /// Once the game's own tick (the stamp's `tick`) reaches this.
    Tick(u64),
    /// `"now"`: at the next read.
    Now(Now),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Now {
    Now,
}

/// `input.pad`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PadParams {
    /// The controller port, 0 to 3 (default 0, player one).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub player: u32,
    /// When the first of these steps starts (default `"now"`; with
    /// `append`, once those before it are done and this tick has come).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_tick: Option<AtTick>,
    pub steps: Vec<PadStep>,
    /// Queue after what's already queued instead of replacing it (the
    /// default replaces it, and the step being held, at once).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub append: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_parse_and_are_checked() {
        let p: PadParams = serde_json::from_str(
            r#"{"steps":[{"buttons":["A","start"],"ms":100},{"left":[0,1],"ms":2000,"reads":3},{}]}"#,
        )
        .unwrap();
        assert_eq!(p.steps.len(), 3);
        assert!(!p.append);
        assert!(p.steps.iter().all(|s| s.check().is_ok()));
        assert_eq!(p.steps[1].reads, Some(3));
        assert_eq!(p.steps[2].ms, None);
        for bad in [
            r#"{"buttons":["jump"]}"#,
            r#"{"left":[2,0]}"#,
            r#"{"rt":-0.5}"#,
        ] {
            let s: PadStep = serde_json::from_str(bad).unwrap();
            assert!(s.check().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<PadStep>(r#"{"button":["a"]}"#).is_err());
        let p: PadParams = serde_json::from_str(
            r#"{"player":1,"at_tick":120,"steps":[{"buttons":["a"],"ticks":2}]}"#,
        )
        .unwrap();
        assert_eq!((p.player, p.at_tick), (1, Some(AtTick::Tick(120))));
        assert_eq!(p.steps[0].ticks, Some(2));
        let p: PadParams = serde_json::from_str(r#"{"at_tick":"now","steps":[]}"#).unwrap();
        assert_eq!(p.at_tick, Some(AtTick::Now(Now::Now)));
        assert!(serde_json::from_str::<PadParams>(r#"{"at_tick":"soon","steps":[]}"#).is_err());
    }
}
