//! Retail screen tag edits as data (docs/game/ui-in-maps.md, "Retail
//! edits as data").
//!
//! A change to a retail screen (a block moved, hidden or copied, a string
//! ID named) is ours to ship, and the screen's `wgit` is the user's. So the
//! change is kept as a list of edits in a YAML file under
//! `assets/ui/retail-edits/` ([`DIR`]), and [`apply`] makes them in the
//! user's map at boot, in the same `vfs::Rewrite` that adds our screens
//! (`inject::ScreensRewrite`). A file names the screen tag, the map, one
//! pane and the edits in order:
//!
//! ```yaml
//! screen: 'ui\screens\game_shell\pregame_lobby\pregame_lobby'
//! map: mainmenu
//! pane: 0
//! edits:
//!   - op: duplicate
//!     block: text.10
//!     rect: [95, -260, 75, 70]
//!     as: text.48
//!   - op: set_string
//!     block: text.48
//!     name: lobby_mode_change
//! ```
//!
//! Blocks are named as `h2 ui` and the editor name them ([`BlockName`]:
//! `text.10`, `button.1`, `bitmap.3`, `title`). A rect is `[top, left,
//! bottom, right]` in UI units, Y up. Nothing of the disc is in a file:
//! numbers we chose and names of our own.
//!
//! [`diff::diff_pane`] goes the other way, from an edited pane to the
//! edits that make it: the editor's `ui.export_edits`.

use std::fmt;
use std::str::FromStr;

use content::{Hasher, Sha256};
use serde::{Deserialize, Serialize};

pub mod apply;
pub mod diff;

#[cfg(test)]
mod tests;

pub use apply::{Applied, Edited, apply_all};
pub use diff::{Diff, PaneBytes, diff_pane};

/// Where the files are, from the checkout's root.
pub const DIR: &str = "assets/ui/retail-edits";

/// Where a hidden block is parked: off screen, where the disc keeps its
/// own spare blocks and neither the game nor the editor's picture draws
/// one (`preview::retail_edit::PARKED` on the editor's side).
pub const PARKED: i16 = 9999;

/// The map a file edits when it names none.
pub const DEFAULT_MAP: &str = "mainmenu";

/// The longest string name a file may give: the map's fixed string slot
/// (`map::build::FIXED_STRING_SLOT`) less its NUL.
pub const MAX_NAME: usize = map::build::FIXED_STRING_SLOT - 1;

/// `[top, left, bottom, right]` in UI units, Y up.
pub type Rect = [i16; 4];

/// The kinds of pane block an edit reaches (docs/formats/ui.md, "Pane").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlockKind {
    Text,
    Button,
    Bitmap,
}

/// Where a block keeps its place: a whole rect, or a top-left point (a
/// bitmap, whose size is its image's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Rect(usize),
    Point(usize),
}

impl BlockKind {
    pub const ALL: [BlockKind; 3] = [BlockKind::Text, BlockKind::Button, BlockKind::Bitmap];

    /// The name's first part (`text` of `text.10`).
    pub fn name(self) -> &'static str {
        match self {
            BlockKind::Text => "text",
            BlockKind::Button => "button",
            BlockKind::Bitmap => "bitmap",
        }
    }

    fn named(s: &str) -> Option<Self> {
        BlockKind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// The pane's block of this kind, as the tag editor names it.
    pub fn field_name(self) -> &'static str {
        match self {
            BlockKind::Text => "text_blocks",
            BlockKind::Button => "buttons",
            BlockKind::Bitmap => "bitmaps",
        }
    }

    /// The pane field holding the block's (count, address).
    pub fn pane_field(self) -> u32 {
        match self {
            BlockKind::Text => 0x1c,
            BlockKind::Button => 0x04,
            BlockKind::Bitmap => 0x24,
        }
    }

    /// One element's size.
    pub fn size(self) -> usize {
        match self {
            BlockKind::Text => 0x2c,
            BlockKind::Button => 0x3c,
            BlockKind::Bitmap => 0x38,
        }
    }

    /// The element's place field.
    pub fn place(self) -> Place {
        match self {
            BlockKind::Text | BlockKind::Button => Place::Rect(0x1c),
            BlockKind::Bitmap => Place::Point(0x0c),
        }
    }

    /// The element's string ID field; a bitmap has none.
    pub fn string_at(self) -> Option<usize> {
        match self {
            BlockKind::Text => Some(0x24),
            BlockKind::Button => Some(0x30),
            BlockKind::Bitmap => None,
        }
    }
}

impl fmt::Display for BlockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A block of a pane, or the screen's title (its header string ID).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum BlockName {
    Title,
    Block(BlockKind, usize),
}

impl FromStr for BlockName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if s == "title" {
            return Ok(BlockName::Title);
        }
        let bad = || {
            format!("{s:?} is not a block name (text.<n>, button.<n>, bitmap.<n> or title)")
        };
        let (k, i) = s.split_once('.').ok_or_else(bad)?;
        let kind = BlockKind::named(k).ok_or_else(bad)?;
        if i.is_empty() || !i.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let index = i.parse().map_err(|_| bad())?;
        Ok(BlockName::Block(kind, index))
    }
}

impl TryFrom<String> for BlockName {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl From<BlockName> for String {
    fn from(b: BlockName) -> String {
        b.to_string()
    }
}

impl fmt::Display for BlockName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockName::Title => f.write_str("title"),
            BlockName::Block(k, i) => write!(f, "{k}.{i}"),
        }
    }
}

/// One edit of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// Move or resize a text or button; a bitmap moves its top-left to
    /// the rect's (its size is its image's).
    SetRect { block: BlockName, rect: Rect },
    /// Park a text or bitmap off screen ([`PARKED`]). A button keeps its
    /// place in the focus order wherever it is, so it is refused.
    Hide { block: BlockName },
    /// Append a byte copy of a text, button or bitmap at the end of its
    /// block, at `rect`; `as` is the copy's name, checked (the block's
    /// count before the copy), so no block before it changes index.
    Duplicate {
        block: BlockName,
        rect: Rect,
        #[serde(rename = "as")]
        copy: BlockName,
    },
    /// Set the title's, a text's or a button's string ID to the string
    /// `name`, added to the map's string table when it lacks it.
    SetString { block: BlockName, name: String },
}

impl Edit {
    /// The edit's `op`.
    pub fn op(&self) -> &'static str {
        match self {
            Edit::SetRect { .. } => "set_rect",
            Edit::Hide { .. } => "hide",
            Edit::Duplicate { .. } => "duplicate",
            Edit::SetString { .. } => "set_string",
        }
    }

    /// The block it edits (for a copy, the original).
    pub fn block(&self) -> BlockName {
        match self {
            Edit::SetRect { block, .. }
            | Edit::Hide { block }
            | Edit::Duplicate { block, .. }
            | Edit::SetString { block, .. } => *block,
        }
    }
}

fn default_map() -> String {
    DEFAULT_MAP.to_owned()
}

/// One file: a screen tag, its map, a pane and the edits in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditFile {
    /// The screen's tag path (`ui\screens\...`), as the map names it.
    pub screen: String,
    /// The map's name: one of the maps `inject::rewrite::MAPS` serves.
    #[serde(default = "default_map")]
    pub map: String,
    #[serde(default)]
    pub pane: usize,
    pub edits: Vec<Edit>,
}

/// Whether `name` can name a string ID of ours: lowercase letters, digits
/// and `_`, as the map's own are, and short enough for its slot.
pub fn string_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn rect_ok(r: &Rect) -> bool {
    r[0] >= r[2] && r[3] >= r[1]
}

impl EditFile {
    /// What is wrong with the file without a map to apply it to, or
    /// nothing. The map checks the rest (the screen, the pane, the block
    /// counts) when it is applied.
    pub fn check(&self) -> Result<(), String> {
        if self.screen.trim().is_empty() {
            return Err("screen: empty".into());
        }
        if self.screen.contains('/') {
            return Err(format!(
                "screen: {:?}: a tag path is written with backslashes",
                self.screen
            ));
        }
        if !crate::ui_kit::inject::rewrite::MAPS.contains(&self.map.as_str()) {
            return Err(format!(
                "map: {:?} is not a map the rewrite serves ({})",
                self.map,
                crate::ui_kit::inject::rewrite::MAPS.join(", ")
            ));
        }
        if self.edits.is_empty() {
            return Err("edits: none".into());
        }
        // The next copy of each kind: copies are appended, so a later one
        // is the earlier's index plus one.
        let mut next: Vec<(BlockKind, usize)> = Vec::new();
        for (i, e) in self.edits.iter().enumerate() {
            let at = |what: String| format!("edits[{i}] ({}): {what}", e.op());
            let block = e.block();
            let kind = match block {
                BlockName::Block(k, _) => Some(k),
                BlockName::Title => None,
            };
            match e {
                Edit::SetRect { rect, .. } => {
                    if kind.is_none() {
                        return Err(at("the title sits where the header layout puts it".into()));
                    }
                    if !rect_ok(rect) {
                        return Err(at(format!("{rect:?} is not [top, left, bottom, right]")));
                    }
                }
                Edit::Hide { .. } => match kind {
                    Some(BlockKind::Text | BlockKind::Bitmap) => {}
                    Some(BlockKind::Button) => {
                        return Err(at(format!(
                            "{block} is a button, which keeps its place in the focus order \
                             wherever its rect is; the game's code shows and hides buttons"
                        )));
                    }
                    None => {
                        return Err(at(
                            "the title is hidden by the tag's no_header flag, not by a rect".into(),
                        ));
                    }
                },
                Edit::Duplicate { rect, copy, .. } => {
                    let Some(kind) = kind else {
                        return Err(at("the title is not a block to copy".into()));
                    };
                    let BlockName::Block(ck, ci) = *copy else {
                        return Err(at(format!("as: {copy} is not a {kind} block")));
                    };
                    if ck != kind {
                        return Err(at(format!("as: {copy} is not a {kind} block")));
                    }
                    if !rect_ok(rect) {
                        return Err(at(format!("{rect:?} is not [top, left, bottom, right]")));
                    }
                    match next.iter_mut().find(|(k, _)| *k == kind) {
                        Some((_, n)) if *n != ci => {
                            return Err(at(format!(
                                "as: {copy}, but the {kind} copy before it makes the next {kind}.{n}"
                            )));
                        }
                        Some((_, n)) => *n += 1,
                        None => next.push((kind, ci + 1)),
                    }
                }
                Edit::SetString { name, .. } => {
                    if kind == Some(BlockKind::Bitmap) {
                        return Err(at(format!("{block} has no string ID")));
                    }
                    if !string_name_ok(name) {
                        return Err(at(format!(
                            "name: {name:?} is not a string ID's name (lowercase letters, digits \
                             and _, at most {MAX_NAME})"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// File `name`'s text as an edit file, checked ([`EditFile::check`]). The
/// error names the file.
pub fn parse(name: &str, text: &str) -> Result<EditFile, String> {
    let file: EditFile =
        serde_yaml_ng::from_str(text).map_err(|e| format!("{name}.yaml: {e}"))?;
    file.check().map_err(|e| format!("{name}.yaml: {e}"))?;
    Ok(file)
}

fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn rect_text(r: &Rect) -> String {
    format!("[{}, {}, {}, {}]", r[0], r[1], r[2], r[3])
}

/// `file` as YAML in the layout the hand-written files use, `comment`
/// (each line) at its head. [`parse`] reads it back as `file`.
pub fn to_yaml(file: &EditFile, comment: &str) -> String {
    let mut out = String::new();
    for line in comment.lines() {
        out.push_str(format!("# {line}").trim_end());
        out.push('\n');
    }
    out.push_str(&format!("screen: {}\n", quoted(&file.screen)));
    out.push_str(&format!("map: {}\n", file.map));
    out.push_str(&format!("pane: {}\n", file.pane));
    out.push_str("edits:\n");
    for e in &file.edits {
        out.push_str(&format!("  - op: {}\n", e.op()));
        out.push_str(&format!("    block: {}\n", e.block()));
        match e {
            Edit::SetRect { rect, .. } => out.push_str(&format!("    rect: {}\n", rect_text(rect))),
            Edit::Hide { .. } => {}
            Edit::Duplicate { rect, copy, .. } => {
                out.push_str(&format!("    rect: {}\n", rect_text(rect)));
                out.push_str(&format!("    as: {copy}\n"));
            }
            Edit::SetString { name, .. } => out.push_str(&format!("    name: {name}\n")),
        }
    }
    out
}

/// The file name the editor gives screen `screen`'s pane `pane`: the
/// tag's last path part, with the pane when it is not the first.
pub fn file_name(screen: &str, pane: usize) -> String {
    let stem = screen.rsplit(['\\', '/']).next().unwrap_or(screen);
    match pane {
        0 => format!("{stem}.yaml"),
        n => format!("{stem}.pane{n}.yaml"),
    }
}

/// The files, by name (the file's stem) and text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edits {
    pub files: Vec<(String, String)>,
}

impl Edits {
    /// The files baked into this build (`assets/ui/retail-edits/*.yaml`,
    /// listed by the build script).
    pub fn baked() -> Edits {
        Edits {
            files: crate::ui_kit::data::baked::RETAIL_EDITS
                .iter()
                .map(|(n, t)| ((*n).into(), (*t).into()))
                .collect(),
        }
    }

    /// Every file parsed and checked, in name order, or every file's
    /// error.
    pub fn parse(&self) -> Result<Vec<(String, EditFile)>, String> {
        let mut files: Vec<&(String, String)> = self.files.iter().collect();
        files.sort();
        let mut ok = Vec::new();
        let mut errors = Vec::new();
        for (name, text) in files {
            match parse(name, text) {
                Ok(f) => ok.push((name.clone(), f)),
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() {
            Ok(ok)
        } else {
            Err(errors.join("; "))
        }
    }

    /// The files' digest: every name and text, in name order.
    pub fn hash(&self) -> Sha256 {
        let mut h = Hasher::default();
        let field = |h: &mut Hasher, b: &[u8]| {
            h.update(&(b.len() as u64).to_le_bytes());
            h.update(b);
        };
        field(&mut h, b"retail-edits");
        let mut files: Vec<&(String, String)> = self.files.iter().collect();
        files.sort();
        for (name, text) in files {
            field(&mut h, name.as_bytes());
            field(&mut h, text.as_bytes());
        }
        h.finish()
    }
}
