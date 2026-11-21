//! A published package's details (docs/game/maps.md §5, "Details"): a
//! longer description, screenshots, and a report of what the mod
//! contains, set by an admin after publishing as a server-signed
//! `MapDetailed` event on the package's stream. Display data only, like
//! the credit: the manifest, its `MapId` and the publisher's signature are
//! untouched. Re-exported from [`crate::maps`].
//!
//! The contents report is what `h2 live maps analyze` reads off the
//! patched map against its base (`cli::live_mod_contents`): which tag
//! classes the mod adds or changes, the references it swaps (a weapon's
//! projectile, an object's model, a shader's bitmap), the stats it
//! changes, and what the scenario places. `lines` say it in words; the
//! other lists are the same facts as fields.
//!
//! Many mods came out in packs (the Phantom Mappack, AI Revision, ...): a
//! package's details may name its pack by id ([`Details::pack`]), and a
//! pack's own description, authors, images, source and date are kept
//! apart ([`PackInfo`], `PUT /v1/mappacks/{id}`, a server-signed
//! `MapPackSet` on [`pack_stream`]) so they are said once for every map
//! of it.
use serde::{Deserialize, Serialize};

use content::{MapId, Sha256};

/// `MapDetailed`: an admin set (or cleared) a package's details
/// (server-signed, on [`crate::maps::package_stream`]). Data: [`Detailed`].
pub const MAP_DETAILED: &str = "MapDetailed";

/// `MapPackSet`: an admin set (or cleared) a map pack's info
/// (server-signed, on [`pack_stream`]). Data: [`PackSet`].
pub const MAP_PACK_SET: &str = "MapPackSet";

/// The longest pack id (`phantom-mappack`).
pub const MAX_PACK_ID: usize = 64;
/// The longest pack name.
pub const MAX_PACK_NAME: usize = 120;

/// Whether `id` is a pack id: 1 to [`MAX_PACK_ID`] of `a-z`, `0-9` and
/// `-`, starting with a letter or digit.
pub fn pack_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_PACK_ID
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The stream a pack's events are on.
pub fn pack_stream(id: &str) -> String {
    format!("mappack-{id}")
}

/// The longest details description.
pub const MAX_DETAILS_DESCRIPTION: usize = 4000;
/// At most this many images.
pub const MAX_DETAIL_IMAGES: usize = 8;
/// The longest caption, image credit or licence.
pub const MAX_CAPTION: usize = 300;
/// At most this many report lines, and entries in each report list.
pub const MAX_CONTENT_LINES: usize = 80;
pub const MAX_CONTENT_ENTRIES: usize = 400;
/// The longest report line or field text.
pub const MAX_CONTENT_TEXT: usize = 300;

/// One screenshot or promo image of a mod: a content blob (PNG or JPEG,
/// uploaded through `PUT /v1/content/{sha256}`) and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapImage {
    pub sha256: Sha256,
    /// Live accepts player reports for gallery captures.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reportable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// The page it was found on (an `http`/`https` URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Who made the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
}

/// A tag class the mod adds, changes or removes tags of.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassChange {
    /// The tag group (`weap`, `snd!`).
    pub group: String,
    /// What the group is, in words (`weapons`).
    pub label: String,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub changed: u32,
    #[serde(default)]
    pub removed: u32,
    /// The added tags' names, when few.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_names: Vec<String>,
}

/// A reference the mod points elsewhere: `tag`'s `field` named `from`
/// and now names `to` (None: no tag).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swap {
    /// `projectile`, `model`, `render_model`, `collision`, `physics`,
    /// `animation`, `shader`, `texture`, `damage`, `effect`, `sound`,
    /// `weapon`, `vehicle`, `palette`, `sky`, `placement` or `reference`.
    pub kind: String,
    /// The tag whose reference changed (`objects\...\battle_rifle`); for
    /// `placement`, the feature (`vehicles`).
    pub tag: String,
    /// The field (`barrels[0].projectile`); for `placement`, how many
    /// spots.
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// What the scenario places of one tag, against the base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementChange {
    /// `vehicles`, `weapons` or `objects`.
    pub feature: String,
    pub tag: String,
    #[serde(default)]
    pub base: u32,
    #[serde(default)]
    pub now: u32,
    /// Spots of the base's placements of it that are elsewhere now.
    #[serde(default)]
    pub moved: u32,
}

/// A stat the mod changes: `stat` of `tag`, `from` and `to` as shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatChange {
    pub tag: String,
    pub stat: String,
    pub from: String,
    pub to: String,
}

/// What a mod contains, read off the patched map against its base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    /// The report in words, most telling first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<ClassChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swaps: Vec<Swap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<StatChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placements: Vec<PlacementChange>,
    /// The scenario's scripts differ.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scripts: bool,
}

fn text_ok(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

impl Contents {
    pub fn is_empty(&self) -> bool {
        self == &Contents::default()
    }

    /// The same report cut to the limits: each list to its first
    /// [`MAX_CONTENT_ENTRIES`], lines to [`MAX_CONTENT_LINES`] (the last
    /// saying how many more there were), texts to [`MAX_CONTENT_TEXT`].
    pub fn bounded(mut self) -> Self {
        fn cut(s: &mut String) {
            if s.chars().count() > MAX_CONTENT_TEXT {
                *s = s.chars().take(MAX_CONTENT_TEXT - 3).collect::<String>() + "...";
            }
            s.retain(|c| !c.is_control());
        }
        if self.lines.len() > MAX_CONTENT_LINES {
            let more = self.lines.len() - (MAX_CONTENT_LINES - 1);
            self.lines.truncate(MAX_CONTENT_LINES - 1);
            self.lines.push(format!("... and {more} more"));
        }
        self.lines.iter_mut().for_each(cut);
        self.classes.truncate(MAX_CONTENT_ENTRIES);
        self.swaps.truncate(MAX_CONTENT_ENTRIES);
        self.stats.truncate(MAX_CONTENT_ENTRIES);
        self.placements.truncate(MAX_CONTENT_ENTRIES);
        for c in &mut self.classes {
            cut(&mut c.group);
            cut(&mut c.label);
            c.added_names.truncate(8);
            c.added_names.iter_mut().for_each(cut);
        }
        for s in &mut self.swaps {
            for t in [&mut s.kind, &mut s.tag, &mut s.field] {
                cut(t);
            }
            for t in [&mut s.from, &mut s.to].into_iter().flatten() {
                cut(t);
            }
        }
        for s in &mut self.stats {
            for t in [&mut s.tag, &mut s.stat, &mut s.from, &mut s.to] {
                cut(t);
            }
        }
        for p in &mut self.placements {
            cut(&mut p.feature);
            cut(&mut p.tag);
        }
        self
    }

    /// Within the limits [`Self::bounded`] cuts to.
    pub fn validate(&self) -> Result<(), String> {
        if self.lines.len() > MAX_CONTENT_LINES {
            return Err(format!("at most {MAX_CONTENT_LINES} lines"));
        }
        if [
            self.classes.len(),
            self.swaps.len(),
            self.stats.len(),
            self.placements.len(),
        ]
        .iter()
        .any(|n| *n > MAX_CONTENT_ENTRIES)
        {
            return Err(format!(
                "at most {MAX_CONTENT_ENTRIES} entries in each list"
            ));
        }
        let mut texts: Vec<&str> = self.lines.iter().map(String::as_str).collect();
        for c in &self.classes {
            texts.extend([c.group.as_str(), c.label.as_str()]);
            texts.extend(c.added_names.iter().map(String::as_str));
        }
        for s in &self.swaps {
            texts.extend([s.kind.as_str(), s.tag.as_str(), s.field.as_str()]);
            texts.extend(s.from.iter().chain(&s.to).map(String::as_str));
        }
        for s in &self.stats {
            texts.extend([
                s.tag.as_str(),
                s.stat.as_str(),
                s.from.as_str(),
                s.to.as_str(),
            ]);
        }
        for p in &self.placements {
            texts.extend([p.feature.as_str(), p.tag.as_str()]);
        }
        if let Some(t) = texts.iter().find(|t| !text_ok(t, MAX_CONTENT_TEXT)) {
            return Err(format!(
                "{t:?}: report texts are at most {MAX_CONTENT_TEXT} characters on one line"
            ));
        }
        Ok(())
    }
}

/// What an admin says about a package beyond its manifest. Every part is
/// optional; a details with nothing in it clears the admin's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Details {
    /// A longer description, shown instead of the manifest's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Screenshots, first the one to show when the package has no
    /// preview of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MapImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contents: Option<Contents>,
    /// What it changes ([`crate::maps::FEATURES`]), shown instead of the
    /// published list; None keeps that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    /// The pack it came out in, by id ([`pack_id_ok`]); its info is the
    /// pack's own ([`PackInfo`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
}

/// The checks of a list of images.
fn validate_images(images: &[MapImage]) -> Result<(), String> {
    if images.len() > MAX_DETAIL_IMAGES {
        return Err(format!("at most {MAX_DETAIL_IMAGES} images"));
    }
    for (n, i) in images.iter().enumerate() {
        if images[..n].iter().any(|o| o.sha256 == i.sha256) {
            return Err(format!("image {} is listed twice", i.sha256));
        }
        for (what, t) in [
            ("caption", &i.caption),
            ("credit", &i.credit),
            ("licence", &i.licence),
        ] {
            if let Some(t) = t
                && (t.trim().is_empty() || !text_ok(t, MAX_CAPTION))
            {
                return Err(format!(
                    "image {what} must be 1 to {MAX_CAPTION} characters on one line"
                ));
            }
        }
        if let Some(u) = &i.source_url
            && !crate::maps::source_url_ok(u)
        {
            return Err(format!(
                "image source_url must be an http or https URL of at most {} characters",
                crate::maps::MAX_SOURCE_URL
            ));
        }
    }
    Ok(())
}

fn tidy_text(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn tidy_images(images: Vec<MapImage>) -> Vec<MapImage> {
    images
        .into_iter()
        .map(|i| MapImage {
            sha256: i.sha256,
            reportable: false,
            caption: tidy_text(i.caption),
            source_url: tidy_text(i.source_url),
            credit: tidy_text(i.credit),
            licence: tidy_text(i.licence),
        })
        .collect()
}

fn description_ok(d: &str) -> bool {
    !d.trim().is_empty()
        && d.chars().count() <= MAX_DETAILS_DESCRIPTION
        && !d.chars().any(|c| c.is_control() && c != '\n')
}

/// A map pack: what it is, who made it, where and when it came out, and
/// its images. Its maps name it by id ([`Details::pack`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Who made or compiled it, as the release credits them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MapImage>,
}

impl PackInfo {
    pub fn tidy(self) -> Self {
        Self {
            name: self.name.trim().to_string(),
            description: tidy_text(self.description),
            authors: crate::maps::Credit::authors(self.authors).authors,
            released: tidy_text(self.released),
            source_url: tidy_text(self.source_url),
            images: tidy_images(self.images),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_PACK_NAME
            || self.name.chars().any(char::is_control)
        {
            return Err(format!(
                "name must be 1 to {MAX_PACK_NAME} characters on one line"
            ));
        }
        if let Some(d) = &self.description
            && !description_ok(d)
        {
            return Err(format!(
                "description must be 1 to {MAX_DETAILS_DESCRIPTION} characters"
            ));
        }
        crate::maps::Credit {
            authors: self.authors.clone(),
            released: self.released.clone(),
            source_url: self.source_url.clone(),
            ..crate::maps::Credit::default()
        }
        .validate()?;
        validate_images(&self.images)
    }
}

/// `MapPackSet` data. `pack` None: the pack's info is cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSet {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<PackInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::Xuid>,
}

/// A pack as listed (`GET /v1/mappacks`, a map list's `packs`, a map's
/// `pack_info`): its id and info. Its images are served at
/// `/v1/mappacks/{id}/images/{n}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackListed {
    pub id: String,
    #[serde(flatten)]
    pub info: PackInfo,
}

impl Details {
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.images.is_empty()
            && self.contents.as_ref().is_none_or(Contents::is_empty)
            && self.features.is_none()
            && self.pack.is_none()
    }

    /// Texts trimmed and blank ones dropped; features in the checklist's
    /// order; an empty report dropped.
    pub fn tidy(self) -> Self {
        Self {
            description: tidy_text(self.description),
            images: tidy_images(self.images),
            contents: self.contents.filter(|c| !c.is_empty()),
            features: self.features,
            pack: tidy_text(self.pack),
        }
    }

    /// The checks an admin's details pass (the images' bytes are the
    /// server's to check).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(d) = &self.description
            && !description_ok(d)
        {
            return Err(format!(
                "description must be 1 to {MAX_DETAILS_DESCRIPTION} characters"
            ));
        }
        validate_images(&self.images)?;
        if let Some(p) = &self.pack
            && !pack_id_ok(p)
        {
            return Err(format!(
                "pack {p:?} must be 1 to {MAX_PACK_ID} of a-z, 0-9 and -"
            ));
        }
        if let Some(c) = &self.contents {
            c.validate().map_err(|e| format!("contents: {e}"))?;
        }
        if let Some(f) = &self.features {
            crate::maps::known_ids(f, &crate::maps::FEATURES)
                .map_err(|e| format!("features: {e}"))?;
        }
        Ok(())
    }
}

/// `MapDetailed` data. `details` None: the admin's details are cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detailed {
    pub map_id: MapId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Details>,
    /// The admin's account (None: the operator's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::Xuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(n: u8) -> MapImage {
        MapImage {
            sha256: Sha256([n; 32]),
            reportable: false,
            caption: Some("the hog".into()),
            source_url: None,
            credit: None,
            licence: None,
        }
    }

    #[test]
    fn details_read_tidy_and_are_checked() {
        let d: Details = serde_json::from_value(json!({
            "description": "  A long one.\nTwo lines.  ",
            "images": [{"sha256": "01".repeat(32), "caption": " shot ", "credit": " ",
                        "source_url": "https://example.invalid/p"}],
            "features": ["vehicles"],
            "contents": {}
        }))
        .unwrap();
        let d = d.tidy();
        assert_eq!(d.description.as_deref(), Some("A long one.\nTwo lines."));
        assert_eq!(d.images[0].caption.as_deref(), Some("shot"));
        assert_eq!(d.images[0].credit, None, "a blank credit is dropped");
        assert_eq!(d.contents, None, "an empty report is dropped");
        assert!(d.validate().is_ok());
        assert!(Details::default().is_empty() && !d.is_empty());
        assert!(serde_json::from_value::<Details>(json!({"blurb": "x"})).is_err());

        let bad = |f: &dyn Fn(&mut Details)| {
            let mut x = d.clone();
            f(&mut x);
            x.validate().is_err()
        };
        assert!(bad(
            &|x| x.description = Some("d".repeat(MAX_DETAILS_DESCRIPTION + 1))
        ));
        assert!(bad(&|x| x.description = Some("bell\u{7}".into())));
        assert!(bad(
            &|x| x.images = (0..=MAX_DETAIL_IMAGES as u8).map(image).collect()
        ));
        assert!(bad(&|x| x.images = vec![image(1), image(1)]), "twice");
        assert!(bad(&|x| x.images[0].caption = Some("a\nb".into())));
        assert!(bad(
            &|x| x.images[0].source_url = Some("javascript:1".into())
        ));
        assert!(bad(&|x| x.features = Some(vec!["cheats".into()])));
        assert!(bad(&|x| {
            x.contents = Some(Contents {
                lines: vec!["l".into(); MAX_CONTENT_LINES + 1],
                ..Contents::default()
            })
        }));
    }

    #[test]
    fn packs_are_named_by_id_and_checked() {
        for ok in ["phantom-mappack", "ai-revision", "ppf-noobs-v2", "doorm4n"] {
            assert!(pack_id_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "Phantom",
            "a b",
            "a_b",
            &"a".repeat(MAX_PACK_ID + 1),
        ] {
            assert!(!pack_id_ok(bad), "{bad}");
        }
        assert_eq!(pack_stream("ai-revision"), "mappack-ai-revision");
        let d = Details {
            pack: Some(" ai-revision ".into()),
            ..Details::default()
        }
        .tidy();
        assert_eq!(d.pack.as_deref(), Some("ai-revision"));
        assert!(d.validate().is_ok() && !d.is_empty());
        let bad = Details {
            pack: Some("AI Revision".into()),
            ..Details::default()
        };
        assert!(bad.validate().is_err());

        let p: PackInfo = serde_json::from_value(json!({
            "name": " Halo 2 Remapped AI Revision ", "authors": ["Zone 117", " "],
            "released": "2007-06", "source_url": "https://example.invalid/pack",
            "description": "Custom maps with AI.",
            "images": [{"sha256": "01".repeat(32), "caption": "the menu"}]
        }))
        .unwrap();
        let p = p.tidy();
        assert_eq!(p.name, "Halo 2 Remapped AI Revision");
        assert_eq!(p.authors, ["Zone 117"]);
        assert!(p.validate().is_ok());
        let listed = PackListed {
            id: "ai-revision".into(),
            info: p.clone(),
        };
        let v = serde_json::to_value(&listed).unwrap();
        assert_eq!(v["id"], "ai-revision");
        assert_eq!(v["name"], "Halo 2 Remapped AI Revision");
        assert_eq!(serde_json::from_value::<PackListed>(v).unwrap(), listed);
        let edits: [&dyn Fn(&mut PackInfo); 4] = [
            &|p: &mut PackInfo| p.name = " ".to_string(),
            &|p: &mut PackInfo| p.released = Some("06/2007".into()),
            &|p: &mut PackInfo| p.source_url = Some("ftp://x".into()),
            &|p: &mut PackInfo| p.images.push(p.images[0].clone()),
        ];
        for f in edits {
            let mut q = p.clone();
            f(&mut q);
            assert!(q.validate().is_err(), "{q:?}");
        }
    }

    #[test]
    fn a_report_is_bounded_to_what_validates() {
        let c = Contents {
            lines: (0..200).map(|i| format!("line {i}")).collect(),
            swaps: vec![
                Swap {
                    kind: "projectile".into(),
                    tag: "t".repeat(1000),
                    field: "f".into(),
                    from: None,
                    to: Some("x\ny".into()),
                };
                MAX_CONTENT_ENTRIES + 5
            ],
            ..Contents::default()
        };
        assert!(c.validate().is_err());
        let b = c.bounded();
        assert!(b.validate().is_ok(), "{:?}", b.validate());
        assert_eq!(b.lines.len(), MAX_CONTENT_LINES);
        assert_eq!(
            b.lines.last().unwrap(),
            &format!("... and {} more", 200 - (MAX_CONTENT_LINES - 1))
        );
        assert_eq!(b.swaps.len(), MAX_CONTENT_ENTRIES);
        assert_eq!(b.swaps[0].to.as_deref(), Some("xy"));
    }

    #[test]
    fn a_report_round_trips_and_says_nothing_it_lacks() {
        let c = Contents {
            lines: vec!["projectile swap: battle rifle fires fuel rod shots".into()],
            swaps: vec![Swap {
                kind: "projectile".into(),
                tag: "objects\\weapons\\rifle\\battle_rifle\\battle_rifle".into(),
                field: "barrels[0].projectile".into(),
                from: Some("bullet".into()),
                to: Some("fuel_rod".into()),
            }],
            scripts: true,
            ..Contents::default()
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("classes").is_none() && v.get("stats").is_none());
        assert_eq!(serde_json::from_value::<Contents>(v).unwrap(), c);
        let v = serde_json::to_value(Contents::default()).unwrap();
        assert_eq!(v, json!({}));
    }
}
