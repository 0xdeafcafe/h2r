//! Where the converted Halo 1 HUD lands on screen, against where Halo 1
//! puts it, from the discs' own tags (docs/halo1/hud.md, "Layout").
//!
//! Each element's rectangle is computed as Halo 2 places the widget the
//! converter writes (`h1hud::halo2_rect`, E `0x22aff1`) and as Halo 1
//! places the element (`h1hud::halo1_rect`, E Halo 1 `0x000e07a0`,
//! `0x000e0330`); the texels inside are the bitmaps', decoded. Prints names
//! and counts only.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test it hud_layout_disc:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::bitmap::Bitmap;
use h1map::{Group, H1Map};
use h1objects::h1hud::{self, Anchor, Element, Rect, Role};
use map::TagId;

fn iso() -> xiso::Xiso<BufReader<File>> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap()
}

fn h1(name: &str) -> H1Map {
    let mut iso = iso();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name).expect("map on the disc").clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_file(suffix: &str) -> Vec<u8> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.replace('\\', "/").ends_with(suffix))
        .unwrap_or_else(|| panic!("{suffix} on the Halo 2 disc"));
    iso.read(&e).unwrap()
}

/// An element as drawn: its rectangle and its texels (RGBA, rows of the
/// rectangle's width).
struct Drawn {
    rect: Rect,
    rgba: Vec<[u8; 4]>,
}

impl Drawn {
    fn at(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        let [x0, y0, x1, y1] = self.rect;
        if x < x0 || y < y0 || x >= x1 || y >= y1 {
            return None;
        }
        Some(self.rgba[((y - y0) * (x1 - x0) + (x - x0)) as usize])
    }

    /// The bounds of the texels with alpha.
    fn ink(&self) -> Rect {
        let [x0, y0, x1, y1] = self.rect;
        let mut r = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for y in y0..y1 {
            for x in x0..x1 {
                if self.at(x, y).unwrap()[3] > 0 {
                    r = [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)];
                }
            }
        }
        r
    }
}

/// The sprite of `bitmap`'s sequence `seq`, decoded, its texels inside the
/// sprite's bounds; with `meter`, after the converter's meter copy
/// (`h1hud::clear_meter_colour`).
fn texels(m: &H1Map, bitmap: h1map::TagId, seq: i16, meter: bool) -> ((i32, i32), Vec<[u8; 4]>) {
    let bm = Bitmap::read(m, m.tag(bitmap).unwrap()).unwrap();
    let (b, _) = h1hud::sprite(&bm, seq).unwrap();
    let s = &bm.sequences[seq as usize];
    let i = s.sprites.first().map_or(s.first_image, |p| p.image) as usize;
    let mut img = bm.images[i].clone();
    let mut px = bm.pixels(m, i).unwrap().to_vec();
    if meter && h1hud::clear_meter_colour(img.format, &mut px).is_none() {
        // As the converter's copy: rewritten as linear A8R8G8B8 first.
        let (argb, mut out) = h1hud::alpha_scaled_argb(&img, &px, 0xff).unwrap();
        h1hud::clear_meter_colour(argb.format, &mut out).unwrap();
        (img, px) = (argb, out);
    }
    let top = h1_assets::bitmaps::decode_image(&img, &px)
        .unwrap()
        .into_iter()
        .find(|l| l.face == 0 && l.level == 0)
        .unwrap();
    let (w, h) = (top.width as usize, top.height as usize);
    let (x0, x1) = ((b[0] * w as f32).round() as usize, (b[1] * w as f32).round() as usize);
    let (y0, y1) = ((b[2] * h as f32).round() as usize, (b[3] * h as f32).round() as usize);
    let size = h1hud::sprite_size(&bm, seq).unwrap();
    assert_eq!(size, ((x1 - x0) as i32, (y1 - y0) as i32));
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let t = &top.rgba[(y * w + x) * 4..][..4];
            out.push([t[0], t[1], t[2], t[3]]);
        }
    }
    (size, out)
}

/// Element `e` as Halo 2 draws the converter's widget, checked against
/// where Halo 1 draws it.
fn drawn(m: &H1Map, e: &Element, meter: bool) -> Drawn {
    let (size, rgba) = texels(m, e.bitmap, e.sequence, meter);
    let w = h1hud::widget_for(e, TagId(1), h1hud::registration(e.anchor), TagId(2), TagId(3));
    let rect = h1hud::halo2_rect(&w, size);
    // Elements hold Halo 2's offset; Halo 1's is the same flip back.
    let halo1 = h1hud::halo1_rect(e.anchor, h1hud::to_halo2_offset(e.anchor, e.offset), size);
    assert_eq!(rect, halo1, "{}: Halo 2's rectangle against Halo 1's", e.name);
    Drawn { rect, rgba }
}

/// Texels where `a` and `b` both show.
fn overlap(a: &Drawn, b: &Drawn, show: impl Fn([u8; 4]) -> bool) -> usize {
    let [x0, y0, x1, y1] = a.rect;
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if let (Some(ta), Some(tb)) = (a.at(x, y), b.at(x, y)) {
                n += usize::from(show(ta) && show(tb));
            }
        }
    }
    n
}

/// What shows of a Halo 1 element: its alpha (Halo 1's meter and element
/// draws, E `0x000e20a0`, `0x000e1830`).
fn halo1_shows(t: [u8; 4]) -> bool {
    t[3] > 0
}

/// What shows of a converted meter, full (the shader's filled side,
/// `h1hud::legacy_meter_alpha`).
fn halo2_meter_shows(t: [u8; 4]) -> bool {
    h1hud::legacy_meter_alpha(t, true) > 0
}

/// Every unit HUD on the disc: each element where Halo 1 puts it, and the
/// shield meter and the health meter showing no texel in common, full, as
/// Halo 1 lays them out. Without the meter copy the shield meter's clear
/// texels that carry a luminance show over the health meter.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn shield_and_health_meters_do_not_overlap() {
    let mut iso = iso();
    let entries = h1map::disc::maps(&iso.entries().unwrap());
    let mut checked = 0;
    for e in entries {
        let name = h1map::disc::map_stem(&e).to_owned();
        let Ok(m) = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())) else {
            continue;
        };
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"unhi")) {
            let els = h1hud::unit_elements(&m, m.tag_data(t).unwrap()).unwrap();
            for el in &els {
                drawn(&m, el, matches!(el.role, Role::Meter(_)));
            }
            let find = |n: &str| els.iter().find(|e| e.name == n);
            let (Some(shield), Some(health)) = (find("h1_shield_meter"), find("h1_health_meter"))
            else {
                continue;
            };
            let (s1, h1m) = (drawn(&m, shield, false), drawn(&m, health, false));
            assert_eq!(overlap(&s1, &h1m, halo1_shows), 0, "{name} {}: Halo 1's own", t.name);
            let before = overlap(&s1, &h1m, halo2_meter_shows);
            let (s2, h2m) = (drawn(&m, shield, true), drawn(&m, health, true));
            let after = overlap(&s2, &h2m, halo2_meter_shows);
            if name == "bloodgulch" {
                eprintln!(
                    "{name} {}: shield {:?} ink {:?}, health {:?} ink {:?}; both full, shown in common: {before} from Halo 1's bitmaps, {after} from the meter copies",
                    t.name,
                    s2.rect,
                    s2.ink(),
                    h2m.rect,
                    h2m.ink()
                );
                if t.name == "ui\\hud\\cyborg" {
                    assert!(before > 0, "the bug: Halo 1's clear texels shown");
                }
            }
            assert_eq!(after, 0, "{name} {}: shield over health", t.name);
            // The copies show exactly Halo 1's texels.
            for (d1, d2) in [(&s1, &s2), (&h1m, &h2m)] {
                for (a, b) in d1.rgba.iter().zip(&d2.rgba) {
                    assert_eq!(halo2_meter_shows(*b), halo1_shows(*a), "{name} {}", t.name);
                }
            }
            checked += 1;
        }
    }
    eprintln!("{checked} unit HUDs with both meters");
    assert!(checked > 0);
}

/// Halo 2's HUD number font (`conduit-13`, the font the text widget's
/// `+0x30` byte picks): its digits' ink rows from the text's top, and per
/// digit its advance and ink columns from the pen.
struct DigitFont {
    ink_rows: (i16, i16),
    glyphs: Vec<(char, i16, i16, i16)>,
}

impl DigitFont {
    /// The ink of `text` (digits) drawn right-justified to end at `x`, its
    /// top at `y` (E 0x22aa16).
    fn ink(&self, text: &str, x: i16, y: i16) -> Rect {
        let g = |c| self.glyphs.iter().find(|g| g.0 == c).unwrap();
        let width: i16 = text.chars().map(|c| g(c).1).sum();
        let mut pen = x - width;
        let mut r = [i32::MAX, i32::from(y + self.ink_rows.0), i32::MIN, i32::from(y + self.ink_rows.1)];
        for c in text.chars() {
            let (_, advance, ink0, ink1) = *g(c);
            r[0] = r[0].min(i32::from(pen + ink0));
            r[2] = r[2].max(i32::from(pen + ink1));
            pen += advance;
        }
        r
    }
}

fn inside(a: Rect, b: Rect) -> bool {
    a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3]
}

fn digit_font() -> DigitFont {
    // The font: font_table.txt line 6 (engine font 6, text font 0).
    let table = String::from_utf8(h2_file("maps/fonts/font_table.txt")).unwrap();
    let file = table
        .split(['\r', '\n'])
        .filter(|l| !l.is_empty())
        .nth(6)
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(h1hud::TEXT_FONT, 0);
    assert_eq!(file, "conduit-13");
    let data = h2_file(&format!("maps/fonts/{file}"));
    let font = font::Font::parse(&data).unwrap();
    // Each digit's ink from the text's top-left: its bitmap's top row is
    // the top (y offset = ascent), its left the pen plus its x offset.
    let mut ink_rows = (i16::MAX, i16::MIN);
    let mut glyphs = Vec::new();
    for c in '0'..='9' {
        let g = font.glyph(c as u16).unwrap();
        let r = g.record.clone();
        assert_eq!(r.y_offset, font.header.ascent);
        let (w, h) = (usize::from(r.width), usize::from(r.height));
        let shows = |x: usize, y: usize| g.pixels[y * w + x] >> 12 != 0;
        let rows: Vec<i16> = (0..h).filter(|y| (0..w).any(|x| shows(x, *y))).map(|y| y as i16).collect();
        let cols: Vec<i16> = (0..w).filter(|x| (0..h).any(|y| shows(*x, y))).map(|x| x as i16).collect();
        ink_rows = (ink_rows.0.min(rows[0]), ink_rows.1.max(rows[rows.len() - 1] + 1));
        glyphs.push((c, r.advance, r.x_offset + cols[0], r.x_offset + cols[cols.len() - 1] + 1));
    }
    assert_eq!(ink_rows, h1hud::TEXT_DIGIT_INK, "{file}'s digit ink rows");
    DigitFont { ink_rows, glyphs }
}

/// The converted grenade count's digits sit inside the grenade indicator
/// as Halo 1's do: every digit of Halo 2's HUD font drawn where the
/// converter's text widget puts it, against the indicator's texels (the
/// `grhi` background) and Halo 1's own digit sprites.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn grenade_count_digits_sit_inside_the_indicator() {
    let DigitFont { ink_rows, glyphs } = digit_font();
    let m = h1("bloodgulch");
    let (w, h) = (h1hud::digit_width(&m).unwrap(), h1hud::digit_height(&m).unwrap());
    let mut checked = 0;
    for (k, g) in h1hud::grenade_huds(&m).unwrap().iter().enumerate() {
        let Some(g) = g else { continue };
        let gt = m.tag(*g).unwrap();
        let gb = m.tag_data(gt).unwrap();
        let n = h1hud::grenade_number(gb).unwrap().expect("a count");
        assert_eq!(n.anchor, Anchor::TopLeft, "the disc's counts");
        let els = h1hud::grenade_elements(&m, gb).unwrap();
        let bkd = els.iter().find(|e| e.name == "h1_grenade_background").unwrap();
        let indicator = drawn(&m, bkd, false).ink();
        let t = h1hud::grenade_count(&n, k, w, h, TagId(1)).unwrap();
        // Halo 2's text (E 0x22aa16): its rectangle's top at the point,
        // justified to end at it (a one-digit count is one advance wide).
        let inside = |y: i16| {
            glyphs.iter().all(|(c, advance, ink0, ink1)| {
                let left = i32::from(t.offset.0 - advance);
                let d = [
                    left + i32::from(*ink0),
                    i32::from(y + ink_rows.0),
                    left + i32::from(*ink1),
                    i32::from(y + ink_rows.1),
                ];
                let ok = d[0] >= indicator[0] && d[1] >= indicator[1] && d[2] <= indicator[2] && d[3] <= indicator[3];
                if !ok {
                    eprintln!("  '{c}' at y {y}: ink {d:?} outside the indicator {indicator:?}");
                }
                ok
            })
        };
        // Halo 1's digit sprite, where its number draw puts the last digit
        // (h1hud::number_point), 18 wide on the disc.
        let cells = i16::from(n.digits) + i16::from(n.flags & 0x04 != 0);
        let start = n.offset.0 + (cells - 2) * w;
        let halo1 = h1hud::halo1_rect(n.anchor, (start, n.offset.1), (18, i32::from(h)));
        eprintln!(
            "{}: indicator ink {indicator:?}, Halo 1's digit sprite {halo1:?}; Halo 2's digit ink rows {}..{} (at Halo 1's y: {}..{})",
            gt.name,
            t.offset.1 + ink_rows.0,
            t.offset.1 + ink_rows.1,
            n.offset.1 + ink_rows.0,
            n.offset.1 + ink_rows.1
        );
        assert!(!inside(n.offset.1), "the bug: the text's top at Halo 1's y spills out");
        assert!(inside(t.offset.1), "{}: the converted count", gt.name);
        let mid = |a: i32, b: i32| (a + b) as f32 / 2.0;
        assert_eq!(
            mid(i32::from(t.offset.1 + ink_rows.0), i32::from(t.offset.1 + ink_rows.1)),
            mid(halo1[1], halo1[3]),
            "centred on Halo 1's digits"
        );
        checked += 1;
    }
    assert_eq!(checked, 2, "frag and plasma");
}

/// The weapon HUDs of Blood Gulch: every converted static and meter where
/// Halo 1 puts it, and each meter copy showing exactly Halo 1's texels.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn weapon_hud_widgets_stand_where_halo1_puts_them() {
    let m = h1("bloodgulch");
    let mut checked = 0;
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        let entries = h1hud::weapon_entries(&m, hud).unwrap();
        for e in &entries {
            let Ok(w) = h1hud::weapon_widget(e, Some(TagId(1)), TagId(2), TagId(3)) else {
                continue;
            };
            let Some(b) = e.bitmap else { continue };
            let bm = Bitmap::read(&m, m.tag(b).unwrap()).unwrap();
            let size = h1hud::sprite_size(&bm, e.sequence).unwrap();
            assert_eq!(
                h1hud::halo2_rect(&w, size),
                h1hud::halo1_rect(e.anchor, e.offset, size),
                "{} {:?}",
                wt.name,
                e.kind
            );
            if matches!(e.kind, h1hud::WeaponEntryKind::Meter(..)) {
                let (_, raw) = texels(&m, b, e.sequence, false);
                let (_, copy) = texels(&m, b, e.sequence, true);
                for (a, c) in raw.iter().zip(&copy) {
                    assert_eq!(halo2_meter_shows(*c), halo1_shows(*a), "{}", wt.name);
                }
            }
            checked += 1;
        }
    }
    eprintln!("{checked} weapon widgets placed as Halo 1's");
    assert!(checked > 10);
}


/// The weapon HUDs' ammo counts: every number the converter makes a text
/// widget of has its digits, every digit at every length up to Halo 1's
/// digit count, inside the ink of the weapon's own panel (a converted
/// static of the same `wphi`) where Halo 1's digits are, ending where
/// Halo 1's last digit cell does and centred on Halo 1's digit sprites'
/// rows. At Halo 1's y (the text's top there, as the v65 grenade count)
/// they would spill out of the panel.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn weapon_ammo_digits_sit_inside_the_ammo_panel() {
    let font = digit_font();
    let m = h1("bloodgulch");
    let (w, h) = (h1hud::digit_width(&m).unwrap(), h1hud::digit_height(&m).unwrap());
    let mut checked = Vec::new();
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        let entries = h1hud::weapon_entries(&m, hud).unwrap();
        for e in &entries {
            let h1hud::WeaponEntryKind::Number(state, n, _) = &e.kind else { continue };
            // This comparison uses the full-screen ammo panel and its digit font.
            if e.screen == h1hud::SPLIT_SCREEN_ONLY || n.fraction_digits != 0 || e.hidden {
                continue;
            }
            let t = h1hud::weapon_number(e, w, h, TagId(1)).unwrap();
            // Halo 1's cells (h1hud::number_point): the last ends at
            // `end`; the digit sprites' rows from Halo 1's y down.
            let cells = i16::from(n.digits) + i16::from(n.flags & 0x04 != 0);
            assert_eq!(n.anchor, Anchor::TopLeft, "the disc's weapon numbers");
            let end = n.offset.0 + (cells - 2) * w + w;
            let rows = (i32::from(n.offset.1), i32::from(n.offset.1 + h));
            // The panel: the converted statics of the same wphi, drawn
            // where Halo 1 puts them, with ink under Halo 1's last cell.
            let mut panel = None;
            for s in entries.iter().filter(|s| s.wphi == e.wphi) {
                if !matches!(s.kind, h1hud::WeaponEntryKind::Static(_)) {
                    continue;
                }
                let Ok(widget) = h1hud::weapon_widget(s, Some(TagId(1)), TagId(2), TagId(3)) else {
                    continue;
                };
                let (size, rgba) = texels(&m, s.bitmap.unwrap(), s.sequence, false);
                let d = Drawn { rect: h1hud::halo2_rect(&widget, size), rgba };
                let ink = d.ink();
                let mid = (rows.0 + rows.1) / 2;
                if ink[0] < i32::from(end) && i32::from(end) <= ink[2] && ink[1] <= mid && mid < ink[3] {
                    panel = Some(ink);
                    break;
                }
            }
            let panel = panel.unwrap_or_else(|| panic!("{} {state:?}: no panel under Halo 1's digits", wt.name));
            for k in 1..=usize::from(n.digits) {
                for (c, ..) in &font.glyphs {
                    let text: String = std::iter::repeat_n(*c, k).collect();
                    let ink = font.ink(&text, t.offset.0, t.offset.1);
                    assert!(inside(ink, panel), "{} '{text}': ink {ink:?} outside the panel {panel:?}", wt.name);
                    assert!(ink[2] <= i32::from(end), "{} '{text}': past Halo 1's last cell", wt.name);
                    assert_eq!(ink[1] + ink[3], rows.0 + rows.1, "{} '{text}': centred on Halo 1's rows", wt.name);
                }
            }
            let at_halo1_y = font.ink(&"0".repeat(usize::from(n.digits)), t.offset.0, n.offset.1);
            assert!(!inside(at_halo1_y, panel), "{}: the text's top at Halo 1's y spills out", wt.name);
            eprintln!(
                "{} {state:?}: input {:#x}, point {:?}, panel ink {panel:?}, Halo 1's last cell ends at {end}, rows {rows:?}",
                wt.name, t.inputs[0], t.offset
            );
            checked.push(wt.name.clone());
        }
    }
    // One full-screen count per ballistic weapon and the plasma weapons'
    // battery: the assault rifle, pistol, shotgun, rocket launcher, sniper
    // rifle and needler; the plasma rifle and plasma pistol.
    assert_eq!(checked.len(), 8, "{checked:?}");
}

/// Halo 1's statics of colour 0 draw nothing (`h1hud::static_alpha`): its
/// HUD element blend, mode 7 of the tables `0x0007f4e0` indexes, read from
/// the disc's XBE, is ONE, ONE_MINUS_SRC_ALPHA, ADD; and each such weapon
/// static on Blood Gulch, the ballistic weapons' `hud_ammo_alphas`, has no
/// visible widget.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_statics_of_colour_zero_have_no_show_bits() {
    let mut iso = iso();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.trim_start_matches(['\\', '/']).eq_ignore_ascii_case("default.xbe"))
        .expect("default.xbe");
    let xbe = iso.read(&e).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(xbe[at..at + 4].try_into().unwrap());
    let base = u32_at(0x104);
    let (count, headers) = (u32_at(0x11c) as usize, (u32_at(0x120) - base) as usize);
    let read = |va: u32| {
        (0..count)
            .map(|i| headers + i * 0x38)
            .find_map(|h| {
                let (sva, raw, size) = (u32_at(h + 4), u32_at(h + 0xc), u32_at(h + 0x10));
                (sva..sva + size).contains(&va).then(|| u32_at((raw + va - sva) as usize))
            })
            .unwrap()
    };
    // D3DBLEND_ONE 1, D3DBLEND_INVSRCALPHA 0x303, D3DBLENDOP_ADD 0x8006.
    let mode = 7;
    assert_eq!(read(0x001e9c0c + mode * 4), 1, "source: ONE");
    assert_eq!(read(0x001e9c30 + mode * 4), 0x303, "destination: ONE_MINUS_SRC_ALPHA");
    assert_eq!(read(0x001e9c54 + mode * 4), 0x8006, "operation: ADD");

    let m = h1("bloodgulch");
    let mut zero = Vec::new();
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        let entries = h1hud::weapon_entries(&m, hud).unwrap();
        for e in entries.iter().filter(|e| matches!(e.kind, h1hud::WeaponEntryKind::Static(_))) {
            let w = h1hud::weapon_widget(e, Some(TagId(1)), TagId(2), TagId(3)).unwrap();
            if e.colour == 0 {
                assert_eq!(&w.state[..8], &[0; 8], "{}", wt.name);
                let b = m.tag(e.bitmap.unwrap()).unwrap();
                assert!(b.name.ends_with("hud_ammo_alphas"), "{}: {}", wt.name, b.name);
                if e.screen != h1hud::SPLIT_SCREEN_ONLY {
                    zero.push(wt.name.clone());
                }
            } else if !e.hidden {
                assert_ne!(&w.state[..8], &[0; 8], "{}", wt.name);
            }
        }
    }
    eprintln!("statics of colour 0: {zero:?}");
    assert_eq!(zero.len(), 6, "{zero:?}");
}

/// Blood Gulch's ammunition type icons: Halo 1's overlay elements
/// (`wphi +0x90`) of the shared `ui\hud\master rounds` and `master plasma`
/// are overlays shown always (mask 0x10), so each weapon that inherits one
/// gets one icon in either view. The plasma icon is shared between views.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn ammo_type_icons_are_drawn_always() {
    let m = h1("bloodgulch");
    let mut icons = Vec::new();
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        let entries = h1hud::weapon_entries(&m, hud).unwrap();
        let drawn: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.kind, h1hud::WeaponEntryKind::Overlay(..)))
            .collect();
        let inherits = |n: &str| {
            entries.iter().any(|e| m.tag(e.wphi).unwrap().name.eq_ignore_ascii_case(n))
        };
        let expected =
            usize::from(inherits("ui\\hud\\master rounds") || inherits("ui\\hud\\master plasma"));
        for split in [false, true] {
            let views = drawn.iter().filter(|e| e.screen == 0 || (e.screen == h1hud::SPLIT_SCREEN_ONLY) == split);
            assert_eq!(views.count(), expected, "{} split={split}", wt.name);
        }
        for e in drawn {
            h1hud::weapon_widget(e, Some(TagId(1)), TagId(2), TagId(3)).unwrap();
            let b = m.tag(e.bitmap.unwrap()).unwrap();
            assert!(b.name.ends_with("hud_ammo_type_icons"), "{}: {}", wt.name, b.name);
            let bm = Bitmap::read(&m, b).unwrap();
            assert!(h1hud::sprite(&bm, e.sequence).is_some(), "{}", wt.name);
            icons.push((wt.name.clone(), e.offset, e.sequence));
        }
    }
    eprintln!("ammo type icons: {icons:?}");
    assert!(icons.len() >= 6, "{icons:?}");
}

/// Inherited warnings keep the weapon's own thresholds, and each warning
/// sprite selects its source predicate at draw time.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn weapon_warnings_keep_their_predicate_and_thresholds() {
    use h1hud::{WeaponEntryKind, crosshair_type as ct};
    let m = h1("bloodgulch");
    let mut reload = std::collections::BTreeMap::new();
    let mut kinds = std::collections::BTreeSet::new();
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        let thresholds = h1hud::wphi::low(m.tag_data(m.tag(hud).unwrap()).unwrap());
        for e in h1hud::weapon_entries(&m, hud).unwrap() {
            let WeaponEntryKind::Warning(t, flags, _) = e.kind else {
                continue;
            };
            let w = h1hud::weapon_widget(&e, Some(TagId(1)), TagId(2), TagId(3)).unwrap();
            let bitmap = Bitmap::read(&m, m.tag(e.bitmap.unwrap()).unwrap()).unwrap();
            let size = h1hud::sprite_size(&bitmap, e.sequence).unwrap();
            assert_eq!(
                h1hud::halo2_rect(&w, size),
                h1hud::halo1_rect(e.anchor, e.offset, size)
            );
            assert_eq!(
                w.inputs,
                [tags::weapon_hud::WARNING, t as u8, flags as u8, 0]
            );
            assert_eq!(e.thresholds, thresholds, "{}", wt.name);
            kinds.insert(t);
            if t == ct::RELOAD {
                assert_eq!(e.sequence, 1, "{}", wt.name);
                reload.insert(
                    wt.name.rsplit('\\').next().unwrap().to_owned(),
                    thresholds[1],
                );
            }
        }
    }
    let expected: std::collections::BTreeMap<_, _> = [
        ("assault rifle", 10),
        ("pistol", 4),
        ("shotgun", 2),
        ("rocket launcher", 1),
        ("sniper rifle", 1),
        ("needler", 8),
    ]
    .map(|(name, n)| (name.to_owned(), n))
    .into_iter()
    .collect();
    assert_eq!(reload, expected);
    for t in [
        ct::LOW_BATTERY,
        ct::RELOADING,
        ct::NO_GRENADES,
        ct::LOW_AMMO,
        ct::NO_AMMO,
        ct::BATTERY_DEPLETED,
    ] {
        assert!(kinds.contains(&t), "missing warning type {t}");
    }
}

/// The zoom-level readout (crosshair type 1, zoomed only): a widget per
/// sprite of its sequence up to Halo 2's two zoom levels, each placed as
/// Halo 1 places that sprite. The sniper rifle's is `hud_reticles_scope`
/// sequence 1, "2x" then "10x" (docs/halo1/hud.md, "Zoom level").
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn the_zoom_level_readout_is_a_widget_per_level() {
    use h1hud::{Skip, WeaponEntryKind, crosshair_type as ct};
    let m = h1("bloodgulch");
    let mut levels = std::collections::BTreeMap::new();
    let mut split = 0;
    for wt in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(hud) = h1objects::h1::reference(&m, m.tag_data(wt).unwrap(), 0x480) else {
            continue;
        };
        for e in h1hud::weapon_entries(&m, hud).unwrap() {
            let WeaponEntryKind::Crosshair(ct::ZOOM_LEVEL, _) = e.kind else { continue };
            if e.screen == h1hud::SPLIT_SCREEN_ONLY {
                split += 1;
            }
            assert_eq!(
                h1hud::weapon_widget(&e, Some(TagId(1)), TagId(2), TagId(3)),
                Err(Skip::ZoomLevels)
            );
            let b = m.tag(e.bitmap.unwrap()).unwrap();
            let bm = Bitmap::read(&m, b).unwrap();
            let sprites = bm.sequences[e.sequence as usize].sprites.len();
            let mut n = 0;
            for level in 0..sprites.min(h1hud::ZOOM_LEVELS) {
                let w = h1hud::zoom_widget(&e, level, Some(TagId(1)), TagId(3)).unwrap();
                assert_eq!(w.state, h1hud::ZOOM_LEVEL_STATES[level]);
                let mut shifted = bm.clone();
                shifted.sequences[e.sequence as usize].sprites.drain(..level);
                let size = h1hud::sprite_size(&shifted, e.sequence).unwrap();
                assert_eq!(h1hud::halo2_rect(&w, size), h1hud::halo1_rect(e.anchor, e.offset, size));
                n += 1;
            }
            let short = wt.name.rsplit('\\').next().unwrap().to_owned();
            if e.screen != h1hud::SPLIT_SCREEN_ONLY {
                levels.insert(short, (b.name.clone(), e.sequence, n));
            }
        }
    }
    eprintln!("zoom readouts {levels:?}; split-screen only {split}");
    assert!(split > 0, "the sniper's split-screen readout is on the disc");
    let sniper = &levels["sniper rifle"];
    assert!(sniper.0.ends_with("hud_reticles_scope"), "{sniper:?}");
    assert_eq!((sniper.1, sniper.2), (1, 2));
}

/// Every overlay element on the disc is shown always (mask `0x10`): Halo 1
/// has none shown only empty, reloading or below a threshold, so there is
/// nothing of that kind to convert (docs/halo1/hud.md).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn no_overlay_on_the_disc_is_shown_on_a_condition() {
    use h1hud::{WeaponEntryKind, overlay_shown};
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let maps: Vec<_> = h1map::disc::maps(&iso.entries().unwrap());
    let mut overlays = 0;
    for e in &maps {
        let m = H1Map::from_reader(Cursor::new(iso.read(e).unwrap())).unwrap();
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"wphi")) {
            for x in h1hud::weapon_entries(&m, t.id).unwrap() {
                if let WeaponEntryKind::Overlay(_, shown, _, _) = x.kind {
                    assert_eq!(shown, overlay_shown::ALWAYS, "{}", t.name);
                    overlays += 1;
                }
            }
        }
    }
    eprintln!("{overlays} overlays on {} maps, all shown always", maps.len());
    assert!(overlays >= 3 && maps.len() >= 10);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn scope_masks_use_the_hud_screen_effect_in_each_view() {
    use h1objects::base::Base;
    use h1objects::convert::{Converter, local_index};
    use h1objects::h1hud::scope::ScopeMask;
    use map::{FourCC, Map};
    use tags::defs::{Defs, Target, Val};
    use tags::material::{EngineInputs, TextureSource};
    use tags::scope_zoom::{Blur, WORDS};

    let h1 = h1("bloodgulch");
    let mut retail = Map::from_reader(Cursor::new(h2_file("/coagulation.map"))).unwrap();
    let base = Base::read(&mut retail).unwrap();
    let mut conv = Converter::new(&h1, &base);
    let mut scopes = Vec::new();
    for (weapon, max_radius) in [
        ("pistol", 15.0),
        ("sniper rifle", 11.0),
        ("rocket launcher", 11.0),
    ] {
        let wphi = h1
            .tags()
            .iter()
            .find(|t| {
                t.group == Group::new(b"wphi")
                    && t.name.starts_with(&format!("weapons\\{weapon}\\"))
            })
            .unwrap();
        let mask = ScopeMask::read(&h1, wphi.id).unwrap().unwrap();
        assert!(mask.filters.blur.zoom_only);
        assert_eq!(mask.filters.blur.fov, [1.1234161, 0.114633225]);
        assert_eq!(mask.filters.blur.radius, [0.0, max_radius]);
        let colours = weapon == "sniper rifle";
        assert_eq!(mask.filters.desaturation.enabled(), colours);
        assert_eq!(mask.filters.tint.enabled(), colours);
        if colours {
            assert_eq!(mask.filters.desaturation.flags, 3);
            assert_eq!(mask.filters.desaturation.channel, 1);
            assert_eq!(mask.filters.tint.flags, 15);
            assert_eq!(mask.filters.tint.channel, 1);
            assert_eq!(mask.filters.tint_rgb, [0.0, 1.0, 0.0]);
        }
        let images = [mask.full, mask.split].map(|id| {
            let source = Bitmap::read(&h1, h1.tag(id.unwrap()).unwrap()).unwrap();
            let image = &source.images[0];
            [image.width, image.height, image.depth]
        });
        let hud = conv.weapon_hud(wphi.id).unwrap();
        scopes.push((local_index(hud).unwrap(), images, mask.filters));
    }
    assert_eq!(
        conv.tree
            .tags
            .iter()
            .filter(|(_, t)| t.group == FourCC(*b"egor"))
            .count(),
        6
    );

    let mut set = map::build::TagSet::from_map(&mut retail).unwrap();
    let ids = conv.tree.install_tags(&mut set).unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut built = Map::from_reader(Cursor::new(bytes)).unwrap();
    let converted = Base::read(&mut built).unwrap();
    let defs = Defs::builtin();
    let nhdt = defs.group(FourCC(*b"nhdt")).unwrap();
    let egor = defs.group(FourCC(*b"egor")).unwrap();
    let mut lib = tags::material::ShaderLibrary::read(&mut built).unwrap();
    for (hud, images, source_profile) in scopes {
        let record = converted.record(nhdt, ids[hud]).unwrap();
        let id = |p: &str| match record.at(p) {
            Some(Val::Ref {
                target: Target::Id(id),
                ..
            }) => *id,
            other => panic!("{p}: {other:?}"),
        };
        let full = id("screen_effects[0].effect_full");
        let split = id("screen_effects[0].effect_half");
        assert_ne!(full, split);
        assert_eq!(id("screen_effects[0].effect_quarter"), split);
        for (datum, single_view) in [(full, true), (split, false)] {
            let mut profile = source_profile;
            if !single_view {
                profile.blur = Blur::default();
            }
            let effect = converted.record(egor, datum).unwrap();
            let Some(Val::Ref {
                target: Target::Id(shader),
                ..
            }) = effect.at("shader")
            else {
                panic!("scope effect is missing its shader")
            };
            let words = if profile.enabled() {
                profile.words()
            } else {
                [0; WORDS]
            };
            assert_eq!(
                effect.at("h1_zoom_profile"),
                Some(&Val::List(
                    words.into_iter().map(|n| Val::Int(n.into())).collect()
                )),
            );
            assert_eq!(
                effect.at("passes[0].render_target"),
                Some(&Val::Int(profile.enabled() as i64)),
            );
            assert_eq!(
                effect.at("passes[0].filters[0].resolution_scale"),
                single_view.then_some(&Val::Float(0.5)),
            );

            let mut inputs = EngineInputs::default();
            inputs.scalars[41] = 0.25;
            inputs.scalars[42] = 0.5;
            let mat = lib.material(*shader, Some(inputs)).unwrap();
            assert_eq!(mat.layers.len(), 1);
            assert_eq!(mat.layers[0].layer, 15);
            let passes = &mat.layers[0].passes;
            let sources = |i: usize| {
                passes[i].implementations[0]
                    .textures
                    .iter()
                    .flatten()
                    .map(|stage| stage.source)
                    .collect::<Vec<_>>()
            };
            assert_eq!(passes.len(), if profile.enabled() { 3 } else { 1 });
            assert_eq!(sources(passes.len() - 1), vec![TextureSource::Engine(16)]);
            if profile.enabled() {
                assert_eq!(sources(0), vec![TextureSource::Engine(5)]);
                let mut expected = vec![TextureSource::Engine(3)];
                if single_view {
                    expected.push(TextureSource::Engine(24));
                }
                expected.push(TextureSource::Engine(16));
                assert_eq!(sources(1), expected);
                let implementation = &passes[1].implementations[0];
                if profile.tint.enabled() {
                    assert_eq!(implementation.input_reads.scalars, (1 << 41) | (1 << 42));
                    let pixel = &implementation.pixel_shader;
                    assert_eq!(pixel[0x2f], 64); // stage 1 c0.a, scalar 41
                    assert_eq!(pixel[0x37], 128); // stage 3 c0.a, scalar 42
                }
            }
        }

        let bitmap = id("screen_effects[0].bitmap");
        let tags::TagData::Bitmap(art) = tags::DecodedTag::read(&mut built, bitmap).unwrap().data
        else {
            panic!("scope effect is missing its mask bitmap")
        };
        assert_eq!(
            art.images
                .iter()
                .map(|im| im.dimensions)
                .collect::<Vec<_>>(),
            images
        );
        assert_eq!(
            record.at("screen_effects[0].sequences"),
            Some(&Val::List(vec![Val::Int(0), Val::Int(1), Val::Int(1)]))
        );
        // Hidden unzoomed, and in the off-hand and backpack passes (word C
        // `0x06`, the primary panel's mask).
        assert_eq!(
            record.at("screen_effects[0].hide_when"),
            Some(&Val::List(vec![
                Val::Int(0x40),
                Val::Int(0),
                Val::Int(6),
                Val::Int(0)
            ]))
        );
    }
}

/// Count authored draw entries directly from the source reflexives. A child
/// interface contributes its entries each time it is inherited.
fn source_weapon_entries(map: &H1Map, mut id: h1map::TagId) -> usize {
    use h1hud::wphi;
    use h1objects::h1::{elements, reference};
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    loop {
        assert!(seen.insert(id), "cyclic HUD inheritance at {id:?}");
        let data = map.tag_data(map.tag(id).unwrap()).unwrap();
        for (at, stride) in [
            (wphi::STATICS, wphi::STATIC_SIZE),
            (wphi::METERS, wphi::METER_SIZE),
            (wphi::NUMBERS, wphi::NUMBER_SIZE),
        ] {
            total += elements(map, data, at, stride).unwrap().len();
        }
        for (at, stride, nested, size) in [
            (
                wphi::CROSSHAIRS,
                wphi::CROSSHAIR_SIZE,
                wphi::crosshair::OVERLAYS,
                wphi::crosshair::OVERLAY_SIZE,
            ),
            (
                wphi::OVERLAYS,
                wphi::OVERLAY_SIZE,
                wphi::entry::ELEMENT + wphi::overlay_element::OVERLAYS,
                wphi::overlay_element::OVERLAY_SIZE,
            ),
        ] {
            for parent in elements(map, data, at, stride).unwrap() {
                total += elements(map, parent, nested, size).unwrap().len();
            }
        }
        let Some(child) = reference(map, data, wphi::CHILD) else {
            return total;
        };
        id = child;
    }
}

#[test]
#[ignore = "reads all 23 gameplay maps ($H1_ISO) and their Halo 2 bases ($H2_DISC)"]
fn every_retail_weapon_hud_element_is_placed() {
    use h1hud::{WeaponEntryKind as Kind, crosshair_type};
    use h1objects::base::Base;
    use h1objects::convert::{Converter, local_index};
    use tags::defs::{self, Defs, Record, Target, Val};
    use tags::weapon_hud as input;

    fn block<'a>(r: &'a Record, field: &str) -> &'a [Record] {
        let Some(Val::Block(rows)) = r.get(field) else {
            panic!("missing {field}")
        };
        rows
    }

    let mut disc = iso();
    let mut bases = std::collections::BTreeMap::<_, Vec<_>>::new();
    for file in h1map::disc::Disc::open(&mut disc).unwrap().maps {
        let name = h1map::disc::map_stem(&file);
        if name == "ui" {
            continue;
        }
        let base = if h1map::disc::CAMPAIGN_LEVELS.contains(&name) {
            "05a_deltaapproach"
        } else {
            h1convert::convert::known(name)
                .unwrap_or_else(|| panic!("unknown map {name}"))
                .1
        };
        bases.entry(base).or_default().push(file);
    }
    let nhdt = Defs::builtin().group(map::FourCC(*b"nhdt")).unwrap();
    let (mut campaign, mut multiplayer, mut interfaces, mut authored) = (0, 0, 0, 0);
    for (base, files) in bases {
        let mut h2 = map::Map::from_reader(Cursor::new(h2_file(&format!("/{base}.map")))).unwrap();
        let templates = Base::read(&mut h2).unwrap();
        for file in files {
            let m = H1Map::from_reader(Cursor::new(disc.read(&file).unwrap())).unwrap();
            match m.header().cache_type {
                h1map::CacheType::Campaign => campaign += 1,
                h1map::CacheType::Multiplayer => multiplayer += 1,
                other => panic!("{} has cache type {other}", file.path),
            }
            let mut converter = Converter::new(&m, &templates);
            for tag in m.tags().iter().filter(|t| t.group == Group::new(b"wphi")) {
                let path = format!("{} {}", file.path, tag.name);
                let id = converter
                    .weapon_hud(tag.id)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                let converted = &converter.tree.tags[local_index(id).unwrap()].1;
                assert_eq!(converted.group, map::FourCC(*b"nhdt"), "{path}");
                let r = defs::from_built(nhdt, &converted.root).unwrap();
                let mut bitmaps = block(&r, "bitmap_widgets").iter();
                let mut texts = block(&r, "text_widgets").iter();
                let entries = h1hud::weapon_entries(&m, tag.id).unwrap();
                assert_eq!(entries.len(), source_weapon_entries(&m, tag.id), "{path}");
                authored += entries.iter().filter(|e| e.wphi == tag.id).count();
                for (i, e) in entries.iter().enumerate() {
                    let desc = format!("{path} entry {i} {:?}", e.kind);
                    let mut copies = 1;
                    if let Kind::Crosshair(crosshair_type::ZOOM_LEVEL, flags) = e.kind {
                        let bm = Bitmap::read(&m, m.tag(e.bitmap.unwrap()).unwrap()).unwrap();
                        let sprites = bm.sequences[e.sequence as usize].sprites.len();
                        let levels = if flags & (h1hud::OVERLAY_WHEN_UNZOOMED | 0x20) != 0 {
                            1
                        } else {
                            h1hud::ZOOM_LEVELS
                                + usize::from(flags & h1hud::OVERLAY_WHEN_ZOOMED == 0)
                        };
                        copies = sprites.min(levels);
                        assert_ne!(copies, 0, "{desc}");
                    }
                    for _ in 0..copies {
                        let text = matches!(e.kind, Kind::Number(..));
                        let row = if text { texts.next() } else { bitmaps.next() }
                            .unwrap_or_else(|| panic!("unplaced {desc}"));
                        assert_eq!(
                            row.get("h1_screen"),
                            Some(&Val::Int((input::SCREEN + e.screen).into())),
                            "{desc}"
                        );
                        for field in if text {
                            &["shader"][..]
                        } else {
                            &["bitmap", "shader"][..]
                        } {
                            assert!(
                                matches!(
                                    row.get(field),
                                    Some(Val::Ref {
                                        target: Target::Id(_),
                                        ..
                                    })
                                ),
                                "{desc} missing {field}"
                            );
                        }
                        if !text {
                            let reticle = matches!(e.kind, Kind::Crosshair(..) | Kind::Warning(..));
                            let half_size = reticle && e.split_flags & 2 == 0;
                            assert_eq!(
                                row.get("h1_half_size"),
                                Some(&Val::Int(i64::from(half_size))),
                                "{desc}"
                            );
                            assert_eq!(
                                row.get("h1_frame_period"),
                                Some(&Val::Int(e.frame_period.into())),
                                "{desc}"
                            );
                            for mode in ["full", "half", "quarter"] {
                                assert_eq!(
                                    row.get(&format!("sequence_{mode}")),
                                    Some(&Val::Int(e.sequence.into())),
                                    "{desc}"
                                );
                            }
                        }
                        let inputs = match e.kind {
                            Kind::Meter(state, [min, mul, add]) => {
                                [h1hud::weapon_meter_input(state).unwrap(), min, mul, add]
                            }
                            Kind::Number(state, number, magazines) => {
                                let value = h1hud::weapon_number_input(state).unwrap();
                                if magazines {
                                    [input::MAGAZINES, value, number.digits, number.flags]
                                } else if matches!(value, input::RANGE | input::ELEVATION) {
                                    [value, number.fraction_digits, number.digits, number.flags]
                                } else {
                                    [value, 0, 0, 0]
                                }
                            }
                            Kind::Warning(kind, flags, _) => {
                                [input::WARNING, kind as u8, flags as u8, 0]
                            }
                            _ => [0; 4],
                        };
                        for (slot, value) in inputs.into_iter().enumerate() {
                            assert_eq!(
                                row.get(&format!("input_{}", slot + 1)),
                                Some(&Val::Int(value.into())),
                                "{desc}"
                            );
                        }
                        if let Kind::Crosshair(kind, flags) = e.kind {
                            let mut expected = vec![kind as u8, flags as u8];
                            expected.extend(e.flash);
                            assert_eq!(
                                row.at("effects[0].block_4c"),
                                Some(&Val::Data(expected)),
                                "{desc} lost its aim or zoom sprite profile"
                            );
                        }
                        if matches!(e.kind, Kind::Warning(..)) {
                            let Some(Val::Data(profile)) = row.at("effects[0].block_38") else {
                                panic!("{desc} has no warning profile");
                            };
                            let mut source =
                                Vec::from_iter(e.thresholds.into_iter().flat_map(i16::to_le_bytes));
                            source.extend(e.flash);
                            assert_eq!(profile, &source, "{desc}");
                        }
                    }
                }
                assert_eq!(
                    (bitmaps.len(), texts.len()),
                    (0, 0),
                    "{path} has extra widgets"
                );
                interfaces += 1;
            }
        }
    }
    assert_eq!(
        (campaign, multiplayer, interfaces, authored),
        (10, 13, 360, 1497)
    );
}
