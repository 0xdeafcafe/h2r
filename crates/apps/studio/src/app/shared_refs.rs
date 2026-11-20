//! Which map a tag's raw data lives in. Every tag a map uses has its
//! metadata in that map (docs/formats/map-builder.md); its raw resources
//! (bitmap pixels, geometry, sound) are reached through raw pointers whose
//! top two bits pick the map: this one, `mainmenu.map`, `shared.map` or
//! `single_player_shared.map` (`map::RawPointer`). The game resolves them
//! that way, and so do the viewport and the tools here (the shared caches
//! of the disc, `geometry::shared_sources`). This module counts a tag's
//! pointers by map so the Tags tool can say when its data is in a shared
//! map: a Map Pack map leans on the disc's `shared.map` like any other.
use map::{RawPointer, ResourceFile, TagId};
use tags::registry::{Kind, Layout};

use super::{App, TagDoc};

/// Fields looked at, at most, for one tag (a large scenario stays quick).
const VISITS: usize = 200_000;
/// Nested blocks followed, at most.
const DEPTH: usize = 16;

/// A tag's raw pointers by the map they address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Homes {
    pub local: usize,
    pub mainmenu: usize,
    pub shared: usize,
    pub single_player_shared: usize,
}

impl Homes {
    fn add(&mut self, file: ResourceFile) {
        match file {
            ResourceFile::Local => self.local += 1,
            ResourceFile::MainMenu => self.mainmenu += 1,
            ResourceFile::Shared => self.shared += 1,
            ResourceFile::SinglePlayerShared => self.single_player_shared += 1,
        }
    }

    /// One line for the Tags tool, when any of the tag's raw data is in a
    /// shared map; `None` when it is all in this map (or there is none).
    pub fn note(&self) -> Option<String> {
        let away: Vec<String> = [
            ("shared.map", self.shared),
            ("single_player_shared.map", self.single_player_shared),
            ("mainmenu.map", self.mainmenu),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(f, n)| format!("{f} ({n})"))
        .collect();
        if away.is_empty() {
            return None;
        }
        Some(if self.local == 0 {
            format!(
                "raw data lives in a shared map: {} (read from the disc's copy)",
                away.join(", ")
            )
        } else {
            format!(
                "raw data partly in a shared map: {}, and {} in this map",
                away.join(", "),
                self.local
            )
        })
    }
}

/// Count tag `id`'s raw pointers by map, through its layout's blocks.
/// `None` when the tag has no data or no layout here.
pub fn raw_homes(doc: &TagDoc, id: TagId) -> Option<Homes> {
    let t = doc.tag(id)?;
    if t.size == 0 {
        return None;
    }
    let layout = doc.layout(id)?.clone();
    let mut homes = Homes::default();
    let mut budget = VISITS;
    walk(doc, t.address, &layout, &mut homes, &mut budget, 0);
    Some(homes)
}

fn walk(doc: &TagDoc, at: u32, layout: &Layout, h: &mut Homes, budget: &mut usize, depth: usize) {
    if depth > DEPTH {
        return;
    }
    for f in &layout.fields {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Some(field) = at.checked_add(f.offset) else {
            return;
        };
        match &f.kind {
            Kind::Raw => {
                let Some(b) = doc.read(field, 4) else { continue };
                let v = u32::from_le_bytes(b[..4].try_into().unwrap());
                // Zero is no resource (offset 0 is the cache header).
                if v != 0
                    && let Some(p) = RawPointer::decode(v)
                {
                    h.add(p.file);
                }
            }
            Kind::Block(child) if child.size > 0 => {
                let Some((n, a)) = doc.block(field, child) else { continue };
                for i in 0..n {
                    let Some(e) = i.checked_mul(child.size).and_then(|o| a.checked_add(o)) else {
                        break;
                    };
                    walk(doc, e, child, h, budget, depth + 1);
                    if *budget == 0 {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Homes {
    /// Pointers into a shared map (not this one).
    pub fn away(&self) -> usize {
        self.mainmenu + self.shared + self.single_player_shared
    }
    /// A short mark for the tag tree: the shared maps the raw data is in
    /// (`shared`, `sp shared`, `mainmenu`), empty when none.
    pub fn badge(&self) -> String {
        let mut out = Vec::new();
        if self.shared > 0 {
            out.push("shared");
        }
        if self.single_player_shared > 0 {
            out.push("sp shared");
        }
        if self.mainmenu > 0 {
            out.push("mainmenu");
        }
        out.join(", ")
    }
    /// As JSON for the agent API: pointers by map.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "local": self.local,
            "mainmenu": self.mainmenu,
            "shared": self.shared,
            "single_player_shared": self.single_player_shared,
        })
    }
}

impl TagDoc {
    /// [`raw_homes`] of tag `id`, kept until block counts or addresses may
    /// have changed ([`TagDoc::shape_rev`]): raw pointers are structural,
    /// so field edits leave them, and the tag tree can mark every row it
    /// draws.
    pub fn homes(&self, id: TagId) -> Option<Homes> {
        let rev = self.shape_rev();
        {
            let cache = self.homes_cache.borrow();
            if cache.0 == rev
                && let Some(h) = cache.1.get(&id)
            {
                return *h;
            }
        }
        let h = raw_homes(self, id);
        let mut cache = self.homes_cache.borrow_mut();
        if cache.0 != rev {
            *cache = (rev, Default::default());
        }
        cache.1.insert(id, h);
        h
    }
}

impl App {
    /// The Tags tool's line on where tag `id`'s raw data lives, when any
    /// of it is in a shared map, for the document shown
    /// ([`App::shown_tags`]). Kept for the tag, the document and its
    /// revision, so drawing it every frame costs one walk.
    pub fn tag_shared_note(&self, id: TagId) -> Option<String> {
        let doc = self.shown_tags();
        // The browsed map's document and the open map's are told apart by
        // the map browsed.
        let key = (id, doc.rev(), self.browsing().map(str::to_owned));
        let mut cache = self.tag_homes.borrow_mut();
        if let Some((cid, crev, cmap, note)) = cache.as_ref()
            && (*cid, *crev, cmap.clone()) == key
        {
            return note.clone();
        }
        let note = raw_homes(doc, id).and_then(|h| h.note());
        *cache = Some((key.0, key.1, key.2, note.clone()));
        note
    }
}
