//! The consistency drive: every screen of
//! [`crate::ui_kit::screens::NAMES`] in every named state, drawn without a
//! disc, held to the design system in `assets/ui/design.yaml`
//! (docs/game/ui-builder.md "Consistency").
//!
//! The rules, each a finding's `rule`:
//!
//! - **palette**: a text's colour is a palette colour, or one dimmed in
//!   proportion (a lobby's unselected tab). A colour the converter had to
//!   name after a screen (`theater.film.0`) is drift too.
//! - **font**: a title and a legend in their role's font, a description, a
//!   dialog's body and its status in the body font, code cells in the cell
//!   font, row texts in a row font. HUDs are their own.
//! - **place**: a title and a legend sit where the rest of their kind's
//!   do.
//! - **legend**: glyphs, one space, words in capitals; prompts two spaces
//!   apart.
//! - **case**: a dialog's, a picker's and a code entry's title in
//!   capitals.
//! - **pitch**: a list's rows are its skin's row height apart.
//! - **fit**: a title and a legend fit their rectangles.
//! - **empty**: a dialog's description area above its list (Quick
//!   Options' help line, beside the page's picture) says something in
//!   every state: the focused row's line or the page's help. An area left
//!   empty is dead space between the title and the rows; a page with
//!   nothing to say there has no description block, and its rows sit
//!   under the title.
//!
//! The pattern rules, from Halo 2's own screens:
//!
//! - **button**: a widget whose component has `pattern: button` in
//!   `design.yaml` (the lobby's START GAME, GAME SETUP) is drawn on a list
//!   skin's art or a row's, as retail draws its buttons, never as bare text.
//! - **handled**: every prompt the legend shows is an input the page takes.
//! - **arrows**: a value is chosen in the picker A opens, never stepped
//!   between `<` and `>` drawn round it.
//!
//! [`ALLOWED`] is the short list of findings that stand, each with its
//! reason.

use std::collections::HashMap;

use serde::Serialize;

use crate::ui_kit::components::{Kind, legend};
use crate::ui_kit::data::{self, DesignDoc};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::preview::{From, overflows};
use crate::ui_kit::screens::{self, NAMES};
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::view::{self, Synthetic, View, Widget};

/// One drift: where, which widgets, what is off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub screen: String,
    pub state: String,
    pub ids: Vec<String>,
    pub rule: &'static str,
    pub text: String,
}

/// What a check covered and what it found.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Screen states checked.
    pub checked: usize,
}

/// Findings that stand: `(rule, screen, why)`. A screen of `*` is every
/// screen.
pub const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "case",
        "signin-code",
        "its title carries the player's gamertag, as typed",
    ),
];

fn allowed(f: &Finding) -> bool {
    ALLOWED
        .iter()
        .any(|(rule, screen, _)| *rule == f.rule && (*screen == "*" || *screen == f.screen))
}

/// `c` is palette colour `p`, or `p` dimmed in proportion (each channel
/// the same fraction of `p`'s, give or take 2).
fn on_palette(c: [u8; 3], palette: &[(&str, [u8; 3])]) -> bool {
    palette.iter().any(|(_, p)| {
        if *p == c {
            return true;
        }
        let max = p.iter().copied().max().unwrap_or(0);
        if max == 0 {
            return c == [0, 0, 0];
        }
        let i = p.iter().position(|v| *v == max).unwrap_or(0);
        let k = f32::from(c[i]) / f32::from(max);
        k > 0.0
            && k <= 1.0
            && p.iter()
                .zip(c)
                .all(|(pv, cv)| (f32::from(*pv) * k - f32::from(cv)).abs() <= 2.0)
    })
}

/// Text widgets of `v` drawn by this screen (not its opener's rows).
fn texts(v: &View) -> impl Iterator<Item = (&Widget, &str, i16, [u8; 3])> {
    v.widgets.iter().filter(|w| !w.id.starts_with("opener.")).filter_map(|w| match &w.kind {
        view::Kind::Text { text, font, colour, .. } => Some((w, text.as_str(), *font, *colour)),
        view::Kind::Art { .. } => None,
    })
}

/// The role a text widget's id gives it, for the font rule.
fn font_role(id: &str) -> &'static [&'static str] {
    const TITLE: &[&str] = &["title"];
    const LEGEND: &[&str] = &["legend"];
    const BODY: &[&str] = &["body"];
    const CELL: &[&str] = &["cell"];
    const ROW: &[&str] = &["row_controls", "row_default"];
    const TEXT: &[&str] = &["body", "title"];
    match id {
        "title" => TITLE,
        "legend" => LEGEND,
        "description" | "name" | "body" | "status" => BODY,
        _ if id.starts_with("cell.") && id.ends_with(".text") => CELL,
        _ if id.starts_with("row.") && (id.ends_with(".label") || id.ends_with(".value")) => ROW,
        _ => TEXT,
    }
}

/// The kind a title's and a legend's place is compared within: a list by
/// its skin, a dialog by the retail dialog it is built on.
fn group(s: &ScreenSpec, v: &View) -> String {
    match (Kind::of(s), &s.dialog) {
        (Kind::List, _) => format!("list.skin{}", v.list.map_or(-1, |l| l.skin)),
        (k, Some(d)) => format!("{} on {:?}", k.name(), d.base),
        (k, None) => k.name().into(),
    }
}

/// Screen states as checked: every named state of every screen.
fn states(name: &str) -> Vec<(String, ScreenSpec)> {
    screens::live(name).map(|l| l.specs()).unwrap_or_default()
}

/// The single-view rules on `v` (spec `s`), into `out`.
fn view_rules(design: &DesignDoc, s: &ScreenSpec, v: &View, out: &mut Vec<Finding>) {
    let palette = design.palette_rgb();
    let find = |ids: Vec<String>, rule: &'static str, text: String| Finding {
        screen: v.screen.clone(),
        state: v.state.clone(),
        ids,
        rule,
        text,
    };
    let kind = Kind::of(s);
    let hud = kind == Kind::Hud;
    for (w, text, font, colour) in texts(v) {
        if w.alpha == 0 || text.is_empty() {
            continue;
        }
        if !on_palette(colour, &palette) {
            out.push(find(vec![w.id.clone()], "palette", format!("{colour:?} is not a palette colour")));
        }
        if !hud {
            let roles = font_role(&w.id);
            let fonts: Vec<i16> = roles.iter().filter_map(|r| design.font(r).ok()).collect();
            if !fonts.contains(&font) {
                out.push(find(
                    vec![w.id.clone()],
                    "font",
                    format!("font {font}, where its role ({}) is {fonts:?}", roles.join(" or ")),
                ));
            }
        }
        if w.id == "legend" {
            let line = text.rsplit('\n').next().unwrap_or(text);
            if let Err(e) = legend::well_formed(line, design.legend.glyph_gap, design.legend.prompt_gap) {
                out.push(find(vec![w.id.clone()], "legend", e));
            }
        }
        if !hud && matches!(w.id.as_str(), "title" | "legend") && overflows(w, &Synthetic) {
            out.push(find(vec![w.id.clone()], "fit", format!("{text:?} overflows its rectangle")));
        }
    }
    let dynamic_title = data::export::DYNAMIC.contains(&(s.name.as_str(), "title"));
    if matches!(kind, Kind::Dialog(_) | Kind::CodeEntry)
        && !dynamic_title
        && let Some(t) = v.widget("title").and_then(Widget::text)
        && t.chars().any(char::is_lowercase)
    {
        out.push(find(vec!["title".into()], "case", format!("{t:?}: a dialog's title is in capitals")));
    }
    if kind == Kind::List
        && let Some(skin) = v.list.map(|l| l.skin)
        && let Some(pitch) = match skin {
            2 => design.spacing("row_controls").ok(),
            0 => design.spacing("row_default").ok(),
            _ => None,
        }
    {
        let tops: Vec<(&str, i32)> = v
            .widgets
            .iter()
            .filter(|w| w.id.starts_with("row.") && w.id.ends_with(".label"))
            .map(|w| (w.id.as_str(), i32::from(w.rect[0])))
            .collect();
        for pair in tops.windows(2) {
            let gap = pair[0].1 - pair[1].1;
            if gap != pitch {
                out.push(find(
                    vec![pair[0].0.into(), pair[1].0.into()],
                    "pitch",
                    format!("rows {gap} apart on skin {skin}, whose rows are {pitch}"),
                ));
            }
        }
    }
}

/// The empty rule on `v` (spec `s`): a dialog's description block that
/// sits above its first row is drawn with text, and the page's picture
/// (Quick Options' game type picture) has its line beside it; a name
/// line (`ScreenSpec::name_line`) names the focused row.
fn empty_rules(s: &ScreenSpec, v: &View, out: &mut Vec<Finding>) {
    let art: Vec<String> = v
        .widgets
        .iter()
        .filter(|w| w.id.starts_with("art.") && matches!(w.kind, view::Kind::Art { .. }))
        .map(|w| w.id.clone())
        .collect();
    let says = |id: &str| {
        v.widget(id)
            .and_then(Widget::text)
            .is_some_and(|t| !t.trim().is_empty())
    };
    let mut flag = |id: &str, text: String| {
        let mut ids = vec![id.to_string()];
        ids.extend(art.iter().cloned());
        out.push(Finding {
            screen: v.screen.clone(),
            state: v.state.clone(),
            ids,
            rule: "empty",
            text,
        });
    };
    if let Some(n) = s.name_line
        && !says("name")
    {
        flag(
            "name",
            format!(
                "the name line beside the picture ({:?}) says nothing: name the focused row",
                n.rect
            ),
        );
    }
    let (Some(d), Some(_)) = (s.description, &s.dialog) else {
        return;
    };
    let Some(first_row) = v
        .widgets
        .iter()
        .filter(|w| w.id.starts_with("row.") && w.id.ends_with(".label") && w.alpha > 0)
        .map(|w| w.rect[0])
        .max()
    else {
        return;
    };
    let [_, _, bottom, _] = d.rect;
    if bottom >= first_row && !says("description") {
        flag(
            "description",
            format!(
                "the description area above the list ({:?}) says nothing: give the focused row a line, or drop the block so the rows sit under the title",
                d.rect
            ),
        );
    }
}

/// The inputs a glyph can stand for.
const INPUTS: [Input; 10] = [
    Input::Up,
    Input::Down,
    Input::Left,
    Input::Right,
    Input::A,
    Input::B,
    Input::X,
    Input::Y,
    Input::Start,
    Input::Back,
];

/// The prompts the legend shows: the focused row's on a list that has
/// them, else the page's.
fn shown_prompts(s: &ScreenSpec) -> &[Prompt] {
    let rows = s.dialog.as_ref().map_or(&s.rows, |d| &d.rows);
    if s.dialog.is_none()
        && s.footer
        && let Some(p) = rows.get(s.focus).and_then(|r| r.prompts.as_deref())
    {
        return p;
    }
    &s.prompts
}

/// Whether design pattern `pattern` (`button.*`, `game`) names widget
/// `id`: equal, or a prefix before a `*`.
fn id_matches(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => id == pattern,
    }
}

/// Text drawn with value arrows round it (`< All >`), or an arrow alone.
fn arrows(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        matches!(l, "<" | ">" | "\u{25c0}" | "\u{25b6}")
            || l.split_once('<').is_some_and(|(_, tail)| tail.contains('>'))
            || l.split_once('\u{25c0}').is_some_and(|(_, tail)| tail.contains('\u{25b6}'))
    })
}

/// The pattern rules on `v` (spec `s`): buttons on button art, the
/// legend's prompts handled, values in pickers.
fn pattern_rules(design: &DesignDoc, s: &ScreenSpec, v: &View, out: &mut Vec<Finding>) {
    let find = |ids: Vec<String>, rule: &'static str, text: String| Finding {
        screen: v.screen.clone(),
        state: v.state.clone(),
        ids,
        rule,
        text,
    };
    let buttons: Vec<(&String, &data::ComponentDoc)> = design
        .components
        .iter()
        .filter(|(_, c)| c.pattern.as_deref() == Some("button"))
        .collect();
    for (w, text, ..) in texts(v) {
        if w.alpha == 0 || text.trim().is_empty() {
            continue;
        }
        if arrows(text) {
            out.push(find(
                vec![w.id.clone()],
                "arrows",
                format!("{text:?}: a value is chosen in the picker A opens, never stepped between arrows"),
            ));
        }
        let Some((name, c)) = buttons
            .iter()
            .find(|(_, c)| c.ids.iter().any(|p| id_matches(p, &w.id)))
        else {
            continue;
        };
        let ink = crate::ui_kit::layout::ink(w, &Synthetic).unwrap_or(w.rect());
        let on_art = v.widgets.iter().any(|a| {
            a.alpha > 0
                && match &a.kind {
                    view::Kind::Art { art, .. } => {
                        matches!(art, view::Art::Skin { .. }) || a.layer == view::Layer::Under
                    }
                    view::Kind::Text { .. } => false,
                }
                && crate::ui_kit::layout::area(crate::ui_kit::layout::intersect(ink, a.rect())) * 2
                    >= crate::ui_kit::layout::area(ink)
        });
        if !on_art {
            out.push(find(
                vec![w.id.clone()],
                "button",
                format!(
                    "{text:?} is a {name} ({}): draw it on a list skin's button art or as a row, not as bare text",
                    c.role
                ),
            ));
        }
    }
    if Kind::of(s) != Kind::Hud {
        let mut seen: Vec<Glyph> = Vec::new();
        for g in shown_prompts(s).iter().flat_map(|p| &p.glyphs) {
            if seen.contains(g) {
                continue;
            }
            seen.push(*g);
            let taken = INPUTS
                .into_iter()
                .any(|i| Glyph::of(i) == *g && s.inputs.contains(&i));
            if !taken {
                out.push(find(
                    vec!["legend".into()],
                    "handled",
                    format!("the legend shows {}, and the page takes no such input", g.name()),
                ));
            }
        }
    }
}

/// Converter-named palette colours and presets: drift the design system
/// carries until a screen is moved onto a token of its own.
fn design_rules(design: &DesignDoc, out: &mut Vec<Finding>) {
    for name in design.palette.keys().filter(|n| n.contains('.')) {
        out.push(Finding {
            screen: "design".into(),
            state: "palette".into(),
            ids: vec![name.clone()],
            rule: "palette",
            text: format!("colour {name:?} is one screen's, not the design system's"),
        });
    }
}

/// The views of screens `names`, each state, drawn without a disc.
fn views(names: &[&str]) -> Vec<(ScreenSpec, View)> {
    let from = From::Synthetic;
    names
        .iter()
        .flat_map(|n| states(n))
        .filter_map(|(state, s)| from.view(&s, &state).ok().map(|v| (s, v)))
        .collect()
}

/// The place rule over `views`: per kind, the title's and the legend's
/// rectangle most of the kind's screens have, and the views that differ.
fn place_rules(views: &[(ScreenSpec, View)], out: &mut Vec<Finding>) {
    for id in ["title", "legend"] {
        let mut by_group: HashMap<String, HashMap<[i16; 4], usize>> = HashMap::new();
        for (s, v) in views {
            if Kind::of(s) == Kind::Hud {
                continue;
            }
            if let Some(w) = v.widget(id) {
                *by_group.entry(group(s, v)).or_default().entry(w.rect).or_default() += 1;
            }
        }
        let usual: HashMap<String, [i16; 4]> = by_group
            .into_iter()
            .filter_map(|(g, rects)| {
                let mut rects: Vec<([i16; 4], usize)> = rects.into_iter().collect();
                rects.sort_by_key(|(r, n)| (std::cmp::Reverse(*n), *r));
                rects.first().map(|(r, _)| (g, *r))
            })
            .collect();
        for (s, v) in views {
            if Kind::of(s) == Kind::Hud {
                continue;
            }
            let g = group(s, v);
            if let (Some(w), Some(r)) = (v.widget(id), usual.get(&g))
                && w.rect != *r
            {
                out.push(Finding {
                    screen: v.screen.clone(),
                    state: v.state.clone(),
                    ids: vec![id.into()],
                    rule: "place",
                    text: format!("{id} at {:?}, where a {g}'s is at {r:?}", w.rect),
                });
            }
        }
    }
}

/// Checks screen `screen` (every screen when `None`) against the design
/// system: the findings [`ALLOWED`] doesn't excuse, and how many screen
/// states were checked. The place rule compares within the whole set, so
/// one screen's check still draws them all.
pub fn check(screen: Option<&str>) -> Report {
    let design = data::design();
    let all = views(&NAMES);
    let mut findings = Vec::new();
    let mut checked = 0;
    for (s, v) in &all {
        if screen.is_some_and(|n| n != v.screen) {
            continue;
        }
        checked += 1;
        view_rules(design, s, v, &mut findings);
        empty_rules(s, v, &mut findings);
        pattern_rules(design, s, v, &mut findings);
    }
    let mut placed = Vec::new();
    place_rules(&all, &mut placed);
    findings.extend(placed.into_iter().filter(|f| screen.is_none_or(|n| n == f.screen)));
    if screen.is_none() {
        design_rules(design, &mut findings);
    }
    findings.retain(|f| !allowed(f));
    Report { findings, checked }
}

/// The rules that read one view alone (all but **place**) on spec `s`
/// drawn as `v`, without what [`ALLOWED`] excuses: what a preview prints
/// for one state.
pub fn view_findings(s: &ScreenSpec, v: &View) -> Vec<Finding> {
    let design = data::design();
    let mut out = Vec::new();
    view_rules(design, s, v, &mut out);
    empty_rules(s, v, &mut out);
    pattern_rules(design, s, v, &mut out);
    out.retain(|f| !allowed(f));
    out
}

/// The report as the agent API gives it (`ui.consistency`).
pub fn report_json(screen: Option<&str>) -> serde_json::Value {
    let r = check(screen);
    serde_json::json!({
        "findings": r.findings,
        "checked": r.checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every screen in every state keeps to the design system. A finding
    /// is fixed in the screen (or as a converter rule,
    /// `crate::ui_kit::data::export`), or listed in [`ALLOWED`] with why.
    #[test]
    fn every_screen_keeps_to_the_design_system() {
        let r = check(None);
        assert!(r.checked > NAMES.len(), "checked {} states", r.checked);
        let lines: Vec<String> = r
            .findings
            .iter()
            .map(|f| format!("{} [{}] {} {:?}: {}", f.screen, f.state, f.rule, f.ids, f.text))
            .collect();
        assert!(lines.is_empty(), "{} findings:\n{}", lines.len(), lines.join("\n"));
    }

    #[test]
    fn a_dimmed_palette_colour_is_on_the_palette_and_a_stray_one_is_not() {
        let p = [("hint_blue", [173, 195, 216]), ("white", [255, 255, 255])];
        assert!(on_palette([173, 195, 216], &p));
        assert!(on_palette([95, 107, 119], &p), "hint blue at 0.55");
        assert!(on_palette([140, 140, 140], &p), "white dimmed");
        assert!(!on_palette([235, 243, 252], &p));
        assert!(!on_palette([255, 158, 143], &p));
    }

    #[test]
    fn each_allowance_names_a_screen_and_a_reason() {
        for (rule, screen, why) in ALLOWED {
            assert!(
                [
                    "palette", "font", "place", "legend", "case", "pitch", "fit", "empty", "button",
                    "handled", "arrows",
                ]
                .contains(rule),
                "{rule}"
            );
            assert!(*screen == "*" || NAMES.contains(screen) || *screen == "design", "{screen}");
            assert!(!why.is_empty());
        }
    }

    #[test]
    fn one_screen_is_checked_alone() {
        let all = check(None);
        let one = check(Some("kit.example"));
        assert!(one.checked > 0 && one.checked < all.checked);
        assert!(one.findings.iter().all(|f| f.screen == "kit.example"));
    }

    /// The Firefight page with its rows' lines gone (the help line drawn
    /// empty above the list, beside the page's picture) is found, on
    /// every state; with them it is not.
    #[test]
    fn an_empty_description_area_above_the_list_is_found() {
        let all = views(&["firefight"]);
        assert!(!all.is_empty());
        for (s, v) in &all {
            let mut out = Vec::new();
            empty_rules(s, v, &mut out);
            assert!(out.is_empty(), "{} [{}]: {out:?}", v.screen, v.state);
            let mut empty = v.clone();
            empty.widgets.retain(|w| w.id != "description");
            empty_rules(s, &empty, &mut out);
            assert_eq!(out.len(), 1, "{} [{}]", v.screen, v.state);
            assert_eq!(out[0].rule, "empty");
            assert!(out[0].ids.contains(&"art.0".to_string()), "{:?}", out[0].ids);
        }
        // The name line beside the picture: an empty one is found too,
        // on every options page that has one.
        let named = views(&["firefight", "clankers", "h1-options"]);
        assert!(named.iter().any(|(s, _)| s.name_line.is_some()));
        for (s, v) in named.iter().filter(|(s, _)| s.name_line.is_some()) {
            let mut out = Vec::new();
            empty_rules(s, v, &mut out);
            assert!(out.is_empty(), "{} [{}]: {out:?}", v.screen, v.state);
            let mut unnamed = v.clone();
            unnamed.widgets.retain(|w| w.id != "name");
            empty_rules(s, &unnamed, &mut out);
            assert_eq!(out.len(), 1, "{} [{}]: {out:?}", v.screen, v.state);
            assert_eq!(out[0].rule, "empty");
            assert_eq!(out[0].ids[0], "name");
        }
        // A list's description under its rows (the Controls list's) is
        // not a header area: an empty one there is not this rule's.
        let (s, mut v) = views(&["kit.example"]).into_iter().next().unwrap();
        v.widgets.retain(|w| w.id != "description");
        let mut out = Vec::new();
        empty_rules(&s, &v, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    /// A screen drifting from its kind is found: a title in the wrong
    /// font, words in lower case, a stray colour.
    #[test]
    fn drift_is_found() {
        let design = data::design();
        let (s, mut v) = views(&["kit.dialog"]).into_iter().next().unwrap();
        for w in &mut v.widgets {
            if let view::Kind::Text { text, font, colour, .. } = &mut w.kind
                && w.id == "title"
            {
                *text = "Choices".into();
                *font = 9;
                *colour = [255, 0, 255];
            }
        }
        let mut out = Vec::new();
        view_rules(design, &s, &v, &mut out);
        let rules: Vec<&str> = out.iter().map(|f| f.rule).collect();
        for r in ["palette", "font", "case"] {
            assert!(rules.contains(&r), "{r} in {rules:?}");
        }
    }
}
