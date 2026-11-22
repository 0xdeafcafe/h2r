//! The Navigation layers as viewport overlays (docs/editor/editor.md,
//! "Navigation"): the game's pathfinding sectors and links, the bots'
//! mesh and links by kind, and the superbounce spots by state
//! ([`scenario_edit::nav_view`]). A hidden layer uploads nothing, so a
//! large map's mesh costs nothing until it is shown. Shared by the window
//! and `view.snapshot`.
use scenario_edit::geometry::{self, Vec3, add, cross, dot, normalize, scale, sub};
use scenario_edit::layers::LayerId;
use scenario_edit::nav_view::{
    self, BOT_LIFT, BotNav, LevelPathfinding, NavPick, PATHFINDING_LIFT, SpotKind,
};
use scenario_edit::render::{self, Viewport};

use crate::app::App;
use crate::theme;

/// Pushes line segments of one colour.
struct Lines(Vec<render::Vertex>);

impl Lines {
    fn segment(&mut self, a: Vec3, b: Vec3, color: [f32; 3]) {
        for position in [a, b] {
            self.0.push(render::Vertex {
                position,
                center: position,
                normal: [0.0, 0.0, 1.0],
                color,
            });
        }
    }

    /// `a` to `b` with an arrowhead at `b`.
    fn arrow(&mut self, a: Vec3, b: Vec3, color: [f32; 3]) {
        self.segment(a, b, color);
        let d = sub(b, a);
        let len = dot(d, d).sqrt();
        if len < 1e-4 {
            return;
        }
        let dir = scale(d, 1.0 / len);
        let side = cross(dir, [0.0, 0.0, 1.0]);
        let side = if dot(side, side) < 1e-6 {
            [1.0, 0.0, 0.0]
        } else {
            normalize(side)
        };
        let head = (0.3f32).min(len * 0.4);
        let back = sub(b, scale(dir, head));
        self.segment(b, add(back, scale(side, head * 0.5)), color);
        self.segment(b, sub(back, scale(side, head * 0.5)), color);
    }
}

fn faces(ring: &[Vec3], normal: Vec3, color: [f32; 3]) -> Vec<render::Vertex> {
    let tris: Vec<geometry::Tri> = (1..ring.len().saturating_sub(1))
        .map(|k| geometry::Tri {
            p: [ring[0], ring[k], ring[k + 1]],
            n: normal,
        })
        .collect();
    render::overlay_vertices(&tris, color)
}

fn ring_lines(lines: &mut Lines, ring: &[Vec3], color: [f32; 3]) {
    for i in 0..ring.len() {
        lines.segment(ring[i], ring[(i + 1) % ring.len()], color);
    }
}

/// The pathfinding sectors as translucent polygons tinted by their flags,
/// and the links as lines: shared ones in the layer's colour, a sector's
/// boundary brighter; the selected sector outlined in the selection
/// colour.
fn pathfinding(
    p: &LevelPathfinding,
    selected: Option<usize>,
) -> (Vec<render::Vertex>, Vec<render::Vertex>) {
    let up = [0.0, 0.0, PATHFINDING_LIFT];
    let (mut tris, mut lines) = (Vec::new(), Lines(Vec::new()));
    for s in &p.sectors {
        let ring = nav_view::lifted(&s.ring, s.normal, PATHFINDING_LIFT);
        tris.extend(faces(
            &ring,
            s.normal,
            theme::sector_color(s.flags, s.floor()),
        ));
    }
    let shared = theme::layer_color(LayerId::Pathfinding);
    let edge = shared.map(|c| (c * 1.3).min(1.0));
    for l in &p.links {
        let color = if l.left.is_some() && l.right.is_some() {
            shared
        } else {
            edge
        };
        lines.segment(add(l.a, up), add(l.b, up), color);
    }
    if let Some(s) = selected.and_then(|i| p.sectors.get(i)) {
        let ring = nav_view::lifted(&s.ring, s.normal, PATHFINDING_LIFT * 2.0);
        ring_lines(&mut lines, &ring, theme::VOLUME_SELECTED);
    }
    (tris, lines.0)
}

/// The bots' polygons as translucent floor, and their links coloured by
/// kind with an arrowhead on each one-way link (drops, mostly); the
/// selected polygon or link in the selection colour.
fn bot_mesh(b: &BotNav, selected: Option<NavPick>) -> (Vec<render::Vertex>, Vec<render::Vertex>) {
    let color = theme::layer_color(LayerId::BotNav);
    let (mut tris, mut lines) = (Vec::new(), Lines(Vec::new()));
    for p in &b.polys {
        tris.extend(faces(
            &nav_view::lifted(&p.verts, p.normal, BOT_LIFT),
            p.normal,
            color,
        ));
    }
    for (i, l) in b.links.iter().enumerate() {
        let c = if selected == Some(NavPick::BotLink(i)) {
            theme::VOLUME_SELECTED
        } else {
            theme::bot_link_color(l.kind)
        };
        let [from, to] = b.lifted_line(l);
        if l.one_way {
            lines.arrow(from, to, c);
        } else {
            lines.segment(from, to, c);
        }
    }
    if let Some(NavPick::BotPoly(i)) = selected
        && let Some(p) = b.polys.get(i)
    {
        let ring = nav_view::lifted(&p.verts, p.normal, BOT_LIFT * 2.0);
        ring_lines(&mut lines, &ring, theme::VOLUME_SELECTED);
    }
    (tris, lines.0)
}

/// Each spot as a post at its stand with a cross at its foot, coloured by
/// state: a bounce's strip, a crouch spot's lip and way out, and a
/// diamond round the stand when the ghosts file keeps a run of it.
fn spots(b: &BotNav, selected: Option<usize>) -> Vec<render::Vertex> {
    let mut lines = Lines(Vec::new());
    let lift = [0.0, 0.0, BOT_LIFT * 2.0];
    for (i, s) in b.spots.iter().enumerate() {
        let c = if selected == Some(i) {
            theme::VOLUME_SELECTED
        } else {
            theme::spot_color(s.state())
        };
        let at = add(s.stand, lift);
        lines.segment(at, add(at, [0.0, 0.0, 0.5]), c);
        let r = 0.15;
        lines.segment(add(at, [-r, 0.0, 0.0]), add(at, [r, 0.0, 0.0]), c);
        lines.segment(add(at, [0.0, -r, 0.0]), add(at, [0.0, r, 0.0]), c);
        let [e0, e1] = s.edge;
        if dot(sub(e1, e0), sub(e1, e0)) > 1e-6 {
            lines.segment(add(e0, lift), add(e1, lift), c);
        }
        if s.kind() == SpotKind::Crouch {
            for [p, q] in &s.lip {
                lines.segment(*p, *q, c);
            }
            if s.out != [0.0, 0.0] {
                let out = [s.out[0] * 0.5, s.out[1] * 0.5, 0.0];
                lines.arrow(at, add(at, out), c);
            }
        }
        if s.ghost {
            let d = 0.3;
            let p = [
                add(at, [d, 0.0, 0.0]),
                add(at, [0.0, d, 0.0]),
                add(at, [-d, 0.0, 0.0]),
                add(at, [0.0, -d, 0.0]),
            ];
            ring_lines(&mut lines, &p, c);
        }
    }
    lines.0
}

/// Upload the three Navigation overlays (empty when hidden or not there).
pub(crate) fn upload(vp: &mut Viewport, device: &wgpu::Device, app: &App) {
    let session = &app.session;
    let selected = session.selected_nav();
    let shown = |id| app.layers.visible(id);
    let (tris, lines) = match session
        .pathfinding()
        .filter(|_| shown(LayerId::Pathfinding))
    {
        Some(p) => pathfinding(
            p,
            match selected {
                Some(NavPick::Sector(i)) => Some(i),
                _ => None,
            },
        ),
        None => Default::default(),
    };
    vp.set_layer_overlay(device, LayerId::Pathfinding, &tris, &lines);
    let bots = session.bot_nav();
    let (tris, lines) = match bots.filter(|_| shown(LayerId::BotNav)) {
        Some(b) => bot_mesh(b, selected),
        None => Default::default(),
    };
    vp.set_layer_overlay(device, LayerId::BotNav, &tris, &lines);
    let lines = match bots.filter(|_| shown(LayerId::BotSpots)) {
        Some(b) => spots(
            b,
            match selected {
                Some(NavPick::Spot(i)) => Some(i),
                _ => None,
            },
        ),
        None => Vec::new(),
    };
    vp.set_layer_overlay(device, LayerId::BotSpots, &[], &lines);
}

/// A navigation pick as the status line and inspector name it.
pub(crate) fn label(app: &App, pick: NavPick) -> String {
    let session = &app.session;
    match pick {
        NavPick::Sector(i) => match session.pathfinding().and_then(|p| p.sectors.get(i)) {
            Some(s) => format!("BSP {} pathfinding sector {}", s.bsp, s.sector),
            None => format!("pathfinding sector {i}"),
        },
        NavPick::BotPoly(i) => match session.bot_nav().and_then(|b| b.polys.get(i)) {
            Some(p) => format!("BSP {} bot polygon {}", p.bsp, p.poly),
            None => format!("bot polygon {i}"),
        },
        NavPick::BotLink(i) => match session.bot_nav().and_then(|b| b.links.get(i)) {
            Some(l) => format!("BSP {} bot {} link {}", l.bsp, l.kind.name(), l.link),
            None => format!("bot link {i}"),
        },
        NavPick::Spot(i) => match session.bot_nav().and_then(|b| b.spots.get(i)) {
            Some(s) => format!("{} {} spot {i}", s.state().name(), s.kind().name()),
            None => format!("spot {i}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::nav_view::{BotLink, BotLinkKind, BotPoly};

    fn quad(x: f32, z: f32) -> BotPoly {
        let verts = vec![
            [x, 0.0, z],
            [x + 1.0, 0.0, z],
            [x + 1.0, 1.0, z],
            [x, 1.0, z],
        ];
        BotPoly {
            bsp: 0,
            poly: 0,
            normal: nav_view::ring_normal(&verts),
            verts,
        }
    }

    /// The invented mesh draws two triangles a polygon, a segment per
    /// two-way link, and a segment and an arrowhead (three) per one-way
    /// drop, each in its kind's colour.
    #[test]
    fn an_invented_bot_mesh_draws_its_polys_and_links_by_kind() {
        let link = |from, to, kind| BotLink {
            bsp: 0,
            link: 0,
            from,
            to,
            a: [1.0, 0.0, 0.0],
            b: [1.0, 1.0, 0.0],
            kind,
            one_way: false,
        };
        let mut nav = BotNav {
            polys: vec![quad(0.0, 0.0), quad(1.0, 0.0), quad(2.0, -1.0)],
            links: vec![
                link(0, 1, BotLinkKind::Walk),
                link(1, 0, BotLinkKind::Walk),
                link(1, 2, BotLinkKind::Drop),
            ],
            ..BotNav::default()
        };
        nav.mark_one_way();
        let (tris, lines) = bot_mesh(&nav, None);
        assert_eq!(tris.len(), 3 * 2 * 3, "two triangles a quad");
        // Walk, walk, then the drop's line and its two head strokes.
        assert_eq!(lines.len(), 2 * (1 + 1 + 3));
        let colors: Vec<[f32; 3]> = lines.chunks(2).map(|s| s[0].color).collect();
        let walk = theme::bot_link_color(BotLinkKind::Walk);
        let drop = theme::bot_link_color(BotLinkKind::Drop);
        assert_eq!(colors, [walk, walk, drop, drop, drop]);
        assert!(tris.iter().all(|v| (v.position[2] - BOT_LIFT).abs() < 1e-6
            || (v.position[2] + 1.0 - BOT_LIFT).abs() < 1e-6));
        // Selecting the drop draws it in the selection colour.
        let (_, picked) = bot_mesh(&nav, Some(NavPick::BotLink(2)));
        assert_eq!(picked[4].color, theme::VOLUME_SELECTED);
    }
}
