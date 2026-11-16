//! The settings schema: which settings the editor shows, on which page,
//! with which values and labels, and each game type's defaults.
//!
//! Read at run time from the main menu map's `goof` tag
//! (`multiplayer_game_settings`) and the `sily` setting definitions its
//! pages name (docs/game/community-gametypes.md has the layout), with labels from
//! the tag's `unic` strings. Without a disc, [`Schema::fallback`] is our
//! symbolic model: every setting the record can store, our names, and no
//! value lists (any value the record accepts).

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};
use tags::{Language, Localization, StringId};

use crate::record::{self, GameType, GameVariant, RecordError, SettingId};

// ---- goof / sily layout (0x149f88, 0x2369b3, 0x236a3a, 0x1489b5) ----

/// `+0x18`: setting pages, `PAGE_SIZE` bytes each.
const GOOF_PAGES: usize = 0x18;
/// `+0x20`: the `unic` reference (datum at `+0x24`).
const GOOF_UNIC: usize = 0x20;
/// `+0x28`: built-in variants, `ENTRY_SIZE` bytes each.
const GOOF_BUILTINS: usize = 0x28;
/// Per-game-type defaults: a `(setting, value)` block at
/// `+0x24 + 0x14 * content type` (`0x1489b5`, indexed by the save-content
/// type of `0x216ab0`; Race and Headhunter's slots are empty).
const GOOF_DEFAULTS: usize = 0x24;
const PAGE_SIZE: usize = 0x18;
/// Page: `+0` page id (what `0x236a3a` looks a page up by), `+8` count and
/// `+0xc` address of 8-byte `sily` tag references.
const PAGE_ID: usize = 0x0;
const PAGE_REFS: usize = 0x8;
const ENTRY_SIZE: usize = 0x14;
/// `sily`: `+0` setting number, `+8` its `unic` reference (datum at
/// `+0xc`), `+0x10` title, `+0x14` the title in capitals (not used) and
/// `+0x18` help string ids (checked against the retail tag's text),
/// `+0x1c` options block of
/// `OPTION_SIZE` bytes (`+0` default flag, `+4` value, `+8` label id).
const SILY_SETTING: usize = 0x0;
const SILY_UNIC: usize = 0xc;
const SILY_TITLE: usize = 0x10;
const SILY_HELP: usize = 0x18;
const SILY_OPTIONS: usize = 0x1c;
const OPTION_SIZE: usize = 0xc;

const MAX_PAGES: u32 = 64;
const MAX_REFS: u32 = 128;
const MAX_OPTIONS: u32 = 256;
const MAX_BUILTINS: u32 = 101;
const MAX_PAIRS: u32 = 128;

/// The `sily` group.
pub const SETTING_DEFINITION: FourCC = FourCC(*b"sily");

/// One value a setting offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: i32,
    pub label: String,
    /// The definition marks it as the default.
    pub default: bool,
}

/// One setting as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingDef {
    pub id: SettingId,
    pub title: String,
    pub help: String,
    /// Empty when the schema does not know the values (fallback).
    pub choices: Vec<Choice>,
}

/// A page of the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: u32,
    pub title: String,
    pub settings: Vec<SettingId>,
}

/// A built-in variant as the tag lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Builtin {
    pub name: String,
    pub game_type: GameType,
    pub description: u8,
    pub pairs: Vec<(SettingId, i32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub settings: BTreeMap<SettingId, SettingDef>,
    pub pages: Vec<Page>,
    /// Per game type, the `(setting, value)` pairs a new variant starts
    /// with (empty in the fallback).
    pub defaults: BTreeMap<GameType, Vec<(SettingId, i32)>>,
    pub builtins: Vec<Builtin>,
    /// Where the schema came from, for the panel.
    pub source: String,
}

/// A schema finding about a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// The game's record clean-up would reject the record.
    Record(record::Problem),
    /// A setting holds a value its definition does not offer.
    NotOffered { id: SettingId, value: i32 },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::Record(p) => write!(f, "+{:#x}: {}", p.offset, p.what),
            Issue::NotOffered { id, value } => {
                write!(
                    f,
                    "setting {} holds {value}, which the editor does not offer",
                    id.0
                )
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("no variant settings (goof) tag in the map")]
    NoTag,
    #[error("the variant settings tag is malformed: {0}")]
    Malformed(&'static str),
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Tags(#[from] tags::Error),
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A tag block `(count, address)` at `at`, read through `read`.
fn block(
    b: &[u8],
    at: usize,
    size: usize,
    max: u32,
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, SchemaError> {
    let n = le32(b, at).ok_or(SchemaError::Malformed("short block header"))?;
    let addr = le32(b, at + 4).ok_or(SchemaError::Malformed("short block header"))?;
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > max {
        return Err(SchemaError::Malformed("implausible block count"));
    }
    read(addr, n * size as u32).ok_or(SchemaError::Malformed("block outside the map"))
}

/// What the schema reader needs from a map, so tests can use synthetic tags.
pub trait Source {
    /// Bytes at a tag-memory address.
    fn read(&mut self, addr: u32, size: u32) -> Option<Vec<u8>>;
    /// The bytes of the tag with this datum.
    fn tag(&mut self, datum: u32) -> Option<Vec<u8>>;
    /// A string id's text in the `unic` tag with datum `unic`.
    fn text(&mut self, unic: u32, id: u32) -> Option<String>;
}

impl Schema {
    /// The symbolic model: every setting the record stores, grouped as
    /// "Game" (settings for every type) and one page per game type.
    pub fn fallback() -> Self {
        let mut settings = BTreeMap::new();
        for id in record::all_settings() {
            let title = record::symbolic_name(id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Setting {}", id.0));
            settings.insert(
                id,
                SettingDef {
                    id,
                    title,
                    help: String::new(),
                    choices: Vec::new(),
                },
            );
        }
        let mut pages = vec![Page {
            id: 9,
            title: "Game".into(),
            settings: settings
                .keys()
                .copied()
                .filter(|s| record::fallback_owner(*s).is_none())
                .collect(),
        }];
        for ty in GameType::ALL {
            pages.push(Page {
                id: 12 + ty.engine() as u32,
                title: format!("{ty} options"),
                settings: settings
                    .keys()
                    .copied()
                    .filter(|s| record::fallback_owner(*s) == Some(ty))
                    .collect(),
            });
        }
        Schema {
            settings,
            pages,
            defaults: BTreeMap::new(),
            builtins: Vec::new(),
            source: "built-in symbolic model (no disc)".into(),
        }
    }

    /// The schema from a main menu map.
    pub fn from_map<R: Read + Seek>(
        map: &mut Map<R>,
        language: Language,
    ) -> Result<Self, SchemaError> {
        let tag = map
            .tags()
            .iter()
            .find(|t| t.group == tags::ui::VARIANT_SETTINGS)
            .ok_or(SchemaError::NoTag)?
            .clone();
        let goof = tag_bytes(map, &tag)?;
        let l = Localization::read(map)?;
        let mut src = MapSource {
            map,
            l: &l,
            unics: BTreeMap::new(),
            language,
        };
        let mut s = Self::from_goof(&goof, &mut src)?;
        s.source = format!("{} ({language:?})", tag.name.as_deref().unwrap_or("goof"));
        Ok(s)
    }

    /// The schema from a `goof` tag's bytes.
    pub fn from_goof(goof: &[u8], src: &mut dyn Source) -> Result<Self, SchemaError> {
        let goof_unic = le32(goof, GOOF_UNIC + 4).unwrap_or(u32::MAX);
        let mut read = |a, s| src.read(a, s);
        let page_bytes = block(goof, GOOF_PAGES, PAGE_SIZE, MAX_PAGES, &mut read)?;
        let mut pages = Vec::new();
        let mut settings = BTreeMap::new();
        for p in page_bytes.as_chunks::<PAGE_SIZE>().0.iter() {
            let id = le32(p, PAGE_ID).unwrap();
            let refs = block(p, PAGE_REFS, 8, MAX_REFS, &mut |a, s| src.read(a, s))?;
            let mut page = Page {
                id,
                title: page_title(id),
                settings: Vec::new(),
            };
            for r in refs.as_chunks::<8>().0.iter() {
                let datum = le32(r, 4).unwrap();
                if datum == u32::MAX {
                    continue;
                }
                let Some(sily) = src.tag(datum) else { continue };
                let Some(def) = setting_def(&sily, src)? else {
                    continue;
                };
                page.settings.push(def.id);
                settings.entry(def.id).or_insert(def);
            }
            pages.push(page);
        }
        let mut defaults = BTreeMap::new();
        for ty in GameType::ALL {
            let at = GOOF_DEFAULTS + ENTRY_SIZE * ty.content_type().unwrap() as usize;
            if goof.len() >= at + 8 {
                defaults.insert(ty, pairs(goof, at, src)?);
            }
        }
        let mut builtins = Vec::new();
        let entries = block(
            goof,
            GOOF_BUILTINS,
            ENTRY_SIZE,
            MAX_BUILTINS,
            &mut |a, s| src.read(a, s),
        )?;
        for e in entries.as_chunks::<ENTRY_SIZE>().0.iter() {
            let Some(game_type) = GameType::from_list_index(le32(e, 4).unwrap()) else {
                continue;
            };
            let name = src.text(goof_unic, le32(e, 0).unwrap()).unwrap_or_default();
            builtins.push(Builtin {
                name,
                game_type,
                description: e[0x10].min(100),
                pairs: pairs(e, 8, src)?,
            });
        }
        Ok(Schema {
            settings,
            pages,
            defaults,
            builtins,
            source: "goof tag".into(),
        })
    }

    /// A new variant of `ty` with the type's defaults, as the game builds
    /// one (`0x1489b5`; that it starts from zeros is unverified).
    pub fn new_variant(&self, ty: GameType) -> GameVariant {
        let mut v = GameVariant::blank(ty);
        for (id, value) in self.defaults.get(&ty).into_iter().flatten() {
            let _ = v.set(*id, *value);
        }
        v
    }

    /// A built-in variant as the game builds it at load (`0x149f88`): the
    /// type's defaults, the name, the description number, then its pairs.
    pub fn build(&self, b: &Builtin) -> Result<GameVariant, RecordError> {
        let mut v = self.new_variant(b.game_type);
        let units: Vec<u16> = b.name.encode_utf16().take(record::MAX_NAME_UNITS).collect();
        v.set_name(&String::from_utf16_lossy(&units))?;
        v.set_description(b.description);
        for (id, value) in &b.pairs {
            v.set(*id, *value)?;
        }
        Ok(v)
    }

    /// The pages the editor shows for a game type, in tag order. Page ids
    /// (read from the retail tag): `engine - 1` match options, 9 player,
    /// 11 vehicles, 12 weapons, `12 + engine` the type's own options,
    /// `21 + engine` a summary page (left out: it repeats the others),
    /// `31 + engine` team options. Unknown ids are shown for every type.
    pub fn pages_for(&self, ty: GameType) -> Vec<Page> {
        let e = ty.engine() as u32;
        self.pages
            .iter()
            .filter(|p| match p.id {
                0..=8 => p.id + 1 == e,
                9 | 11 | 12 => true,
                13..=21 => p.id == 12 + e,
                22..=30 => false,
                32..=40 => p.id == 31 + e,
                _ => true,
            })
            .map(|p| Page {
                id: p.id,
                title: p.title.clone(),
                settings: p
                    .settings
                    .iter()
                    .copied()
                    .filter(|s| applies(*s, ty))
                    .collect(),
            })
            .filter(|p| !p.settings.is_empty())
            .collect()
    }

    /// Findings for a variant: the clean-up's rules, then every setting of
    /// its type whose value the definition does not offer.
    pub fn validate(&self, v: &GameVariant) -> Vec<Issue> {
        let mut out: Vec<Issue> = record::check(v).into_iter().map(Issue::Record).collect();
        let ty = v.game_type();
        for page in self.pages_for(ty) {
            for id in page.settings {
                let Some(def) = self.settings.get(&id) else {
                    continue;
                };
                if def.choices.is_empty() {
                    continue;
                }
                let Some(value) = v.get(id) else { continue };
                if !def.choices.iter().any(|c| c.value == value) {
                    let issue = Issue::NotOffered { id, value };
                    if !out.contains(&issue) {
                        out.push(issue);
                    }
                }
            }
        }
        out
    }

    /// The settings of `v` that differ from its type's defaults.
    pub fn diff_from_defaults(&self, v: &GameVariant) -> Vec<(SettingId, i32, i32)> {
        let base = self.new_variant(v.game_type());
        let ty = v.game_type();
        record::all_settings()
            .filter(|s| applies(*s, ty))
            .filter_map(|s| {
                let (a, b) = (base.get(s)?, v.get(s)?);
                (a != b).then_some((s, a, b))
            })
            .collect()
    }

    /// The label of a setting's value, if the schema knows it.
    pub fn value_label(&self, id: SettingId, value: i32) -> Option<&str> {
        self.settings
            .get(&id)?
            .choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label.as_str())
    }

    pub fn title(&self, id: SettingId) -> String {
        self.settings
            .get(&id)
            .map(|d| d.title.clone())
            .unwrap_or_else(|| format!("Setting {}", id.0))
    }
}

/// Our title for a page id (the tag has none; see [`Schema::pages_for`]).
pub fn page_title(id: u32) -> String {
    match id {
        0..=8 => "Match options".into(),
        9 => "Player options".into(),
        11 => "Vehicle options".into(),
        12 => "Weapon options".into(),
        13..=21 => format!("{} options", GameType::from_engine(id as i32 - 12)),
        22..=30 => "Summary".into(),
        32..=40 => "Team options".into(),
        n => format!("Page {n}"),
    }
}

/// Whether setting `s` applies to game type `ty`.
pub fn applies(s: SettingId, ty: GameType) -> bool {
    if (1..=9).contains(&s.0) {
        return ty.score_setting() == Some(s);
    }
    match record::fallback_owner(s) {
        Some(owner) => owner == ty,
        None => true,
    }
}

fn pairs(b: &[u8], at: usize, src: &mut dyn Source) -> Result<Vec<(SettingId, i32)>, SchemaError> {
    let raw = block(b, at, 8, MAX_PAIRS, &mut |a, s| src.read(a, s))?;
    Ok(raw
        .as_chunks::<8>()
        .0
        .iter()
        .map(|p| (SettingId(le32(p, 0).unwrap()), le32(p, 4).unwrap() as i32))
        .collect())
}

fn setting_def(sily: &[u8], src: &mut dyn Source) -> Result<Option<SettingDef>, SchemaError> {
    let Some(setting) = le32(sily, SILY_SETTING) else {
        return Ok(None);
    };
    let id = SettingId(setting);
    let unic = le32(sily, SILY_UNIC).unwrap_or(u32::MAX);
    let text = |src: &mut dyn Source, at| le32(sily, at).and_then(|sid| src.text(unic, sid));
    let title = text(src, SILY_TITLE)
        .or_else(|| record::symbolic_name(id).map(str::to_string))
        .unwrap_or_else(|| format!("Setting {setting}"));
    let help = text(src, SILY_HELP).unwrap_or_default();
    let opts = block(sily, SILY_OPTIONS, OPTION_SIZE, MAX_OPTIONS, &mut |a, s| {
        src.read(a, s)
    })?;
    let choices = opts
        .as_chunks::<OPTION_SIZE>()
        .0
        .iter()
        .map(|o| {
            let value = le32(o, 4).unwrap() as i32;
            Choice {
                value,
                label: le32(o, 8)
                    .and_then(|sid| src.text(unic, sid))
                    .unwrap_or_else(|| value.to_string()),
                default: le32(o, 0).unwrap() & 1 != 0,
            }
        })
        .collect();
    Ok(Some(SettingDef {
        id,
        title,
        help,
        choices,
    }))
}

fn tag_bytes<R: Read + Seek>(map: &mut Map<R>, tag: &map::Tag) -> Result<Vec<u8>, SchemaError> {
    let loc = tag
        .location
        .ok_or(SchemaError::Malformed("the tag has no data"))?;
    Ok(map.read_file_range(loc.file_offset()?, loc.size.into())?)
}

struct MapSource<'a, R> {
    map: &'a mut Map<R>,
    l: &'a Localization,
    unics: BTreeMap<u32, Option<tags::Unic>>,
    language: Language,
}

impl<R: Read + Seek> Source for MapSource<'_, R> {
    fn read(&mut self, addr: u32, size: u32) -> Option<Vec<u8>> {
        let at = self.map.file_offset(addr, size).ok()?;
        self.map.read_file_range(at, size.into()).ok()
    }
    fn tag(&mut self, datum: u32) -> Option<Vec<u8>> {
        let tag = self.map.tag(TagId(datum)).ok()?.clone();
        if tag.group != SETTING_DEFINITION {
            return None;
        }
        tag_bytes(self.map, &tag).ok()
    }
    fn text(&mut self, unic: u32, id: u32) -> Option<String> {
        if !self.unics.contains_key(&unic) {
            let u = self.l.unic(self.map, TagId(unic)).ok();
            self.unics.insert(unic, u);
        }
        let u = self.unics.get(&unic)?.as_ref()?;
        self.l
            .text_in(u, StringId(id), self.language)
            .ok()
            .flatten()
    }
}
