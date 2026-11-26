//! The Firefight HUD's banners (docs/game/firefight.md, "The HUD"): the
//! centred line that names a set, a round, a skull or a wave when it
//! starts, and the ends that stay up.
//!
//! [`events`] reads what began between two mode blobs; [`Banners`] queues
//! them, three seconds each, easing in and out. A newer tick's events
//! replace an older one's, the queue included; the events of one tick keep
//! their order: the set, the round, the skull, the wave last so it is the
//! one that lingers. A banner that ends the match stays until the blob
//! stops.

use std::collections::VecDeque;

use super::{Mode, Stage, lives_of};

/// The game's tick rate.
pub const TICKS_PER_SECOND: u16 = 30;
/// A banner is up three seconds.
pub const HOLD: u16 = 3 * TICKS_PER_SECOND;
/// It eases in over 200 ms, and runs out over 200 ms.
pub const EASE: u16 = TICKS_PER_SECOND / 5;

/// What a banner says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Banner {
    /// One-based.
    Set(u16),
    Round(u16),
    /// A skull switched on, by its name.
    Skull(&'static str),
    Wave(u16),
    BossWave,
    WaveClear,
    RoundComplete,
    SetComplete,
    BonusRound,
    /// The bonus round is over, and the lives it gave the viewer (0: none).
    BonusOver(u16),
    LastLife,
    Victory,
    GameOver,
    RedWins,
    BlueWins,
    Draw,
}

impl Banner {
    /// The words, as the HUD font shows them.
    pub fn text(&self) -> String {
        match self {
            Banner::Set(n) => format!("SET {n}"),
            Banner::Round(n) => format!("ROUND {n}"),
            Banner::Skull(name) => format!("{} ON", name.to_uppercase()),
            Banner::Wave(n) => format!("WAVE {n}"),
            Banner::BossWave => "BOSS WAVE".into(),
            Banner::WaveClear => "WAVE CLEAR".into(),
            Banner::RoundComplete => "ROUND COMPLETE".into(),
            Banner::SetComplete => "SET COMPLETE".into(),
            Banner::BonusRound => "BONUS ROUND".into(),
            Banner::BonusOver(0) => "BONUS OVER".into(),
            Banner::BonusOver(1) => "BONUS OVER 1 LIFE".into(),
            Banner::BonusOver(n) => format!("BONUS OVER {} LIVES", (*n).min(99)),
            Banner::LastLife => "LAST LIFE".into(),
            Banner::Victory => "VICTORY".into(),
            Banner::GameOver => "GAME OVER".into(),
            Banner::RedWins => "RED TEAM WINS".into(),
            Banner::BlueWins => "BLUE TEAM WINS".into(),
            Banner::Draw => "DRAW".into(),
        }
    }

    /// The match is over: the banner stays up.
    pub fn sticky(&self) -> bool {
        matches!(
            self,
            Banner::Victory | Banner::GameOver | Banner::RedWins | Banner::BlueWins | Banner::Draw
        )
    }
}

/// The ending banner of an over stage.
fn ending(stage: Stage) -> Banner {
    match stage {
        Stage::Won => Banner::Victory,
        Stage::Team0Won => Banner::RedWins,
        Stage::Team1Won => Banner::BlueWins,
        Stage::Draw => Banner::Draw,
        _ => Banner::GameOver,
    }
}

/// The banners that begin between `before` and `now`, for the player at
/// roster slot `local`, in the order they show: what completed, the bonus
/// round, the set, the round, each skull (lowest index first, by its name
/// from `skull_name`; one with no name has none), the last life, the wave.
/// None at a match's first blob (the skulls on from the start are the
/// lobby's) and after its end.
pub fn events(
    before: Option<&Mode>,
    now: &Mode,
    local: u8,
    skull_name: impl Fn(u8) -> Option<&'static str>,
) -> Vec<Banner> {
    let Some(b) = before.filter(|b| !b.stage.over()) else {
        return Vec::new();
    };
    if now.stage.over() {
        return vec![ending(now.stage)];
    }
    let mut out = Vec::new();
    if now.stage == Stage::Intermission && b.stage != Stage::Intermission {
        out.push(if now.wave == 0 && now.round == 0 && now.set > 0 {
            Banner::SetComplete
        } else if now.wave == 0 && now.round > 0 {
            Banner::RoundComplete
        } else {
            Banner::WaveClear
        });
    }
    if now.bonus_left > 0 && b.bonus_left == 0 {
        out.push(Banner::BonusRound);
    } else if now.bonus_left == 0 && b.bonus_left > 0 {
        let gained = match (lives_of(b, local), lives_of(now, local)) {
            (Some(was), Some(is)) => is.saturating_sub(was),
            _ => 0,
        };
        out.push(Banner::BonusOver(gained));
    }
    if now.set > b.set {
        out.push(Banner::Set(u16::from(now.set) + 1));
    }
    if (now.set, now.round) != (b.set, b.round) {
        out.push(Banner::Round(u16::from(now.round) + 1));
    }
    let on = now.skulls & !b.skulls;
    out.extend(
        (0..32u8)
            .filter(|n| on & (1 << n) != 0)
            .filter_map(&skull_name)
            .map(Banner::Skull),
    );
    if let (Some(was), Some(is)) = (lives_of(b, local), lives_of(now, local)) {
        if was > 1 && is == 1 {
            out.push(Banner::LastLife);
        }
    }
    let new_wave = (now.set, now.round, now.wave) != (b.set, b.round, b.wave);
    if now.stage == Stage::Spawning && (b.stage != Stage::Spawning || new_wave) {
        out.push(if now.boss {
            Banner::BossWave
        } else {
            Banner::Wave(u16::from(now.wave) + 1)
        });
    }
    out
}

/// A banner on screen: how opaque it is, 0 to 255.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub banner: Banner,
    pub alpha: u8,
}

/// The banner queue of one viewer.
#[derive(Clone, Debug, Default)]
pub struct Banners {
    /// The banner up and the ticks it has been up.
    current: Option<(Banner, u16)>,
    queue: VecDeque<Banner>,
}

fn alpha(age: u16, sticky: bool) -> u8 {
    let ramp = |t: u16| (u32::from(t.min(EASE)) * 255 / u32::from(EASE)) as u8;
    if age < EASE {
        ramp(age)
    } else if !sticky && age + EASE >= HOLD {
        ramp(HOLD - age)
    } else {
        255
    }
}

impl Banners {
    /// One tick with the events [`events`] gave for it: the banner up, if
    /// any. Events replace what is up and what is waiting; with none the
    /// banner up ages, and the next waiting takes its place when its three
    /// seconds are over.
    pub fn tick(&mut self, events: Vec<Banner>) -> Option<Shown> {
        if !events.is_empty() {
            self.queue = events.into();
            self.current = None;
        } else if let Some((banner, age)) = &mut self.current {
            *age += 1;
            if !banner.sticky() && *age >= HOLD {
                self.current = None;
            }
        }
        if self.current.is_none() {
            self.current = self.queue.pop_front().map(|b| (b, 0));
        }
        self.current.as_ref().map(|(b, age)| Shown {
            banner: b.clone(),
            alpha: alpha(*age, b.sticky()),
        })
    }

    /// No match runs: nothing is up or waiting.
    pub fn clear(&mut self) {
        self.current = None;
        self.queue.clear();
    }
}
