//! `bots.bounce_to`, `bots.bounce_status`: send a brain-driven bot
//! somewhere by way of a superbounce (docs/runtime/agent-api.md, "Bounce
//! routes"; the plan is `bots::bounce_route`). Wire types only.

use serde::{Deserialize, Serialize};

/// `bots.bounce_to`: ask a bot for a bounce route to a point. An act.
pub const BOUNCE_TO: &str = "bots.bounce_to";
/// `bots.bounce_status`: where a bot's bounce route stands. A read.
pub const BOUNCE_STATUS: &str = "bots.bounce_status";

/// The methods above.
pub const ALL: &[&str] = &[BOUNCE_TO, BOUNCE_STATUS];

/// Which bot: its `handle` from `bots.stats`, its gamertag, or its
/// `player.<n>` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BotRef {
    Handle(u32),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BounceToParams {
    pub bot: BotRef,
    /// Where to end up, world units.
    pub to: [f32; 3],
    /// `help` (default): a bounce only where it gets the bot there
    /// when walking cannot, or much sooner; else the plain walk. `fun`:
    /// the highest launch within reach, then on to `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BounceStatusParams {
    pub bot: BotRef,
}

/// The result of `bots.bounce_to`: the route is planned at the bot's next
/// sample, so read `bots.bounce_status` for the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceAccepted {
    pub bot: String,
    pub handle: u32,
    pub mode: String,
    pub to: [f32; 3],
}

/// The result of `bots.bounce_status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceStatus {
    pub bot: String,
    pub handle: u32,
    /// `none`, `queued`, `running`, `done` or `failed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `helps`, `fun`, or `plain` (no bounce helped: the plain walk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The plan's steps: `walk_crouched`, `prime`, `walk`, `bounce`,
    /// `drop`, `fly`, `nav`.
    pub steps: Vec<String>,
    /// The step it is on (an index into `steps`; for a failed route, the
    /// step it failed on).
    pub step: usize,
    /// The step's phase while running: a prime's `start`, `in`, `release`,
    /// `out` (or `crouch`, `stand` in place); a drop's `line_up`,
    /// `run_off`, `air`; a bounce's `settle`, `air`; a walk's `walking`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
    /// The step's try, from 1 (0 for a step that does not retry).
    #[serde(default)]
    pub tries: u32,
    /// The last touchdown during a bounce or a drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<BounceLanding>,
    /// What the route did, oldest first (at most 64 lines).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<BounceLog>,
    /// The level pair word is at -3: primed and not yet bounced.
    pub primed: bool,
    /// Whether `primed` was read from the game or assumed after the
    /// crouch and the stand-up.
    pub primed_read: bool,
    /// Why a failed route failed: `timeout`, `no_map`, `no_path`,
    /// `goal_off_mesh`, `path_lost`, `prime_failed`, `pair_lost`,
    /// `no_bounce`, `no_drop` (walked off the launch floor and did not
    /// fall), `stalled`, `state` (an internal fault), `enemy` (a
    /// show-off), `no_route` (a show-off or a practice with no route),
    /// `practice_off` (the practice stopped), `died` or `map_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The bot's bounce practice while the superbounce mode is on: it
    /// fights no one and practises bounces instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice: Option<BouncePractice>,
    /// The knobs the route's site is flown with (its learned line's, or a
    /// training try's; docs/game/bounce-search.md); `null` for the
    /// route's own.
    #[serde(default)]
    pub knobs: Option<LearnedKnobs>,
}

/// A bot's bounce practice so far.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BouncePractice {
    /// `idle` (between tries), `route` (on one) or `waiting` (no site
    /// within reach).
    pub state: String,
    /// The site being tried: its stand point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<[f32; 3]>,
    pub attempts: u32,
    pub bounces: u32,
    /// By site, in the order first tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<PracticeSpot>,
    /// In a sim's training run, the round the bot flies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub train: Option<PracticeTrain>,
}

/// A training bot's round (docs/game/bounce-search.md, "The search").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PracticeTrain {
    /// The site's stand point, as its learned line keys it.
    pub site: [f32; 3],
    /// `arms`, `shift`, `compass` or `scatter`.
    pub stage: String,
    /// In `compass`, the round's axis: `aim_along`, `aim_side`,
    /// `from_side`, `yaw`, `back`, `run`, `early`, `gain`, `letgo`,
    /// `drift` or `still`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    /// The try's slot in the round, from 0.
    pub slot: usize,
    /// The site's scored tries so far.
    pub tries: u32,
}

/// A bounce site's knobs, by the learned file's keys
/// (docs/game/bounce-search.md, "The knobs"): only those set. One left out
/// is the route's own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedKnobs {
    /// `runoff`, `jump` or `stand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    /// The launch point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fy: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fz: Option<f32>,
    /// The aim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ay: Option<f32>,
    /// Degrees off the launch-to-aim line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    /// The run-up behind the launch point (world units).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<f32>,
    /// The run's stick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<f32>,
    /// How far short of the launch point a running jump is pressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early: Option<f32>,
    /// The air stick's correction, and how near the aim it lets go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letgo: Option<f32>,
    /// A standing jump's air stick toward the drift, and its stand before
    /// the jump (ticks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still: Option<u32>,
}

/// What a map's learned file says of a bounce spot (`superbounce.spots`'
/// `learned`; docs/game/bounce-search.md, "The file").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpotLearned {
    /// `open`, `learned`, `partial`, `dead` or `blocked`.
    pub state: String,
    /// The knobs the line sets.
    #[serde(default)]
    pub knobs: LearnedKnobs,
    /// Scored tries, and of them bounced.
    pub tries: u32,
    pub hits: u32,
    /// Tries at the best knobs, of them bounced, and their mean score.
    pub n: u32,
    pub ok: u32,
    pub score: f32,
    /// The best launch seen (world units a second, up).
    #[serde(default)]
    pub best: f32,
    /// A `dead` or `blocked` line's reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One site's practice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PracticeSpot {
    pub stand: [f32; 3],
    pub attempts: u32,
    pub bounces: u32,
    /// Why the other tries did not bounce, by count: a route's error, or
    /// the landing's result for one that landed without launching.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub why: std::collections::BTreeMap<String, u32>,
}

/// A touchdown by the route's strip, as the bot saw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceLanding {
    pub tick: u32,
    /// The origin on the tick before touchdown.
    pub at: [f32; 3],
    /// Down, world units a second.
    pub speed_in: f32,
    /// Up, the best of the touchdown tick and the two after.
    pub vz_out: f32,
    /// Sideways from the site's target, world units.
    pub miss: f32,
    /// `bounce`, `not_primed`, `missed`, `too_slow` or `no_bounce`, as the
    /// game's landing classifier names them.
    pub result: String,
}

/// One line of a route's log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceLog {
    pub tick: u32,
    /// The step's name.
    pub step: String,
    /// `start`, `done`, `retry` (another drop), `reprime` (the word was
    /// lost on the way), a landing's result, or why the step failed.
    pub what: String,
    pub at: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_shapes_round_trip_and_omit_what_is_unset() {
        let k = LearnedKnobs {
            how: Some("runoff".into()),
            fx: Some(-8.33),
            fy: Some(2.54),
            fz: Some(-0.76),
            ax: Some(-10.54),
            ay: Some(4.09),
            back: Some(1.2),
            run: Some(1.0),
            ..LearnedKnobs::default()
        };
        let v = serde_json::to_value(&k).unwrap();
        assert_eq!(v["how"], "runoff");
        assert!(v.get("yaw").is_none() && v.get("still").is_none(), "{v}");
        assert_eq!(serde_json::from_value::<LearnedKnobs>(v).unwrap(), k);
        // No knob set: an empty object.
        assert_eq!(
            serde_json::to_value(LearnedKnobs::default()).unwrap(),
            serde_json::json!({})
        );
        assert!(serde_json::from_value::<LearnedKnobs>(serde_json::json!({"wat": 1})).is_err());

        let l = SpotLearned {
            state: "dead".into(),
            knobs: k.clone(),
            tries: 120,
            hits: 0,
            n: 6,
            ok: 0,
            score: 0.412,
            best: 0.0,
            why: Some("budget".into()),
        };
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["why"], "budget");
        assert_eq!(serde_json::from_value::<SpotLearned>(v).unwrap(), l);

        let t = PracticeTrain {
            site: [-10.62, 3.88, -3.19],
            stage: "compass".into(),
            axis: Some("aim_along".into()),
            slot: 3,
            tries: 54,
        };
        let p = BouncePractice {
            state: "route".into(),
            train: Some(t),
            ..BouncePractice::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["train"]["axis"], "aim_along");
        assert_eq!(serde_json::from_value::<BouncePractice>(v).unwrap(), p);
        // A plain practice has no train.
        let v = serde_json::to_value(BouncePractice::default()).unwrap();
        assert!(v.get("train").is_none());

        // The status's knobs: null for the route's own, else the set ones.
        let mut s = BounceStatus {
            bot: "Clanker".into(),
            handle: 2,
            state: "running".into(),
            to: None,
            mode: None,
            why: None,
            steps: vec!["walk".into(), "drop".into()],
            step: 1,
            phase: String::new(),
            tries: 1,
            landing: None,
            log: Vec::new(),
            primed: true,
            primed_read: true,
            error: None,
            practice: None,
            knobs: None,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(v["knobs"].is_null(), "{v}");
        assert_eq!(serde_json::from_value::<BounceStatus>(v).unwrap(), s);
        s.knobs = Some(k);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["knobs"]["back"], serde_json::json!(1.2f32));
        assert_eq!(serde_json::from_value::<BounceStatus>(v).unwrap(), s);
    }
}
