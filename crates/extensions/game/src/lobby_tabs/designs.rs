//! Three designs for the strip, each a specimen page
//! (`design.lobby-tabs.a`, `.b`, `.c`; `h2 ui preview design.lobby-tabs.a
//! --state all`), drawn over the game shell so the header band they sit
//! on shows.
//!
//! **The room** ([`BAND_LEFT`] to [`BAND_RIGHT`], [`TOP`] to [`BOTTOM`]).
//! The shell's header band is blue from the screen's left edge to its cut
//! corner: its top rows end at about x +43 and its lower rows, where a
//! text line's ink is, at about x +70 (measured on a preview of the
//! shell, the band's fill from pixel row 32 to 58). Text starts at the
//! title-safe edge, x -270, as retail's header does. So a line of the
//! strip has 338 units, and the five labels in capitals in font 1 are
//! 326 wide before any gap or glyph: no design here puts all five in
//! capitals on the band.
//!
//! - **A, a window of three** ([`a`]): three tabs in capitals, the
//!   selected one white, with the bumpers at the band's ends in the form
//!   retail's keyboard has for its shoulder buttons (`< glyph`, `glyph >`).
//! - **B, the header and where the bumpers go** ([`b`]): the selected
//!   tab is the lobby's header, in the header's font and place; the
//!   bumpers are a legend at the band's right end, each naming the tab it
//!   moves to, in the form of the footer's legends (`glyph WORDS`).
//! - **C, all five** ([`c`]): the five tabs in the lobby's own text case
//!   across the band, the bumpers past the cut corner beside the player
//!   count.
//!
//! Every text takes its font, colour and justification from a `tab.*`
//! component of `assets/ui/design.yaml`. The bumpers are
//! [`Glyph::White`] (left) and [`Glyph::Black`] (right), which a modern
//! pad's icon set draws as its left and right bumper
//! (docs/input/button-icons.md).

use super::{Avail, Dir, Facts, Look, Place, Present, Step, Tab, glyph_on, look, row, step};
use crate::ui_kit::components::Screen;
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::view::{Kind, Layer, Widget};

/// The three pages, as `ui_kit::screens::NAMES` lists them.
pub const PAGES: [&str; 3] = [
    "design.lobby-tabs.a",
    "design.lobby-tabs.b",
    "design.lobby-tabs.c",
];

/// The band's room for a line of text: from the title-safe edge to the
/// cut corner.
pub const BAND_LEFT: i16 = -270;
pub const BAND_RIGHT: i16 = 68;
/// A line of the strip in font 1: low enough in the band that its ink is
/// under the band's upper notch.
pub const TOP: i16 = 200;
pub const BOTTOM: i16 = 180;
/// The lobby's header rectangle's top and bottom (`strip::HEADER`).
pub const HEADER_TOP: i16 = 204;
pub const HEADER_BOTTOM: i16 = 180;
/// The player count's rectangle in the lobby's tag (`strip::COUNT`),
/// its right edge held to the title-safe area's (the tag's runs to 280).
pub const COUNT: [i16; 4] = [199, 85, 179, 272];
/// The count's text in a specimen: the tag's own form (`player_count2`).
pub const COUNT_TEXT: &str = "4 player(s) in party";

/// A lobby a specimen draws the strip for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lobby {
    pub present: Present,
    pub place: Place,
    pub facts: Facts,
    /// This console leads the party: the bumpers show.
    pub leader: bool,
    pub selected: Tab,
}

impl Lobby {
    /// An Xbox Live lobby with every tab present and on, led from this
    /// console, on `selected`.
    pub const fn live(selected: Tab) -> Lobby {
        Lobby {
            present: Present::ALL,
            place: Place::XboxLive,
            facts: Facts {
                playlists: true,
                live_co_op: true,
                remote_members: false,
            },
            leader: true,
            selected,
        }
    }

    /// The tabs present, each with whether it runs here.
    pub fn row(&self) -> Vec<(Tab, Avail)> {
        row(self.present, self.place, self.facts)
    }
}

/// A text of the strip in component `component`'s font, colour and
/// justification (`assets/ui/design.yaml`).
fn text(id: &str, component: &str, rect: [i16; 4], s: &str) -> Widget {
    let d = crate::ui_kit::data::design();
    let (font, colour) = d.text_style(component).unwrap_or_else(|e| panic!("{e}"));
    let justify = d
        .component(component)
        .unwrap_or_else(|e| panic!("{e}"))
        .justify();
    Widget {
        id: id.into(),
        rect,
        layer: Layer::Content,
        alpha: 255,
        kind: Kind::Text {
            text: s.into(),
            font,
            justify,
            colour: crate::ui_kit::components::rgb(colour),
        },
    }
}

/// The component a tab's look is drawn in.
pub const fn tab_component(look: Look) -> &'static str {
    match look {
        Look::Selected => "tab.selected",
        Look::On => "tab.on",
        Look::Off => "tab.off",
    }
}

/// The component a bumper's glyph is drawn in: bright when a tab that is
/// on lies that way.
pub const fn glyph_component(on: bool) -> &'static str {
    if on { "tab.glyph" } else { "tab.glyph.off" }
}

fn tab_id(tab: Tab) -> String {
    format!("tab.{}", tab.name())
}

fn count(rect: [i16; 4]) -> Widget {
    text("count", "tab.count", rect, COUNT_TEXT)
}

// ---- A: a window of three ----

/// Design A: three tabs at a time, in capitals.
pub mod a {
    use super::*;

    /// How many tabs show.
    pub const WINDOW: usize = 3;
    /// The gap between two tabs' cells.
    pub const GAP: i16 = 16;
    /// A bumper's cell: its glyph and its chevron.
    pub const PROMPT_WIDTH: i16 = 34;
    pub const PREV: [i16; 4] = [TOP, BAND_LEFT, BOTTOM, BAND_LEFT + PROMPT_WIDTH];
    pub const NEXT: [i16; 4] = [TOP, BAND_RIGHT - PROMPT_WIDTH, BOTTOM, BAND_RIGHT];
    /// The tabs' room, between the bumpers' cells.
    pub const RUN_LEFT: i16 = PREV[3] + 6;
    pub const RUN_RIGHT: i16 = NEXT[1] - 6;

    /// The bumpers as retail's keyboard writes its shoulder buttons
    /// (`left_arrow` "< glyph", `right_arrow` "glyph >").
    pub fn prompt(dir: Dir) -> String {
        let g = dir.glyph().char();
        match dir {
            Dir::Prev => format!("< {g}"),
            Dir::Next => format!("{g} >"),
        }
    }

    /// The tabs that show with `selected` the lobby's: the selected one
    /// with a neighbour each side, or the first or last three at an end.
    pub fn window(tabs: &[Tab], selected: Tab) -> &[Tab] {
        if tabs.len() <= WINDOW {
            return tabs;
        }
        let at = tabs.iter().position(|t| *t == selected).unwrap_or(0);
        let start = at.saturating_sub(1).min(tabs.len() - WINDOW);
        &tabs[start..start + WINDOW]
    }

    /// The gap between the cells of `shown`: [`GAP`], or what the run has
    /// left when the three widest labels meet (MULTIPLAYER, CUSTOM GAMES
    /// and FIREFIGHT, with FORGE switched off).
    pub fn gap(shown: &[Tab]) -> i16 {
        let labels: i16 = shown.iter().map(|t| t.width()).sum();
        let between = shown.len().saturating_sub(1).max(1) as i16;
        GAP.min((RUN_RIGHT - RUN_LEFT - labels) / between)
    }

    /// Each shown tab's cell: its label's width ([`Tab::width`]), [`gap`]
    /// apart, the run centred between the bumpers.
    pub fn cells(tabs: &[Tab], selected: Tab) -> Vec<(Tab, [i16; 4])> {
        let shown = window(tabs, selected);
        let gap = gap(shown);
        let labels: i16 = shown.iter().map(|t| t.width()).sum();
        let width = labels + gap * shown.len().saturating_sub(1) as i16;
        let mut left = RUN_LEFT + (RUN_RIGHT - RUN_LEFT - width) / 2;
        shown
            .iter()
            .map(|&t| {
                let cell = (t, [TOP, left, BOTTOM, left + t.width()]);
                left += t.width() + gap;
                cell
            })
            .collect()
    }

    /// The strip's widgets for `l`.
    pub fn widgets(l: &Lobby) -> Vec<Widget> {
        let row = l.row();
        let mut out = Vec::new();
        if l.leader {
            for (dir, id, rect) in [
                (Dir::Prev, "prompt.prev", PREV),
                (Dir::Next, "prompt.next", NEXT),
            ] {
                let on = glyph_on(&row, l.selected, dir);
                out.push(text(id, glyph_component(on), rect, &prompt(dir)));
            }
        }
        for (tab, cell) in cells(&l.present.tabs(), l.selected) {
            let avail = row
                .iter()
                .find(|(t, _)| *t == tab)
                .map_or(Avail::On, |(_, a)| *a);
            let look = look(tab, l.selected, avail);
            out.push(text(&tab_id(tab), tab_component(look), cell, tab.label()));
        }
        out.push(count(COUNT));
        out
    }
}

// ---- B: the header and where the bumpers go ----

/// Design B: the selected tab as the lobby's header, the bumpers a legend
/// naming the tabs they move to.
pub mod b {
    use super::*;

    /// A prompt's glyph and the space after it.
    pub const GLYPH: i16 = 26;
    /// The gap between the two prompts, and between the header and them.
    pub const PROMPT_GAP: i16 = 12;
    pub const HEADER_GAP: i16 = 8;

    /// Where `dir`'s bumper goes from `selected`: the tab [`step`] moves
    /// to, bright; with only tabs that are off that way, the nearest one,
    /// not bright (its reason shows on a press); at the strip's end,
    /// nothing.
    pub fn target(row: &[(Tab, Avail)], selected: Tab, dir: Dir) -> Option<(Tab, bool)> {
        match step(row, selected, dir) {
            Step::Move { to, .. } => Some((to, true)),
            Step::Stay(None) => None,
            Step::Stay(Some(_)) => {
                let at = row.iter().position(|(t, _)| *t == selected)?;
                let next = match dir {
                    Dir::Prev => at.checked_sub(1)?,
                    Dir::Next => at + 1,
                };
                row.get(next).map(|(t, _)| (*t, false))
            }
        }
    }

    /// A prompt as the footer's legends write one: glyph, space, WORDS.
    pub fn prompt(dir: Dir, tab: Tab) -> String {
        format!("{} {}", dir.glyph().char(), tab.label())
    }

    /// One prompt of the legend.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Prompt {
        pub dir: Dir,
        pub tab: Tab,
        /// The bumper moves there.
        pub on: bool,
        pub rect: [i16; 4],
    }

    /// The legend for `l`: Black's prompt at the band's right end, White's
    /// before it, each cut for its glyph and its tab's label. A member's
    /// console has none.
    pub fn prompts(l: &Lobby) -> Vec<Prompt> {
        if !l.leader {
            return Vec::new();
        }
        let row = l.row();
        let mut right = BAND_RIGHT;
        let mut out = Vec::new();
        for dir in [Dir::Next, Dir::Prev] {
            if let Some((tab, on)) = target(&row, l.selected, dir) {
                let left = right - GLYPH - tab.width();
                out.push(Prompt {
                    dir,
                    tab,
                    on,
                    rect: [TOP, left, BOTTOM, right],
                });
                right = left - PROMPT_GAP;
            }
        }
        out.reverse();
        out
    }

    /// The header's rectangle: the lobby's own, up to the legend.
    pub fn header(prompts: &[Prompt]) -> [i16; 4] {
        let right = prompts
            .first()
            .map_or(BAND_RIGHT, |p| p.rect[1] - HEADER_GAP);
        [HEADER_TOP, BAND_LEFT, HEADER_BOTTOM, right]
    }

    /// The strip's widgets for `l`.
    pub fn widgets(l: &Lobby) -> Vec<Widget> {
        let prompts = prompts(l);
        let mut out = vec![text(
            "tab.selected",
            "tab.header",
            header(&prompts),
            l.selected.label(),
        )];
        for p in &prompts {
            let id = match p.dir {
                Dir::Prev => "prompt.prev",
                Dir::Next => "prompt.next",
            };
            let component = if p.on { "tab.prompt" } else { "tab.prompt.off" };
            out.push(text(id, component, p.rect, &prompt(p.dir, p.tab)));
        }
        out.push(count(COUNT));
        out
    }
}

// ---- C: all five ----

/// Design C: every tab on the band, in the lobby's text case.
pub mod c {
    use super::*;

    /// The gap between two tabs' cells.
    pub const GAP: i16 = 14;
    /// The bumpers' cells, past the band's cut corner, and the player
    /// count narrowed to leave them room.
    pub const PREV: [i16; 4] = [199, 90, 179, 114];
    pub const NEXT: [i16; 4] = [199, 118, 179, 142];
    pub const COUNT: [i16; 4] = [199, 142, 179, 272];

    /// A tab's label in the lobby's text case.
    pub const fn label(tab: Tab) -> &'static str {
        match tab {
            Tab::Multiplayer => "Multiplayer",
            Tab::CustomGames => "Custom Games",
            Tab::Forge => "Forge",
            Tab::Firefight => "Firefight",
            Tab::CoOp => "Co-op",
        }
    }

    /// The width of a tab's cell, cut for [`label`] in font 1.
    pub const fn width(tab: Tab) -> i16 {
        match tab {
            Tab::Multiplayer => 70,
            Tab::CustomGames => 80,
            Tab::Forge => 36,
            Tab::Firefight => 58,
            Tab::CoOp => 36,
        }
    }

    /// Each present tab's cell, left-aligned at the band's left: a tab
    /// absent closes the run up.
    pub fn cells(tabs: &[Tab]) -> Vec<(Tab, [i16; 4])> {
        let mut left = BAND_LEFT;
        tabs.iter()
            .map(|&t| {
                let cell = (t, [TOP, left, BOTTOM, left + width(t)]);
                left += width(t) + GAP;
                cell
            })
            .collect()
    }

    /// The strip's widgets for `l`.
    pub fn widgets(l: &Lobby) -> Vec<Widget> {
        let row = l.row();
        let mut out = Vec::new();
        for (tab, cell) in cells(&l.present.tabs()) {
            let avail = row
                .iter()
                .find(|(t, _)| *t == tab)
                .map_or(Avail::On, |(_, a)| *a);
            let look = look(tab, l.selected, avail);
            out.push(text(&tab_id(tab), tab_component(look), cell, label(tab)));
        }
        if l.leader {
            for (dir, id, rect) in [
                (Dir::Prev, "glyph.prev", PREV),
                (Dir::Next, "glyph.next", NEXT),
            ] {
                let on = glyph_on(&row, l.selected, dir);
                let glyph = dir.glyph().char().to_string();
                out.push(text(id, glyph_component(on), rect, &glyph));
            }
        }
        out.push(count(COUNT));
        out
    }
}

// ---- the pages ----

/// Design `name`'s widgets for `l`; `None` for a name that is no design.
pub fn widgets(name: &str, l: &Lobby) -> Option<Vec<Widget>> {
    Some(match name {
        "design.lobby-tabs.a" => a::widgets(l),
        "design.lobby-tabs.b" => b::widgets(l),
        "design.lobby-tabs.c" => c::widgets(l),
        _ => return None,
    })
}

/// The state whose texts are widest in design `name`: A's widest window
/// (MULTIPLAYER, CUSTOM GAMES, FORGE), B's widest legend (CUSTOM GAMES
/// and FIREFIGHT, from FORGE); C always shows every label.
fn longest(name: &str) -> Tab {
    match name {
        "design.lobby-tabs.a" => Tab::CustomGames,
        _ => Tab::Forge,
    }
}

/// The lobbies a design is drawn for, by state name: each tab selected in
/// turn in a Live lobby where all are on; the widest; a Split Screen
/// lobby, where MULTIPLAYER is off; a System Link member's, with no
/// bumpers; and a run with only retail's three modes present.
pub fn lobbies(name: &str) -> Vec<(&'static str, Lobby)> {
    let split = Lobby {
        place: Place::SplitScreen,
        ..Lobby::live(Tab::CustomGames)
    };
    vec![
        ("multiplayer", Lobby::live(Tab::Multiplayer)),
        ("custom-games", Lobby::live(Tab::CustomGames)),
        ("forge", Lobby::live(Tab::Forge)),
        ("firefight", Lobby::live(Tab::Firefight)),
        ("co-op", Lobby::live(Tab::CoOp)),
        ("longest", Lobby::live(longest(name))),
        ("unavailable", split),
        (
            "member",
            Lobby {
                place: Place::SystemLink,
                leader: false,
                ..split
            },
        ),
        (
            "retail-tabs",
            Lobby {
                present: Present::default(),
                ..Lobby::live(Tab::CustomGames)
            },
        ),
    ]
}

/// Page `name`'s named states, each the strip over the game shell;
/// `None` for a name that is no design.
pub fn states(name: &str) -> Option<Vec<(String, ScreenSpec)>> {
    if !PAGES.contains(&name) {
        return None;
    }
    lobbies(name)
        .into_iter()
        .map(|(state, l)| {
            let mut spec = Screen::hud(name, l.selected.label(), widgets(name, &l)?).build();
            // Over the shell, not a live picture: the band is the point.
            spec.transparent = false;
            Some((state.to_string(), spec))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::input::Glyph;
    use crate::ui_kit::preview::overflows;
    use crate::ui_kit::view::Synthetic;

    /// Every lobby a design can meet: each set of tabs present, each
    /// place, a kit map installed or not, a member elsewhere or not, the
    /// leader's console and a member's, on each tab present.
    fn every_lobby() -> Vec<Lobby> {
        let mut out = Vec::new();
        for bits in 0..4u8 {
            let present = Present {
                forge: bits & 1 != 0,
                firefight: bits & 2 != 0,
            };
            for place in [Place::SplitScreen, Place::SystemLink, Place::XboxLive] {
                for facts_bits in 0..8u8 {
                    let facts = Facts {
                        playlists: facts_bits & 1 != 0,
                        live_co_op: facts_bits & 2 != 0,
                        remote_members: facts_bits & 4 != 0,
                    };
                    for leader in [true, false] {
                        for selected in present.tabs() {
                            out.push(Lobby {
                                present,
                                place,
                                facts,
                                leader,
                                selected,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    fn on_band(id: &str) -> bool {
        id.starts_with("tab.") || id.starts_with("prompt.")
    }

    fn overlap(a: [i16; 4], b: [i16; 4]) -> bool {
        a[1] < b[3] && b[1] < a[3] && a[2] < b[0] && b[2] < a[0]
    }

    /// The layout check: for every tab count and every lobby, each text
    /// of each design fits its rectangle, no two rectangles meet, the
    /// band's texts stay between the title-safe edge and the cut corner
    /// inside the header's rows, and the rest stay in the title-safe
    /// area.
    #[test]
    fn every_design_fits_the_band_for_every_lobby() {
        let safe = crate::ui_kit::layout::SAFE;
        for name in PAGES {
            for l in every_lobby() {
                let ws = widgets(name, &l).unwrap();
                for (i, w) in ws.iter().enumerate() {
                    let [top, left, bottom, right] = w.rect;
                    assert!(left < right && bottom < top, "{name} {l:?}: {}", w.id);
                    assert!(!overflows(w, &Synthetic), "{name} {l:?}: {} overflows", w.id);
                    assert!(
                        top <= safe.top
                            && bottom >= safe.bottom
                            && left >= safe.left
                            && right <= safe.right,
                        "{name} {l:?}: {} is outside the title-safe area",
                        w.id
                    );
                    if on_band(&w.id) {
                        assert!(
                            left >= BAND_LEFT && right <= BAND_RIGHT,
                            "{name} {l:?}: {} is off the band: {:?}",
                            w.id,
                            w.rect
                        );
                        assert!(top <= HEADER_TOP && bottom >= HEADER_BOTTOM, "{name}: {}", w.id);
                    }
                    for o in &ws[i + 1..] {
                        assert!(
                            !overlap(w.rect, o.rect),
                            "{name} {l:?}: {} and {} meet",
                            w.id,
                            o.id
                        );
                    }
                }
            }
        }
    }

    /// The bumpers are White on the left and Black on the right: U+E105
    /// and U+E104, which a modern pad's set draws as LB and RB.
    #[test]
    fn the_bumpers_are_white_then_black() {
        assert_eq!(Dir::Prev.glyph(), Glyph::White);
        assert_eq!(Dir::Next.glyph(), Glyph::Black);
        assert_eq!(Glyph::White.char(), '\u{e105}');
        assert_eq!(Glyph::Black.char(), '\u{e104}');
        for name in PAGES {
            for (state, l) in lobbies(name) {
                let ws = widgets(name, &l).unwrap();
                let glyphs = |white: bool| {
                    let want = (if white { Glyph::White } else { Glyph::Black }).char();
                    ws.iter()
                        .filter(|w| w.text().is_some_and(|t| t.contains(want)))
                        .map(|w| w.id.clone())
                        .collect::<Vec<_>>()
                };
                let (prev, next) = (glyphs(true), glyphs(false));
                assert!(prev.iter().all(|id| id.ends_with(".prev")), "{name}/{state}: {prev:?}");
                assert!(next.iter().all(|id| id.ends_with(".next")), "{name}/{state}: {next:?}");
                if !l.leader {
                    assert!(prev.is_empty() && next.is_empty(), "{name}/{state}: a member has no bumpers");
                }
                // The left bumper is drawn left of the right one.
                if let (Some(p), Some(n)) = (
                    ws.iter().find(|w| w.id.ends_with(".prev")),
                    ws.iter().find(|w| w.id.ends_with(".next")),
                ) {
                    assert!(p.rect[3] <= n.rect[1], "{name}/{state}");
                }
            }
        }
    }

    /// A and C draw both bumpers for the leader in every lobby; B draws
    /// one only where it has somewhere to name.
    #[test]
    fn the_leader_always_has_the_bumpers_in_a_and_c() {
        for l in every_lobby().into_iter().filter(|l| l.leader) {
            for (name, ids) in [
                ("design.lobby-tabs.a", ["prompt.prev", "prompt.next"]),
                ("design.lobby-tabs.c", ["glyph.prev", "glyph.next"]),
            ] {
                let ws = widgets(name, &l).unwrap();
                for id in ids {
                    assert!(ws.iter().any(|w| w.id == id), "{name} {l:?}: no {id}");
                }
            }
        }
    }

    #[test]
    fn exactly_one_tab_is_drawn_selected() {
        let (font, colour) = crate::ui_kit::data::design()
            .text_style("tab.selected")
            .unwrap();
        let white = crate::ui_kit::components::rgb(colour);
        for name in ["design.lobby-tabs.a", "design.lobby-tabs.c"] {
            for l in every_lobby() {
                let ws = widgets(name, &l).unwrap();
                let selected: Vec<&str> = ws
                    .iter()
                    .filter(|w| w.id.starts_with("tab."))
                    .filter(|w| {
                        matches!(&w.kind, Kind::Text { colour, font: f, .. } if *colour == white && *f == font)
                    })
                    .map(|w| w.id.as_str())
                    .collect();
                assert_eq!(selected, [tab_id(l.selected)], "{name} {l:?}");
            }
        }
        for l in every_lobby() {
            let ws = b::widgets(&l);
            assert_eq!(ws[0].id, "tab.selected");
            assert_eq!(ws[0].text(), Some(l.selected.label()));
        }
    }

    #[test]
    fn a_shows_the_selected_tab_and_at_most_three() {
        for l in every_lobby() {
            let tabs = l.present.tabs();
            let shown = a::window(&tabs, l.selected);
            assert!(shown.contains(&l.selected), "{l:?}");
            assert_eq!(shown.len(), tabs.len().min(a::WINDOW));
            // In the strip's order, with no tab skipped.
            let at = tabs.iter().position(|t| *t == shown[0]).unwrap();
            assert_eq!(shown, &tabs[at..at + shown.len()]);
        }
        let all = Tab::ALL;
        assert_eq!(a::window(&all, Tab::Multiplayer), &all[..3]);
        assert_eq!(a::window(&all, Tab::Forge), &all[1..4]);
        assert_eq!(a::window(&all, Tab::CoOp), &all[2..]);
    }

    /// The widest window a lobby can have keeps a gap; the usual ones
    /// have the full one.
    #[test]
    fn a_keeps_a_gap_between_the_widest_three() {
        assert_eq!(a::gap(&Tab::ALL[..3]), a::GAP);
        assert_eq!(a::gap(&Tab::ALL[1..4]), a::GAP);
        let widest = [Tab::Multiplayer, Tab::CustomGames, Tab::Firefight];
        assert!(a::gap(&widest) >= 4, "{}", a::gap(&widest));
        assert_eq!(a::gap(&[Tab::Forge]), a::GAP);
        for l in every_lobby() {
            let cells = a::cells(&l.present.tabs(), l.selected);
            let (first, last) = (cells[0].1, cells[cells.len() - 1].1);
            assert!(first[1] >= a::RUN_LEFT && last[3] <= a::RUN_RIGHT, "{l:?}");
            // Centred between the bumpers, to the unit.
            let (before, after) = (first[1] - a::RUN_LEFT, a::RUN_RIGHT - last[3]);
            assert!((before - after).abs() <= 1, "{l:?}: {before} and {after}");
        }
    }

    #[test]
    fn b_names_the_tab_each_bumper_moves_to() {
        let live = Lobby::live(Tab::Forge);
        let p = b::prompts(&live);
        assert_eq!(
            p.iter().map(|p| (p.dir, p.tab, p.on)).collect::<Vec<_>>(),
            [
                (Dir::Prev, Tab::CustomGames, true),
                (Dir::Next, Tab::Firefight, true)
            ]
        );
        assert_eq!(b::prompt(Dir::Prev, Tab::CustomGames), "\u{e105} CUSTOM GAMES");
        assert_eq!(b::prompt(Dir::Next, Tab::Firefight), "\u{e104} FIREFIGHT");
        // The ends have one prompt, at the band's right end.
        let first = b::prompts(&Lobby::live(Tab::Multiplayer));
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].dir, first[0].rect[3]), (Dir::Next, BAND_RIGHT));
        let last = b::prompts(&Lobby::live(Tab::CoOp));
        assert_eq!(last.len(), 1);
        assert_eq!((last[0].dir, last[0].rect[3]), (Dir::Prev, BAND_RIGHT));
        // Split Screen: MULTIPLAYER is off, so White from CUSTOM GAMES
        // names it without being bright.
        let split = Lobby {
            place: Place::SplitScreen,
            ..Lobby::live(Tab::CustomGames)
        };
        let p = b::prompts(&split);
        assert_eq!((p[0].dir, p[0].tab, p[0].on), (Dir::Prev, Tab::Multiplayer, false));
        // A bright prompt names where `step` goes, past a tab that is off.
        for l in every_lobby().into_iter().filter(|l| l.leader) {
            let row = l.row();
            for p in b::prompts(&l) {
                match step(&row, l.selected, p.dir) {
                    Step::Move { to, .. } => assert_eq!((p.tab, p.on), (to, true), "{l:?}"),
                    Step::Stay(_) => assert!(!p.on, "{l:?}"),
                }
            }
        }
    }

    #[test]
    fn each_page_has_every_tab_selected_in_turn_and_the_longest() {
        for name in PAGES {
            let states = states(name).unwrap();
            let names: Vec<&str> = states.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(
                names,
                [
                    "multiplayer",
                    "custom-games",
                    "forge",
                    "firefight",
                    "co-op",
                    "longest",
                    "unavailable",
                    "member",
                    "retail-tabs"
                ]
            );
            for (tab, (state, spec)) in Tab::ALL.iter().zip(&states) {
                assert_eq!(spec.title, tab.label(), "{name}/{state}");
                assert!(!spec.transparent, "{name}/{state} is drawn over the shell");
            }
        }
        assert!(states("design.lobby-tabs.d").is_none());
    }
}
