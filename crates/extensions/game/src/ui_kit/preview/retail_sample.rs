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
//! the screen tag's path. [`State`] answers the renderer from it.
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
        Ok(RetailSample {
            screen: d.screen,
            tokens,
            footer: d.footer,
            cells: d
                .cells
                .into_iter()
                .map(|c| Cell {
                    text: c.text,
                    blocks: keyed(c.blocks),
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
    ALL.get_or_init(|| {
        crate::ui_kit::data::baked::RETAIL_SAMPLES
            .iter()
            .map(|(name, text)| {
                RetailSample::parse(text)
                    .map_err(|e| format!("assets/ui/retail-samples/{name}.yaml: {e}"))
            })
            .collect()
    })
    .as_ref()
    .map(Vec::as_slice)
    .map_err(Clone::clone)
}

/// Screen tag `tag`'s sample, if it has one.
pub fn for_screen(tag: &str) -> Result<Option<&'static RetailSample>, String> {
    let want = key(tag);
    Ok(all()?.iter().find(|s| key(&s.screen) == want))
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
pub struct Shown {
    /// The image to draw in place of the block's own.
    pub image: Option<i16>,
    pub tint: Option<[u8; 3]>,
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
    pub fn bitmap(&self, widget: &str, tag: Option<&str>) -> Option<Shown> {
        match self.fill(widget, tag) {
            Some(f) if f.hide => None,
            Some(f) => Some(Shown {
                image: f.image,
                tint: f.tint,
            }),
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
pub fn cell_bitmap(cell: &Cell, widget: &str, tag: Option<&str>) -> Option<Shown> {
    let f = cell
        .blocks
        .get(&key(widget))
        .or_else(|| cell.blocks.get(&key(tag?)));
    match f {
        Some(f) if f.hide => None,
        Some(f) => Some(Shown {
            image: f.image,
            tint: f.tint,
        }),
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
                tint: None
            })
        );
        assert_eq!(st.bitmap("bitmap.6", Some(r"ui\art\frame")), Some(Shown::default()));
        assert_eq!(st.bitmap("bitmap.7", None), Some(Shown::default()));
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
                image: None,
                tint: Some([200, 40, 40])
            })
        );
        assert_eq!(
            cell_bitmap(&cells[0], "bitmap.3", Some(r"ui\art\rank")).unwrap().image,
            Some(12)
        );
        assert_eq!(cell_bitmap(&cells[1], "bitmap.3", Some(r"ui\art\rank")), None);
        assert_eq!(cell_bitmap(&cells[1], "bitmap.0", None), Some(Shown::default()));
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
    fn a_sample_with_a_wrong_key_does_not_read() {
        assert!(RetailSample::parse("screen: x\ntokens:\n  e100: A\n").is_err());
        assert!(RetailSample::parse("screen: x\ntokens:\n  zz: A\n").is_err());
        assert!(RetailSample::parse("screen: x\nplayers: []\n").is_err());
        assert!(
            RetailSample::parse("screen: x\nfooter: { string: a, text: b }\n").is_err(),
            "one or the other"
        );
    }

    /// Every sample in the checkout reads, and names a screen once.
    #[test]
    fn the_baked_samples_read() {
        let all = all().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for s in all {
            assert!(seen.insert(key(&s.screen)), "{} has two samples", s.screen);
            assert_eq!(for_screen(&s.screen.replace('\\', "/")).unwrap(), Some(s));
        }
        assert_eq!(for_screen(r"ui\no\such\screen").unwrap(), None);
    }
}
