//! The HUD editor's working state: one map open, its HUDs listed, the HUDs
//! edited so far, undo and redo, and the save as a patched `.map`.
//!
//! An edit changes a widget's bytes in memory; the save streams the source
//! map through `map::writer::MapEditor` with one patch per run of changed
//! bytes at the widget's metadata address, so the output is the source
//! byte for byte except for the edited fields (and the header checksum,
//! which the writer repairs).

use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use map::{Map, TagId};
use scenario_edit::MapSource;
use scenario_edit::save::MapReader;

use crate::layout::{Field, Mode, offset_keys};
use crate::model::{self, Hud, HudEntry, Value, Widget, WidgetKey};
use crate::render::{self, Art, Fonts, Picture};
use crate::state::HudState;

/// One undo step: a widget's bytes before and after.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub hud: TagId,
    pub key: WidgetKey,
    pub label: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    /// What the step changed (widget and field): a step asked to merge
    /// joins the last one when this matches (a drag, typing in a field).
    merge: String,
    serial: u64,
}

pub struct HudSession {
    pub source: MapSource,
    /// The map's name, for titles.
    pub name: String,
    map: Map<MapReader>,
    pub huds: Vec<HudEntry>,
    /// HUDs read so far, edited or not, by datum.
    open: BTreeMap<u32, Hud>,
    /// The HUD shown and edited.
    pub current: Option<TagId>,
    /// HUDs drawn with it (a weapon held, a vehicle seat), read-only.
    pub with: Vec<TagId>,
    art: Art,
    fonts: Option<Fonts>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    serial: u64,
    /// The serial of the top undo step at the last save (0: none).
    saved: u64,
    pub saved_path: Option<PathBuf>,
}

impl HudSession {
    /// Opens the map `source` (named `name`) and lists its HUDs.
    pub fn open(source: MapSource, name: &str) -> Result<Self> {
        let mut map = source.open().with_context(|| format!("open {name}"))?;
        let huds = model::list(&mut map)?;
        let files = scenario_edit::scene::resource_files(&source);
        let fonts = Fonts::find(&source.path, source.region.is_some());
        Ok(HudSession {
            name: name.to_owned(),
            map,
            huds,
            open: BTreeMap::new(),
            current: None,
            with: Vec::new(),
            art: Art::new(files),
            fonts,
            undo: Vec::new(),
            redo: Vec::new(),
            serial: 0,
            saved: 0,
            saved_path: None,
            source,
        })
    }

    pub fn fonts_found(&self) -> bool {
        self.fonts.is_some()
    }

    fn load(&mut self, id: TagId) -> Result<()> {
        if !self.open.contains_key(&id.0) {
            let hud = model::read(&mut self.map, id)?;
            self.open.insert(id.0, hud);
        }
        Ok(())
    }

    /// Shows and edits HUD `id`.
    pub fn select(&mut self, id: TagId) -> Result<()> {
        self.load(id)?;
        self.current = Some(id);
        self.with.retain(|w| *w != id);
        Ok(())
    }

    /// Draws HUD `id` with the current one (or stops), read-only.
    pub fn set_with(&mut self, id: TagId, on: bool) -> Result<()> {
        self.with.retain(|w| *w != id);
        if on && Some(id) != self.current {
            self.load(id)?;
            self.with.push(id);
        }
        Ok(())
    }

    pub fn hud(&self) -> Option<&Hud> {
        self.open.get(&self.current?.0)
    }
    pub fn hud_by(&self, id: TagId) -> Option<&Hud> {
        self.open.get(&id.0)
    }
    pub fn entry(&self, id: TagId) -> Option<&HudEntry> {
        self.huds.iter().find(|e| e.id == id)
    }

    /// Any HUD has edits not saved.
    pub fn is_dirty(&self) -> bool {
        self.undo.last().map_or(0, |e| e.serial) != self.saved
    }
    /// The HUDs whose widgets differ from the map.
    pub fn edited(&self) -> Vec<TagId> {
        self.open.values().filter(|h| h.is_dirty()).map(|h| h.id).collect()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn history(&self) -> impl Iterator<Item = &Edit> {
        self.undo.iter()
    }

    /// Sets widget `key`'s field `field` of the current HUD to `v`. With
    /// `merge` the step joins the previous one when that had the same
    /// merge key (one undo for a whole drag).
    pub fn set(&mut self, key: WidgetKey, field: &str, v: Value, merge: bool) -> Result<()> {
        let f = *key
            .kind
            .fields()
            .iter()
            .find(|f| f.key == field)
            .with_context(|| format!("no field {field}"))?;
        let bytes = Widget::encode(&f, v).with_context(|| format!("{field}: {v:?} does not fit"))?;
        self.change(key, &label(&f, v), format!("{key:?}:{field}"), merge, |w| {
            w.bytes[Widget::value_range(&f)].copy_from_slice(&bytes);
        })
    }

    /// Moves widget `key` to `offset` in screen mode `mode` (a drag: with
    /// `merge` the moves of one drag are one undo step).
    pub fn move_to(&mut self, key: WidgetKey, mode: Mode, offset: (i16, i16), merge: bool) -> Result<()> {
        let (x, y) = offset_keys(mode);
        self.change(
            key,
            &format!("move to ({}, {}) {}", offset.0, offset.1, mode.label()),
            format!("{key:?}:move:{x}{y}"),
            merge,
            |w| w.set_offset(mode, offset),
        )
    }

    /// Writes `bytes` at metadata `address` inside one widget of the
    /// current HUD: the entry point for a generic field panel, which knows
    /// addresses, not field keys. `merge` (the panel's merge key) joins
    /// the step to the last one with the same key. Bytes outside one
    /// widget's record (a nested block's elements) are refused: they are
    /// the Tags tool's.
    pub fn write(&mut self, address: u32, bytes: &[u8], label: &str, merge: Option<u64>) -> Result<WidgetKey> {
        let hud = self.hud().context("no HUD open")?;
        let end = address
            .checked_add(u32::try_from(bytes.len())?)
            .context("write past the address space")?;
        let w = hud
            .widgets
            .iter()
            .find(|w| w.address <= address && end <= w.address + w.bytes.len() as u32)
            .with_context(|| {
                format!("{address:#x} is not inside one widget of {}: edit it in the Tags tool", hud.name)
            })?;
        let key = w.key;
        let at = (address - w.address) as usize;
        let merge_key = format!("{key:?}:bytes:{}", merge.unwrap_or(u64::from(address)));
        self.change(key, &format!("set {label}"), merge_key, merge.is_some(), |w| {
            w.bytes[at..at + bytes.len()].copy_from_slice(bytes);
        })?;
        Ok(key)
    }

    /// Every widget of every HUD read so far, with its metadata address
    /// and its bytes edits applied (for a view of the map's bytes, such as
    /// the field panel's document, to mirror).
    pub fn widget_bytes(&self) -> impl Iterator<Item = (u32, &[u8])> {
        self.open
            .values()
            .flat_map(|h| h.widgets.iter().map(|w| (w.address, w.bytes.as_slice())))
    }

    /// The open map, to read more of it (the field panel's document).
    pub fn map_mut(&mut self) -> &mut Map<MapReader> {
        &mut self.map
    }

    fn change(
        &mut self,
        key: WidgetKey,
        what: &str,
        merge_key: String,
        merge: bool,
        f: impl FnOnce(&mut Widget),
    ) -> Result<()> {
        let hud_id = self.current.context("no HUD open")?;
        let hud = self.open.get_mut(&hud_id.0).context("no HUD open")?;
        let w = hud.widget_mut(key).with_context(|| format!("no widget {key:?}"))?;
        let before = w.bytes.clone();
        f(w);
        if w.bytes == before {
            return Ok(());
        }
        let after = w.bytes.clone();
        let label = format!("{}: {what}", w.name);
        self.redo.clear();
        if merge
            && let Some(last) = self.undo.last_mut()
            && last.merge == merge_key
            && last.hud == hud_id
            && last.serial != self.saved
        {
            last.after = after;
            last.label = label;
            return Ok(());
        }
        self.serial += 1;
        self.undo.push(Edit {
            hud: hud_id,
            key,
            label,
            before,
            after,
            merge: merge_key,
            serial: self.serial,
        });
        Ok(())
    }

    fn apply(&mut self, e: &Edit, bytes: &[u8]) {
        if let Some(w) = self.open.get_mut(&e.hud.0).and_then(|h| h.widget_mut(e.key)) {
            w.bytes = bytes.to_vec();
        }
        self.current = Some(e.hud);
    }

    /// Undoes the last step; its label.
    pub fn undo(&mut self) -> Option<String> {
        let e = self.undo.pop()?;
        self.apply(&e, &e.before.clone());
        let l = e.label.clone();
        self.redo.push(e);
        Some(l)
    }
    pub fn redo(&mut self) -> Option<String> {
        let e = self.redo.pop()?;
        self.apply(&e, &e.after.clone());
        let l = e.label.clone();
        self.undo.push(e);
        Some(l)
    }

    /// Draws the current HUD (and those drawn with it) at `st`.
    pub fn draw(&mut self, st: &HudState) -> Option<Picture> {
        let id = self.current?;
        let mut huds = vec![self.open.get(&id.0)?];
        huds.extend(self.with.iter().filter_map(|w| self.open.get(&w.0)));
        Some(render::draw(&mut self.map, &mut self.art, self.fonts.as_ref(), &huds, st))
    }

    /// Tag `id`'s name.
    pub fn tag_name(&self, id: TagId) -> String {
        self.map
            .tag(id)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_else(|| format!("{id}"))
    }
    /// The tags of `group` (for a reference field's picker).
    pub fn tags_of(&self, group: map::FourCC) -> Vec<(TagId, String)> {
        let mut v: Vec<_> = self
            .map
            .tags()
            .iter()
            .filter(|t| t.group == group)
            .map(|t| (t.id, t.name.clone().unwrap_or_else(|| format!("{}", t.id))))
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }
    /// String ID `v`'s name.
    pub fn string(&self, v: u32) -> Option<String> {
        model::string(&self.map, v)
    }
    /// A bitmap's sequence count (for the sequence field).
    pub fn sequences(&mut self, id: TagId) -> Option<usize> {
        tags::DecodedTag::read(&mut self.map, id)
            .ok()
            .and_then(|d| match d.data {
                tags::TagData::Bitmap(b) => Some(b.sequences.len()),
                _ => None,
            })
    }

    /// Where each widget lands at `st` (the edited HUD's, then those
    /// drawn with it): point, rectangle, shown, drawn.
    pub fn placed(&mut self, st: &HudState) -> Vec<crate::render::Placed> {
        self.draw(st).map(|p| p.placed).unwrap_or_default()
    }

    /// The layout checks at `st`.
    pub fn check(&mut self, st: &HudState) -> Vec<crate::checks::Finding> {
        self.draw(st).map(|p| crate::checks::check(&p)).unwrap_or_default()
    }

    /// The HUD drawn at `st` as a PNG at `path`. Refused inside a Git
    /// checkout: the picture is the disc's art.
    pub fn export_png(&mut self, st: &HudState, path: &Path) -> Result<()> {
        let abs = std::path::absolute(path)?;
        if let Some(repo) = abs.ancestors().find(|a| a.join(".git").exists()) {
            bail!(
                "{} is inside the Git checkout {}; pick a folder outside it",
                path.display(),
                repo.display()
            );
        }
        let p = self.draw(st).context("no HUD open")?;
        p.canvas.write_png(path)
    }

    /// The patches the save writes: (metadata address, bytes), one per run
    /// of changed bytes in each edited widget.
    pub fn patches(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        for hud in self.open.values() {
            for w in &hud.widgets {
                let mut i = 0;
                while i < w.bytes.len() {
                    if w.bytes[i] == w.original[i] {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < w.bytes.len() && w.bytes[i] != w.original[i] {
                        i += 1;
                    }
                    out.push((w.address + start as u32, w.bytes[start..i].to_vec()));
                }
            }
        }
        out
    }

    /// Writes the map with the edits to `out` (never over the source): a
    /// sibling temporary file renamed into place. Returns the bytes
    /// written.
    pub fn save_as(&mut self, out: &Path) -> Result<u64> {
        if self.source.bytes.is_none()
            && let (Ok(a), Ok(b)) = (self.source.path.canonicalize(), out.canonicalize())
            && a == b
        {
            bail!("refusing to write over the source map");
        }
        let mut map = self.source.open()?;
        let mut editor = map.edit();
        for (address, bytes) in self.patches() {
            editor.patch_metadata(address, &bytes)?;
        }
        let mut name = out.file_name().context("no file name")?.to_owned();
        name.push(".partial");
        let tmp = out.with_file_name(name);
        let r = (|| -> Result<u64> {
            let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
            let n = editor.write_to(&mut w)?;
            w.flush()?;
            w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            std::fs::rename(&tmp, out)?;
            Ok(n)
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        let n = r?;
        self.saved = self.undo.last().map_or(0, |e| e.serial);
        self.saved_path = Some(out.to_path_buf());
        Ok(n)
    }
}

fn label(f: &Field, v: Value) -> String {
    let mode = f.mode.map(|m| format!(" ({})", m.label())).unwrap_or_default();
    let v = match v {
        Value::Int(i) => format!("{i}"),
        Value::Float(x) => format!("{x}"),
        Value::Tag(t) => format!("{t}"),
        Value::String(s) => format!("{s:#x}"),
    };
    format!("{}{mode} = {v}", f.label)
}
