//! The floating menu (docs/game/ui-screens.md, "The floating menu"): a
//! short list drawn over the 3D view with no backdrop, its first row at a
//! point of the screen or at a point of the world projected each frame.
//! Forge's object actions are one.
//!
//! It is a list screen of ours and nothing else: a transparent page
//! ([`ScreenSpec::transparent`]) whose skin-0 list is moved to the
//! anchor, so the game's own list carries the focus, the press and the
//! draw. A screen gets its menu from its doc's `floating:` block
//! ([`crate::ui_kit::data::FloatingDoc`]); [`FloatingPage`] is every such
//! screen's page.
//!
//! Every placement goes through the one anchor rule
//! (`canvas_fit::anchor_fit`): [`screen_point`] for a held point of the
//! screen, [`window_point`] for a projected one. Both answer in the
//! canvas's own units, so the page's menu fit lands the menu where the
//! rule says at any aspect.

use std::sync::Mutex;

use serde::Serialize;

use crate::list_class::ListLayout;
use crate::ui_kit::data::{self, FloatingDoc, ScreenDoc};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::Live;
use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::text_fit;

/// The most actions a menu holds: what fits beside an object without
/// covering it.
pub const MAX_ACTIONS: usize = 6;

/// A skin-0 list item (`crate::ui_kit::view::Geometry::fallback`): its
/// block is 210 wide, starts 33 above its anchor, and items are 22 apart.
const ITEM_WIDTH: i16 = 210;
const ITEM_ABOVE: i16 = 33;
const ITEM_BELOW: i16 = 1;
const ITEM_PITCH: i16 = 22;

/// Where a menu may be: the title-safe area's sides, under the header's
/// rectangle and over the footer's (`crate::ui_kit::layout::SAFE`).
pub const ROOM: [i16; 4] = [176, -272, -180, 272];

/// The canvas's centre, guest window units.
const CENTRE: [f32; 2] = [320.0, 240.0];

/// What a menu is anchored to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// A point of the world, projected each frame ([`project`]).
    World,
    /// A held point of the screen; `pinned` holds an edge to the window's
    /// own edge, else to the safe area's.
    Screen { at: canvas_fit::Anchor, pinned: bool },
}

impl Anchor {
    /// The anchor a doc names ([`data::FLOATING_ANCHORS`]).
    pub fn named(name: &str, pinned: bool) -> Option<Anchor> {
        use canvas_fit::Anchor as A;
        let at = match name {
            "world" => return Some(Anchor::World),
            "centre" => A::Centre,
            "top" => A::Top,
            "bottom" => A::Bottom,
            "left" => A::Left,
            "right" => A::Right,
            "top-left" => A::TopLeft,
            "top-right" => A::TopRight,
            "bottom-left" => A::BottomLeft,
            "bottom-right" => A::BottomRight,
            _ => return None,
        };
        Some(Anchor::Screen { at, pinned })
    }
}

/// The page's own fit on `output`: the canvas drawn uniformly about its
/// centre (`canvas_fit::window_fit`), the identity at 4:3.
fn menu_fit(output: (u32, u32)) -> [f32; 4] {
    canvas_fit::window_fit(output, CENTRE).unwrap_or([1.0, 1.0, 0.0, 0.0])
}

/// A guest-window point (640 by 480 stretched over the window, Y down)
/// as the canvas point the page's fit draws there, in UI units (origin at
/// the centre, Y up).
fn canvas_of(guest: [f32; 2], output: (u32, u32)) -> [f32; 2] {
    let fit = menu_fit(output);
    let x = (guest[0] - fit[2]) / fit[0];
    let y = (guest[1] - fit[3]) / fit[1];
    [x - CENTRE[0], CENTRE[1] - y]
}

/// A point of the window, in normalised device coordinates (-1 to 1, Y
/// up), as the canvas point drawn there on `output`.
pub fn window_point(ndc: [f32; 2], output: (u32, u32)) -> [f32; 2] {
    canvas_of([(ndc[0] + 1.0) * CENTRE[0], (1.0 - ndc[1]) * CENTRE[1]], output)
}

/// A held anchor's point moved by `offset` (`x, y`, Y up), as the canvas
/// point drawn where the one anchor rule holds it on `output`
/// (`canvas_fit::anchor_fit`): an edge on the safe area's edge, or on the
/// window's own when `pinned`; a middle on the window's middle. The
/// authored point itself at 4:3.
pub fn screen_point(at: canvas_fit::Anchor, offset: [i16; 2], pinned: bool, output: (u32, u32)) -> [f32; 2] {
    let frac = at.at().unwrap_or([0.5, 0.5]);
    let authored = [
        frac[0] * canvas_fit::CANVAS[0] + f32::from(offset[0]),
        frac[1] * canvas_fit::CANVAS[1] - f32::from(offset[1]),
    ];
    let safe = canvas_fit::stretch_ratio(output).map_or(1.0, f32::recip);
    let view = [0.0, 0.0, canvas_fit::CANVAS[0], canvas_fit::CANVAS[1]];
    let fit = canvas_fit::anchor_fit(at, view, safe, safe, pinned);
    canvas_of([authored[0] * fit[0] + fit[2], authored[1] * fit[1] + fit[3]], output)
}

/// The camera a world point is projected through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: [f32; 3],
    /// Unit vectors.
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// The vertical field of view as the window shows it, radians.
    pub fov_y: f32,
    /// The window's width over its height.
    pub aspect: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `world` through `camera`, in normalised device coordinates (-1 to 1, Y
/// up). `None` for a point behind the camera or a camera that is not one.
pub fn project(camera: &Camera, world: [f32; 3]) -> Option<[f32; 2]> {
    let right = cross(camera.forward, camera.up);
    let up = cross(right, camera.forward);
    let d = [
        world[0] - camera.position[0],
        world[1] - camera.position[1],
        world[2] - camera.position[2],
    ];
    let depth = dot(d, camera.forward);
    let half = (camera.fov_y * 0.5).tan();
    let lens = half * camera.aspect;
    if !depth.is_finite() || depth <= 0.01 || !lens.is_finite() || half <= 0.0 || lens <= 0.0 {
        return None;
    }
    let ndc = [dot(d, right) / (depth * half * camera.aspect), dot(d, up) / (depth * half)];
    ndc.iter().all(|v| v.is_finite()).then_some(ndc)
}

/// A menu of `rows` rows with its corner at `point` moved by `offset`:
/// the list's anchor, kept inside [`ROOM`], and whether it had to move.
pub fn place(point: [f32; 2], offset: [i16; 2], rows: usize) -> ([i16; 2], bool) {
    let rows = rows.clamp(1, MAX_ACTIONS) as i16;
    let height = ITEM_ABOVE + ITEM_BELOW + ITEM_PITCH * (rows - 1);
    let [top, left, bottom, right] = ROOM.map(f32::from);
    let want = [point[0] + f32::from(offset[0]), point[1] + f32::from(offset[1])];
    let want = want.map(|v| if v.is_finite() { v } else { 0.0 });
    let x = want[0].clamp(left, right - f32::from(ITEM_WIDTH));
    let y = want[1].clamp(bottom + f32::from(height), top);
    let moved = (x - want[0]).abs() >= 0.5 || (y - want[1]).abs() >= 0.5;
    ([x.round() as i16, y.round() as i16 - ITEM_ABOVE], moved)
}

/// One action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Action {
    pub text: String,
    /// What the feature does on A, by name.
    pub act: Option<String>,
    /// The screen A opens instead.
    pub open: Option<String>,
}

/// A floating menu: its anchor, its corner's offset from the anchor's
/// point and its actions.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub anchor: Anchor,
    pub offset: [i16; 2],
    pub actions: Vec<Action>,
}

/// What a press did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Focus(usize),
    /// A on an action: the menu closes, then the feature acts.
    Act { action: usize, act: String },
    /// A on an action that opens a screen.
    Open { action: usize, screen: String },
    Close,
}

impl Menu {
    /// The menu a doc's `floating:` block declares.
    pub fn of(doc: &FloatingDoc) -> Menu {
        Menu {
            anchor: Anchor::named(&doc.anchor, doc.pinned).unwrap_or(Anchor::World),
            offset: doc.offset.unwrap_or([0, 0]),
            actions: doc
                .actions
                .values()
                .take(MAX_ACTIONS)
                .map(|a| Action {
                    text: a.text.clone(),
                    act: a.act.clone(),
                    open: a.open.clone(),
                })
                .collect(),
        }
    }

    /// The anchor's point on `output`, in UI units: the held point, or
    /// for a world anchor `projected` (the canvas's centre when the point
    /// is behind the camera).
    pub fn point(&self, projected: Option<[f32; 2]>, output: (u32, u32)) -> [f32; 2] {
        match self.anchor {
            Anchor::World => projected.unwrap_or([0.0, 0.0]),
            Anchor::Screen { at, pinned } => screen_point(at, [0, 0], pinned, output),
        }
    }

    /// The list's layout with the anchor's point at `point`.
    pub fn layout(&self, point: [f32; 2]) -> (ListLayout, bool) {
        let (anchor, moved) = place(point, self.offset, self.actions.len());
        (
            ListLayout {
                skin: 0,
                anchor,
                bitmaps_left_of: i16::MIN,
                visible: self.actions.len().clamp(1, MAX_ACTIONS) as i16,
            },
            moved,
        )
    }

    /// One press at `focus`: up and down move it, A acts, B and Back
    /// close.
    pub fn press(&self, focus: &mut usize, input: Input) -> Effect {
        match input {
            Input::Up if *focus > 0 => {
                *focus -= 1;
                Effect::Focus(*focus)
            }
            Input::Down if *focus + 1 < self.actions.len() => {
                *focus += 1;
                Effect::Focus(*focus)
            }
            Input::A => match self.actions.get(*focus) {
                Some(Action { open: Some(screen), .. }) => Effect::Open {
                    action: *focus,
                    screen: screen.clone(),
                },
                Some(Action { act: Some(act), .. }) => Effect::Act {
                    action: *focus,
                    act: act.clone(),
                },
                _ => Effect::None,
            },
            Input::B | Input::Back => Effect::Close,
            _ => Effect::None,
        }
    }

    /// The menu as a screen: `title` in the header, the actions as the
    /// list's rows at the anchor's `point`, the legend in the footer, and
    /// nothing behind them.
    pub fn spec(&self, name: &str, title: &str, point: [f32; 2], focus: usize) -> ScreenSpec {
        let mut spec = ScreenSpec::new(name, title);
        spec.transparent = true;
        spec.style = ValueStyle::Inline;
        spec.list = ListPlace::from(Some(self.layout(point).0));
        spec.focus = focus.min(self.actions.len().saturating_sub(1));
        spec.rows = self
            .actions
            .iter()
            .map(|a| {
                let row = RowSpec::action(&text_fit::line(&a.text, text_fit::ROW));
                match &a.open {
                    Some(screen) => row.press(Press::Open(screen.clone())),
                    None => row,
                }
            })
            .collect();
        spec.footer = true;
        spec.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        spec
    }
}

/// What a floating menu shows now, for `ui.stack`
/// (docs/runtime/agent-api.md, "ui: what the menus show").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Shown {
    pub screen: String,
    /// `world` or `screen`.
    pub anchor: &'static str,
    /// The anchor's point, UI units; `None` while a world point is behind
    /// the camera.
    pub point: Option<[i16; 2]>,
    /// The list's anchor: its first item's top-left.
    pub list: [i16; 2],
    /// The menu was moved to stay inside the safe area.
    pub clamped: bool,
    pub focus: usize,
    pub actions: Vec<String>,
}

impl Shown {
    pub fn of(screen: &str, menu: &Menu, projected: Option<[f32; 2]>, output: (u32, u32), focus: usize) -> Shown {
        let point = menu.point(projected, output);
        let (layout, clamped) = menu.layout(point);
        let seen = !matches!(menu.anchor, Anchor::World) || projected.is_some();
        Shown {
            screen: screen.into(),
            anchor: if matches!(menu.anchor, Anchor::World) { "world" } else { "screen" },
            point: seen.then(|| point.map(|v| v.round() as i16)),
            list: layout.anchor,
            clamped,
            focus,
            actions: menu.actions.iter().map(|a| a.text.clone()).collect(),
        }
    }
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

/// Says what the floating menu on top shows; its host calls this when the
/// anchor or the focus moves, and with `None` when it closes.
pub fn publish(shown: Option<Shown>) {
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = shown;
}

/// The floating menu on top, as last published.
pub fn shown() -> Option<Shown> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The window the offline page lays out for: the canvas's own shape.
const PREVIEW: (u32, u32) = (640, 480);

/// The page of every screen whose doc has a `floating:` block.
pub struct FloatingPage {
    doc: std::sync::Arc<ScreenDoc>,
    menu: Menu,
    focus: usize,
    /// A world anchor's projected point in this state.
    projected: Option<[f32; 2]>,
}

/// Screen `name`'s page, when its doc declares a floating menu.
pub fn live(name: &str) -> Option<Box<dyn Live>> {
    let doc = data::screen(name)?;
    let menu = Menu::of(doc.floating.as_ref()?);
    let mut page = FloatingPage {
        doc,
        menu,
        focus: 0,
        projected: Some([0.0, 0.0]),
    };
    if let Some(first) = page.state_names().first().cloned() {
        page.go(&first);
    }
    Some(Box::new(page))
}

impl FloatingPage {
    fn spec_at(&self, focus: usize, projected: Option<[f32; 2]>) -> ScreenSpec {
        let title = self.doc.text("title", &[]).unwrap_or_default();
        let point = self.menu.point(projected, PREVIEW);
        let mut spec = self.menu.spec(&self.doc.screen, &title, point, focus);
        spec.prompts = self.doc.prompts("legend").unwrap_or_default();
        spec
    }

    fn named(&self, name: &str) -> Option<(usize, Option<[f32; 2]>)> {
        match self.doc.states.get(name) {
            Some(s) => Some((s.focus, Some(s.point.unwrap_or([0, 0]).map(f32::from)))),
            None if self.doc.states.is_empty() && name == "first" => Some((0, Some([0.0, 0.0]))),
            None => None,
        }
    }

    /// What `ui.stack` would say of this page.
    pub fn shown(&self) -> Shown {
        Shown::of(&self.doc.screen, &self.menu, self.projected, PREVIEW, self.focus)
    }
}

impl Live for FloatingPage {
    fn press(&mut self, input: Input) -> String {
        format!("{:?}", self.menu.press(&mut self.focus, input))
    }

    fn audit_press(&mut self, input: Input) -> Option<bool> {
        Some(self.menu.press(&mut self.focus, input) != Effect::None)
    }

    fn spec(&self) -> ScreenSpec {
        self.spec_at(self.focus, self.projected)
    }

    fn summary(&self) -> String {
        let s = self.shown();
        format!(
            "{}: {} anchor at {:?}, list at {:?}{}, focus {} of {:?}",
            s.screen,
            s.anchor,
            s.point,
            s.list,
            if s.clamped { " (clamped)" } else { "" },
            s.focus,
            s.actions
        )
    }

    fn state_names(&self) -> Vec<String> {
        if self.doc.states.is_empty() {
            return vec!["first".into()];
        }
        self.doc.states.keys().cloned().collect()
    }

    fn specs(&self) -> Vec<(String, ScreenSpec)> {
        self.state_names()
            .into_iter()
            .filter_map(|name| {
                let (focus, projected) = self.named(&name)?;
                Some((name, self.spec_at(focus, projected)))
            })
            .collect()
    }

    fn go(&mut self, name: &str) -> bool {
        match self.named(name) {
            Some((focus, projected)) => {
                self.focus = focus.min(self.menu.actions.len().saturating_sub(1));
                self.projected = projected;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::layout::{Rules, check_bounds};
    use crate::ui_kit::preview::From;

    const WIDE: (u32, u32) = (1920, 1080);
    const TALL: (u32, u32) = (1080, 1920);

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    }

    fn menu(anchor: Anchor, n: usize) -> Menu {
        Menu {
            anchor,
            offset: [12, 0],
            actions: (0..n)
                .map(|i| Action {
                    text: format!("Action {i}"),
                    act: (i != 1).then(|| format!("act-{i}")),
                    open: (i == 1).then(|| "forge.properties".to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn at_four_by_three_every_anchor_is_its_authored_point() {
        use canvas_fit::Anchor as A;
        for (at, want) in [
            (A::Centre, [0.0, 0.0]),
            (A::TopLeft, [-320.0, 240.0]),
            (A::BottomRight, [320.0, -240.0]),
            (A::Right, [320.0, 0.0]),
        ] {
            for pinned in [false, true] {
                let p = screen_point(at, [0, 0], pinned, PREVIEW);
                assert!(near(p, want), "{at:?} pinned {pinned}: {p:?}");
            }
        }
        assert!(near(screen_point(A::Centre, [10, -20], false, PREVIEW), [10.0, -20.0]));
        assert!(near(window_point([0.5, -0.5], PREVIEW), [160.0, -120.0]));
    }

    /// The anchor rule on a wide window: an edge held to the safe area
    /// stays the authored point; pinned, it is the window's own edge,
    /// which the uniform canvas reaches at 426.67 units from the centre.
    /// An offset is the same distance on both axes.
    #[test]
    fn a_wide_window_holds_an_edge_to_the_safe_area_or_the_window() {
        use canvas_fit::Anchor as A;
        assert!(near(screen_point(A::Right, [0, 0], false, WIDE), [320.0, 0.0]));
        let pinned = screen_point(A::Right, [0, 0], true, WIDE);
        assert!((pinned[0] - 426.666).abs() < 0.01 && pinned[1].abs() < 0.01, "{pinned:?}");
        let moved = screen_point(A::Right, [-30, 40], true, WIDE);
        assert!(near([moved[0] - pinned[0], moved[1] - pinned[1]], [-30.0, 40.0]), "{moved:?}");
        // The middle is the window's middle either way.
        assert!(near(screen_point(A::Centre, [0, 0], true, WIDE), [0.0, 0.0]));
        // A tall window: the top edge pinned is past the canvas's top.
        let top = screen_point(A::Top, [0, 0], true, TALL);
        assert!(top[0].abs() < 0.01 && top[1] > 240.0, "{top:?}");
        assert!(near(screen_point(A::Top, [0, 0], false, TALL), [0.0, 240.0]));
    }

    /// A projected point keeps its place in the window: the window's edge
    /// is the uniform canvas's edge, not the 4:3 box's.
    #[test]
    fn a_projected_point_lands_on_its_window_fraction() {
        assert!(near(window_point([0.0, 0.0], WIDE), [0.0, 0.0]));
        let edge = window_point([1.0, 1.0], WIDE);
        assert!((edge[0] - 426.666).abs() < 0.01 && (edge[1] - 240.0).abs() < 0.01, "{edge:?}");
        let tall = window_point([1.0, 1.0], TALL);
        assert!((tall[0] - 320.0).abs() < 0.01 && tall[1] > 240.0, "{tall:?}");
    }

    #[test]
    fn a_world_point_is_projected_through_the_camera() {
        // Halo's axes: x forward, y left, z up.
        let camera = Camera {
            position: [0.0, 0.0, 1.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect: 4.0 / 3.0,
        };
        assert!(near(project(&camera, [5.0, 0.0, 1.0]).unwrap(), [0.0, 0.0]));
        // Up by the distance at a 90 degree lens: the top edge.
        assert!(near(project(&camera, [5.0, 0.0, 6.0]).unwrap(), [0.0, 1.0]));
        // To the right (negative y) by the distance: three quarters across.
        assert!(near(project(&camera, [4.0, -4.0, 1.0]).unwrap(), [0.75, 0.0]));
        assert!(project(&camera, [4.0, 4.0, 1.0]).unwrap()[0] < 0.0);
        // Behind the camera, and a camera with no lens: no point.
        assert_eq!(project(&camera, [-1.0, 0.0, 1.0]), None);
        assert_eq!(project(&Camera { fov_y: 0.0, ..camera }, [5.0, 0.0, 1.0]), None);
    }

    #[test]
    fn a_menu_stays_inside_its_room() {
        let (anchor, moved) = place([0.0, 0.0], [12, 0], 4);
        assert_eq!((anchor, moved), ([12, -ITEM_ABOVE], false));
        // Past the right edge and under the footer: brought back in.
        let (anchor, moved) = place([400.0, -300.0], [12, 0], 4);
        assert!(moved);
        assert_eq!(anchor[0], ROOM[3] - ITEM_WIDTH);
        let bottom = anchor[1] - ITEM_BELOW - ITEM_PITCH * 3;
        assert_eq!(bottom, ROOM[2]);
        // Past the top left.
        let (anchor, moved) = place([-900.0, 900.0], [0, 0], 1);
        assert_eq!((anchor, moved), ([ROOM[1], ROOM[0] - ITEM_ABOVE], true));
        // A point that is no number is the centre.
        assert_eq!(place([f32::NAN, f32::INFINITY], [0, 0], 2).0, [0, -ITEM_ABOVE]);
    }

    #[test]
    fn the_menu_is_a_transparent_list_at_its_anchor() {
        let m = menu(Anchor::World, 4);
        let spec = m.spec("t", "WARTHOG", [40.0, 60.0], 2);
        assert!(spec.transparent);
        assert_eq!(spec.rows.len(), 4);
        assert_eq!(spec.focus, 2);
        assert_eq!(
            spec.list,
            ListPlace::Moved {
                skin: 0,
                anchor: [52, 60 - ITEM_ABOVE],
                bitmaps_left_of: i16::MIN,
                visible: 4
            }
        );
        // No backdrop: nothing but the list's own art.
        assert!(spec.panel.is_empty() && spec.art.is_empty() && spec.texts.is_empty());
        assert_eq!(spec.rows[1].press, Press::Open("forge.properties".into()));
    }

    #[test]
    fn up_and_down_move_the_focus_and_a_acts() {
        let m = menu(Anchor::World, 3);
        let mut focus = 0;
        assert_eq!(m.press(&mut focus, Input::Up), Effect::None);
        assert_eq!(m.press(&mut focus, Input::A), Effect::Act { action: 0, act: "act-0".into() });
        assert_eq!(m.press(&mut focus, Input::Down), Effect::Focus(1));
        assert_eq!(
            m.press(&mut focus, Input::A),
            Effect::Open { action: 1, screen: "forge.properties".into() }
        );
        assert_eq!(m.press(&mut focus, Input::Down), Effect::Focus(2));
        assert_eq!(m.press(&mut focus, Input::Down), Effect::None);
        assert_eq!(m.press(&mut focus, Input::B), Effect::Close);
        assert_eq!(m.press(&mut focus, Input::Left), Effect::None);
    }

    #[test]
    fn ui_stack_says_where_the_menu_is() {
        let m = menu(Anchor::World, 2);
        let s = Shown::of("t", &m, Some([400.0, 0.0]), PREVIEW, 1);
        assert_eq!((s.anchor, s.point, s.clamped, s.focus), ("world", Some([400, 0]), true, 1));
        assert_eq!(s.list[0], ROOM[3] - ITEM_WIDTH);
        // Behind the camera: no point, the menu at the centre.
        let s = Shown::of("t", &m, None, PREVIEW, 0);
        assert_eq!((s.point, s.list), (None, [12, -ITEM_ABOVE]));
        let held = menu(Anchor::Screen { at: canvas_fit::Anchor::Right, pinned: true }, 2);
        let s = Shown::of("t", &held, None, WIDE, 0);
        assert_eq!((s.anchor, s.point), ("screen", Some([427, 0])));
        assert!(s.clamped);
    }

    /// Every floating menu, at every point of a grid over and past the
    /// canvas: its rows fit and stay inside the safe area.
    #[test]
    fn every_floating_menu_stays_in_the_safe_area_wherever_its_point_is() {
        let source = From::Synthetic;
        let mut seen = 0;
        for name in data::names() {
            let Some(doc) = data::screen(name) else { continue };
            let Some(block) = &doc.floating else { continue };
            seen += 1;
            let m = Menu::of(block);
            for x in (-480..=480).step_by(160) {
                for y in (-360..=360).step_by(120) {
                    let spec = m.spec(name, "TITLE", [x as f32, y as f32], 0);
                    let view = source.view(&spec, "t").unwrap();
                    let issues = check_bounds(&view, source.source(), &Rules::for_spec(&spec));
                    assert!(issues.is_empty(), "{name} at {x},{y}: {issues:?}");
                }
            }
        }
        assert!(seen >= 1, "{seen} floating menus");
    }
}
