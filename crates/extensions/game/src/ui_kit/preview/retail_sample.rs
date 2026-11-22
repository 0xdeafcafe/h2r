//! What the game's code fills in on a retail screen, as data
//! (docs/game/ui-builder.md "Retail screens in a believable state").
//!
//! A screen tag lays out every block the game might show: the text for
//! the party leader over the buttons everyone else lacks, a status line
//! with no text of its own, sixteen speaker icons, a list with no rows.
//! The game's code picks among them, writes the empty ones and replaces
//! the substitution units in the strings (U+E408 to U+E45E, the table at
//! `0x470200` walked by `0x22d2ee`). A [`RetailSample`] stands in for that
//! code: one state of one screen, in `assets/ui/retail-samples/`, keyed by
//! the screen tag's path and, for a screen's other states, a variant's
//! name. [`State`] answers the renderer from it.
//!
//! Without a sample, a string that holds a substitution unit is not
//! drawn: the unit has no glyph, and what the game would write there is
//! not in the tag.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::sync::OnceLock;

use serde::Deserialize;

/// The units the game replaces before it measures and draws a string:
/// the entries of the table at `0x470200` that have a handler.
pub const TOKENS: RangeInclusive<char> = '\u{e408}'..='\u{e45e}';

/// What the game does to one block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    /// The game does not show the block in this state.
    #[serde(default)]
    pub hide: bool,
    /// The game writes another string of the screen's own table here, by
    /// its name. Read from the disc when drawn; never copied here.
    #[serde(default)]
    pub string: Option<String>,
    /// The game writes text of its own making here (a name, a count).
    #[serde(default)]
    pub text: Option<String>,
    /// The image of the block's bitmap the game picks.
    #[serde(default)]
    pub image: Option<i16>,
    /// The colour the game draws the bitmap in (a player's, a team's).
    #[serde(default)]
    pub tint: Option<[u8; 3]>,
    /// The game puts a map's picture here in place of the block's
    /// bitmap: the map file's name without `.map`. The picture is read
    /// from the disc's level list when drawn.
    #[serde(default)]
    pub level: Option<String>,
}

/// A player's emblem: an image of each of the disc's two emblem bitmaps
/// and the four colours they are drawn in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emblem {
    /// The image of the foreground bitmap.
    pub foreground: i16,
    /// The image of the background bitmap.
    pub background: i16,
    /// The foreground's two colours, then the background's two.
    pub colours: [[u8; 3]; 4],
}

/// One filled cell of the screen's tables (a player in a lobby).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    /// The cell's first text block.
    pub text: String,
    /// The cell skin's bitmaps, by `bitmap.N` or by bitmap tag path.
    #[serde(default)]
    pub blocks: BTreeMap<String, Fill>,
    /// The emblem the cell skin's shader bitmap shows; none draws none.
    #[serde(default)]
    pub emblem: Option<Emblem>,
}

/// The rows the game's code gives a list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListSample {
    pub rows: Vec<Fill>,
    #[serde(default)]
    pub focus: usize,
}

/// One pane's blocks and lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSample {
    /// By widget id (`text.4`, `button.0`, `bitmap.21`, `title`), by the
    /// name of the block's string, or by its bitmap's tag path. A widget
    /// id wins over a name.
    #[serde(default)]
    pub blocks: BTreeMap<String, Fill>,
    /// By the list's index in the pane.
    #[serde(default)]
    pub lists: BTreeMap<usize, ListSample>,
}

/// The file as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    screen: String,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    over: Option<String>,
    #[serde(default)]
    tokens: BTreeMap<String, String>,
    #[serde(default)]
    footer: Option<Fill>,
    #[serde(default)]
    cells: Vec<Cell>,
    #[serde(default)]
    panes: BTreeMap<usize, PaneSample>,
}

/// One state of one retail screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetailSample {
    /// The screen tag's path.
    pub screen: String,
    /// The state's name among the screen's samples; none is the screen's
    /// plain sample, the one every caller gets unless it names another.
    pub variant: Option<String>,
    /// The screen this one opens over (a dialog's opener), drawn under
    /// it in its own sample state.
    pub over: Option<String>,
    /// What each substitution unit reads as.
    pub tokens: BTreeMap<char, String>,
    /// The button legend in the footer.
    pub footer: Option<Fill>,
    /// The filled cells of the screen's tables, first cell first.
    pub cells: Vec<Cell>,
    pub panes: BTreeMap<usize, PaneSample>,
}

/// A block key or a tag path as the lookups compare them.
fn key(s: &str) -> String {
    s.trim().replace('/', "\\").to_ascii_lowercase()
}

fn keyed(blocks: BTreeMap<String, Fill>) -> BTreeMap<String, Fill> {
    blocks.into_iter().map(|(k, f)| (key(&k), f)).collect()
}

impl RetailSample {
    /// A sample from its YAML, or what is wrong with it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let d: Doc = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
        let mut tokens = BTreeMap::new();
        for (k, v) in d.tokens {
            let hex = k.trim().trim_start_matches("U+").trim_start_matches("u+");
            let c = u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .filter(|c| TOKENS.contains(c))
                .ok_or_else(|| {
                    format!("tokens: {k} is not a substitution unit (e408 to e45e, in hex)")
                })?;
            tokens.insert(c, v);
        }
        let fills = d
            .footer
            .iter()
            .chain(d.panes.values().flat_map(|p| p.blocks.values()))
            .chain(d.panes.values().flat_map(|p| p.lists.values()).flat_map(|l| l.rows.iter()));
        for f in fills {
            if f.string.is_some() && f.text.is_some() {
                return Err("a block has both string and text".to_owned());
            }
        }
        if d.variant.as_deref().is_some_and(|v| v.trim().is_empty()) {
            return Err("variant: an empty name".to_owned());
        }
        Ok(RetailSample {
            screen: d.screen,
            variant: d.variant,
            over: d.over,
            tokens,
            footer: d.footer,
            cells: d
                .cells
                .into_iter()
                .map(|c| Cell {
                    text: c.text,
                    blocks: keyed(c.blocks),
                    emblem: c.emblem,
                })
                .collect(),
            panes: d
                .panes
                .into_iter()
                .map(|(i, p)| {
                    (
                        i,
                        PaneSample {
                            blocks: keyed(p.blocks),
                            lists: p.lists,
                        },
                    )
                })
                .collect(),
        })
    }
}

/// The samples of `assets/ui/retail-samples/`, baked into the build
/// (`build.rs`, `ui_docs`), or the first file that does not read.
pub fn all() -> Result<&'static [RetailSample], String> {
    static ALL: OnceLock<Result<Vec<RetailSample>, String>> = OnceLock::new();
    ALL.get_or_init(|| parse_all(crate::ui_kit::data::baked::RETAIL_SAMPLES))
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

/// The sample files `files` (name, text), or the first that does not
/// read: a file that fails to parse, or a second sample of a screen and
/// variant another file has.
fn parse_all(files: &[(&str, &str)]) -> Result<Vec<RetailSample>, String> {
    let mut out: Vec<RetailSample> = Vec::new();
    for (name, text) in files {
        let at = format!("assets/ui/retail-samples/{name}.yaml");
        let s = RetailSample::parse(text).map_err(|e| format!("{at}: {e}"))?;
        let variant = s.variant.as_deref().map(key);
        if out
            .iter()
            .any(|o| key(&o.screen) == key(&s.screen) && o.variant.as_deref().map(key) == variant)
        {
            return Err(match &s.variant {
                Some(v) => format!("{at}: {} has two samples named {v}", s.screen),
                None => format!("{at}: {} has two plain samples", s.screen),
            });
        }
        out.push(s);
    }
    Ok(out)
}

/// Screen tag `tag`'s plain sample (no variant), if it has one.
pub fn for_screen(tag: &str) -> Result<Option<&'static RetailSample>, String> {
    let want = key(tag);
    Ok(all()?.iter().find(|s| key(&s.screen) == want && s.variant.is_none()))
}

/// Screen tag `tag`'s sample named `variant`, if it has one.
pub fn for_screen_variant(
    tag: &str,
    variant: &str,
) -> Result<Option<&'static RetailSample>, String> {
    let (want, name) = (key(tag), key(variant));
    Ok(all()?.iter().find(|s| {
        key(&s.screen) == want && s.variant.as_deref().is_some_and(|v| key(v) == name)
    }))
}

/// The names of screen tag `tag`'s variants, sorted.
pub fn variants(tag: &str) -> Result<Vec<&'static str>, String> {
    let want = key(tag);
    let mut v: Vec<&'static str> = all()?
        .iter()
        .filter(|s| key(&s.screen) == want)
        .filter_map(|s| s.variant.as_deref())
        .collect();
    v.sort_unstable();
    Ok(v)
}

/// `s` with each substitution unit replaced from `tokens`, or `None` when
/// it holds one `tokens` has no text for: the game would write something
/// there that nobody has said.
pub fn substitute(s: &str, tokens: &BTreeMap<char, String>) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if TOKENS.contains(&c) {
            out.push_str(tokens.get(&c)?);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The screen's own string table, as the renderer reads it off the disc.
pub struct Strings<'a> {
    /// A string id's text.
    pub text: &'a dyn Fn(u32) -> Option<String>,
    /// A string id's name.
    pub name: &'a dyn Fn(u32) -> Option<String>,
    /// The text of the string named so.
    pub named: &'a dyn Fn(&str) -> Option<String>,
}

/// How a bitmap block is drawn in a state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown<'a> {
    /// The image to draw in place of the block's own.
    pub image: Option<i16>,
    pub tint: Option<[u8; 3]>,
    /// The map whose picture is drawn in place of the block's bitmap.
    pub level: Option<&'a str>,
}

impl<'a> Shown<'a> {
    fn of(f: &'a Fill) -> Self {
        Shown {
            image: f.image,
            tint: f.tint,
            level: f.level.as_deref(),
        }
    }
}

/// One pane of a screen in a state: the tag's strings and the sample's
/// answers, or the fallback's with no sample.
pub struct State<'a> {
    pub sample: Option<&'a RetailSample>,
    pub pane: usize,
    pub strings: Strings<'a>,
}

static NO_TOKENS: BTreeMap<char, String> = BTreeMap::new();

impl State<'_> {
    fn tokens(&self) -> &BTreeMap<char, String> {
        self.sample.map_or(&NO_TOKENS, |s| &s.tokens)
    }

    fn fill(&self, widget: &str, name: Option<&str>) -> Option<&Fill> {
        let blocks = &self.sample?.panes.get(&self.pane)?.blocks;
        blocks
            .get(&key(widget))
            .or_else(|| blocks.get(&key(name?)))
    }

    /// A [`Fill`]'s text: its own, or the named string's with its units
    /// replaced. `None` when it writes none.
    fn written(&self, f: &Fill) -> Option<Option<String>> {
        if let Some(t) = &f.text {
            return Some(Some(t.clone()));
        }
        let name = f.string.as_deref()?;
        Some((self.strings.named)(name).and_then(|s| substitute(&s, self.tokens())))
    }

    /// The text of widget `widget` (a text block, a button or the title)
    /// whose string is `string`: what the sample writes there, else the
    /// tag's own string with its units replaced. `None` is a block the
    /// state does not show.
    pub fn text(&self, widget: &str, string: u32) -> Option<String> {
        let name = (self.strings.name)(string);
        if let Some(f) = self.fill(widget, name.as_deref()) {
            if f.hide {
                return None;
            }
            if let Some(t) = self.written(f) {
                return t;
            }
        }
        substitute(&(self.strings.text)(string)?, self.tokens())
    }

    /// Whether the state hides widget `widget`, whose string is `string`:
    /// a button's art goes with its text.
    pub fn hidden(&self, widget: &str, string: u32) -> bool {
        let name = (self.strings.name)(string);
        self.fill(widget, name.as_deref()).is_some_and(|f| f.hide)
    }

    /// How bitmap `widget` (`bitmap.N`), whose bitmap tag is `tag`, is
    /// drawn; `None` when the state hides it.
    pub fn bitmap(&self, widget: &str, tag: Option<&str>) -> Option<Shown<'_>> {
        match self.fill(widget, tag) {
            Some(f) if f.hide => None,
            Some(f) => Some(Shown::of(f)),
            None => Some(Shown::default()),
        }
    }

    /// List `list`'s rows and its focused row, if the sample gives it
    /// any. A row the state hides, or whose string the table lacks, is
    /// left out.
    pub fn rows(&self, list: usize) -> Option<(Vec<String>, usize)> {
        let l = self.sample?.panes.get(&self.pane)?.lists.get(&list)?;
        let rows: Vec<String> = l
            .rows
            .iter()
            .filter(|f| !f.hide)
            .filter_map(|f| self.written(f).flatten())
            .collect();
        (!rows.is_empty()).then(|| {
            let focus = l.focus.min(rows.len() - 1);
            (rows, focus)
        })
    }

    /// The footer's legend.
    pub fn footer(&self) -> Option<String> {
        let f = self.sample?.footer.as_ref().filter(|f| !f.hide)?;
        self.written(f).flatten()
    }

    /// The tables' filled cells.
    pub fn cells(&self) -> &[Cell] {
        self.sample.map_or(&[][..], |s| s.cells.as_slice())
    }

    /// The strings the sample names that the screen's table lacks: a
    /// sample written for another screen, or a misspelt name.
    pub fn missing(&self) -> Vec<String> {
        let Some(s) = self.sample else {
            return Vec::new();
        };
        let pane = s.panes.get(&self.pane);
        s.footer
            .iter()
            .chain(pane.into_iter().flat_map(|p| p.blocks.values()))
            .chain(pane.into_iter().flat_map(|p| p.lists.values()).flat_map(|l| l.rows.iter()))
            .filter_map(|f| f.string.as_deref())
            .filter(|n| (self.strings.named)(n).is_none())
            .map(str::to_owned)
            .collect()
    }
}

/// How a cell's skin bitmap is drawn: [`State::bitmap`] for a [`Cell`].
pub fn cell_bitmap<'a>(cell: &'a Cell, widget: &str, tag: Option<&str>) -> Option<Shown<'a>> {
    let f = cell
        .blocks
        .get(&key(widget))
        .or_else(|| cell.blocks.get(&key(tag?)));
    match f {
        Some(f) if f.hide => None,
        Some(f) => Some(Shown::of(f)),
        None => Some(Shown::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOBBY: &str = r"
screen: ui\screens\lobby
tokens:
  e416: Kestrel
  e425: '4'
footer: { string: key_leader }
cells:
  - text: Kestrel
    blocks:
      ui\bars\row: { tint: [200, 40, 40] }
      bitmap.3: { image: 12 }
    emblem:
      foreground: 7
      background: 2
      colours: [[240, 240, 240], [20, 20, 20], [176, 44, 40], [44, 84, 176]]
  - text: Marlow
    blocks:
      ui/art/rank: { hide: true }
panes:
  0:
    blocks:
      party_leader: { hide: true }
      status: { string: waiting_for_you }
      text.4: { text: 'Score to win: 50' }
      UI\Art\Speaker: { hide: true }
      ui\art\icons: { image: 3 }
      ui\art\unknown_map: { level: glasshouse }
    lists:
      0:
        focus: 1
        rows:
          - { string: change_map }
          - { text: Change Rules }
          - { string: change_map, hide: true }
";

    /// A screen's table: (id, name, text). U+E416 is the leader, U+E425
    /// the player count, U+E408 the map.
    const TABLE: &[(u32, &str, &str)] = &[
        (1, "party_leader", "\u{e416} leads"),
        (2, "player_count", "\u{e425} here"),
        (3, "status", ""),
        (4, "waiting_for_you", "Ready, \u{e416}"),
        (5, "map", "on \u{e408}"),
        (6, "change_map", "Change Map"),
        (7, "key_leader", "\u{e100} SELECT"),
        (8, "plain", "PREGAME"),
    ];

    fn text(id: u32) -> Option<String> {
        TABLE.iter().find(|t| t.0 == id).map(|t| t.2.to_owned())
    }
    fn name(id: u32) -> Option<String> {
        TABLE.iter().find(|t| t.0 == id).map(|t| t.1.to_owned())
    }
    fn named(n: &str) -> Option<String> {
        TABLE.iter().find(|t| t.1 == n).map(|t| t.2.to_owned())
    }
    fn state(sample: Option<&RetailSample>, pane: usize) -> State<'_> {
        State {
            sample,
            pane,
            strings: Strings {
                text: &text,
                name: &name,
                named: &named,
            },
        }
    }

    #[test]
    fn tokens_are_substituted_from_the_sample() {
        let s = RetailSample::parse(LOBBY).unwrap();
        let st = state(Some(&s), 0);
        assert_eq!(st.text("text.9", 2).as_deref(), Some("4 here"));
        // A string the sample writes in has its units replaced too.
        assert_eq!(st.text("text.0", 3).as_deref(), Some("Ready, Kestrel"));
        // Button glyphs are the font's, not substitutions.
        assert_eq!(st.footer().as_deref(), Some("\u{e100} SELECT"));
        assert_eq!(
            substitute("a \u{e416} b \u{e425}", &s.tokens).as_deref(),
            Some("a Kestrel b 4")
        );
    }

    #[test]
    fn a_unit_the_sample_lacks_hides_the_block() {
        let s = RetailSample::parse(LOBBY).unwrap();
        // U+E408 (the map) is not in the sample.
        assert_eq!(state(Some(&s), 0).text("text.3", 5), None);
    }

    #[test]
    fn runtime_only_blocks_are_hidden_without_a_sample() {
        let st = state(None, 0);
        assert_eq!(st.text("text.43", 1), None, "the leader line");
        assert_eq!(st.text("text.44", 2), None, "the player count");
        assert_eq!(st.text("text.3", 5), None, "the map line");
        // What the tag says outright still shows.
        assert_eq!(st.text("title", 8).as_deref(), Some("PREGAME"));
        assert_eq!(st.text("text.1", 7).as_deref(), Some("\u{e100} SELECT"));
        assert_eq!(st.text("text.2", 99), None, "no such string");
        assert_eq!(st.footer(), None);
        assert!(st.cells().is_empty());
        assert_eq!(st.rows(0), None);
        assert_eq!(st.bitmap("bitmap.0", Some(r"ui\art\speaker")), Some(Shown::default()));
    }

    #[test]
    fn the_sample_hides_writes_and_picks_blocks() {
        let s = RetailSample::parse(LOBBY).unwrap();
        let st = state(Some(&s), 0);
        assert_eq!(st.text("text.43", 1), None, "hidden by its string's name");
        assert_eq!(st.text("text.4", 0).as_deref(), Some("Score to win: 50"));
        assert!(st.hidden("button.0", 1), "a button named for a hidden string");
        assert!(!st.hidden("button.1", 8));
        assert!(!state(None, 0).hidden("button.0", 1));
        // A tag path matches whatever its case and slashes.
        assert_eq!(st.bitmap("bitmap.21", Some(r"ui\art\speaker")), None);
        assert_eq!(
            st.bitmap("bitmap.5", Some(r"ui\art\icons")),
            Some(Shown {
                image: Some(3),
                ..Shown::default()
            })
        );
        assert_eq!(st.bitmap("bitmap.6", Some(r"ui\art\frame")), Some(Shown::default()));
        assert_eq!(st.bitmap("bitmap.7", None), Some(Shown::default()));
        // A map's picture in place of the block's own bitmap.
        assert_eq!(
            st.bitmap("bitmap.8", Some(r"ui\art\unknown_map")).unwrap().level,
            Some("glasshouse")
        );
    }

    #[test]
    fn a_sample_is_one_panes_state() {
        let s = RetailSample::parse(LOBBY).unwrap();
        // Pane 1 has no blocks in the sample: nothing of pane 0's applies.
        let st = state(Some(&s), 1);
        assert_eq!(st.text("text.43", 1).as_deref(), Some("Kestrel leads"));
        assert_eq!(st.text("text.4", 0), None);
        assert_eq!(st.bitmap("bitmap.21", Some(r"ui\art\speaker")), Some(Shown::default()));
        assert_eq!(st.rows(0), None);
    }

    #[test]
    fn a_code_drawn_list_takes_its_rows_from_the_sample() {
        let s = RetailSample::parse(LOBBY).unwrap();
        let (rows, focus) = state(Some(&s), 0).rows(0).unwrap();
        assert_eq!(rows, ["Change Map", "Change Rules"]);
        assert_eq!(focus, 1);
        assert_eq!(state(Some(&s), 0).rows(1), None);
    }

    #[test]
    fn cells_pick_their_skins_bitmaps() {
        let s = RetailSample::parse(LOBBY).unwrap();
        let st = state(Some(&s), 0);
        let cells = st.cells();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].text, "Kestrel");
        assert_eq!(
            cell_bitmap(&cells[0], "bitmap.0", Some(r"ui\bars\row")),
            Some(Shown {
                tint: Some([200, 40, 40]),
                ..Shown::default()
            })
        );
        assert_eq!(
            cell_bitmap(&cells[0], "bitmap.3", Some(r"ui\art\rank")).unwrap().image,
            Some(12)
        );
        assert_eq!(cell_bitmap(&cells[1], "bitmap.3", Some(r"ui\art\rank")), None);
        assert_eq!(cell_bitmap(&cells[1], "bitmap.0", None), Some(Shown::default()));
        let e = cells[0].emblem.as_ref().expect("the first cell has an emblem");
        assert_eq!((e.foreground, e.background), (7, 2));
        assert_eq!(e.colours[2], [176, 44, 40]);
        assert_eq!(cells[1].emblem, None);
    }

    #[test]
    fn a_sample_naming_a_string_the_table_lacks_is_reported() {
        let s = RetailSample::parse(LOBBY).unwrap();
        assert!(state(Some(&s), 0).missing().is_empty());
        let bad = RetailSample::parse(
            "screen: x\npanes:\n  0:\n    blocks:\n      status: { string: no_such }\n",
        )
        .unwrap();
        assert_eq!(state(Some(&bad), 0).missing(), ["no_such"]);
    }

    #[test]
    fn a_dialog_names_the_screen_it_opens_over() {
        let s = RetailSample::parse("screen: a\\dialog\nover: a\\lobby\n").unwrap();
        assert_eq!(s.over.as_deref(), Some(r"a\lobby"));
        assert_eq!(RetailSample::parse(LOBBY).unwrap().over, None);
    }

    #[test]
    fn a_sample_with_a_wrong_key_does_not_read() {
        assert!(RetailSample::parse("screen: x\ntokens:\n  e100: A\n").is_err());
        assert!(RetailSample::parse("screen: x\ntokens:\n  zz: A\n").is_err());
        assert!(RetailSample::parse("screen: x\nplayers: []\n").is_err());
        assert!(
            RetailSample::parse("screen: x\nfooter: { string: a, text: b }\n").is_err(),
            "one or the other"
        );
    }

    /// Every sample in the checkout reads, and names a screen and variant
    /// once.
    #[test]
    fn the_baked_samples_read() {
        let all = all().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for s in all {
            assert!(
                seen.insert((key(&s.screen), s.variant.clone())),
                "{} {:?} has two samples",
                s.screen,
                s.variant
            );
            let tag = s.screen.replace('\\', "/");
            let found = match &s.variant {
                None => for_screen(&tag).unwrap(),
                Some(v) => for_screen_variant(&tag, v).unwrap(),
            };
            assert_eq!(found, Some(s));
        }
        assert_eq!(for_screen(r"ui\no\such\screen").unwrap(), None);
    }

    /// A screen's variant is a sample of its own: the plain lookup never
    /// returns it, the named one does, and an unknown name finds none.
    #[test]
    fn a_variant_is_found_by_its_name_alone() {
        let files = [
            ("a", "screen: ui\\dialog\nfooter: { text: plain }\n"),
            ("a.wide", "screen: ui\\dialog\nvariant: wide\nfooter: { text: wide }\n"),
        ];
        let all = parse_all(&files).unwrap();
        assert_eq!(all[0].variant, None);
        assert_eq!(all[1].variant.as_deref(), Some("wide"));
        // The baked set has Game Setup's Switch To rows as a variant.
        let setup = r"ui\screens\game_shell\pregame_lobby\squad_settings_dialog";
        let plain = for_screen(setup).unwrap().expect("Game Setup has a sample");
        assert_eq!(plain.variant, None);
        let switch = for_screen_variant(setup, "switch-to").unwrap().expect("the variant");
        assert_eq!(switch.variant.as_deref(), Some("switch-to"));
        assert_eq!(switch.panes[&0].lists[&0].rows.len(), 7, "seven rows");
        assert_eq!(plain.panes[&0].lists[&0].rows.len(), 5, "the plain sample's five");
        assert_eq!(variants(setup).unwrap(), ["switch-to"]);
        assert_eq!(for_screen_variant(setup, "no-such").unwrap(), None);
    }

    /// A screen's plain draw is its plain file alone, whatever variants it
    /// has: the sample every caller gets without a name is the parse of
    /// `squad_settings_dialog.yaml`, so its render is byte for byte the
    /// one it was without `squad_settings_dialog.switch-to.yaml`.
    #[test]
    fn the_plain_sample_is_its_file_alone() {
        let setup = r"ui\screens\game_shell\pregame_lobby\squad_settings_dialog";
        let (_, text) = crate::ui_kit::data::baked::RETAIL_SAMPLES
            .iter()
            .find(|(n, _)| *n == "squad_settings_dialog")
            .expect("the plain file is baked");
        let file = RetailSample::parse(text).unwrap();
        assert_eq!(for_screen(setup).unwrap(), Some(&file));
        let without: Vec<(&str, &str)> = crate::ui_kit::data::baked::RETAIL_SAMPLES
            .iter()
            .copied()
            .filter(|(n, _)| !n.contains('.'))
            .collect();
        let alone = parse_all(&without).unwrap();
        assert_eq!(alone.iter().find(|s| key(&s.screen) == key(setup)), Some(&file));
    }

    /// Two files for one screen and variant do not read, nor two plain
    /// ones; one of each does.
    #[test]
    fn a_second_sample_of_a_screen_and_variant_fails() {
        let plain = "screen: ui\\dialog\n";
        let wide = "screen: ui\\dialog\nvariant: wide\n";
        assert!(parse_all(&[("a", plain), ("b", wide)]).is_ok());
        let e = parse_all(&[("a", wide), ("b", wide)]).unwrap_err();
        assert!(e.contains("b.yaml") && e.contains("wide"), "{e}");
        assert!(parse_all(&[("a", plain), ("b", "screen: UI/Dialog\n")]).is_err());
        assert!(RetailSample::parse("screen: x\nvariant: ''\n").is_err());
    }
}
