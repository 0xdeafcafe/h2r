//! Playback UI models shared by the shell, the agent and the offline preview.

use agent_proto::theater::{CameraMode, CompatibilityState};
use serde::Deserialize;
use std::collections::BTreeSet;

use super::adapter::Player;
use super::overlay::Track;
use super::selector::clock;
use super::{OVERLAY, PAUSE_MENU};
use crate::list_class::ListLayout;
use crate::ui_kit::{
    input::{Glyph, Input, Prompt, footer_legend},
    picker,
    spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle},
    text_fit,
    view::{Art, PaneArt},
};

const BAR: [i16; 4] = [-105, -272, -208, 272];
const RAIL: [i16; 4] = [-144, -252, -154, 252];
const PANEL: [u8; 4] = [12, 23, 39, 238];
const BODY: [f32; 3] = [0.86, 0.91, 0.96];
const MUTED: [f32; 3] = [0.65, 0.76, 0.87];

/// Scrubbing uses the same rectangle as the rail, in the kit's canvas pixels.
pub const TIMELINE: [u32; 4] = [
    (320 + RAIL[1]) as u32,
    (240 - RAIL[0] - 4) as u32,
    (RAIL[3] - RAIL[1]) as u32,
    (RAIL[0] - RAIL[2] + 8) as u32,
];

#[derive(Clone, Copy, Debug, Deserialize)]
pub(super) struct Jump {
    pub target: u64,
    pub from_tick: u64,
    pub remaining_ticks: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Divergence {
    pub tick: u64,
    pub kind: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct Keyframes {
    #[serde(default)]
    pub sidecar: Vec<u64>,
    #[serde(default)]
    pub ring: Vec<u64>,
}

fn cell(id: &str, rect: [i16; 4], text: &str, colour: [f32; 3], right: bool) -> PaneText {
    PaneText {
        id: id.into(),
        rect,
        text: text_fit::line(text, i32::from(rect[3] - rect[1])),
        font: 1,
        right,
        colour,
    }
}

fn fill(s: &mut ScreenSpec, rect: [i16; 4], rgba: [u8; 4]) {
    let [top, left, bottom, right] = rect;
    if top > bottom && right > left {
        s.panel.push(PaneArt {
            art: Art::Solid { rgba },
            at: [left, top],
            size: [(right - left) as u16, (top - bottom) as u16],
            depth: 0,
        });
    }
}

fn transparent_page(name: &str, title: &str, rows: i16) -> ScreenSpec {
    let mut spec = ScreenSpec::new(name, title);
    spec.transparent = true;
    spec.list = ListPlace::from(Some(ListLayout {
        skin: 0,
        anchor: [-262, 135],
        bitmaps_left_of: i16::MIN,
        visible: rows,
    }));
    spec.style = ValueStyle::Inline;
    spec
}

fn selected_colour(player: Option<&Player>) -> Option<[u8; 3]> {
    use super::adapter::team_colour;
    player.and_then(|p| p.emblem_colour.or_else(|| team_colour(p.team.as_deref()?)))
}

pub(super) fn bar(track: &Track, ready: bool, notice: Option<&str>) -> ScreenSpec {
    let mut spec = transparent_page(OVERLAY, "", 0);
    if !track.visible {
        return spec;
    }

    fill(&mut spec, BAR, PANEL);
    let status = if track.compatibility == CompatibilityState::Unavailable {
        "Unavailable"
    } else if !ready {
        "Opening film"
    } else if track.divergence.is_some() {
        "Playback stopped"
    } else if track.seeking {
        "Jumping"
    } else if track.paused {
        "Paused"
    } else {
        "Playing"
    };
    let time = format!(
        "{status}    {} / {}",
        clock(track.tick, track.tick_rate),
        clock(track.length, track.tick_rate)
    );
    let speed = match track.speed_reached {
        Some(reached) => format!("{}x / {reached:.2}x", track.speed_requested),
        None => format!("{}x / --", track.speed_requested),
    };
    let camera = track.camera_label();
    let subject = track.camera_target();
    let camera = if let Some(player) = subject {
        format!("{camera} / {}", player.name)
    } else {
        camera.into()
    };
    spec.texts.extend([
        cell(
            "transport.time",
            [-111, -252, -134, -63],
            &time,
            BODY,
            false,
        ),
        cell(
            "transport.speed",
            [-111, -54, -134, 60],
            &speed,
            MUTED,
            false,
        ),
        cell(
            "transport.camera",
            [-111, 68, -134, 234],
            &camera,
            BODY,
            true,
        ),
    ]);

    if let Some([r, g, b]) = selected_colour(subject) {
        fill(&mut spec, [-116, 240, -128, 252], [230, 238, 255, 255]);
        fill(&mut spec, [-117, 241, -127, 251], [r, g, b, 255]);
    }

    if ready && track.compatibility == CompatibilityState::Playable {
        timeline(&mut spec, track);
    }
    let unavailable = (track.compatibility == CompatibilityState::Unavailable).then(|| {
        track
            .compatibility_reason
            .as_deref()
            .unwrap_or("Playback is unavailable")
            .to_owned()
    });
    let detail = unavailable
        .or_else(|| {
            track.divergence.as_ref().map(|d| {
                format!(
                    "Replay diverged at {} ({})",
                    clock(d.tick, track.tick_rate),
                    d.kind
                )
            })
        })
        .or_else(|| {
            track.jump.filter(|_| track.seeking).map(|j| {
                format!(
                    "Target {}   /   {} ticks left",
                    clock(j.target, track.tick_rate),
                    j.remaining_ticks
                )
            })
        })
        .or_else(|| notice.map(str::to_owned));
    let clip = track.marks.0.map(|start| {
        let end = track
            .marks
            .1
            .map_or_else(|| "--".into(), |tick| clock(tick, track.tick_rate));
        format!("Clip {} - {end}", clock(start, track.tick_rate))
    });
    spec.texts.push(cell(
        "transport.detail",
        [-159, -252, -183, if clip.is_some() { 75 } else { 252 }],
        detail
            .as_deref()
            .unwrap_or("Kills  Sidecar keyframes  Ring"),
        if track.divergence.is_some() {
            [1.0, 0.62, 0.56]
        } else {
            MUTED
        },
        false,
    ));
    if let Some(clip) = clip {
        spec.texts.push(cell(
            "transport.clip",
            [-159, 83, -183, 252],
            &clip,
            BODY,
            true,
        ));
    }
    spec.prompts = vec![
        Prompt::new(&[Glyph::A], "Play / Pause"),
        Prompt::new(&[Glyph::Start], "Menu"),
        Prompt::new(&[Glyph::Back], "Hide"),
    ];
    if track.seeking {
        spec.prompts = vec![Prompt::new(&[Glyph::B], "Cancel Jump")];
    }
    spec.texts.push(PaneText {
        id: "transport.legend".into(),
        rect: [-184, -252, -207, 252],
        font: 1,
        right: true,
        colour: BODY,
        text: footer_legend(&spec.prompts),
    });
    spec.inputs = vec![
        Input::A,
        Input::B,
        Input::Up,
        Input::Down,
        Input::Left,
        Input::Right,
        Input::X,
        Input::Y,
        Input::Start,
        Input::Back,
    ];
    spec
}

fn timeline(spec: &mut ScreenSpec, t: &Track) {
    let [top, left, bottom, right] = RAIL;
    let (start, end) = t.bounds();
    let x = |tick: u64| {
        let tick = tick.clamp(start, end) - start;
        left + (u128::from(tick) * (right - left - 1) as u128 / u128::from((end - start).max(1)))
            as i16
    };
    let tick = x(t.tick);
    fill(spec, RAIL, [33, 48, 70, 255]);
    fill(spec, [top, left, bottom, tick + 1], [70, 105, 142, 255]);
    if let (Some(first), Some(last)) =
        (t.keyframes.ring.iter().min(), t.keyframes.ring.iter().max())
        && *first <= end
        && *last >= start
    {
        fill(
            spec,
            [top, x(*first), bottom, x(*last) + 1],
            [52, 106, 100, 255],
        );
    }
    if let Some(jump) = t.jump.filter(|_| t.seeking) {
        let total = jump.target.abs_diff(jump.from_tick).max(1);
        let remaining = jump.remaining_ticks.min(total);
        let reached = if jump.from_tick <= jump.target {
            jump.target.saturating_sub(remaining)
        } else {
            jump.target.saturating_add(remaining)
        };
        let from = x(jump.from_tick);
        let to = x(reached);
        fill(
            spec,
            [top, from.min(to), bottom, from.max(to) + 1],
            [136, 106, 59, 255],
        );
        let target = x(jump.target);
        fill(
            spec,
            [top + 3, target, bottom - 3, target + 1],
            [250, 203, 114, 255],
        );
    }
    for (marks, colour, y1, y2) in [
        (&t.kills, [230, 117, 101, 255], top, top - 4),
        (
            &t.keyframes.sidecar,
            [146, 188, 251, 255],
            bottom + 4,
            bottom,
        ),
        (&t.keyframes.ring, [132, 219, 190, 255], bottom + 4, bottom),
    ] {
        let positions = marks
            .iter()
            .copied()
            .filter(|tick| (start..=end).contains(tick))
            .map(x)
            .collect::<BTreeSet<_>>();
        for at in positions {
            fill(spec, [y1, at, y2, at + 1], colour);
        }
    }
    for mark in [t.marks.0, t.marks.1]
        .into_iter()
        .flatten()
        .filter(|t| (start..=end).contains(t))
    {
        let at = x(mark);
        fill(
            spec,
            [top + 2, at, bottom - 2, at + 1],
            [248, 207, 102, 255],
        );
    }
    fill(
        spec,
        [top + 2, tick, bottom - 2, tick + 1],
        [230, 238, 255, 255],
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Resume,
    Restart,
    Player,
    Camera,
    Screenshot,
    Mark,
    Export,
    Save,
    Share,
    End,
}

impl Row {
    const ALL: [Self; 10] = [
        Self::Resume,
        Self::Restart,
        Self::Player,
        Self::Camera,
        Self::Screenshot,
        Self::Mark,
        Self::Export,
        Self::Save,
        Self::Share,
        Self::End,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume",
            Self::Restart => "Restart Film",
            Self::Player => "Change Player",
            Self::Camera => "Camera",
            Self::Screenshot => "Take Screenshot",
            Self::Mark => "Mark Clip",
            Self::Export => "Export Clip",
            Self::Save => "Save Film",
            Self::Share => "Share",
            Self::End => "End Film",
        }
    }

    fn enabled(self, track: &Track) -> bool {
        match self {
            Self::Player => !track.players.is_empty(),
            Self::Export => track.clip_range().is_some(),
            _ => true,
        }
    }

    fn choices(self, track: &Track) -> Option<Choices> {
        let mut current = 0;
        let values = match self {
            Self::Player => {
                current = track.selected_player;
                track
                    .players
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.name.clone(), Command::Player(i)))
                    .collect()
            }
            Self::Camera => {
                let cameras = track.cameras();
                current = cameras
                    .iter()
                    .position(|&(mode, _)| mode == track.camera)
                    .unwrap_or(0);
                cameras
                    .iter()
                    .map(|&(mode, label)| (label.into(), Command::Camera(mode)))
                    .collect()
            }
            Self::Mark => {
                let mut marks = vec![("Set In".into(), Command::MarkIn)];
                if track.can_mark_out() {
                    marks.push(("Set Out".into(), Command::MarkOut));
                }
                if track.marks.0.is_some() || track.marks.1.is_some() {
                    marks.push(("Clear Marks".into(), Command::ClearMarks));
                }
                marks
            }
            Self::Export => ["png", "mp4"]
                .into_iter()
                .flat_map(|format| {
                    [30, 60].map(|fps| {
                        (
                            format!("{} / {fps} fps", format.to_ascii_uppercase()),
                            Command::Export(format, fps),
                        )
                    })
                })
                .collect(),
            Self::Share => [
                ("Private link", Command::Share("unlisted")),
                ("Public film", Command::Share("public")),
            ]
            .into_iter()
            .map(|(label, action)| (label.into(), action))
            .collect(),
            Self::End => vec![
                ("Cancel".into(), Command::Cancel),
                ("End Film".into(), Command::End),
            ],
            _ => return None,
        };
        (!values.is_empty()).then(|| Choices {
            row: self,
            focus: current.min(values.len() - 1),
            values,
        })
    }

    fn command(self) -> Option<Command> {
        Some(match self {
            Self::Resume => Command::Resume,
            Self::Restart => Command::Restart,
            Self::Screenshot => Command::Screenshot,
            Self::Save => Command::Save,
            _ => return None,
        })
    }

    fn help(self) -> &'static str {
        match self {
            Self::Player => "Choose the camera's subject.",
            Self::Camera => "Choose how to view the film.",
            Self::Mark => "Set or clear the clip range.",
            Self::Export => "Export the marked range.",
            Self::Share => "Choose who can see the film.",
            Self::End => "Return to the film library.",
            _ => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Resume,
    Restart,
    Player(usize),
    Camera(CameraMode),
    Screenshot,
    MarkIn,
    MarkOut,
    ClearMarks,
    Export(&'static str, u8),
    Save,
    Share(&'static str),
    End,
    Cancel,
}

struct Choices {
    row: Row,
    focus: usize,
    values: Vec<(String, Command)>,
}

#[derive(Default)]
pub(super) struct Menu {
    focus: usize,
    picker: Option<Choices>,
}

impl Menu {
    pub(super) fn press(&mut self, track: &Track, input: Input) -> Option<Command> {
        if let Some(dialog) = &mut self.picker {
            match input {
                Input::Up => dialog.focus = dialog.focus.saturating_sub(1),
                Input::Down => dialog.focus = (dialog.focus + 1).min(dialog.values.len() - 1),
                Input::A => {
                    let action = dialog.values[dialog.focus].1;
                    self.picker = None;
                    return Some(action);
                }
                Input::B | Input::Back => self.picker = None,
                _ => {}
            }
            return None;
        }
        match input {
            Input::Up => self.focus = self.focus.saturating_sub(1),
            Input::Down => self.focus = (self.focus + 1).min(Row::ALL.len() - 1),
            Input::A => {
                let row = Row::ALL[self.focus];
                if row.enabled(track) {
                    self.picker = row.choices(track);
                    return row.command();
                }
            }
            Input::B | Input::Start => return Some(Command::Resume),
            _ => {}
        }
        None
    }

    pub(super) fn spec(&self, track: &Track, notice: Option<&str>) -> ScreenSpec {
        let mut spec = transparent_page(PAUSE_MENU, "THEATER", Row::ALL.len() as i16);
        spec.focus = self.focus;
        spec.rows = Row::ALL
            .into_iter()
            .map(|row| {
                if !row.enabled(track) {
                    let value = if row == Row::Export {
                        "No marks"
                    } else {
                        "No players"
                    };
                    return RowSpec::info(row.label(), value);
                }
                let value = match row {
                    Row::Player => track.camera_target().map(|p| p.name.as_str()),
                    Row::Camera => Some(track.camera_label()),
                    _ => None,
                };
                if let Some(value) = value {
                    let room = text_fit::ROW - text_fit::width(row.label()) - text_fit::width(": ");
                    RowSpec::info(row.label(), &text_fit::line(value, room))
                        .press(Press::Open(picker::GAME_NAME.into()))
                } else if row.command().is_some() {
                    RowSpec::action(row.label())
                } else {
                    RowSpec::action(row.label()).press(Press::Open(picker::GAME_NAME.into()))
                }
            })
            .collect();
        spec.footer = true;
        spec.prompts = vec![
            Prompt::new(&[Glyph::A], "Select"),
            Prompt::new(&[Glyph::B], "Resume"),
        ];
        spec.inputs = vec![Input::Up, Input::Down, Input::A, Input::B, Input::Start];
        fill(&mut spec, [178, -272, -180, 272], PANEL);
        scoreboard(&mut spec, &track.scoreboard);
        if let Some(notice) = notice {
            spec.texts.push(cell(
                "pause.notice",
                [-110, -242, -135, -28],
                notice,
                MUTED,
                false,
            ));
        }
        if let Some(pick) = &self.picker {
            let values: Vec<_> = pick
                .values
                .iter()
                .map(|(label, _)| text_fit::line(label, text_fit::ROW))
                .collect();
            let mut dialog = picker::playback_spec(
                &picker::title(pick.row.label()),
                &values,
                pick.focus,
                pick.row.help(),
            );
            dialog.underlay = Some(Box::new(spec));
            return dialog;
        }
        spec
    }
}

fn scoreboard(spec: &mut ScreenSpec, players: &[Player]) {
    spec.texts.push(cell(
        "scoreboard.title",
        [170, 19, 149, 164],
        "Scoreboard",
        BODY,
        false,
    ));
    if players.is_empty() {
        spec.texts.push(cell(
            "scoreboard.empty",
            [147, 19, 126, 256],
            "No match stats available",
            MUTED,
            false,
        ));
        return;
    }
    spec.texts.push(cell(
        "scoreboard.score",
        [170, 168, 149, 210],
        "Score",
        MUTED,
        true,
    ));
    let has_kd = players
        .iter()
        .any(|p| p.kills.is_some() || p.deaths.is_some());
    if has_kd {
        spec.texts.push(cell(
            "scoreboard.kd",
            [170, 215, 149, 258],
            "K / D",
            MUTED,
            true,
        ));
    }
    for (row, p) in players.iter().take(16).enumerate() {
        let top = 148 - row as i16 * 20;
        let bottom = top - 20;
        spec.texts.push(cell(
            &format!("scoreboard.{row}.name"),
            [top, 19, bottom, 164],
            &p.name,
            BODY,
            false,
        ));
        if let Some(score) = p.score {
            spec.texts.push(cell(
                &format!("scoreboard.{row}.score"),
                [top, 168, bottom, 210],
                &score.to_string(),
                BODY,
                true,
            ));
        }
        if has_kd {
            let count = |n: Option<u32>| n.map_or_else(|| "-".into(), |n| n.to_string());
            let kd = format!("{} / {}", count(p.kills), count(p.deaths));
            spec.texts.push(cell(
                &format!("scoreboard.{row}.kd"),
                [top, 215, bottom, 258],
                &kd,
                BODY,
                true,
            ));
        }
    }
}

#[cfg(test)]
mod tests;
