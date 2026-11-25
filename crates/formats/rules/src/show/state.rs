//! The show state: what a script's HUD and waypoints hold until the script
//! changes them (docs/game/rules-world-api.md, "The show channel"). It is
//! part of the VM's state (`vm::RulesExt::show`), so a checkpoint, a film's
//! keyframe and the sim hash carry it on every machine that runs the VM,
//! and it encodes to at most [`MAX_BYTES`] bytes for the actor channel's
//! `Show` record.
//!
//! [`view`] is a pure function of the state, the blob's tables and who is
//! looking. The host, a replica and a client holding only the state's
//! bytes draw the same HUD from it.

use super::{Audience, EVERYONE, player};
use crate::ir::{Ir, MAX_WAYPOINTS as DECLARED_WAYPOINTS, MAX_WIDGETS as DECLARED_WIDGETS, WidgetKind};
use crate::ty::{MAX_PLAYERS, fixed};

/// HUD widgets the state holds: the script's declarations.
pub const MAX_WIDGETS: usize = DECLARED_WIDGETS;
/// Waypoints the state holds: the script's declarations.
pub const MAX_WAYPOINTS: usize = DECLARED_WAYPOINTS;
/// The most bytes [`State::encode`] writes: the actor channel's `Show`
/// record carries one length byte, so 255.
pub const MAX_BYTES: usize = 255;

/// One HUD widget's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Widget {
    /// Whether the widget draws at all (`hud_show`).
    pub shown: bool,
    /// The number the HUD words (`hud_value`): a count, a distance, a
    /// side (a team's index, or 16 and a roster slot in a game without
    /// teams).
    pub value: i32,
    /// The fill, 0 to 255 for 0 to 1 (`hud_fill`).
    pub fill: u8,
    /// A style, a small number each HUD reads its own way (`hud_style`).
    pub style: u8,
    /// The text, an index into the blob's string table (`hud_text`).
    pub text: Option<u16>,
    /// Who sees it: a mask of player slots.
    pub audience: Audience,
}

impl Default for Widget {
    fn default() -> Self {
        Widget {
            shown: false,
            value: 0,
            fill: 0,
            style: 0,
            text: None,
            audience: EVERYONE,
        }
    }
}

/// One waypoint's state: the object it marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waypoint {
    /// The object's handle (`ty::object_id`).
    pub at: u32,
    pub audience: Audience,
}

/// The values one widget reads per viewer (`hud_value_for`): a count that
/// is each player's own, as the Territories HUD's held count is. A viewer
/// reads the entry of its roster slot; the widget's own `value` is not
/// shown. One widget at a time keeps one, so the state stays inside the
/// `Show` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerViewer {
    /// The widget's number.
    pub widget: u8,
    /// The value for each roster slot, 0 until set, at most 255.
    pub values: [u8; MAX_PLAYERS],
}

/// The whole show state: a table of widgets and one of waypoints, by the
/// script's declaration numbers, and the widget that reads per viewer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub widgets: [Option<Widget>; MAX_WIDGETS],
    pub waypoints: [Option<Waypoint>; MAX_WAYPOINTS],
    pub per_viewer: Option<PerViewer>,
}

/// Why bytes are not a show state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateError {
    /// The bytes end inside a field.
    Truncated,
    /// Bytes past the last field.
    Trailing,
    /// A flag no field has.
    Malformed,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "show state error: {self:?}")
    }
}

impl std::error::Error for StateError {}

const SHOWN: u8 = 1;
const HAS_TEXT: u8 = 2;
const FLAGS: u8 = SHOWN | HAS_TEXT;
/// The per-viewer byte of the encoding when no widget reads per viewer.
const NO_PER_VIEWER: u8 = 0xff;

impl State {
    /// Whether no widget and no waypoint is set.
    pub fn is_empty(&self) -> bool {
        self.widgets.iter().all(Option::is_none)
            && self.waypoints.iter().all(Option::is_none)
            && self.per_viewer.is_none()
    }

    /// Sets what the player in roster `slot` reads at `widget`: the
    /// widget's value becomes per viewer. Another widget that already reads
    /// per viewer, a widget or a slot past the tables is refused (`false`).
    pub fn set_for(&mut self, widget: usize, slot: usize, value: i32) -> bool {
        if widget >= MAX_WIDGETS || slot >= MAX_PLAYERS {
            return false;
        }
        let table = self.per_viewer.get_or_insert(PerViewer {
            widget: widget as u8,
            values: [0; MAX_PLAYERS],
        });
        if usize::from(table.widget) != widget {
            return false;
        }
        table.values[slot] = value.clamp(0, i32::from(u8::MAX)) as u8;
        self.widget(widget).is_some()
    }

    /// The widget `index`, made if it is not yet there. `None` for a
    /// number past the table.
    pub fn widget(&mut self, index: usize) -> Option<&mut Widget> {
        let slot = self.widgets.get_mut(index)?;
        Some(slot.get_or_insert_with(Widget::default))
    }

    /// Takes away what the script no longer declares: widgets from
    /// `widgets` on and waypoints from `waypoints` on.
    pub fn truncate(&mut self, widgets: usize, waypoints: usize) {
        for w in self.widgets.iter_mut().skip(widgets) {
            *w = None;
        }
        for w in self.waypoints.iter_mut().skip(waypoints) {
            *w = None;
        }
        if self.per_viewer.is_some_and(|p| usize::from(p.widget) >= widgets) {
            self.per_viewer = None;
        }
    }

    /// The canonical bytes: a mask of the widgets set, a mask of the
    /// waypoints set, then each widget (flags, value, fill, style, the text
    /// when it has one, audience), each waypoint (object, audience) and the
    /// per-viewer table (its widget's number, `0xff` for none, then 16
    /// values when there is one). At most 11 bytes a widget, 6 a waypoint
    /// and 17 for the table: 244 in all.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut widget_mask = 0u16;
        for (i, w) in self.widgets.iter().enumerate() {
            widget_mask |= u16::from(w.is_some()) << i;
        }
        let mut waypoint_mask = 0u8;
        for (i, w) in self.waypoints.iter().enumerate() {
            waypoint_mask |= u8::from(w.is_some()) << i;
        }
        out.extend(widget_mask.to_le_bytes());
        out.push(waypoint_mask);
        for w in self.widgets.iter().flatten() {
            out.push(u8::from(w.shown) * SHOWN | u8::from(w.text.is_some()) * HAS_TEXT);
            out.extend(w.value.to_le_bytes());
            out.push(w.fill);
            out.push(w.style);
            if let Some(t) = w.text {
                out.extend(t.to_le_bytes());
            }
            out.extend(w.audience.to_le_bytes());
        }
        for w in self.waypoints.iter().flatten() {
            out.extend(w.at.to_le_bytes());
            out.extend(w.audience.to_le_bytes());
        }
        match &self.per_viewer {
            Some(p) => {
                out.push(p.widget);
                out.extend(p.values);
            }
            None => out.push(NO_PER_VIEWER),
        }
        out
    }

    /// Reads [`State::encode`]'s bytes and nothing else.
    pub fn decode(bytes: &[u8]) -> Result<State, StateError> {
        let mut r = Bytes(bytes);
        let widget_mask = r.u16()?;
        let waypoint_mask = r.u8()?;
        let mut state = State::default();
        for (i, slot) in state.widgets.iter_mut().enumerate() {
            if widget_mask >> i & 1 == 0 {
                continue;
            }
            let flags = r.u8()?;
            if flags & !FLAGS != 0 {
                return Err(StateError::Malformed);
            }
            let value = r.i32()?;
            let fill = r.u8()?;
            let style = r.u8()?;
            let text = if flags & HAS_TEXT != 0 { Some(r.u16()?) } else { None };
            let audience = r.u16()?;
            *slot = Some(Widget {
                shown: flags & SHOWN != 0,
                value,
                fill,
                style,
                text,
                audience,
            });
        }
        for (i, slot) in state.waypoints.iter_mut().enumerate() {
            if waypoint_mask >> i & 1 == 0 {
                continue;
            }
            *slot = Some(Waypoint {
                at: r.u32()?,
                audience: r.u16()?,
            });
        }
        state.per_viewer = match r.u8()? {
            NO_PER_VIEWER => None,
            widget if usize::from(widget) < MAX_WIDGETS && widget_mask >> widget & 1 != 0 => Some(PerViewer {
                widget,
                values: r.take()?,
            }),
            _ => return Err(StateError::Malformed),
        };
        if !r.0.is_empty() {
            return Err(StateError::Trailing);
        }
        Ok(state)
    }
}

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], StateError> {
        let (head, rest) = self.0.split_first_chunk::<N>().ok_or(StateError::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }
    fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, StateError> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, StateError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take()?))
    }
}

/// A fixed-point fill (16.16, 0 to 1) as the state keeps it.
pub fn fill_byte(fixed_fill: i32) -> u8 {
    let scaled = i64::from(fixed_fill.clamp(0, fixed::ONE)) * 255 / i64::from(fixed::ONE);
    scaled as u8
}

/// Who is looking: a local player's roster slot and team. A machine with
/// no player at the screen sees no widget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    pub slot: Option<u8>,
    pub team: Option<u8>,
}

impl Viewer {
    /// Whether this viewer is on the side a widget's number names: a team's
    /// index (below 16), or 16 and a roster slot for a game without teams.
    pub fn on_side(&self, side: i32) -> bool {
        match side {
            0..=15 => self.team.is_some_and(|t| i32::from(t) == side),
            16..=31 => self.slot.is_some_and(|s| i32::from(s) == side - 16),
            _ => false,
        }
    }
}

/// A widget as one viewer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetView {
    /// The declared name.
    pub name: String,
    /// The kit element the widget is at (`koth.hill`).
    pub slot: String,
    pub kind: WidgetKind,
    pub shown: bool,
    pub value: i32,
    pub fill: u8,
    pub style: u8,
    pub text: Option<String>,
}

/// A waypoint as one viewer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaypointView {
    pub name: String,
    /// The kit navpoint element it is drawn as.
    pub slot: Option<String>,
    /// The marked object's handle.
    pub at: u32,
}

/// What one viewer sees of the state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub widgets: Vec<WidgetView>,
    pub waypoints: Vec<WaypointView>,
}

impl View {
    /// The widget at kit element `slot`.
    pub fn at(&self, slot: &str) -> Option<&WidgetView> {
        self.widgets.iter().find(|w| w.slot == slot)
    }

    /// The waypoint drawn as navpoint element `slot`.
    pub fn waypoint_at(&self, slot: &str) -> Option<&WaypointView> {
        self.waypoints.iter().find(|w| w.slot.as_deref() == Some(slot))
    }
}

/// What `viewer` sees of `state` in the script `ir` declares: each widget
/// and waypoint that is set, that the declarations name and that the
/// viewer is in the audience of. A number past the declarations and a text
/// index outside the blob's string table are skipped, not drawn.
pub fn view(state: &State, ir: &Ir, viewer: Viewer) -> View {
    let Some(me) = viewer.slot.map(player).filter(|&bit| bit != 0) else {
        return View::default();
    };
    let string = |i: u16| ir.strings.get(usize::from(i)).cloned();
    let mut out = View::default();
    for (i, (def, set)) in ir.widgets.iter().zip(&state.widgets).enumerate() {
        let Some(w) = set.filter(|w| w.audience & me != 0) else {
            continue;
        };
        let (Some(name), Some(slot)) = (string(def.name), string(def.slot)) else {
            continue;
        };
        out.widgets.push(WidgetView {
            name,
            slot,
            kind: def.kind,
            shown: w.shown,
            value: match &state.per_viewer {
                Some(p) if usize::from(p.widget) == i => {
                    viewer.slot.map_or(0, |s| i32::from(p.values[usize::from(s) % MAX_PLAYERS]))
                }
                _ => w.value,
            },
            fill: w.fill,
            style: w.style,
            text: w.text.and_then(string),
        });
    }
    for (def, set) in ir.waypoints.iter().zip(&state.waypoints) {
        let Some(w) = set.filter(|w| w.audience & me != 0) else {
            continue;
        };
        let Some(name) = string(def.name) else {
            continue;
        };
        out.waypoints.push(WaypointView {
            name,
            slot: def.slot.and_then(string),
            at: w.at,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{WaypointDef, WidgetDef};

    fn full() -> State {
        let mut s = State::default();
        for i in 0..MAX_WIDGETS {
            s.widgets[i] = Some(Widget {
                shown: i % 2 == 0,
                value: i as i32 * -1000,
                fill: i as u8 * 16,
                style: i as u8,
                text: Some(i as u16),
                audience: 0xa5a5,
            });
        }
        for i in 0..MAX_WAYPOINTS {
            s.waypoints[i] = Some(Waypoint {
                at: 0x4001_0000 | i as u32,
                audience: 1 << i,
            });
        }
        s.per_viewer = Some(PerViewer {
            widget: 3,
            values: std::array::from_fn(|i| 255 - i as u8),
        });
        s
    }

    #[test]
    fn the_state_round_trips_and_the_fullest_fits_the_record() {
        let s = full();
        let bytes = s.encode();
        assert!(bytes.len() <= MAX_BYTES, "{} bytes", bytes.len());
        assert_eq!(State::decode(&bytes), Ok(s));
        assert_eq!(State::decode(&State::default().encode()), Ok(State::default()));
        assert_eq!(State::default().encode().len(), 4);
    }

    #[test]
    fn the_codec_refuses_a_cut_state_a_trailing_byte_and_a_bad_flag() {
        let bytes = full().encode();
        assert_eq!(State::decode(&bytes[..bytes.len() - 1]), Err(StateError::Truncated));
        let mut more = bytes.clone();
        more.push(0);
        assert_eq!(State::decode(&more), Err(StateError::Trailing));
        let mut bad = bytes.clone();
        bad[3] |= 0x80;
        assert_eq!(State::decode(&bad), Err(StateError::Malformed));
        // A mask that names waypoints the bytes do not hold.
        assert_eq!(State::decode(&[0, 0, 0xff, 0, 0]), Err(StateError::Truncated));
        // A per-viewer table for a widget the state does not hold, or past
        // the tables.
        assert_eq!(State::decode(&[0, 0, 0, 3]), Err(StateError::Malformed));
        assert_eq!(State::decode(&[0, 0, 0, 16]), Err(StateError::Malformed));
    }

    #[test]
    fn a_per_viewer_widget_reads_each_viewers_own_value() {
        let ir = ir();
        let mut s = State::default();
        *s.widget(0).unwrap() = Widget { shown: true, value: 9, style: 3, ..Widget::default() };
        assert!(s.set_for(0, 2, 4));
        assert!(s.set_for(0, 5, 1000));
        // A second widget cannot read per viewer, nor a slot past the roster.
        assert!(!s.set_for(1, 2, 4));
        assert!(!s.set_for(0, MAX_PLAYERS, 4));
        let read = |slot| view(&s, &ir, Viewer { slot: Some(slot), team: None }).at("koth.hill").map(|w| w.value);
        assert_eq!(read(2), Some(4));
        assert_eq!(read(5), Some(255));
        assert_eq!(read(0), Some(0));
        // The widget's other fields are shared, and the bytes carry the table.
        let v = view(&s, &ir, Viewer { slot: Some(2), team: None });
        assert_eq!(v.at("koth.hill").map(|w| (w.style, w.shown)), Some((3, true)));
        let held = State::decode(&s.encode()).unwrap();
        assert_eq!(held, s);
        assert_eq!(view(&held, &ir, Viewer { slot: Some(2), team: None }), v);
        // A script that drops the widget drops its table.
        s.truncate(0, 0);
        assert!(s.is_empty());
    }

    #[test]
    fn a_fill_is_clamped_to_the_byte() {
        assert_eq!(fill_byte(-5), 0);
        assert_eq!(fill_byte(fixed::ONE / 2), 127);
        assert_eq!(fill_byte(fixed::ONE), 255);
        assert_eq!(fill_byte(fixed::ONE * 9), 255);
    }

    #[test]
    fn a_side_is_a_team_or_a_slot_with_no_teams() {
        let viewer = Viewer { slot: Some(3), team: Some(1) };
        assert!(viewer.on_side(1));
        assert!(!viewer.on_side(0));
        assert!(viewer.on_side(16 + 3));
        assert!(!viewer.on_side(16 + 4));
        assert!(!viewer.on_side(-1));
        assert!(!Viewer::default().on_side(0));
    }

    fn ir() -> Ir {
        let mut ir = crate::ir::tests::sample();
        ir.strings = ["hill", "koth.hill", "banner", "koth.banner", "HILL MOVED", "mark", "koth.mark"]
            .map(String::from)
            .to_vec();
        ir.widgets = vec![
            WidgetDef { name: 0, kind: WidgetKind::Text, slot: 1 },
            WidgetDef { name: 2, kind: WidgetKind::Banner, slot: 3 },
        ];
        ir.waypoints = vec![WaypointDef { name: 5, slot: Some(6) }];
        ir
    }

    #[test]
    fn a_view_names_what_the_declarations_name_and_hides_from_outside_the_audience() {
        let ir = ir();
        let mut s = State::default();
        *s.widget(0).unwrap() = Widget { shown: true, value: 1, style: 2, ..Widget::default() };
        *s.widget(1).unwrap() = Widget {
            shown: true,
            text: Some(4),
            audience: player(2),
            ..Widget::default()
        };
        s.waypoints[0] = Some(Waypoint { at: 77, audience: EVERYONE });
        // A number past the declarations is skipped.
        s.widgets[5] = Some(Widget::default());

        let v = view(&s, &ir, Viewer { slot: Some(2), team: Some(0) });
        assert_eq!(v.widgets.len(), 2);
        assert_eq!(v.at("koth.hill").map(|w| (w.value, w.style, w.shown)), Some((1, 2, true)));
        assert_eq!(v.at("koth.banner").and_then(|w| w.text.as_deref()), Some("HILL MOVED"));
        assert_eq!(v.waypoint_at("koth.mark").map(|w| w.at), Some(77));

        let other = view(&s, &ir, Viewer { slot: Some(1), team: Some(0) });
        assert!(other.at("koth.banner").is_none());
        assert!(other.at("koth.hill").is_some());
        assert_eq!(view(&s, &ir, Viewer::default()), View::default());
    }

    #[test]
    fn a_text_outside_the_string_table_is_not_drawn() {
        let ir = ir();
        let mut s = State::default();
        *s.widget(1).unwrap() = Widget { shown: true, text: Some(900), ..Widget::default() };
        let v = view(&s, &ir, Viewer { slot: Some(0), team: None });
        assert_eq!(v.at("koth.banner").map(|w| w.text.clone()), Some(None));
    }
}
