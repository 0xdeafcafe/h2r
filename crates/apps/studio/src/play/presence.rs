//! Presentation leases on the paired game's control connection. A slow host gets
//! one outstanding call per feature. Its next replacement uses the latest selection.
mod camera;
pub(crate) use camera::{CameraMode, PlayerMove, SharedCamera};

use std::time::{Duration, Instant};

use agent_proto::{RpcError, editor};
use editor::{Mark, MarkBox, MarkGeometry, MarkStyle};
use scenario_edit::geometry::{Bounds, Marker, glyph_bounds, glyph_of, scale, sub};
use serde_json::{Value, json};

use super::{
    LinkState, Play,
    link::{Call, Purpose},
    select,
};
use crate::{App, app::Selection, theme};

const RENEW: Duration = Duration::from_secs(1);
const TAG_LIMIT: usize = 64;

#[derive(Debug, PartialEq)]
enum Target {
    Object(u32, MarkStyle),
    Box(MarkBox),
}

impl Target {
    fn mark(&self) -> Mark {
        let (geometry, style) = match self {
            Self::Object(id, style) => (MarkGeometry::Object(format!("{id:08x}")), *style),
            Self::Box(frame) => (MarkGeometry::Box(frame.clone()), MarkStyle::Selected),
        };
        let [r, g, b, _] = match style {
            MarkStyle::Selected => theme::ACCENT,
            MarkStyle::Candidate => theme::WARN,
        };
        Mark {
            geometry,
            style: Some(style),
            color: [r, g, b],
            label: None,
            xray: None,
        }
    }
}

fn placement_box(app: &App, marker: &Marker) -> MarkBox {
    let bounds = app
        .session
        .scene()
        .and_then(|scene| scene.models.get(scene.model_of(marker)?))
        .map(|mesh| mesh.bounds)
        .filter(|b| !b.is_empty() && b.max.iter().zip(b.min).all(|(a, b)| *a > b))
        .unwrap_or_else(|| {
            let (lift, r) = glyph_bounds(glyph_of(marker.block));
            Bounds {
                min: [-r, -r, lift - r],
                max: [r, r, lift + r],
            }
        });
    MarkBox {
        origin: scenario_edit::scene::model_to_world(marker, bounds.min),
        forward: marker.axes[0],
        up: marker.axes[2],
        size: scale(sub(bounds.max, bounds.min), marker.scale),
    }
}

/// Keep handles numeric until a replacement actually goes over the socket.
fn targets(
    app: &App,
    selection: Option<Selection>,
    objects: &[agent_proto::live::Object],
    live: Option<u32>,
    candidate: Option<u32>,
    out: &mut Vec<Target>,
) {
    out.clear();
    if let Some(id) = live {
        out.push(Target::Object(id, MarkStyle::Selected));
    } else {
        match selection {
            Some(Selection::Placement(..)) => {
                for marker in app
                    .session
                    .markers()
                    .iter()
                    .filter(|m| app.session.is_selected((m.block, m.index)))
                    .take(editor::MAX_MARKS - usize::from(candidate.is_some()))
                {
                    let kind = scenario_edit::layout::block(marker.block).object_type;
                    let preview = app.pairing.previews.object(app, marker.block, marker.index);
                    let linked = app.pairing.placement_matches(&app.session, marker.block);
                    let object = objects.iter().find(|o| {
                        kind == Some(i16::from(o.kind))
                            && (preview == Some(o.id)
                                || (preview.is_none() && linked
                                    && o.placement.map(usize::from) == Some(marker.index)))
                    });
                    out.push(match object {
                        Some(o) => Target::Object(o.id, MarkStyle::Selected),
                        None => Target::Box(placement_box(app, marker)),
                    });
                }
            }
            Some(Selection::Tag(tag)) => {
                out.extend(
                    objects
                        .iter()
                        .filter(|o| o.tag == Some(tag.0))
                        .take(TAG_LIMIT)
                        .map(|o| Target::Object(o.id, MarkStyle::Selected)),
                );
            }
            None => {}
        }
    }
    if let Some(id) = candidate {
        out.push(Target::Object(id, MarkStyle::Candidate));
    }
}

#[derive(Clone, Copy, PartialEq)]
struct MarkInputs {
    selection: crate::app::SelectionRevision,
    placement_geometry: Option<u64>,
    previews: u64,
    sample: Option<(u64, u64)>,
    segment: Option<u64>,
    epoch: Option<u64>,
    matching: bool,
    live: Option<u32>,
    candidate: Option<u32>,
}

#[derive(Default)]
pub(super) struct Presence {
    link_sent: Option<Instant>,
    link_pending: bool,
    no_indicator: bool,
    link_why: Option<String>,
    marks_sent: Option<Instant>,
    marks_pending: bool,
    marks_why: Option<String>,
    marks_waiting: Option<String>,
    marks_refused: Vec<String>,
    // A replacement may have reached the game even if its reply was lost.
    // Keep renewing an empty set until the host acknowledges its clear.
    clear_required: bool,
    sent_epoch: Option<u64>,
    sent: Vec<Target>,
    desired: Vec<Target>,
    inputs: Option<MarkInputs>,
    last_check: Option<Instant>,
    selection: Option<Selection>,
}

impl Presence {
    pub(super) fn reasons(&self) -> impl Iterator<Item = &str> {
        self.link_why
            .iter()
            .chain(&self.marks_why)
            .chain(&self.marks_waiting)
            .chain(&self.marks_refused)
            .map(String::as_str)
    }

    pub(super) fn transport_lost(&mut self) {
        *self = Self {
            clear_required: self.clear_required,
            ..Default::default()
        };
    }
}

fn due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= RENEW)
}

/// Windowed hosts can accept a lease before they present a frame with a camera.
fn waiting_for_frame(reason: &str) -> bool {
    reason == "no camera for the presented frame"
        || reason.split_ascii_whitespace().next()
            .is_some_and(|word| word.eq_ignore_ascii_case("waiting"))
}

impl Play {
    pub(super) fn update_presence(&mut self, app: &App, now: Instant) {
        self.closing_links.retain(super::link::ClosingLink::pending);
        if !self.is_paired()
            || self.closing_links.iter().any(|old| Some(&old.endpoint) == self.endpoint())
        {
            return;
        }
        if self.has_method(editor::method::LINK)
            && !self.presence.link_pending
            && !self.presence.no_indicator
            && due(self.presence.link_sent, now)
        {
            let mode = match app.pairing.link.as_ref().map(|l| &l.state) {
                Some(LinkState::Editing { .. }) => editor::Mode::Edit,
                Some(LinkState::Blocked { .. } | LinkState::OtherMap { .. }) => {
                    editor::Mode::Blocked
                }
                _ => editor::Mode::View,
            };
            let text = app.pairing.link.as_ref().and_then(|link| link.issue_summary())
                .or_else(|| app.pairing.selection.enabled.then(|| "Aim select".into()));
            let params = json!(editor::LinkParams {
                editor: Some("h2-editor".into()),
                mode: Some(mode),
                text,
                ttl_ms: Some(editor::LINK_TTL_MS),
                clear: None,
            });
            self.call(editor::method::LINK, params, Purpose::Indicator);
            self.presence.link_sent = Some(now);
            self.presence.link_pending = true;
        }
        if !self.has_method(editor::method::MARKS) || self.presence.marks_pending {
            return;
        }

        let sample = app.pairing.shown.as_ref();
        let other_map = app
            .pairing
            .link
            .as_ref()
            .is_some_and(|link| matches!(link.state, LinkState::OtherMap { .. }));
        let matching = !other_map
            && sample.is_some_and(|s| self.edits.epoch.is_none_or(|epoch| epoch == s.epoch))
            && select::matching(app);
        let selection = app.selection_revision();
        if self
            .presence
            .inputs
            .is_none_or(|old| old.selection != selection)
        {
            self.presence.selection = app.selection();
        }
        let inputs = MarkInputs {
            selection,
            previews: app.pairing.previews.revision(),
            placement_geometry: matches!(self.presence.selection, Some(Selection::Placement(..)))
                .then(|| app.session.markers_rev()),
            sample: sample.map(|s| (s.epoch, s.tick)),
            segment: app.pairing.display.segment(),
            epoch: self.edits.epoch,
            matching,
            live: select::selected_object(app),
            candidate: app.pairing.selection.candidate.map(|h| h.object),
        };
        let renew = self.presence.clear_required && due(self.presence.marks_sent, now);
        if self.presence.inputs == Some(inputs)
            && !renew
            && !due(self.presence.last_check, now)
        {
            return;
        }
        self.presence.inputs = Some(inputs);
        self.presence.last_check = Some(now);
        // A host can offer marks without the binary stream. A matched edit.map
        // then locates authored boxes, even in a read-only session.
        let static_only = sample.is_none()
            && self.presence.selection.is_some()
            && self.map_refusal(app).is_none();
        if matching || static_only {
            targets(
                app,
                self.presence.selection,
                sample.map_or(&[], |s| s.objects.as_slice()),
                inputs.live,
                inputs.candidate,
                &mut self.presence.desired,
            );
        } else {
            self.presence.desired.clear();
        }
        let epoch = inputs.sample.map(|(epoch, _)| epoch).or(inputs.epoch);
        let changed = self.presence.sent != self.presence.desired
            || (!self.presence.desired.is_empty() && self.presence.sent_epoch != epoch);
        if !changed && !renew {
            return;
        }
        let params = json!({
            "marks": self.presence.desired.iter().map(Target::mark).collect::<Vec<_>>(),
            "ttl_ms": editor::LINK_TTL_MS,
        });
        self.call(editor::method::MARKS, params, Purpose::Marks);
        std::mem::swap(&mut self.presence.sent, &mut self.presence.desired);
        self.presence.sent_epoch = epoch;
        self.presence.marks_sent = Some(now);
        self.presence.marks_pending = true;
        self.presence.clear_required |= !self.presence.sent.is_empty();
    }

    pub(super) fn presence_answer(
        &mut self,
        app: &mut App,
        purpose: Purpose,
        result: Result<Value, RpcError>,
    ) {
        match purpose {
            Purpose::Indicator => {
                self.presence.link_pending = false;
                self.presence.no_indicator = matches!(&result, Ok(v) if v["shown"] == false);
                let why = match result {
                    Ok(v) if v["shown"] == true => None,
                    Ok(v) if v["shown"] == false => Some(
                        v["why"].as_str().unwrap_or("the host has no editor indicator").to_owned(),
                    ),
                    Ok(_) => Some("missing shown in the indicator reply".into()),
                    Err(e) => Some(e.to_string()),
                };
                let why = why.map(|reason| format!("{}: {reason}", editor::method::LINK));
                // A transient error keeps the lease on its normal renewal schedule.
                // Several failed renewals belong to one failure streak.
                if (self.presence.link_why.is_none() || self.presence.no_indicator)
                    && let Some(reason) = &why
                {
                    app.error(reason.clone());
                }
                self.presence.link_why = why;
            }
            Purpose::Marks => {
                self.presence.marks_pending = false;
                let reply = result.map_err(|e| e.to_string()).and_then(|v| {
                    serde_json::from_value::<editor::MarksResult>(v)
                        .map_err(|e| format!("unreadable marks reply: {e}"))
                });
                let why = match reply {
                    Ok(reply) => {
                        let clear = self.presence.sent.is_empty();
                        let cleared = clear && reply.held == 0
                            && reply.refused.is_empty() && !reply.drawn;
                        if cleared {
                            self.presence.clear_required = false;
                        }
                        let refused: Vec<_> = reply.refused.iter().map(|mark| {
                            format!("{}: mark {}: {}", editor::method::MARKS, mark.index, mark.why)
                        }).collect();
                        for reason in &refused {
                            if !self.presence.marks_refused.contains(reason) {
                                self.feedback.problem(app, reason.clone());
                            }
                        }
                        self.presence.marks_refused = refused;

                        self.presence.marks_waiting = None;
                        match reply.why {
                            _ if clear && !cleared => Some("the host did not acknowledge the marks clear".into()),
                            _ if clear || reply.drawn => None,
                            Some(why) if waiting_for_frame(&why) => {
                                self.presence.marks_waiting = Some(format!("{}: {why}", editor::method::MARKS));
                                None
                            }
                            problem => problem,
                        }
                    }
                    Err(reason) => Some(reason),
                };
                let why = why.map(|reason| format!("{}: {reason}", editor::method::MARKS));
                if self.presence.marks_why != why {
                    if let Some(reason) = &why {
                        self.feedback.problem(app, reason.clone());
                    }
                    self.presence.marks_why = why;
                }
            }
            _ => {}
        }
    }

    /// The worker issues only these final calls on the original socket before
    /// closing it. Queued edits must never escape an unpair or socket switch.
    pub(super) fn close_presence(&mut self) {
        let Some(link) = self.link.take() else { return };
        let mut calls = Vec::new();
        if self.has_method(editor::method::MARKS) && self.presence.clear_required {
            calls.push(Call {
                method: editor::method::MARKS.into(),
                params: json!({"marks":[]}),
                purpose: Purpose::Marks,
            });
        }
        if self.has_method(editor::method::LINK)
            && self.presence.link_sent.is_some()
            && !self.presence.no_indicator
        {
            calls.push(Call {
                method: editor::method::LINK.into(),
                params: json!({"clear":true}),
                purpose: Purpose::Indicator,
            });
        }
        self.closing_links.push(link.close_with(calls));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::Object;
    use scenario_edit::{BlockKind, Reference, geometry::rotation};

    #[test]
    fn tag_selection_limits_matching_objects_and_keeps_the_candidate_style() {
        let objects: Vec<_> = (1..80)
            .map(|id| Object {
                id,
                tag: Some(if id == 1 { 9 } else { 42 }),
                ..Default::default()
            })
            .collect();
        let mut marks = Vec::new();
        targets(
            &App::new(),
            Some(Selection::Tag(map::TagId(42))),
            &objects,
            None,
            Some(79),
            &mut marks,
        );
        assert_eq!(marks.len(), TAG_LIMIT + 1);
        assert_eq!(marks[0], Target::Object(2, MarkStyle::Selected));
        assert_eq!(
            marks[TAG_LIMIT - 1],
            Target::Object(65, MarkStyle::Selected)
        );
        assert_eq!(
            marks.last(),
            Some(&Target::Object(79, MarkStyle::Candidate))
        );
        assert_ne!(marks[0].mark().color, marks[TAG_LIMIT].mark().color);
    }

    #[test]
    fn a_drag_updates_the_mark_box_and_unrelated_edits_keep_a_loose_handle_selected() {
        use crate::app::{Action, NavAction};
        use crate::play::{Conn, testing::Temp};
        use scenario_edit::fixture;

        let temp = Temp::new("presence-drag-selection");
        let mut app = App::new();
        app.set_data_root(temp.0.join("data"));
        app.dispatch(Action::OpenSource(temp.map("example")));
        let tag = fixture::id(2);
        app.pairing.stream_map = json!({
            "epoch": 1, "name": "example", "checksum": app.session.source_checksum()
        });
        app.pairing.shown = Some(live_mirror::Sample {
            epoch: 1,
            tick: 1,
            stale: false,
            players: Vec::new(),
            objects: [51, 59]
                .map(|id| Object {
                    id,
                    tag: Some(tag.0),
                    ..Default::default()
                })
                .to_vec(),
        });
        let now = Instant::now();
        let mut play = Play::new();
        play.conn = Conn::Connected;
        play.methods = Some([editor::method::MARKS.to_owned()].into_iter().collect());
        let acknowledge = |play: &mut Play, app: &mut App| {
            play.presence_answer(
                app,
                Purpose::Marks,
                Ok(json!({"held":1,"drawn":true,"refused":[]})),
            );
        };
        app.dispatch(Action::Select(Some((BlockKind::Weapons, 0))));
        select::update(&mut app, now);
        play.update_presence(&app, now);
        let [Target::Box(before)] = play.presence.sent.as_slice() else {
            panic!("expected the authored placement's box");
        };
        let origin = before.origin;
        acknowledge(&mut play, &mut app);

        // Run the gizmo's real drag path with a fixed stream tick and lease clock.
        let selection = app.selection_revision();
        let markers = app.session.markers_rev();
        app.dispatch(Action::GestureSet {
            position: [9., 8., 7.],
            ypr: [0.; 3],
        });
        assert!(app.session.in_gesture());
        assert!(app.session.markers_rev() > markers);
        assert_eq!(app.selection_revision(), selection);
        play.update_presence(&app, now);
        let [Target::Box(after)] = play.presence.sent.as_slice() else {
            panic!("expected the moved box");
        };
        assert_ne!(after.origin, origin);
        acknowledge(&mut play, &mut app);

        // Aim can pick a loose weapon during a drag. Its definition is the
        // inspector subject, while only its own salted handle is marked.
        select::apply(
            &mut app,
            select::Hit {
                object: 59,
                placement: None,
                tag: Some(tag.0),
                point: [0.; 3],
                distance: 1.,
            },
            false,
        );
        play.update_presence(&app, now);
        assert_eq!(app.selection(), Some(Selection::Tag(tag)));
        assert_eq!(
            play.presence.sent,
            [Target::Object(59, MarkStyle::Selected)]
        );
        acknowledge(&mut play, &mut app);
        let selection = app.selection_revision();
        let markers = app.session.markers_rev();
        app.dispatch(Action::MovePlacement {
            block: BlockKind::Weapons,
            index: 0,
            position: [4., 3., 2.],
            ypr: None,
        });
        app.dispatch(Action::EndGesture);
        select::update(&mut app, now);
        assert!(app.session.markers_rev() > markers);
        assert_eq!(selection, app.selection_revision());
        assert_eq!(select::selected_object(&app), Some(59));
        play.update_presence(&app, now);
        assert_eq!(
            play.presence.sent,
            [Target::Object(59, MarkStyle::Selected)]
        );
        assert!(
            !play.presence.marks_pending,
            "an unrelated drag edit must not replace the loose object mark"
        );

        app.dispatch(Action::Navigate(NavAction::SelectTag(tag)));
        play.update_presence(&app, now);
        assert_eq!(
            play.presence.sent,
            [
                Target::Object(51, MarkStyle::Selected),
                Target::Object(59, MarkStyle::Selected),
            ]
        );
    }

    #[test]
    fn a_placement_uses_its_linked_handle_or_an_oriented_box() {
        let mut app = App::new();
        let entry = (BlockKind::Vehicles, 3);
        let marker = Marker {
            block: entry.0,
            index: entry.1,
            position: [1., 4., 7.],
            axes: rotation([std::f32::consts::FRAC_PI_2, 0., 0.]),
            scale: 2.,
            object: None,
        };
        app.session.open_reference(Reference {
            name: "invented".into(),
            markers: vec![marker],
            world: Vec::new(),
            scene: None,
        });
        app.session.select(Some(entry));
        let mut marks = Vec::new();
        let subject = Some(Selection::Placement(entry.0, entry.1));
        let mut object = Object {
            id: 93,
            kind: 1,
            placement: Some(3),
            ..Default::default()
        };
        targets(&app, subject, &[object], None, None, &mut marks);
        assert_eq!(marks, [Target::Object(93, MarkStyle::Selected)]);
        object.kind = 2; // Other object kinds can repeat the same placement index.
        targets(&app, subject, &[object], None, None, &mut marks);
        let [Target::Box(frame)] = marks.as_slice() else {
            panic!("expected a placement box")
        };
        assert_eq!(frame.forward, marker.axes[0]);
        assert_eq!(frame.up, marker.axes[2]);
        let radius = glyph_bounds(glyph_of(marker.block)).1;
        assert_eq!(frame.size, [4. * radius; 3]);
        assert_eq!(
            frame.origin,
            scenario_edit::scene::model_to_world(&marker, [-radius; 3])
        );
    }

    #[test]
    fn renewal_precedes_expiry_and_a_dropped_socket_retains_only_its_clear() {
        let now = Instant::now();
        assert!(due(None, now));
        assert!(!due(Some(now), now + RENEW - Duration::from_millis(1)));
        assert!(due(Some(now), now + RENEW));
        assert!(RENEW < Duration::from_millis(editor::LINK_TTL_MS));
        let mut state = Presence {
            marks_pending: true,
            clear_required: true,
            link_pending: true,
            link_sent: Some(now),
            link_why: Some("first host had no window".into()),
            ..Default::default()
        };
        state.transport_lost();
        assert!(state.clear_required);
        assert!(!state.marks_pending && !state.link_pending);
        assert!(state.link_sent.is_none() && state.link_why.is_none());
    }
}

#[cfg(all(test, unix))]
mod indicator_tests;
