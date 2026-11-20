//! 1.0's multiplayer map list entry for a new map: a version 2 `patch.lvl`
//! built from a package's manifest (docs/game/maps.md §4, layout in
//! docs/game/map-pack.md 8.1).
//!
//! Field sources:
//!
//! | field | from |
//! |---|---|
//! | type | 1, the only one 1.0 applies |
//! | scenario path | `scenarios\multi\<map>\<map>`; 1.0 uses only the basename, and opens `<content dir>\<map>.map` |
//! | map id | [`list_ids`]: from the `MapId`, in [`LIST_ID_FIRST`]..=[`LIST_ID_LAST`] |
//! | `+0x110` | `-1`, as in every shipped record; 1.0 never reads it |
//! | names | `[list] name.<lang>`, else `[package] name`, in all nine languages |
//! | descriptions | `[list] description.<lang>`, else `[package] description`, else empty |
//! | sort order | `[list] sort`, else [`DEFAULT_SORT`] (after every retail and Map Pack map) |
//! | flags, team counts | the `[list] base` map's own entry in `mainmenu.map`; else the installer passes the list's first retail entry (lowest sort order); with no entry at all, [`DEFAULT_FLAGS`] and [`DEFAULT_TEAMS`] |
//! | preview | the package image, else the base entry's preview, else [`generated_card`] |
//!
//! The preview is 207 rows of 220 pixels, 32 bits each, top row first, in
//! the D3D `A8R8G8B8` linear layout (bytes B, G, R, A). Every shipped
//! record's preview has the same alpha: a one-pixel opaque frame, `0xbf`
//! inside, and a transparent triangle cut from the bottom-right corner;
//! [`preview_alpha`] draws that shape.
use crate::{
    Manifest, MapId,
    manifest::{LANGUAGES, ListInfo},
    patch_lvl::{
        self, CRC, KIND, KIND_MAP, LvlError, SCENARIO, SCENARIO_LEN, V2_LEN, V2_MAP_ID, VERSION,
    },
};

const UNUSED: usize = 0x110;
const NAMES: usize = 0x114;
const NAME_UNITS: usize = 32;
const DESCRIPTIONS: usize = 0x354;
const DESCRIPTION_UNITS: usize = 128;
const SORT: usize = 0xc54;
const FLAGS: usize = 0xc58;
const TEAMS: usize = 0xc5c;
const PREVIEW: usize = 0xc6c;

/// Preview width and height in pixels.
pub const PREVIEW_W: usize = 220;
pub const PREVIEW_H: usize = 207;
/// Bytes of one preview: 207 rows of `0x370`.
pub const PREVIEW_LEN: usize = PREVIEW_W * PREVIEW_H * 4;
const _: () = assert!(PREVIEW + PREVIEW_LEN == V2_LEN);

/// One element of `mainmenu.map`'s multiplayer level list (globals `+0x178`,
/// element 0, block at `+0x10`).
pub const SLOT_LEN: usize = 0xc64;
const SLOT_PATH: usize = 0xb4c;
const SLOT_SORT: usize = 0xc4c;
const SLOT_FLAGS: usize = 0xc50;
const SLOT_TEAMS: usize = 0xc54;

/// Sort order of a new map without `[list] sort`: after the retail maps and
/// the Map Pack's (whose largest is in the low hundreds).
pub const DEFAULT_SORT: i32 = 1000;
/// Flags without a base: none set, as on every Map Pack record.
pub const DEFAULT_FLAGS: u8 = 0;
/// Per-game-type team counts when no retail entry can be read at all (the
/// installer normally copies one from the user's `mainmenu.map`).
pub const DEFAULT_TEAMS: [u8; 16] = [8; 16];

/// The map ids new maps take: above every retail and Map Pack id (those are
/// small numbers, in the thousands at most), and below `0x8000_0000` so the
/// id stays positive as a signed word and leaves the top bit for the offer
/// ID ([`offer_content`]).
pub const LIST_ID_FIRST: u32 = 0x0001_0000;
pub const LIST_ID_LAST: u32 = 0x7fff_fffe;

/// The low half of the offer ID of the package holding the map with list id
/// `id`: its top bit set, so it never meets the Map Pack's `0x0101..0x03ff`
/// (docs/game/map-pack.md §5) or a disc package number.
pub fn offer_content(id: u32) -> u32 {
    0x8000_0000 | id
}

/// Whether `low` (an offer ID's low half) is one [`offer_content`] makes.
pub fn is_new_map_offer(low: u32) -> bool {
    low & 0x8000_0000 != 0 && (LIST_ID_FIRST..=LIST_ID_LAST).contains(&(low & 0x7fff_ffff))
}

/// The map id a `MapId` would like: its first four bytes, folded into the
/// range.
pub fn preferred_id(id: &MapId) -> u32 {
    let n = u32::from_le_bytes(id.0[..4].try_into().unwrap());
    LIST_ID_FIRST + n % (LIST_ID_LAST - LIST_ID_FIRST + 1)
}

/// A map id for each of `ids`, in the same order: each its
/// [`preferred_id`], or on a collision with `taken` (retail ids) or an
/// earlier choice the next free one up, wrapping in the range. Choices are
/// made in `MapId` order, so they don't depend on the order of `ids`.
pub fn list_ids(ids: &[MapId], taken: &[u32]) -> Vec<u32> {
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by_key(|&i| ids[i]);
    let mut used: std::collections::HashSet<u32> = taken.iter().copied().collect();
    let mut out = vec![0; ids.len()];
    for i in order {
        let mut id = preferred_id(&ids[i]);
        while used.contains(&id) {
            id = if id == LIST_ID_LAST {
                LIST_ID_FIRST
            } else {
                id + 1
            };
        }
        used.insert(id);
        out[i] = id;
    }
    out
}

/// The fields a new map takes from a retail entry of the map list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseEntry {
    pub map_id: u32,
    /// The basename of its scenario path: the map's name.
    pub map: String,
    pub sort: i32,
    pub flags: u8,
    pub teams: [u8; 16],
}

impl BaseEntry {
    /// Reads one `mainmenu.map` level list element. `None` for an empty
    /// slot (map id `-1`) or a short one.
    pub fn from_slot(slot: &[u8]) -> Option<Self> {
        let slot = slot.get(..SLOT_LEN)?;
        let map_id = u32_at(slot, 0);
        if map_id == u32::MAX {
            return None;
        }
        let path = &slot[SLOT_PATH..SLOT_PATH + SCENARIO_LEN];
        let path = &path[..path.iter().position(|&b| b == 0).unwrap_or(path.len())];
        let path = String::from_utf8_lossy(path);
        let map = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_owned();
        Some(Self {
            map_id,
            map,
            sort: u32_at(slot, SLOT_SORT) as i32,
            flags: slot[SLOT_FLAGS],
            teams: slot[SLOT_TEAMS..SLOT_TEAMS + 16].try_into().unwrap(),
        })
    }
}

/// A v2 record's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub scenario: String,
    pub map_id: u32,
    pub names: [String; 9],
    pub descriptions: [String; 9],
    pub sort: i32,
    pub flags: u8,
    pub teams: [u8; 16],
    /// [`PREVIEW_LEN`] bytes.
    pub preview: Vec<u8>,
}

impl Entry {
    /// The entry for `manifest`'s map, with list id `map_id`, the retail
    /// entry to take flags and team counts from (its `[list] base`'s, or
    /// any other), and its preview.
    pub fn for_manifest(
        manifest: &Manifest,
        map_id: u32,
        base: Option<&BaseEntry>,
        preview: Vec<u8>,
    ) -> Self {
        let list = manifest.list.clone().unwrap_or_default();
        let map = &manifest.base.map;
        let text =
            |own: &std::collections::BTreeMap<String, String>, fallback: &str| -> [String; 9] {
                LANGUAGES.map(|l| own.get(l).cloned().unwrap_or_else(|| fallback.to_owned()))
            };
        let ListInfo {
            sort,
            name,
            description,
            ..
        } = &list;
        Self {
            scenario: format!("scenarios\\multi\\{map}\\{map}"),
            map_id,
            names: text(name, &manifest.package.name),
            descriptions: text(
                description,
                manifest.package.description.as_deref().unwrap_or(""),
            ),
            sort: sort.unwrap_or(DEFAULT_SORT),
            flags: base.map_or(DEFAULT_FLAGS, |b| b.flags),
            teams: base.map_or(DEFAULT_TEAMS, |b| b.teams),
            preview,
        }
    }

    /// The version 2 `patch.lvl`: one record that passes 1.0's check.
    pub fn to_v2(&self) -> Result<Vec<u8>, LvlError> {
        if !self.scenario.is_ascii()
            || self.scenario.len() >= SCENARIO_LEN
            || self.scenario.contains('\0')
        {
            return Err(LvlError(format!(
                "scenario path {:?} does not fit",
                self.scenario
            )));
        }
        if self.preview.len() != PREVIEW_LEN {
            return Err(LvlError(format!(
                "preview is {} bytes, not {PREVIEW_LEN}",
                self.preview.len()
            )));
        }
        let mut r = vec![0u8; V2_LEN];
        put(&mut r, VERSION, 2);
        put(&mut r, KIND, KIND_MAP);
        r[SCENARIO..SCENARIO + self.scenario.len()].copy_from_slice(self.scenario.as_bytes());
        put(&mut r, V2_MAP_ID, self.map_id);
        put(&mut r, UNUSED, u32::MAX);
        for (i, name) in self.names.iter().enumerate() {
            utf16_into(&mut r[NAMES + i * NAME_UNITS * 2..][..NAME_UNITS * 2], name);
        }
        for (i, d) in self.descriptions.iter().enumerate() {
            utf16_into(
                &mut r[DESCRIPTIONS + i * DESCRIPTION_UNITS * 2..][..DESCRIPTION_UNITS * 2],
                d,
            );
        }
        put(&mut r, SORT, self.sort as u32);
        r[FLAGS] = self.flags;
        r[TEAMS..TEAMS + 16].copy_from_slice(&self.teams);
        r[PREVIEW..].copy_from_slice(&self.preview);
        let crc = patch_lvl::record_crc(&r);
        put(&mut r, CRC, crc);
        patch_lvl::validate_v2(&r)?;
        Ok(r)
    }

    /// Reads a v2 record back (for checks).
    pub fn from_v2(r: &[u8]) -> Result<Self, LvlError> {
        patch_lvl::validate_v2(r)?;
        let path = &r[SCENARIO..SCENARIO + SCENARIO_LEN];
        let path = &path[..path.iter().position(|&b| b == 0).unwrap_or(path.len())];
        Ok(Self {
            scenario: String::from_utf8_lossy(path).into_owned(),
            map_id: u32_at(r, V2_MAP_ID),
            names: std::array::from_fn(|i| {
                utf16_from(&r[NAMES + i * NAME_UNITS * 2..][..NAME_UNITS * 2])
            }),
            descriptions: std::array::from_fn(|i| {
                utf16_from(&r[DESCRIPTIONS + i * DESCRIPTION_UNITS * 2..][..DESCRIPTION_UNITS * 2])
            }),
            sort: u32_at(r, SORT) as i32,
            flags: r[FLAGS],
            teams: r[TEAMS..TEAMS + 16].try_into().unwrap(),
            preview: r[PREVIEW..].to_vec(),
        })
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn put(r: &mut [u8], at: usize, v: u32) {
    r[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// `text` as UTF-16LE into `field`, cut at a character boundary so a
/// terminating zero always fits.
fn utf16_into(field: &mut [u8], text: &str) {
    let room = field.len() / 2 - 1;
    let mut units = Vec::new();
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let enc = c.encode_utf16(&mut buf);
        if units.len() + enc.len() > room {
            break;
        }
        units.extend_from_slice(enc);
    }
    field.fill(0);
    for (i, u) in units.iter().enumerate() {
        field[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
}

fn utf16_from(field: &[u8]) -> String {
    let units: Vec<u16> = field
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

// ---------------------------------------------------------------------------
// The preview.

/// The alpha every shipped preview has at `(x, y)`: transparent in a
/// 16-pixel triangle cut from the bottom-right corner, opaque on the
/// one-pixel frame (the cut's edge included), `0xbf` inside.
pub fn preview_alpha(x: usize, y: usize) -> u8 {
    const CUT: usize = (PREVIEW_W - 1) + (PREVIEW_H - 1) - 16;
    if x + y > CUT {
        0
    } else if x == 0 || y == 0 || x == PREVIEW_W - 1 || y == PREVIEW_H - 1 || x + y == CUT {
        0xff
    } else {
        0xbf
    }
}

/// `preview` in a retail preview's frame: each pixel on the one-pixel
/// frame ([`preview_alpha`] opaque) takes `retail`'s colour there. A
/// retail preview's frame is in its pixels (a pale line the Select Map
/// dialog shows as the picture's border); one made from an image or a
/// [`generated_card`] has the image's own colours there, so it shows no
/// border. Only the user's disc's pixels go in: nothing of the frame is
/// kept here. Either of the wrong length: `preview` as it was.
pub fn with_frame(preview: &mut [u8], retail: &[u8]) {
    if preview.len() != PREVIEW_LEN || retail.len() != PREVIEW_LEN {
        return;
    }
    for (i, (p, r)) in preview
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(retail.as_chunks::<4>().0)
        .enumerate()
    {
        if preview_alpha(i % PREVIEW_W, i / PREVIEW_W) == 0xff {
            p[..3].copy_from_slice(&r[..3]);
            p[3] = 0xff;
        }
    }
}

/// A PNG or JPEG decoded to 8-bit RGBA: `(width, height, pixels)`.
pub fn decode_image(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    match crate::manifest::image_signature(bytes) {
        Some("png") => decode_png(bytes),
        Some(_) => decode_jpeg(bytes),
        None => Err("not a PNG or JPEG file".into()),
    }
}

fn decode_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (r.info().width as usize, r.info().height as usize);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(format!("{w}x{h} is not a preview size"));
    }
    let mut buf = vec![0; r.output_buffer_size().ok_or("image too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0xff])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 0xff]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    };
    Ok((w, h, rgba))
}

fn decode_jpeg(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), opts);
    let px = dec.decode().map_err(|e| e.to_string())?;
    let info = dec.info().ok_or("no JPEG header")?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if px.len() != w * h * 4 {
        return Err(format!("decoded {} bytes for {w}x{h} RGBA", px.len()));
    }
    Ok((w, h, px))
}

/// An RGBA image scaled to cover 220x207 and cropped to its centre, as a
/// preview (bilinear, then [`preview_alpha`]).
pub fn preview_from_rgba(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), w * h * 4);
    let scale = (PREVIEW_W as f64 / w as f64).max(PREVIEW_H as f64 / h as f64);
    let (sw, sh) = (PREVIEW_W as f64 / scale, PREVIEW_H as f64 / scale);
    let (x0, y0) = ((w as f64 - sw) / 2.0, (h as f64 - sh) / 2.0);
    let at = |x: usize, y: usize, c: usize| f64::from(rgba[(y * w + x) * 4 + c]);
    let mut out = vec![0u8; PREVIEW_LEN];
    for y in 0..PREVIEW_H {
        let fy = (y0 + (y as f64 + 0.5) / scale - 0.5).clamp(0.0, (h - 1) as f64);
        let (ya, ty) = (fy.floor() as usize, fy.fract());
        let yb = (ya + 1).min(h - 1);
        for x in 0..PREVIEW_W {
            let fx = (x0 + (x as f64 + 0.5) / scale - 0.5).clamp(0.0, (w - 1) as f64);
            let (xa, tx) = (fx.floor() as usize, fx.fract());
            let xb = (xa + 1).min(w - 1);
            let px = |c| {
                let top = at(xa, ya, c) * (1.0 - tx) + at(xb, ya, c) * tx;
                let bot = at(xa, yb, c) * (1.0 - tx) + at(xb, yb, c) * tx;
                (top * (1.0 - ty) + bot * ty).round() as u8
            };
            let o = (y * PREVIEW_W + x) * 4;
            out[o..o + 4].copy_from_slice(&[px(2), px(1), px(0), preview_alpha(x, y)]);
        }
    }
    out
}

/// A plain card for a map with neither an image nor a base: a vertical
/// gradient in a colour taken from its `MapId`.
pub fn generated_card(id: &MapId) -> Vec<u8> {
    let [r, g, b] = [id.0[0], id.0[1], id.0[2]].map(|c| 0x30 + c / 3);
    let mut out = vec![0u8; PREVIEW_LEN];
    for y in 0..PREVIEW_H {
        let shade =
            |c: u8| (u32::from(c) * (PREVIEW_H + 96 - y) as u32 / (PREVIEW_H + 96) as u32) as u8;
        for x in 0..PREVIEW_W {
            let o = (y * PREVIEW_W + x) * 4;
            out[o..o + 4].copy_from_slice(&[shade(b), shade(g), shade(r), preview_alpha(x, y)]);
        }
    }
    out
}

/// The preview for `manifest`: its image (in `dir`) if it has one, else
/// `base` (already preview pixels), else a [`generated_card`].
pub fn preview_for(
    manifest: &Manifest,
    dir: &std::path::Path,
    base: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    if let Some(image) = &manifest.package.image {
        let path = dir.join(crate::manifest::relative(image).map_err(|e| e.to_string())?);
        let bytes = std::fs::read(&path).map_err(|e| format!("{image}: {e}"))?;
        let (w, h, rgba) = decode_image(&bytes).map_err(|e| format!("{image}: {e}"))?;
        return Ok(preview_from_rgba(w, h, &rgba));
    }
    if let Some(px) = base.filter(|p| p.len() == PREVIEW_LEN) {
        return Ok(px);
    }
    Ok(generated_card(&manifest.map_id()))
}

#[cfg(test)]
mod tests;
