//! The editor over the invented map ([`crate::fixture`]): listing, the
//! preview's placement and pixels, the checks, edits with undo, and a save
//! that changes nothing but the edited bytes. With the owner's disc
//! (ignored tests): every retail HUD's widgets land on the screen.

use std::path::PathBuf;

use scenario_edit::MapSource;

use crate::checks::{self, Severity};
use crate::fixture;
use crate::layout::Mode;
use crate::model::{Value, WidgetKey, WidgetKind};
use crate::place::Rect;
use crate::session::HudSession;
use crate::state::{Grenade, HudState};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open() -> (Temp, PathBuf, HudSession, map::TagId, map::TagId) {
    let dir = std::env::temp_dir().join(format!(
        "hud-edit-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (bytes, player, weapon, _) = fixture::build();
    let path = dir.join("hudtest.map");
    std::fs::write(&path, bytes).unwrap();
    let s = HudSession::open(MapSource::file(&path), "hudtest").unwrap();
    (Temp(dir), path, s, player, weapon)
}

const fn bitmap(index: usize) -> WidgetKey {
    WidgetKey {
        kind: WidgetKind::Bitmap,
        index,
    }
}
const TEXT0: WidgetKey = WidgetKey {
    kind: WidgetKind::Text,
    index: 0,
};

fn rect(p: &crate::render::Picture, hud: usize, key: WidgetKey) -> Rect {
    p.placed
        .iter()
        .find(|w| w.hud == hud && w.key == key)
        .and_then(|w| w.rect)
        .unwrap()
}

#[test]
fn the_huds_are_listed_with_who_names_them() {
    let (_t, _, s, player, weapon) = open();
    let names: Vec<_> = s.huds.iter().map(|e| (e.category(), e.name.as_str())).collect();
    assert_eq!(
        names,
        [
            ("Players and bipeds", "invented\\hud\\player"),
            ("Weapons", "invented\\hud\\blaster")
        ]
    );
    let p = s.entry(player).unwrap();
    assert_eq!(p.owners.len(), 1);
    assert_eq!(p.owners[0].name, "invented\\bipeds\\chief");
    assert_eq!(p.owners[0].at, "unit +0x1a0[0]");
    assert_eq!(s.entry(weapon).unwrap().owners[0].at, "weap +0x2b0");
}

#[test]
fn widgets_read_with_their_names_and_fields() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let hud = s.hud().unwrap();
    let names: Vec<_> = hud.widgets.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["panel", "meter", "stray", "count"]);
    let meter = hud.widget(bitmap(1)).unwrap();
    assert_eq!(meter.anchor(), 2);
    assert_eq!(meter.offset(Mode::Full), (0, -56));
    assert_eq!(meter.offset(Mode::Half), (0, -28));
    assert_eq!(meter.sequence(Mode::Full), 1);
    assert_eq!(meter.inputs(), [17, 0, 0, 0]);
    assert_eq!(meter.get("y0"), Some(Value::Int(-56)));
    assert_eq!(meter.get("rx0"), Some(Value::Float(0.0)));
    let count = hud.widget(TEXT0).unwrap();
    assert_eq!(count.justification(), 2);
    assert_eq!(s.string(count.string()).as_deref(), Some("hud_input1"));
}

/// The preview places each widget by the engine's rules (crate::place)
/// from the bitmap's own sizes: a whole image, a sprite's bounds.
#[test]
fn the_preview_places_and_draws_the_art() {
    let (_t, _, mut s, player, weapon) = open();
    s.select(player).unwrap();
    s.set_with(weapon, true).unwrap();
    let st = HudState::default();
    let p = s.draw(&st).unwrap();
    assert_eq!(rect(&p, 0, bitmap(0)).pixels(), [48, 404, 112, 420]);
    // Sequence 1 is the left half sprite: 32 by 16.
    assert_eq!(rect(&p, 0, bitmap(1)).pixels(), [48, 388, 80, 404]);
    assert_eq!(rect(&p, 0, bitmap(2)).pixels(), [88, 396, 152, 412]);
    // The weapon's, top right.
    assert_eq!(rect(&p, 1, bitmap(0)).pixels(), [528, 36, 592, 52]);
    // The number ends at (68, 46), the frag count's input 20.
    let count = p.placed.iter().find(|w| w.key == TEXT0).unwrap();
    assert_eq!(count.text.as_deref(), Some("2"));
    let r = count.rect.unwrap();
    assert_eq!((r.x1, r.y0), (68.0, 46.0));
    // The panel's pixels: the texture's own colour (R 60, G 160).
    let px = |x: u32, y: u32| {
        let i = ((y * p.canvas.width + x) * 4) as usize;
        p.canvas.rgba[i..i + 3].to_vec()
    };
    assert_eq!(px(50, 418)[..2], [60, 160]);
}

/// A meter fills where its texture's blue is below its input: at half
/// health only the left part of the sprite is drawn full.
#[test]
fn meters_fill_from_their_input() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let full = s.draw(&HudState::default()).unwrap();
    let half = s
        .draw(&HudState {
            body: 0.25,
            ..HudState::default()
        })
        .unwrap();
    let at = |p: &crate::render::Picture, x: u32| {
        let i = ((392 * p.canvas.width + x) * 4) as usize;
        p.canvas.rgba[i + 1]
    };
    // Column 4 has blue 16/255, under 0.25; column 30 has 120/255.
    assert_eq!(at(&full, 52), at(&half, 52));
    assert_ne!(at(&full, 78), at(&half, 78));
}

#[test]
fn the_checks_find_the_stray_and_a_spilling_number() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let st = HudState::default();
    let found = checks::check(&s.draw(&st).unwrap());
    let warnings: Vec<_> = found
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .collect();
    assert_eq!(warnings.len(), 1, "{found:#?}");
    assert!(warnings[0].text.starts_with("panel and stray overlap (192 opaque"), "{}", warnings[0].text);
    // The number moved onto the panel's right edge spills out of it.
    s.set(TEXT0, "anchor", Value::Int(2), false).unwrap();
    s.set(TEXT0, "justify", Value::Int(0), false).unwrap();
    s.move_to(TEXT0, Mode::Full, (60, -38), false).unwrap();
    let found = checks::check(&s.draw(&st).unwrap());
    assert!(
        found.iter().any(|f| f.text.starts_with("count") && f.text.contains("spills out of panel")),
        "{found:#?}"
    );
    // Off the screen.
    s.move_to(bitmap(2), Mode::Full, (560, -48), false).unwrap();
    let found = checks::check(&s.draw(&st).unwrap());
    assert!(found.iter().any(|f| f.text.starts_with("stray runs off the screen")));
}

/// With the gate on, the frag count shows only with frag grenades
/// selected (word A 0x04), as the engine's gate (0x13ac87) decides.
#[test]
fn the_gate_hides_widgets_by_state() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let mut st = HudState {
        gate: true,
        words: [0x1000, 0, 0, 0],
        ..HudState::default()
    };
    let shown = |s: &mut HudSession, st: &HudState| {
        s.draw(st)
            .unwrap()
            .placed
            .iter()
            .filter(|w| w.drawn)
            .map(|w| w.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(&mut s, &st), ["panel", "meter", "stray", "count"]);
    st.grenade = Grenade::Plasma;
    assert_eq!(shown(&mut s, &st), ["panel", "meter", "stray"]);
    st.words = [0; 4];
    st.grenade = Grenade::Frag;
    assert_eq!(shown(&mut s, &st), ["count"]);
}

#[test]
fn edits_undo_redo_and_a_drag_is_one_step() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    assert!(!s.is_dirty());
    for (i, y) in [-55, -54, -50].into_iter().enumerate() {
        s.move_to(bitmap(1), Mode::Full, (2, y), i > 0).unwrap();
    }
    assert!(s.is_dirty());
    assert_eq!(s.history().count(), 1, "one drag, one step");
    s.set(bitmap(1), "anchor", Value::Int(3), false).unwrap();
    assert_eq!(s.history().count(), 2);
    let w = |s: &HudSession| s.hud().unwrap().widget(bitmap(1)).unwrap().clone();
    assert_eq!((w(&s).offset(Mode::Full), w(&s).anchor()), ((2, -50), 3));
    s.undo().unwrap();
    assert_eq!(w(&s).anchor(), 2);
    s.undo().unwrap();
    assert_eq!(w(&s).offset(Mode::Full), (0, -56));
    assert!(!s.is_dirty());
    assert!(s.undo().is_none());
    s.redo().unwrap();
    assert_eq!(w(&s).offset(Mode::Full), (2, -50));
    assert!(s.is_dirty());
    // Out of range values are refused.
    assert!(s.set(bitmap(1), "seq0", Value::Int(300), false).is_err());
    assert!(s.set(bitmap(1), "nope", Value::Int(1), false).is_err());
}

/// The save is the source byte for byte but for the edited fields and the
/// header checksum, and it reads back with the edits.
#[test]
fn a_save_changes_only_the_edited_bytes() {
    let (t, path, mut s, player, weapon) = open();
    s.select(player).unwrap();
    s.move_to(bitmap(2), Mode::Full, (40, -60), false).unwrap();
    s.set(bitmap(0), "rx0", Value::Float(0.5), false).unwrap();
    s.select(weapon).unwrap();
    s.set(bitmap(0), "seq0", Value::Int(1), false).unwrap();
    let patches = s.patches();
    let changed: usize = patches.iter().map(|(_, b)| b.len()).sum();
    // y (-48 to -60: the low byte only), rx (0.0 to 0.5: 0x3f000000, the
    // high byte only), the sequence (1).
    assert_eq!(changed, 3, "{patches:x?}");
    assert_eq!(patches.len(), 3);
    let out = t.0.join("edited.map");
    s.save_as(&out).unwrap();
    assert!(!s.is_dirty());
    assert!(s.save_as(&path).is_err(), "never over the source");
    let a = std::fs::read(&path).unwrap();
    let b = std::fs::read(&out).unwrap();
    assert_eq!(a.len(), b.len());
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let checksum = 0x2d0..0x2d4;
    let edits: usize = diff.iter().filter(|i| !checksum.contains(*i)).count();
    assert_eq!(edits, changed, "only the edited bytes");
    // It reads back with the edits.
    let mut again = HudSession::open(MapSource::file(&out), "edited").unwrap();
    again.select(player).unwrap();
    let hud = again.hud().unwrap();
    assert_eq!(hud.widget(bitmap(2)).unwrap().offset(Mode::Full), (40, -60));
    assert_eq!(hud.widget(bitmap(0)).unwrap().registration(Mode::Full), [0.5, 0.0]);
    assert_eq!(hud.widget(bitmap(1)).unwrap().bytes, s.hud_by(player).unwrap().widget(bitmap(1)).unwrap().original);
    again.select(weapon).unwrap();
    assert_eq!(again.hud().unwrap().widget(bitmap(0)).unwrap().sequence(Mode::Full), 1);
    // No edits: the save is the source exactly.
    let mut clean = HudSession::open(MapSource::file(&path), "clean").unwrap();
    let same = t.0.join("same.map");
    clean.save_as(&same).unwrap();
    assert_eq!(std::fs::read(&same).unwrap(), a);
}

/// The owner's disc: every HUD of a multiplayer map draws, and the
/// retail `ui\hud\masterchief` lands where docs/halo1/health.md says:
/// its shield meter (special type 5) at x 47..175, y 346..378 and its
/// motion tracker at x 47..130, y 360..443; every drawn widget of every
/// HUD stays on the screen.
#[test]
#[ignore = "needs the owner's disc image (H2_DISC)"]
fn retail_huds_land_on_the_screen() {
    let iso = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso"));
    let maps = MapSource::disc_maps(&iso).unwrap();
    let (_, lockout) = maps.iter().find(|(n, _)| n.eq_ignore_ascii_case("lockout")).unwrap();
    let mut s = HudSession::open(lockout.clone(), "lockout").unwrap();
    assert!(s.fonts_found(), "the disc's fonts");
    let st = HudState::default();
    let chief = s.huds.iter().find(|e| e.name == "ui\\hud\\masterchief").unwrap().id;
    s.select(chief).unwrap();
    let p = s.draw(&st).unwrap();
    let hud = s.hud().unwrap();
    let shield = hud.widgets.iter().find(|w| w.key.kind == WidgetKind::Bitmap && w.special() == 5).unwrap();
    assert_eq!(rect(&p, 0, shield.key).pixels(), [47, 346, 175, 378]);
    // The motion tracker rim seen in a frame of a10 (x 47..129, y
    // 360..442): some bitmap widget lands exactly there. Found by its
    // place, not a name (the retail names are not ours to rely on).
    let rects: Vec<_> = p
        .placed
        .iter()
        .filter(|w| w.hud == 0 && w.key.kind == WidgetKind::Bitmap)
        .filter_map(|w| Some((w.name.clone(), w.rect?.pixels())))
        .collect();
    assert!(
        rects.iter().any(|(_, r)| *r == [47, 360, 130, 443]),
        "no widget on the tracker's rectangle: {rects:?}"
    );
    let ids: Vec<_> = s.huds.iter().map(|e| (e.id, e.name.clone())).collect();
    let mut drawn = 0;
    for (id, name) in ids {
        s.select(id).unwrap();
        let p = s.draw(&st).unwrap();
        for w in p.placed.iter().filter(|w| w.drawn && w.key.kind == WidgetKind::Bitmap) {
            let r = w.rect.unwrap();
            assert!(
                r.x0 >= -1.0 && r.y0 >= -1.0 && r.x1 <= 641.0 && r.y1 <= 481.0,
                "{name} {}: {r:?}",
                w.name
            );
            drawn += 1;
        }
    }
    assert!(drawn > 50, "{drawn} widgets drawn");
}

/// The plain entry points an API calls: rectangles, checks and a PNG,
/// which is refused inside a Git checkout.
#[test]
fn placed_checks_and_png_without_the_ui() {
    let (t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let st = HudState::default();
    let placed = s.placed(&st);
    assert_eq!(placed.len(), 4);
    assert!(placed.iter().all(|p| p.rect.is_some()));
    assert_eq!(s.check(&st).len(), 1);
    let png = t.0.join("hud.png");
    s.export_png(&st, &png).unwrap();
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    let inside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("hud.png");
    let e = s.export_png(&st, &inside).unwrap_err().to_string();
    assert!(e.contains("Git checkout"), "{e}");
    assert!(!inside.exists());
}

/// A generic field panel writes bytes at an address: inside one widget it
/// is an edit like any other (undo, merge by key); across or outside a
/// widget it is refused.
#[test]
fn byte_writes_inside_a_widget_are_edits() {
    let (_t, _, mut s, player, _) = open();
    s.select(player).unwrap();
    let w = s.hud().unwrap().widget(bitmap(1)).unwrap().clone();
    // The fullscreen offset x (+0x34) by bytes, twice with one merge key.
    for x in [5i16, 7] {
        let key = s.write(w.address + 0x34, &x.to_le_bytes(), "offset_full", Some(9)).unwrap();
        assert_eq!(key, bitmap(1));
    }
    assert_eq!(s.history().count(), 1);
    assert_eq!(s.hud().unwrap().widget(bitmap(1)).unwrap().offset(Mode::Full), (7, -56));
    assert!(s.write(w.address + 0x62, &[0; 4], "across", None).is_err());
    assert!(s.write(w.address.wrapping_sub(0x1000), &[0], "outside", None).is_err());
    let mirrored: Vec<_> = s.widget_bytes().filter(|(a, _)| *a == w.address).collect();
    assert_eq!(&mirrored[0].1[0x34..0x36], &7i16.to_le_bytes());
    s.undo().unwrap();
    assert!(!s.is_dirty());
}

/// The `nhdt` definition the generic field panel labels widgets from
/// (`tags/src/defs/nhdt.txt`) has the widget blocks where the layout says
/// and a typed field over every field of the tables; only the two spans
/// the schema types otherwise are left to the HUD tool's own rows.
#[test]
fn the_nhdt_definition_agrees_with_the_layout_tables() {
    use crate::layout::{self, BITMAP_FIELDS, TEXT_FIELDS};
    use tags::defs::{Defs, Kind};
    let root = Defs::builtin().group(map::FourCC(*b"nhdt")).expect("an nhdt definition");
    assert_eq!(root.size as usize, layout::ROOT_SIZE);
    let block = |at: usize| match &root.fields.iter().find(|f| f.offset as usize == at).unwrap().kind {
        Kind::Block(s) => s.clone(),
        k => panic!("{at:#x} is {k:?}"),
    };
    for (at, size, fields) in [
        (layout::ROOT_BITMAPS, layout::BITMAP_SIZE, BITMAP_FIELDS),
        (layout::ROOT_TEXTS, layout::TEXT_SIZE, TEXT_FIELDS),
    ] {
        let def = block(at);
        assert_eq!(def.size as usize, size);
        for f in fields {
            let range = crate::model::Widget::value_range(f);
            let d = def
                .fields
                .iter()
                .find(|d| d.offset as usize <= range.start && range.start < (d.offset + d.kind.width()) as usize)
                .unwrap();
            assert!(range.end <= (d.offset + d.kind.width()) as usize, "{} spans fields", f.key);
            let ok = match &d.kind {
                Kind::Unknown { .. } | Kind::Pad { .. } => false,
                Kind::RawSize { .. } => f.key == "justify",
                _ => true,
            };
            assert!(ok, "{} ({:#x}) is {} {:?}", f.key, f.offset, d.name, d.kind);
        }
    }
}

/// The registry the generic field panel draws from (schema and
/// definitions merged) names the widget fields from the nhdt definition.
#[test]
fn the_registry_names_the_widget_fields() {
    use tags::registry::{Kind, Registry};
    let root = Registry::builtin().group(map::FourCC(*b"nhdt")).unwrap();
    for (block, names) in [(0x08, [(0x1c, "anchor"), (0x34, "offset_full")]), (0x10, [(0x1c, "anchor"), (0x40, "offset_full")])] {
        let Kind::Block(child) = &root.field_at(block).unwrap().kind else {
            panic!("no block at {block:#x}");
        };
        for (at, name) in names {
            let f = child.field_at(at).unwrap();
            assert_eq!((f.name.as_str(), f.named), (name, true), "{} +{at:#x}", child.name);
        }
    }
}
