use super::canvas::*;
use super::*;
use crate::encode::{alpha3_code, encode_pixels};
use crate::patch::apply;
use crate::testutil::{G, build};
use crate::{MAX_GLYPH_BYTES, decode_pixels};

const SETS: [IconSet; 3] = [IconSet::Xbox360, IconSet::PlayStation, IconSet::Keyboard];

/// Every button bound, the way the default bindings would.
fn keys() -> KeyLabels {
    let mut k = KeyLabels::new();
    for (i, (_, b)) in BUTTON_UNITS.iter().enumerate() {
        k.insert(
            *b,
            KeyIcon::Key(["W", "SPACE", "CTRL", "\u{2191}", "F12"][i % 5].to_owned()),
        );
    }
    k.insert(Button::RightTrigger, KeyIcon::Mouse(MouseIcon::Left));
    k.insert(Button::RightStickClick, KeyIcon::Mouse(MouseIcon::Right));
    k.insert(Button::RightStick, KeyIcon::Mouse(MouseIcon::Move));
    k
}

/// Equal where it shows: same alpha everywhere, same colour on every pixel
/// that is not fully transparent (the stream does not keep those colours).
fn same_visible(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(p, q)| p >> 12 == q >> 12 && (p >> 12 == 0 || p == q))
}

fn alpha(c: &Canvas, x: usize, y: usize) -> f32 {
    c.get(x, y)[3]
}

#[test]
fn circle_is_solid_inside_empty_outside_and_symmetric() {
    let mut c = Canvas::new(16, 16);
    c.fill_flat(circle(8.0, 8.0, 6.0), white(1.0));
    assert_eq!(alpha(&c, 8, 8), 1.0);
    assert_eq!(alpha(&c, 0, 0), 0.0);
    assert_eq!(alpha(&c, 15, 8), 0.0);
    let mut area = 0.0;
    for y in 0..16 {
        for x in 0..16 {
            area += alpha(&c, x, y);
            assert!((alpha(&c, x, y) - alpha(&c, 15 - x, y)).abs() < 1e-5);
            assert!((alpha(&c, x, y) - alpha(&c, x, 15 - y)).abs() < 1e-5);
            assert!((alpha(&c, x, y) - alpha(&c, y, x)).abs() < 1e-5);
        }
    }
    // Coverage sums to the disc's area, within the anti-aliasing error.
    let exact = std::f32::consts::PI * 36.0;
    assert!(
        (area - exact).abs() < exact * 0.02,
        "area {area} vs {exact}"
    );
}

#[test]
fn edge_pixels_are_partly_covered() {
    // A half-plane edge through pixel centres covers them by half.
    let mut c = Canvas::new(4, 1);
    c.fill_flat(|x, _| x - 1.5, white(1.0));
    assert_eq!(
        (
            alpha(&c, 0, 0),
            alpha(&c, 1, 0),
            alpha(&c, 2, 0),
            alpha(&c, 3, 0)
        ),
        (1.0, 0.5, 0.0, 0.0)
    );
}

#[test]
fn round_rect_fills_its_box_and_rounds_its_corners() {
    let mut c = Canvas::new(12, 10);
    c.fill_flat(round_rect(1.0, 1.0, 11.0, 9.0, 3.0), white(1.0));
    assert_eq!(alpha(&c, 6, 5), 1.0);
    assert_eq!(alpha(&c, 1, 5), 1.0, "left edge pixel is inside");
    assert_eq!(alpha(&c, 0, 5), 0.0, "outside the box");
    assert_eq!(alpha(&c, 6, 0), 0.0);
    assert!(alpha(&c, 1, 1) < 0.5, "corner is cut");
    // A square-cornered box covers its corner pixel.
    let mut s = Canvas::new(12, 10);
    s.fill_flat(round_rect(1.0, 1.0, 11.0, 9.0, 0.0), white(1.0));
    assert_eq!(alpha(&s, 1, 1), 1.0);
}

#[test]
fn inner_outline_is_one_pixel_and_leaves_the_middle_empty() {
    let mut c = Canvas::new(12, 10);
    c.fill_flat(
        inner_outline(round_rect(0.0, 0.0, 12.0, 10.0, 0.0), 1.0),
        white(1.0),
    );
    for x in 0..12 {
        assert_eq!(alpha(&c, x, 0), 1.0);
        assert_eq!(alpha(&c, x, 9), 1.0);
    }
    for y in 1..9 {
        assert_eq!(alpha(&c, 0, y), 1.0);
        for x in 1..11 {
            assert_eq!(alpha(&c, x, y), 0.0, "({x}, {y})");
        }
    }
}

#[test]
fn segment_and_polygon_cover_what_they_should() {
    let mut c = Canvas::new(10, 5);
    c.fill_flat(segment((1.0, 2.5), (9.0, 2.5), 1.0), white(1.0));
    for x in 1..9 {
        assert_eq!(alpha(&c, x, 2), 1.0);
        assert_eq!(alpha(&c, x, 0), 0.0);
        assert_eq!(alpha(&c, x, 4), 0.0);
    }
    // Both windings of a triangle give the same picture.
    let (mut a, mut b) = (Canvas::new(10, 10), Canvas::new(10, 10));
    a.fill_flat(polygon([(5.0, 1.0), (9.0, 9.0), (1.0, 9.0)]), white(1.0));
    b.fill_flat(polygon([(1.0, 9.0), (9.0, 9.0), (5.0, 1.0)]), white(1.0));
    assert_eq!(a, b);
    assert_eq!(alpha(&a, 5, 6), 1.0);
    assert_eq!(alpha(&a, 0, 0), 0.0);
    assert_eq!(alpha(&a, 9, 1), 0.0);
}

#[test]
fn blending_is_source_over() {
    let mut c = Canvas::new(1, 1);
    c.plot(0, 0, [1.0, 0.0, 0.0, 1.0]);
    c.plot(0, 0, [0.0, 0.0, 1.0, 0.5]);
    let p = c.get(0, 0);
    assert_eq!(p, [0.5, 0.0, 0.5, 1.0]);
    // Out of range is ignored.
    c.plot(-1, 0, white(1.0));
    c.plot(0, 5, white(1.0));
    assert_eq!(c.get(0, 0), p);
}

#[test]
fn pixel_conversion_keeps_colour_or_snaps_white_alpha() {
    let mut c = Canvas::new(3, 1);
    c.plot(0, 0, [1.0, 0.0, 0.4, 1.0]);
    c.plot(1, 0, white(0.4));
    assert_eq!(c.to_argb4444(), [0xff06, 0x6fff, 0x0fff]);
    // 6/15 is not a run level; the monochrome form snaps it to 7/15.
    assert_eq!(c.to_white_argb4444(), [0xffff, 0x7fff, 0x0fff]);
}

#[test]
fn label_set_covers_every_key_name_the_input_crate_accepts() {
    let named = [
        "space",
        "enter",
        "escape",
        "tab",
        "backspace",
        "capslock",
        "delete",
        "insert",
        "home",
        "end",
        "pageup",
        "pagedown",
        "arrowup",
        "arrowdown",
        "arrowleft",
        "arrowright",
        "shiftleft",
        "shiftright",
        "controlleft",
        "controlright",
        "altleft",
        "altright",
        "comma",
        "period",
        "slash",
        "semicolon",
        "quote",
        "bracketleft",
        "bracketright",
        "backslash",
        "minus",
        "equal",
        "backquote",
    ];
    let letters = ('a'..='z').chain('0'..='9').map(String::from);
    let function = (1..=12).map(|n| format!("f{n}"));
    for key in named
        .iter()
        .map(|s| s.to_string())
        .chain(letters)
        .chain(function)
    {
        let text = key_label(&key);
        assert!(
            !text.is_empty() && text.chars().count() <= MAX_LABEL_CHARS,
            "{key}"
        );
        for ch in text.chars() {
            assert!(
                label::glyph(ch).is_some(),
                "no glyph for {ch:?} in the label of {key}"
            );
        }
    }
    assert_eq!(key_label("w"), "W");
    assert_eq!(key_label("controlleft"), "CTRL");
    assert_eq!(key_label("f5"), "F5");
}

#[test]
fn label_glyphs_fit_five_columns_and_are_distinct() {
    let chars: Vec<char> = ('A'..='Z')
        .chain('0'..='9')
        .chain(",./\\;'`[]-=\u{2190}\u{2191}\u{2192}\u{2193}".chars())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for c in &chars {
        let rows = label::glyph(*c).unwrap();
        assert!(
            rows.iter().all(|r| *r < 0x20),
            "{c} is wider than five columns"
        );
        assert!(rows.iter().any(|r| *r != 0), "{c} is blank");
        assert!(seen.insert(rows), "{c} repeats another glyph");
    }
    assert_eq!(label::glyph('q'), label::glyph('Q'));
    assert_eq!(label::glyph('\u{e100}'), None);
    assert_eq!(label::width(""), 0);
    assert_eq!(label::width("A"), 5);
    assert_eq!(label::width("LB"), 11);
    // Unknown characters take no room and draw nothing.
    assert_eq!(label::width("A\u{e100}"), 5);
    let mut lit = Vec::new();
    label::draw("I", 2, 3, |x, y| lit.push((x, y)));
    assert_eq!(lit.len(), 3 + 5 + 3);
    assert!(lit.contains(&(4, 3)) && lit.contains(&(4, 9)) && !lit.contains(&(2, 5)));
}

#[test]
fn every_icon_fits_its_cell_and_the_glyph_limits() {
    let mut keys = keys();
    // The longest label drawn.
    keys.insert(Button::Y, KeyIcon::Key("W".repeat(MAX_LABEL_CHARS)));
    for set in SETS {
        for (_, button) in BUTTON_UNITS {
            for height in [9u16, 11, 16, 18, 19, 24] {
                let Some(icon) = icon(set, button, height, &keys) else {
                    continue;
                };
                let what = format!("{set:?} {button:?} at {height}");
                assert_eq!(icon.height, height, "{what}");
                assert!(
                    (9..=128).contains(&icon.width),
                    "{what}: width {}",
                    icon.width
                );
                assert_eq!(
                    icon.pixels.len(),
                    usize::from(icon.width) * usize::from(height)
                );
                let stream = encode_pixels(&icon.pixels);
                assert!(
                    stream.len() <= usize::from(MAX_GLYPH_BYTES),
                    "{what}: {} bytes",
                    stream.len()
                );
                let back = decode_pixels(&stream, icon.pixels.len()).unwrap();
                assert!(same_visible(&back, &icon.pixels), "{what}");
                // Something is drawn, and the corners of the cell are clear.
                assert!(icon.pixels.iter().any(|p| p >> 12 == 15), "{what}");
                assert_eq!(icon.pixels[0] >> 12, 0, "{what}");
            }
        }
    }
}

#[test]
fn heights_outside_the_supported_range_are_clamped() {
    let keys = keys();
    assert_eq!(
        icon(IconSet::PlayStation, Button::A, 2, &keys)
            .unwrap()
            .height,
        MIN_HEIGHT
    );
    assert_eq!(
        icon(IconSet::Keyboard, Button::A, 500, &keys)
            .unwrap()
            .height,
        MAX_HEIGHT
    );
}

#[test]
fn bumpers_are_a_monochrome_pair_with_the_pressed_one_lit() {
    let keys = KeyLabels::new();
    let right = icon(IconSet::Xbox360, Button::Black, 18, &keys).unwrap();
    let left = icon(IconSet::Xbox360, Button::White, 18, &keys).unwrap();
    assert_eq!((right.width, right.height), (37, 18));
    for icon in [&right, &left] {
        // White at run levels only, like the retail trigger and stick pairs,
        // so the game tints it with the text and the stream needs no literals.
        for p in &icon.pixels {
            assert_eq!(p & 0xfff, 0xfff);
            assert!(alpha3_code(p >> 12).is_some(), "alpha {}", p >> 12);
        }
        assert!(encode_pixels(&icon.pixels).len() < icon.pixels.len() / 3);
    }
    // Total opacity of each half: the lit bumper outweighs the idle one.
    let weight = |icon: &Icon, half: std::ops::Range<usize>| -> u32 {
        icon.pixels
            .chunks(usize::from(icon.width))
            .flat_map(|row| row[half.clone()].iter())
            .map(|p| u32::from(p >> 12))
            .sum()
    };
    assert!(weight(&right, 20..37) > 2 * weight(&right, 0..17));
    assert!(weight(&left, 0..17) > 2 * weight(&left, 20..37));
    // Both bumpers are drawn in both pictures, and one is the other mirrored.
    assert!(weight(&right, 0..17) > 0 && weight(&left, 20..37) > 0);
    let w = usize::from(left.width);
    for (l, r) in left.pixels.chunks(w).zip(right.pixels.chunks(w)) {
        assert_eq!(l, r.iter().rev().copied().collect::<Vec<u16>>());
    }
    // A PlayStation pad's L1 and R1 are the same pictures.
    assert_eq!(
        icon(IconSet::PlayStation, Button::Black, 18, &keys).unwrap(),
        right
    );
    assert_eq!(
        icon(IconSet::PlayStation, Button::White, 18, &keys).unwrap(),
        left
    );
}

/// The strongest channel of the opaque pixel seen most often: the body colour.
fn body_rgb(icon: &Icon) -> (u16, u16, u16) {
    let mut sum = (0u32, 0u32, 0u32, 0u32);
    for p in icon.pixels.iter().filter(|p| *p >> 12 == 15) {
        sum = (
            sum.0 + u32::from((p >> 8) & 15),
            sum.1 + u32::from((p >> 4) & 15),
            sum.2 + u32::from(p & 15),
            sum.3 + 1,
        );
    }
    (
        (sum.0 / sum.3) as u16,
        (sum.1 / sum.3) as u16,
        (sum.2 / sum.3) as u16,
    )
}

#[test]
fn playstation_faces_are_coloured_balls_with_a_light_symbol() {
    let keys = KeyLabels::new();
    let get = |b: Button| {
        let icon = icon(IconSet::PlayStation, b, 18, &keys).unwrap();
        assert_eq!((icon.width, icon.height), (16, 18));
        // The middle of the left edge is rim, the cell's corner is empty.
        assert_ne!(icon.pixels[9 * 16] >> 12, 0);
        assert_eq!(icon.pixels[0] >> 12, 0);
        // A symbol in a light tint is drawn over the ball.
        let light = |p: &&u16| {
            **p >> 12 == 15 && (**p >> 8) & 15 >= 13 && (**p >> 4) & 15 >= 13 && **p & 15 >= 13
        };
        assert!(
            icon.pixels.iter().filter(light).count() >= 8,
            "{b:?} has no symbol"
        );
        icon
    };
    let (r, g, b) = body_rgb(&get(Button::A));
    assert!(b > r && b > g, "cross is on blue: {r} {g} {b}");
    let (r, g, b) = body_rgb(&get(Button::B));
    assert!(r > g && r > b, "circle is on red: {r} {g} {b}");
    let (r, g, b) = body_rgb(&get(Button::X));
    assert!(r > g && b > g, "square is on pink: {r} {g} {b}");
    let (r, g, b) = body_rgb(&get(Button::Y));
    assert!(g > r && g > b, "triangle is on green: {r} {g} {b}");
    // Options and Share are dark, as the retail Start and Back are.
    let (r, g, b) = body_rgb(&get(Button::Start));
    assert!(r < 8 && g < 8 && b < 8, "options is dark: {r} {g} {b}");
    // All six faces differ from each other.
    let faces: Vec<Icon> = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::Start,
        Button::Back,
    ]
    .map(get)
    .to_vec();
    for i in 0..faces.len() {
        for j in 0..i {
            assert_ne!(faces[i], faces[j]);
        }
    }
}

#[test]
fn sets_replace_only_what_differs_from_the_xbox_pad() {
    let keys = keys();
    let replaced = |set| -> Vec<Button> {
        BUTTON_UNITS
            .iter()
            .map(|u| u.1)
            .filter(|b| icon(set, *b, 18, &keys).is_some())
            .collect()
    };
    use Button::*;
    assert!(replaced(IconSet::Xbox).is_empty());
    assert_eq!(replaced(IconSet::Xbox360), [Black, White]);
    assert_eq!(
        replaced(IconSet::PlayStation),
        [A, B, X, Y, Black, White, Start, Back]
    );
    assert_eq!(replaced(IconSet::Keyboard).len(), BUTTON_UNITS.len());
    // `replaces` and `icon` agree for every set and button.
    for set in [
        IconSet::Xbox,
        IconSet::Xbox360,
        IconSet::PlayStation,
        IconSet::Keyboard,
    ] {
        for (_, b) in BUTTON_UNITS {
            assert_eq!(
                replaces(set, b, &keys),
                icon(set, b, 18, &keys).is_some(),
                "{set:?} {b:?}"
            );
        }
    }
}

#[test]
fn each_set_has_its_own_code_units_inside_the_icon_range() {
    let keys = keys();
    let mut seen = std::collections::HashSet::new();
    for (unit, _) in BUTTON_UNITS {
        assert!(seen.insert(unit));
        assert_eq!(alias_unit(IconSet::Xbox, unit), unit);
        for set in SETS {
            let alias = alias_unit(set, unit);
            assert!((0xe200..=0xe3ff).contains(&alias), "{alias:#x}");
            assert!(seen.insert(alias), "{alias:#x} is used twice");
        }
    }
    // What is drawn: the set's unit where it has a picture, else the retail one.
    assert_eq!(shown_unit(IconSet::Xbox360, 0xe104, &keys), 0xe204);
    assert_eq!(shown_unit(IconSet::Xbox360, 0xe100, &keys), 0xe100);
    assert_eq!(shown_unit(IconSet::PlayStation, 0xe100, &keys), 0xe220);
    assert_eq!(shown_unit(IconSet::PlayStation, 0xe106, &keys), 0xe106);
    assert_eq!(shown_unit(IconSet::Keyboard, 0xe111, &keys), 0xe251);
    assert_eq!(
        shown_unit(IconSet::Keyboard, 0xe100, &KeyLabels::new()),
        0xe100
    );
    assert_eq!(shown_unit(IconSet::Xbox, 0xe104, &keys), 0xe104);
    // Not a button: untouched in every set.
    for unit in [0x41, 0xe0ff, 0xe112, 0xe204] {
        assert_eq!(shown_unit(IconSet::Keyboard, unit, &keys), unit);
    }
}

#[test]
fn keyboard_icons_follow_the_labels_given() {
    let mut keys = KeyLabels::new();
    keys.insert(Button::A, KeyIcon::Key("SPACE".into()));
    keys.insert(Button::B, KeyIcon::Key("V".into()));
    keys.insert(Button::X, KeyIcon::Mouse(MouseIcon::Left));
    keys.insert(Button::Y, KeyIcon::Mouse(MouseIcon::Right));
    let get = |b| icon(IconSet::Keyboard, b, 18, &keys);
    let (space, v) = (get(Button::A).unwrap(), get(Button::B).unwrap());
    // A one-letter key is a square cap; a longer label widens it.
    assert_eq!((v.width, v.height), (16, 18));
    assert_eq!(space.width, 5 * 6 - 1 + 8);
    // A light cap with dark lettering: both are present, and the cap is
    // brighter at the top than at the bottom.
    let (r, g, b) = body_rgb(&v);
    assert!(r >= 9 && g >= 9 && b >= 9, "cap is light: {r} {g} {b}");
    assert!(
        v.pixels.iter().any(|p| p >> 12 == 15 && p & 0xfff < 0x333),
        "no dark lettering"
    );
    let green = |x: usize, y: usize| (v.pixels[y * 16 + x] >> 4) & 15;
    assert!(green(3, 3) > green(3, 12), "no top-to-bottom shading");
    // Left and right mouse buttons are mirror images, lit in colour.
    let (l, r) = (get(Button::X).unwrap(), get(Button::Y).unwrap());
    assert_eq!(l.width, r.width);
    let w = usize::from(l.width);
    for (row_l, row_r) in l.pixels.chunks(w).zip(r.pixels.chunks(w)) {
        let mirrored: Vec<u16> = row_r.iter().rev().copied().collect();
        assert_eq!(row_l, mirrored);
    }
    assert_ne!(l, r);
    let amber = |p: &&u16| **p >> 12 == 15 && (**p >> 8) & 15 >= 14 && **p & 15 <= 5;
    assert!(
        l.pixels.iter().filter(amber).count() >= 6,
        "lit button is not amber"
    );
    // An unbound button keeps the retail glyph.
    assert_eq!(get(Button::Black), None);
    // A changed binding changes the picture.
    keys.insert(Button::B, KeyIcon::Key("K".into()));
    assert_ne!(icon(IconSet::Keyboard, Button::B, 18, &keys).unwrap(), v);
    // Over-long labels are cut, not rejected.
    keys.insert(Button::B, KeyIcon::Key("X".repeat(40)));
    assert_eq!(
        icon(IconSet::Keyboard, Button::B, 18, &keys).unwrap().width as usize,
        MAX_LABEL_CHARS * 6 - 1 + 8
    );
}

#[test]
fn choice_parses_and_resolves() {
    for (text, want) in [
        ("auto", IconChoice::Auto),
        ("xbox", IconChoice::Set(IconSet::Xbox)),
        ("Xbox360", IconChoice::Set(IconSet::Xbox360)),
        (" playstation ", IconChoice::Set(IconSet::PlayStation)),
        ("KEYBOARD", IconChoice::Set(IconSet::Keyboard)),
    ] {
        assert_eq!(text.parse::<IconChoice>(), Ok(want));
    }
    assert!("ps5".parse::<IconChoice>().unwrap_err().contains("ps5"));
    assert!("".parse::<IconChoice>().is_err());
    assert_eq!(IconChoice::default(), IconChoice::Auto);

    let auto = IconChoice::Auto;
    assert_eq!(
        auto.resolve(Some("DualSense Wireless Controller"), true, true),
        IconSet::PlayStation
    );
    assert_eq!(
        auto.resolve(
            Some("Sony Interactive Entertainment Controller"),
            true,
            false
        ),
        IconSet::PlayStation
    );
    assert_eq!(
        auto.resolve(Some("Xbox Wireless Controller"), true, true),
        IconSet::Xbox360
    );
    assert_eq!(
        auto.resolve(Some("Generic Gamepad"), true, false),
        IconSet::Xbox360
    );
    // No pad name: pads enabled wins over the keyboard; neither keeps retail.
    assert_eq!(auto.resolve(None, true, true), IconSet::Xbox360);
    assert_eq!(auto.resolve(None, false, true), IconSet::Keyboard);
    assert_eq!(auto.resolve(None, false, false), IconSet::Xbox);
    // A fixed set ignores the devices.
    assert_eq!(
        IconChoice::Set(IconSet::Xbox).resolve(Some("DualSense"), true, true),
        IconSet::Xbox
    );
}

/// A font with a glyph of the retail sizes at every button unit, plus 'A'
/// and a filler record that the rest of the private-use range shares.
fn button_font() -> Vec<u8> {
    let mut glyphs = vec![
        G {
            unit: 0x20,
            advance: 4,
            x_offset: 0,
            y_offset: 19,
            width: 0,
            height: 0,
            data: vec![],
        },
        G {
            unit: 0x41,
            advance: 3,
            x_offset: 0,
            y_offset: 19,
            width: 2,
            height: 2,
            data: vec![0x44],
        },
        G {
            unit: 0xe000,
            advance: 3,
            x_offset: 0,
            y_offset: 19,
            width: 2,
            height: 2,
            data: vec![0x44],
        },
    ];
    for (unit, _) in BUTTON_UNITS {
        // 16 x 18 opaque: 288 pixels in runs of at most 63.
        glyphs.push(G {
            unit,
            advance: 16,
            x_offset: 0,
            y_offset: 17,
            width: 16,
            height: 18,
            data: vec![0x7f, 0x7f, 0x7f, 0x7f, 0x40 | 36],
        });
    }
    let mut f = build(19, 5, 0, &glyphs, &[]);
    // As on the disc: unused private-use units all point at one filler record.
    for unit in 0xe200..=0xe3ffusize {
        let o = crate::CHARMAP_OFFSET + unit * 4;
        f[o..o + 4].copy_from_slice(&2u32.to_le_bytes());
    }
    f
}

#[test]
fn replacements_add_every_set_beside_the_retail_glyphs() {
    let data = button_font();
    let font = Font::parse(&data).unwrap();
    let keys = keys();
    assert!(alias_range_free(&font));
    let reps = replacements(&font, &keys);
    // Two bumpers, eight PlayStation pictures, every keyboard button.
    assert_eq!(reps.len(), 2 + 8 + BUTTON_UNITS.len());
    let out = apply(&data, &reps).unwrap();
    let patched = Font::parse(&out).unwrap();
    assert_eq!(
        patched.validate().unwrap(),
        font.header.glyph_count + reps.len() as u32
    );
    for (unit, button) in BUTTON_UNITS {
        // The retail glyph is still what its own unit draws.
        let (old, mut same) = (font.glyph(unit).unwrap(), patched.glyph(unit).unwrap());
        same.record.data_offset = old.record.data_offset;
        assert_eq!(same, old, "{button:?}");
        for set in SETS {
            let alias = patched.glyph(alias_unit(set, unit)).unwrap();
            match icon(set, button, 18, &keys) {
                Some(icon) => {
                    assert!(
                        same_visible(&alias.pixels, &icon.pixels),
                        "{set:?} {button:?}"
                    );
                    assert_eq!(alias.record.width, icon.width);
                    // Advance is the width; the retail glyph's placement is kept.
                    assert_eq!(alias.record.advance, icon.width as i16);
                    assert_eq!((alias.record.height, alias.record.y_offset), (18, 17));
                }
                // No picture in this set: the unit still draws the filler.
                None => assert_eq!(alias.pixels, font.glyph(0xe000).unwrap().pixels),
            }
        }
    }
    assert_eq!(
        patched.glyph(0x41).unwrap().pixels,
        font.glyph(0x41).unwrap().pixels
    );
    // An unused private-use unit still draws the filler.
    assert_eq!(patched.glyph_index(0xe3ff), 2);
}

#[test]
fn replacements_use_each_fonts_own_cell_height_and_skip_missing_glyphs() {
    // A font whose A icon is 16 pixels tall and which has no X icon bitmap.
    let data = build(
        13,
        3,
        2,
        &[
            G {
                unit: 0x20,
                advance: 4,
                x_offset: 0,
                y_offset: 13,
                width: 0,
                height: 0,
                data: vec![],
            },
            G {
                unit: 0xe100,
                advance: 16,
                x_offset: 0,
                y_offset: 13,
                width: 16,
                height: 16,
                data: vec![0x7f, 0x7f, 0x7f, 0x7f, 0x44],
            },
            // A placeholder record: mapped, but no bitmap.
            G {
                unit: 0xe102,
                advance: 0,
                x_offset: 0,
                y_offset: 13,
                width: 0,
                height: 0,
                data: vec![],
            },
        ],
        &[],
    );
    let font = Font::parse(&data).unwrap();
    let reps = replacements(&font, &KeyLabels::new());
    // Only the PlayStation set has a picture for A with no key labels.
    assert_eq!(reps.len(), 1);
    assert_eq!(
        (reps[0].unit, reps[0].width, reps[0].height),
        (0xe220, 16, 16)
    );
    assert_eq!(reps[0].y_offset, Some(13));
    let out = apply(&data, &reps).unwrap();
    Font::parse(&out).unwrap().validate().unwrap();
}

#[test]
fn a_font_that_uses_the_sets_code_units_is_left_alone() {
    // A real glyph of the font's own at U+E220, owned by that unit alone.
    let data = build(
        13,
        3,
        2,
        &[
            G {
                unit: 0x20,
                advance: 4,
                x_offset: 0,
                y_offset: 13,
                width: 0,
                height: 0,
                data: vec![],
            },
            G {
                unit: 0xe100,
                advance: 16,
                x_offset: 0,
                y_offset: 13,
                width: 2,
                height: 2,
                data: vec![0x44],
            },
            G {
                unit: 0xe220,
                advance: 3,
                x_offset: 0,
                y_offset: 13,
                width: 2,
                height: 2,
                data: vec![0x44],
            },
        ],
        &[],
    );
    let font = Font::parse(&data).unwrap();
    assert!(!alias_range_free(&font));
    assert!(replacements(&font, &keys()).is_empty());
}
