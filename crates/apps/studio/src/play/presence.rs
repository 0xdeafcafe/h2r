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
                    let object = if app.pairing.placement_matches(&app.session, marker.block) {
                        objects.iter().find(|o| {
                            kind == Some(i16::from(o.kind))
                                && o.placement.map(usize::from) == Some(marker.index)
                        })
                    } else {
                        None
                    };
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
    selection: (u64, u64, u64),
    sample: Option<(u64, u64)>,
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
    // A disconnected socket might have accepted the last call without answering.
    replace_on_return: bool,
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
            .map(String::as_str)
    }

    pub(super) fn transport_lost(&mut self) {
        let replace_on_return =
            self.marks_pending || !self.sent.is_empty() || self.replace_on_return;
        *self = Self {
            replace_on_return,
            ..Default::default()
        };
    }
}

fn due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= RENEW)
}

impl Play {
    pub(super) fn update_presence(&mut self, app: &App, now: Instant) {
        if !self.is_paired() {
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
            let reloads = app
                .pairing
                .pending
                .iter()
                .filter(|row| row["class"] == super::tag_sync::RELOAD)
                .count();
            let mut params =
                json!({"editor":"h2-editor", "mode":mode, "ttl_ms":editor::LINK_TTL_MS});
            if reloads > 0 {
                params["text"] = json!(if reloads == 1 {
                    "1 edit needs a reload".into()
                } else {
                    format!("{reloads} edits need a reload")
                });
            } else if app.pairing.selection.enabled {
                params["text"] = json!("Aim select");
            }
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
        let inputs = MarkInputs {
            selection: app.selection_revision(),
            sample: sample.map(|s| (s.epoch, s.tick)),
            epoch: self.edits.epoch,
            matching,
            live: select::selected_object(app),
            candidate: app.pairing.selection.candidate.map(|h| h.object),
        };
        let renew = !self.presence.sent.is_empty() && due(self.presence.marks_sent, now);
        if self.presence.inputs == Some(inputs)
            && !self.presence.replace_on_return
            && !renew
            && !due(self.presence.last_check, now)
        {
            return;
        }
        if self
            .presence
            .inputs
            .is_none_or(|old| old.selection != inputs.selection)
        {
            self.presence.selection = app.selection();
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
        if !changed && !renew && !self.presence.replace_on_return {
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
        self.presence.replace_on_return = false;
    }

    pub(super) fn presence_answer(
        &mut self,
        app: &mut App,
        purpose: Purpose,
        result: Result<Value, RpcError>,
    ) {
        let (method, slot, why) = match purpose {
            Purpose::Indicator => {
                self.presence.link_pending = false;
                self.presence.no_indicator = matches!(&result, Ok(v) if v["shown"] == false);
                let why = match result {
                    Ok(v) if v["shown"] == true => None,
                    Ok(v) => Some(
                        v["why"]
                            .as_str()
                            .unwrap_or("the host did not show the editor indicator")
                            .to_owned(),
                    ),
                    Err(e) => Some(e.to_string()),
                };
                (editor::method::LINK, &mut self.presence.link_why, why)
            }
            Purpose::Marks => {
                self.presence.marks_pending = false;
                let why = match result {
                    Ok(_) if self.presence.sent.is_empty() => None,
                    Ok(v) => v["why"].as_str().map(str::to_owned).or_else(|| {
                        let first = v["refused"].as_array()?.first()?;
                        Some(format!(
                            "mark {}: {}",
                            first["index"],
                            first["why"].as_str()?
                        ))
                    }),
                    Err(e) => Some(e.to_string()),
                };
                (editor::method::MARKS, &mut self.presence.marks_why, why)
            }
            _ => return,
        };
        let why = why.map(|reason| format!("{method}: {reason}"));
        if *slot != why {
            if let Some(reason) = &why {
                app.error(reason.clone());
            }
            *slot = why;
        }
    }

    /// The worker issues only these final calls on the original socket before
    /// closing it. Queued edits must never escape an unpair or socket switch.
    pub(super) fn close_presence(&mut self) {
        let Some(link) = self.link.take() else { return };
        let mut calls = Vec::new();
        if self.has_method(editor::method::MARKS)
            && (!self.presence.sent.is_empty()
                || self.presence.marks_pending
                || self.presence.replace_on_return)
        {
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
        link.close_with(calls);
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
    fn loose_object_marks_only_the_selected_handle_even_when_inspecting_its_tag() {
        let mut marks = Vec::new();
        targets(
            &App::new(),
            Some(Selection::Tag(map::TagId(42))),
            &[],
            Some(59),
            None,
            &mut marks,
        );
        assert_eq!(marks, [Target::Object(59, MarkStyle::Selected)]);
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
            link_pending: true,
            link_sent: Some(now),
            link_why: Some("first host had no window".into()),
            ..Default::default()
        };
        state.transport_lost();
        assert!(state.replace_on_return);
        assert!(!state.marks_pending && !state.link_pending);
        assert!(state.link_sent.is_none() && state.link_why.is_none());
    }
}
