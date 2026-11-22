//! The checks on what the preview really drew (docs/game/ui-builder.md
//! "The checks"): each widget's [`Footprint`], the pixels it touched with
//! alpha above zero, from [`render_traced`]. Nothing here looks at the
//! picture as a picture; it compares footprints, rectangles and ids.
//!
//! A widget's **group** is its id's first part, the spec's own naming:
//! `button.start` is the buttons', `player.2` the roster's, `game` its
//! own; every row and cell (`row.3.label`, `cell.2.art`) is the list's
//! ([`group`]). Two widgets of one group belong together; two of different
//! groups don't, unless one is a declared backing (a panel).

use std::collections::HashMap;

use tags::ui::Rect;

use super::{Issue, Rules, area, row_of};
use crate::ui_kit::preview::{Footprint, Source, render_traced};
use crate::ui_kit::view::{Art, Kind, Layer, View, Widget};

/// Kit screens whose modal may cover the content of the screen beneath
/// it, each with the retail precedent: `(screen, why)`. A modal over a kit
/// screen that isn't here and covers its pictures, status or roster is a
/// [`Issue::CoversUnder`].
pub const COVERS: &[(&str, &str)] = &[(
    crate::theater_ui::PAUSE_MENU,
    "its value pickers open over the pause menu and its scoreboard as \
     retail's Game Setup dialog (squad_settings_dialog, screen 23) opens \
     over the pregame lobby and its player list",
)];

/// Kit screens that draw the lobby's panel with the list in it and a table
/// of their own texts, each with the retail precedent: `(screen, why)`. A
/// panel holding the list's rows and another group's texts on a screen not
/// here is a [`Issue::PanelShared`].
pub const SHARED_PANELS: &[(&str, &str)] = &[
    (
        crate::theater_ui::DETAIL,
        "the film's actions and its details table share the lobby's panel \
         as retail's pregame lobby holds Start Game, Game Setup and its \
         status lines in one pane",
    ),
    (
        crate::theater_ui::OVERLAY,
        "the playback panel's rows and its table share the lobby's panel \
         as retail's pregame lobby holds its list and status lines in one pane",
    ),
];

/// Groups never held to another group: the header and the legend sit on
/// every screen's chrome.
const CHROME: [&str; 2] = ["title", "legend"];

/// The list's own texts and the chrome, which a modal opened from the list
/// covers as retail's do: the rows, the help line about the focused row,
/// the dialog's body and status, and the HUD's reticle, which the pause
/// menu and every dialog in retail cover when they open over the world.
const NOT_CONTENT: [&str; 8] = [
    "reticle",
    "list",
    "title",
    "legend",
    "description",
    "name",
    "body",
    "status",
];

/// Widget `id`'s group: `list` for a row's or a cell's widgets, else the
/// id's first part.
pub fn group(id: &str) -> &str {
    if row_of(id).is_some() {
        "list"
    } else {
        id.split('.').next().unwrap_or(id)
    }
}

fn is_picture(w: &Widget) -> bool {
    matches!(w.kind, Kind::Art { art: Art::Level { .. }, .. })
}

/// The checks on `v`'s footprints, leaving out what `already` reports.
pub(super) fn check(v: &View, src: &dyn Source, rules: &Rules, already: &[Issue]) -> Vec<Issue> {
    let traced = render_traced(v, src);
    let drawn: HashMap<&str, &Footprint> = traced
        .footprints
        .iter()
        .filter(|f| f.pixels > 0 || f.bbox.is_some())
        .map(|f| (f.id.as_str(), f))
        .collect();
    let shown: Vec<(&Widget, &Footprint)> = v
        .widgets
        .iter()
        .filter(|w| w.alpha > 0)
        .filter_map(|w| drawn.get(w.id.as_str()).map(|f| (w, *f)))
        .collect();
    let mut out = Vec::new();
    outside(&shown, rules, &mut out);
    on_drawn(&shown, rules, already, &mut out);
    if rules.panel.is_none() && !SHARED_PANELS.iter().any(|(s, _)| *s == v.screen) {
        panel_shared(&shown, rules, &mut out);
    }
    if let Some(under) = &v.underlay
        && !COVERS.iter().any(|(s, _)| *s == v.screen)
    {
        covers_under(&shown, under, src, rules, &mut out);
    }
    out
}

/// Art drawn past its declared rectangle.
fn outside(shown: &[(&Widget, &Footprint)], rules: &Rules, out: &mut Vec<Issue>) {
    for (w, f) in shown {
        if !matches!(w.kind, Kind::Art { .. }) {
            continue;
        }
        let by = f.past(w.rect());
        if by > rules.max_outside {
            out.push(Issue::DrawnOutside {
                id: w.id.clone(),
                by,
            });
        }
    }
}

/// Whether `already` reports texts `a` and `b`, or text `a` on art `b`.
fn reported(already: &[Issue], a: &str, b: &str) -> bool {
    already.iter().any(|i| match i {
        Issue::TextOverlap { a: x, b: y } => (x == a && y == b) || (x == b && y == a),
        Issue::OnOtherArt { text, art } => text == a && art == b,
        _ => false,
    })
}

/// A text's glyphs over another group's row or cell art, picture or text.
/// Frame art and panels are left to the frame checks: texts sit on them
/// by design.
fn on_drawn(
    shown: &[(&Widget, &Footprint)],
    rules: &Rules,
    already: &[Issue],
    out: &mut Vec<Issue>,
) {
    for (t, tf) in shown.iter().filter(|(w, _)| w.text().is_some()) {
        for (o, of) in shown {
            if o.id == t.id || group(&o.id) == group(&t.id) {
                continue;
            }
            let eligible = match o.kind {
                Kind::Text { .. } => true,
                Kind::Art { .. } => o.layer == Layer::Under || is_picture(o),
            };
            if !eligible || reported(already, &t.id, &o.id) {
                continue;
            }
            // Each text pair once.
            if o.text().is_some() && o.id < t.id {
                continue;
            }
            let pixels = tf.overlap(of);
            if pixels > rules.max_covered {
                out.push(Issue::OnDrawn {
                    text: t.id.clone(),
                    on: o.id.clone(),
                    pixels,
                });
            }
        }
    }
}

/// Whether at least half of `f`'s pixels are inside `r`.
fn mostly_in(f: &Footprint, r: Rect) -> bool {
    f.pixels > 0 && f.pixels_in(r) * 2 >= f.pixels
}

/// A panel (a box of the screen's own, not a modal's) that holds the
/// list's rows and another group's texts. A panel over more than half the
/// title-safe area is a backdrop, not a box, and holds what it likes.
fn panel_shared(shown: &[(&Widget, &Footprint)], rules: &Rules, out: &mut Vec<Issue>) {
    for (p, pf) in shown
        .iter()
        .filter(|(w, _)| w.layer == Layer::Panel && matches!(w.kind, Kind::Art { .. }) && !is_picture(w))
    {
        let Some(box_) = pf.rect() else { continue };
        if area(box_) * 2 > area(rules.safe) {
            continue;
        }
        let inside: Vec<&Widget> = shown
            .iter()
            .filter(|(w, _)| w.id != p.id && w.layer != Layer::Panel)
            .filter(|(w, _)| w.text().is_some() || w.layer == Layer::Under)
            .filter(|(_, f)| mostly_in(f, box_))
            .map(|(w, _)| *w)
            .collect();
        let Some(rows) = inside.iter().find(|w| group(&w.id) == "list") else {
            continue;
        };
        let with: Vec<String> = inside
            .iter()
            .filter(|w| w.text().is_some_and(|t| !t.trim().is_empty()))
            .filter(|w| {
                let g = group(&w.id);
                g != "list" && !CHROME.contains(&g)
            })
            .map(|w| w.id.clone())
            .collect();
        if !with.is_empty() {
            out.push(Issue::PanelShared {
                panel: p.id.clone(),
                rows: rows.id.clone(),
                with,
            });
        }
    }
}

/// The modal (`shown`) over kit screen `under`: what of `under`'s content
/// (pictures, and texts that are neither the list's nor chrome) it covers.
fn covers_under(
    shown: &[(&Widget, &Footprint)],
    under: &View,
    src: &dyn Source,
    rules: &Rules,
    out: &mut Vec<Issue>,
) {
    let theirs = render_traced(under, src);
    for u in under.widgets.iter().filter(|w| w.alpha > 0) {
        let content = is_picture(u)
            || (u.text().is_some_and(|t| !t.trim().is_empty())
                && !NOT_CONTENT.contains(&group(&u.id)));
        if !content {
            continue;
        }
        let Some(uf) = theirs.footprint(&u.id) else {
            continue;
        };
        let pixels = uf
            .touched()
            .filter(|&(x, y)| shown.iter().any(|(_, f)| f.hits(x, y)))
            .count();
        if pixels > rules.max_covered {
            out.push(Issue::CoversUnder {
                under: u.id.clone(),
                pixels,
            });
        }
    }
}

#[cfg(test)]
mod tests;
