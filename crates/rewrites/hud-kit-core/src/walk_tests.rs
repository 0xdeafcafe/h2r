//! The walk over invented definitions and records: every number here is
//! worked from the guest's code (`walk`'s table), not from a capture.

use crate::Mode;
use crate::walk::*;

const FULL: Screen = Screen { window: [0, 0, 480, 640], frame: [36, 48, 444, 592], quarter: false };

fn session() -> Session {
    Session { user_state: false, engine: 1, clock: (30, 0.5), player_value: 0.75, user_value: 0.125 }
}

fn art() -> Art {
    Art { user_state: false, meter: [0.0, 0.0], shield_colour: [1.0f32.to_bits(); 3], list_colour: [0.5f32.to_bits(); 3], sprite: None }
}

/// A record with a frag grenade selected, unzoomed, full shield.
fn record() -> Vec<u8> {
    let mut r = vec![0u8; RECORD_SIZE];
    put_i16(&mut r, 0x36, 0);
    put_i16(&mut r, 0x3c, -1);
    put_f32(&mut r, 0x2c, 1.0);
    put_f32(&mut r, 0x28, 0.5);
    r
}

fn put_i16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut [u8], o: usize, v: f32) {
    put_u32(b, o, v.to_bits());
}

/// A widget of `size` bytes shown on word A's `show` and hidden on its
/// `hide`, with the four input bytes.
fn widget(size: usize, show: u16, hide: u16, inputs: [u8; 4]) -> Vec<u8> {
    let mut w = vec![0u8; size];
    w[4..8].copy_from_slice(&inputs);
    put_u16(&mut w, 0x08, show);
    put_u16(&mut w, 0x10, hide);
    w
}

/// A bitmap widget at `anchor` with a bitmap, a shader and sequence 0 in
/// every mode.
fn bitmap(anchor: u16, show: u16, hide: u16) -> Vec<u8> {
    let mut w = widget(BITMAP_SIZE, show, hide, [0x10, 0, 0, 0]);
    put_u16(&mut w, 0x1c, anchor);
    put_u32(&mut w, 0x24, 0xe100_0040);
    put_u32(&mut w, 0x2c, 0xe200_0007);
    w
}

/// One sequence of one sprite (or of images), over 32 by 16 images.
struct Tag {
    sprites: i32,
    images: i16,
    bounds: [f32; 4],
}

impl Bitmap for Tag {
    fn sequences(&self) -> Option<i32> {
        Some(1)
    }
    fn images(&self) -> Option<i32> {
        Some(4)
    }
    fn sequence(&self, index: i32) -> Option<Sequence> {
        (index == 0).then_some(Sequence { first_image: 2, image_count: self.images, sprite_count: self.sprites })
    }
    fn sprite(&self, sequence: i32, index: i32) -> Option<SpriteRecord> {
        (sequence == 0 && index < self.sprites).then_some(SpriteRecord { image: index as i16, bounds: self.bounds })
    }
    fn image(&self, _: i32) -> Option<Image> {
        Some(Image { width: 32, height: 16, texels: true })
    }
}

const WHOLE: Tag = Tag { sprites: 1, images: 0, bounds: [0.0, 1.0, 0.0, 1.0] };

fn no_effects(_: (u32, u32), _: &[f32; 4], _: &mut Basis) -> Result<(), Refusal> {
    Ok(())
}

fn draw(w: &[u8], record: &[u8], tag: &Tag, art: &Art) -> BitmapDraw {
    bitmap_widget(w, &[0.25, 0.0, 0.0, 0.0], record, &FULL, None, tag, art, &no_effects).unwrap().unwrap()
}

#[test]
fn hud_items_match_the_definition_order() {
    let record = record();
    // Word A: bit 0 always, 0x04 the frag grenade, 0x08 the plasma.
    let screens = [widget(SCREEN_SIZE, 1, 0, [1, 0, 0, 0]), widget(SCREEN_SIZE, 0x08, 0, [0; 4])].concat();
    let bitmaps = [bitmap(2, 0x04, 0), bitmap(2, 0x08, 0), bitmap(1, 1, 0)].concat();
    let texts = [widget(TEXT_SIZE, 1, 0x04, [0; 4]), widget(TEXT_SIZE, 1, 0, [0x11, 2, 3, 0x42])].concat();
    let definition = Definition { screens: &screens, bitmaps: &bitmaps, texts: &texts };
    let words = state_words(&record, 0, &session()).unwrap();
    let order: Vec<(Kind, usize)> = shown(&definition, &words, &record, &session()).iter().map(|s| (s.kind, s.index)).collect();
    assert_eq!(order, [(Kind::Screen, 0), (Kind::Bitmap, 0), (Kind::Bitmap, 2), (Kind::Text, 1)]);
    // Each carries its own inputs: the shield, then the body, the clock,
    // the player's value and the user's.
    let all = shown(&definition, &words, &record, &session());
    assert_eq!(all[1].inputs, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(all[3].inputs, [0.5, 15.0, 0.75, 0.125]);
    // The plasma grenade swaps the grenade widgets and brings the text back.
    let mut plasma = record.clone();
    put_i16(&mut plasma, 0x36, 1);
    let words = state_words(&plasma, 0, &session()).unwrap();
    let order: Vec<(Kind, usize)> = shown(&definition, &words, &plasma, &session()).iter().map(|s| (s.kind, s.index)).collect();
    assert_eq!(order, [(Kind::Screen, 0), (Kind::Screen, 1), (Kind::Bitmap, 1), (Kind::Bitmap, 2), (Kind::Text, 0), (Kind::Text, 1)]);
}

#[test]
fn the_scope_mask_draws_at_zoom() {
    // The retail masks: shown always, hidden while unzoomed (word A 0x40).
    let mut mask = bitmap(4, 1, 0x40);
    // Mirrored across and down, and stretched past the art.
    put_u16(&mut mask, 0x1e, 4 | 8 | 0x10);
    put_f32(&mut mask, 0x40, 1.0);
    put_f32(&mut mask, 0x44, 1.0);
    let mut effect = widget(SCREEN_SIZE, 1, 0x40, [1, 0, 0, 0]);
    put_u16(&mut effect, 0x1c, 4);
    put_u32(&mut effect, 0x24, u32::MAX);
    for mode in 0..3 {
        put_u32(&mut effect, 0x2c + 8 * mode, 0xe300_0001);
    }
    let definition = Definition { screens: &effect, bitmaps: &mask, texts: &[] };
    let mut record = record();
    let words = state_words(&record, 0, &session()).unwrap();
    assert!(shown(&definition, &words, &record, &session()).is_empty(), "unzoomed");

    put_i16(&mut record, 0x3c, 0);
    let words = state_words(&record, 0, &session()).unwrap();
    let all = shown(&definition, &words, &record, &session());
    assert_eq!(all.iter().map(|s| s.kind).collect::<Vec<_>>(), [Kind::Screen, Kind::Bitmap]);

    // The mask's quarter ends at the centre (320, 240) and reaches ten
    // sprites up and left; its three copies mirror it about the centre.
    let d = draw(&mask, &record, &WHOLE, &art());
    assert_eq!(d.point, [320.0, 240.0]);
    assert_eq!(d.quads.len(), 4);
    assert_eq!(d.quads[0].corners, [[-32.0, 64.0], [320.0, 64.0], [320.0, 240.0], [-32.0, 240.0]]);
    assert_eq!(d.quads[0].bounds, [-10.0, 1.0, -10.0, 1.0]);
    assert_eq!(d.quads[1].corners, [[672.0, 64.0], [320.0, 64.0], [320.0, 240.0], [672.0, 240.0]]);
    assert_eq!(d.quads[2].corners, [[-32.0, 416.0], [320.0, 416.0], [320.0, 240.0], [-32.0, 240.0]]);
    assert_eq!(d.quads[3].corners, [[672.0, 416.0], [320.0, 416.0], [320.0, 240.0], [672.0, 240.0]]);

    // The screen effect's grid spans the window through the centre.
    let s = screen_widget(&effect, &all[0].inputs, &FULL, None, None, false).unwrap().unwrap();
    assert_eq!((s.effect, s.image, s.values), (0xe300_0001, -1, [1.0, 0.0, 0.0, 0.0]));
    assert_eq!(s.grid[0], [0.0, 0.0, 0.75, 0.75]);
    assert_eq!(s.grid[4], [320.0, 240.0, 1.0, 1.0]);
    assert_eq!(s.grid[8], [640.0, 480.0, 0.75, 0.75]);
    assert_eq!(s.grid[5], [640.0, 240.0, 0.75, 1.0]);
}

#[test]
fn the_state_words_follow_the_record() {
    let mut r = record();
    let w = state_words(&r, 0, &session()).unwrap();
    // The frag grenade, unzoomed; no unit flags; no weapon; not a team game.
    assert_eq!((w.weapon, w.a, w.b, w.c), (None, 0x1 | 0x4 | 0x40, 0, 0));
    assert_eq!(w.d, 0x1 | 0x20 | 0x80 | 0x200);
    r[0x4c] = 0x08 | 0x02;
    r[0x131] = 1;
    r[0x132] = 1;
    put_i16(&mut r, 0x3c, 1);
    let team = Session { engine: 2, user_state: true, ..session() };
    let w = state_words(&r, 2, &team).unwrap();
    assert_eq!(w.a, 0x1 | 0x4 | 0x100 | 0x2000);
    assert_eq!(w.b, 0x2 | 0x10);
    assert_eq!((w.weapon, w.c), (Some(0xc0), 0x2));
    assert_eq!(w.d & 0xf, 0x2 | 0x8);
    // A weapon out of rounds with rounds to load, or fully heated, is 0x80.
    put_i16(&mut r, 0xc0 + 0x0a, 30);
    put_i16(&mut r, 0xc0 + 0x0c, 60);
    assert_eq!(state_words(&r, 2, &team).unwrap().c, 0x2 | 0x80);
    put_i16(&mut r, 0xc0 + 0x06, 4);
    assert_eq!(state_words(&r, 2, &team).unwrap().c, 0x2);
    put_f32(&mut r, 0xc0 + 0x10, 1.0);
    assert_eq!(state_words(&r, 2, &team).unwrap().c, 0x2 | 0x80);
    assert!(state_words(&r[..0x100], 0, &team).is_none(), "a short record");
}

#[test]
fn the_gate_sets_word_cs_threshold_bits_from_the_weapon() {
    let mut r = record();
    put_f32(&mut r, 0x88 + 0x10, 20.0);
    put_i16(&mut r, 0x88 + 0x06, 5);
    put_i16(&mut r, 0x88 + 0x08, 40);
    let words = state_words(&r, 1, &session()).unwrap();
    // Shown on word C's 0x10: fewer rounds loaded than the threshold byte.
    let mut state = [0u8; 20];
    put_u16(&mut state, 4, 0x10);
    state[0x11] = 6;
    assert!(gate(&state, &words, &r));
    state[0x11] = 5;
    assert!(!gate(&state, &words, &r));
    // 0x08: the threshold above the weapon's value; 0x20: the reserve.
    put_u16(&mut state, 4, 0x08);
    state[0x10] = 21;
    assert!(gate(&state, &words, &r));
    put_u16(&mut state, 4, 0x20);
    state[0x12] = 40;
    assert!(!gate(&state, &words, &r));
    // A hide word wins over a show word.
    put_u16(&mut state, 0, 1);
    assert!(gate(&state, &words, &r));
    put_u16(&mut state, 8, 0x4);
    assert!(!gate(&state, &words, &r));
}

#[test]
fn an_input_reads_its_source_and_an_unknown_one_is_zero() {
    let mut r = record();
    put_i16(&mut r, 0x38, 3);
    put_i16(&mut r, 0x3a, 1);
    put_f32(&mut r, 0x44, 0.25);
    put_f32(&mut r, 0x1cc, 7.0);
    let s = session();
    assert_eq!(inputs(&[0, 1, 2, 3], &r, None, &s), [0.0, 1.0, 15.0, 0.75]);
    assert_eq!(inputs(&[0x14, 0x15, 0x18, 0x40], &r, None, &s), [3.0, 1.0, 0.75, 7.0]);
    assert_eq!(inputs(&[0x13, 0x12, 0x16, 0x05], &r, None, &s), [1.0, 0.0, 0.0, 0.0]);
    // The shield under a fifth flashes on the clock.
    put_f32(&mut r, 0x2c, 0.1);
    assert_eq!(inputs(&[0x16, 0x10, 0x11, 0x17], &r, None, &s), [15.0, 0.1, 0.5, 0.0]);
    // The weapon's inputs are zero with no weapon record.
    assert_eq!(inputs(&[0x30, 0x36, 0x38, 0x32], &r, None, &s), [0.0; 4]);
    let w = 0x88;
    put_i16(&mut r, w + 6, 9);
    put_i16(&mut r, w + 0x0a, 36);
    put_f32(&mut r, w + 0x10, 0.25);
    assert_eq!(inputs(&[0x30, 0x36, 0x38, 0x32], &r, Some(w), &s), [9.0, 0.25, 0.75, 75.0]);
    r[w + 0x20] = 1;
    assert_eq!(inputs(&[0x35, 0x37, 0x33, 0x39], &r, Some(w), &s), [1.0, 15.0, 0.0, 0.0]);
}

#[test]
fn the_screen_mode_follows_the_window() {
    let window = |w: i16, h: i16, quarter| Screen { window: [0, 0, h, w], frame: [0; 4], quarter };
    assert_eq!(mode(2, &window(640, 480, false)), Mode::Full);
    assert_eq!(mode(2, &window(640, 240, false)), Mode::Half);
    assert_eq!(mode(2, &window(320, 480, false)), Mode::Half);
    assert_eq!(mode(2, &window(320, 240, false)), Mode::Quarter);
    // A half view is a quarter for the bottom-right anchor alone.
    assert_eq!(mode(3, &window(640, 240, true)), Mode::Quarter);
    assert_eq!(mode(2, &window(640, 240, true)), Mode::Half);
}

#[test]
fn an_anchor_is_a_corner_of_the_frame_or_its_centre() {
    assert_eq!(anchor_point(0, &FULL, None), Ok([592.0, 36.0]));
    assert_eq!(anchor_point(1, &FULL, None), Ok([48.0, 36.0]));
    assert_eq!(anchor_point(2, &FULL, None), Ok([48.0, 444.0]));
    assert_eq!(anchor_point(3, &FULL, None), Ok([592.0, 444.0]));
    assert_eq!(anchor_point(4, &FULL, None), Ok([320.0, 240.0]));
    // The aim point replaces the centre's y, rounded to a pixel.
    assert_eq!(anchor_point(4, &FULL, Some(251.5)), Ok([320.0, 252.0]));
    assert_eq!(anchor_point(5, &FULL, None), Err(Refusal::Anchor));
}

#[test]
fn a_sprite_is_its_sequences_first_sprite_or_image() {
    let sprites = Tag { sprites: 2, images: 0, bounds: [0.25, 0.75, 0.0, 0.5] };
    let s = sprite(&sprites, 0, false).unwrap().unwrap();
    assert_eq!((s.image, s.width, s.height, s.bounds), (0, 32, 16, [0.25, 0.75, 0.0, 0.5]));
    // The second sprite for a user in the second state, when there is one.
    assert_eq!(sprite(&sprites, 0, true).unwrap().unwrap().image, 1);
    assert_eq!(sprite(&WHOLE, 0, true).unwrap().unwrap().image, 0);
    // A sequence of images draws its first image whole.
    let images = Tag { sprites: 0, images: 2, bounds: [0.0; 4] };
    let s = sprite(&images, 0, false).unwrap().unwrap();
    assert_eq!((s.image, s.bounds), (2, [0.0, 1.0, 0.0, 1.0]));
    assert_eq!(sprite(&images, 0, true).unwrap().unwrap().image, 3);
    // No sequence, an empty one, a negative one: nothing.
    assert_eq!(sprite(&images, 1, false), Ok(None));
    assert_eq!(sprite(&images, -1, false), Ok(None));
    assert_eq!(sprite(&Tag { sprites: 0, images: 0, bounds: [0.0; 4] }, 0, false), Ok(None));
}

#[test]
fn a_bitmap_lands_its_registration_on_the_anchor_plus_its_offset() {
    let mut w = bitmap(2, 1, 0);
    put_i16(&mut w, 0x34, 10);
    put_i16(&mut w, 0x36, -20);
    put_f32(&mut w, 0x40, 0.25);
    put_f32(&mut w, 0x44, 1.0);
    let d = draw(&w, &record(), &WHOLE, &art());
    assert_eq!(d.point, [48.0, 444.0]);
    // 32 by 16 with a quarter of it left of (58, 424) and all of it above.
    assert_eq!(d.quads.len(), 1);
    assert_eq!(d.quads[0].corners, [[50.0, 408.0], [82.0, 408.0], [82.0, 424.0], [50.0, 424.0]]);
    assert_eq!(d.quads[0].bounds, [0.0, 1.0, 0.0, 1.0]);
    // The inputs go to the shader inputs before the quad.
    assert_eq!(d.quads[0].writes, [(global::INPUTS, 0.25f32.to_bits()), (global::INPUTS + 4, 0), (global::INPUTS + 8, 0), (global::INPUTS + 12, 0)]);
    assert!(d.writes.is_empty());
    assert_eq!((d.bitmap, d.shader, d.anchor), (0xe100_0040, 0xe200_0007, 2));
    // The mirror flags swap the image's edges, not the corners.
    put_u16(&mut w, 0x1e, 1 | 2);
    let m = draw(&w, &record(), &WHOLE, &art());
    assert_eq!(m.quads[0].corners, d.quads[0].corners);
    assert_eq!(m.quads[0].bounds, [1.0, 0.0, 1.0, 0.0]);
}

#[test]
fn a_widget_with_no_bitmap_shader_or_image_draws_nothing() {
    let r = record();
    let mut w = bitmap(2, 1, 0);
    put_u32(&mut w, 0x2c, u32::MAX);
    assert_eq!(bitmap_widget(&w, &[0.0; 4], &r, &FULL, None, &WHOLE, &art(), &no_effects), Ok(None));
    let mut w = bitmap(2, 1, 0);
    w[0x30] = 0xff;
    assert_eq!(bitmap_widget(&w, &[0.0; 4], &r, &FULL, None, &WHOLE, &art(), &no_effects), Ok(None));
}

#[test]
fn the_effects_change_the_basis_the_corners_are_laid_on() {
    let w = bitmap(1, 1, 0);
    let half = |block: (u32, u32), inputs: &[f32; 4], basis: &mut Basis| {
        assert_eq!((block, inputs[0]), ((0, 0), 0.25));
        basis.x = [0.5, 0.0];
        basis.origin[1] += 4.0;
        Ok(())
    };
    let d = bitmap_widget(&w, &[0.25, 0.0, 0.0, 0.0], &record(), &FULL, None, &WHOLE, &art(), &half).unwrap().unwrap();
    assert_eq!(d.quads[0].corners, [[48.0, 40.0], [64.0, 40.0], [64.0, 56.0], [48.0, 56.0]]);
    let refuse = |_: (u32, u32), _: &[f32; 4], _: &mut Basis| Err(Refusal::Effect);
    assert_eq!(bitmap_widget(&w, &[0.0; 4], &record(), &FULL, None, &WHOLE, &art(), &refuse), Err(Refusal::Effect));
}

#[test]
fn the_shield_meter_draws_a_layer_for_each_bar_that_holds_charge() {
    let mut w = bitmap(2, 1, 0);
    put_u16(&mut w, 0x60, 5);
    // One and a half bars of shield, the shown level still at two.
    let mut r = record();
    put_f32(&mut r, 0x2c, 1.5);
    let falling = Art { meter: [2.0, 0.25], ..art() };
    let d = draw(&w, &r, &WHOLE, &falling);
    assert_eq!(d.quads.len(), 2);
    let value = |q: &Quad, at: u32| q.writes.iter().rev().find(|w| w.0 == at).map(|w| f32::from_bits(w.1));
    // The first bar is full on both, so it is not falling: no flash.
    assert_eq!((value(&d.quads[0], global::INPUTS), value(&d.quads[0], global::INPUTS + 4)), (Some(1.0), Some(1.0)));
    assert_eq!(value(&d.quads[0], global::COLOUR_A), Some(0.0));
    // The second holds half and shows whole: it flashes at a quarter.
    assert_eq!((value(&d.quads[1], global::INPUTS), value(&d.quads[1], global::INPUTS + 4)), (Some(1.0), Some(0.5)));
    assert_eq!(value(&d.quads[1], global::COLOUR_A), Some(0.75));
    // The first layer's colours are the user's, the next the layer's own.
    assert_eq!(value(&d.quads[0], global::COLOUR_B + 8), Some(170.0 * f32::from_bits(0x3b80_8081)));
    assert_eq!(value(&d.quads[1], global::COLOUR_B), Some(255.0 * f32::from_bits(0x3b80_8081)));
    assert_eq!(value(&d.quads[1], global::COLOUR_C + 4), Some(0.0));
    // An empty shield draws no layer, and its inputs are still written.
    put_f32(&mut r, 0x2c, 0.0);
    let d = draw(&w, &r, &WHOLE, &art());
    assert!(d.quads.is_empty());
    assert_eq!(d.writes[0], (global::INPUTS, 0.25f32.to_bits()));
}

#[test]
fn the_list_type_draws_one_quad_for_each_entry_about_its_point() {
    let mut w = bitmap(1, 1, 0);
    put_u16(&mut w, 0x60, 7);
    let mut r = record();
    put_u32(&mut r, 0x22c, 3);
    for i in 0..3 {
        put_u32(&mut r, 0x230 + 4 * i, 0x00ff_0000 >> (8 * i));
        put_f32(&mut r, 0x250 + 4 * i, i as f32);
    }
    let d = draw(&w, &r, &WHOLE, &art());
    // Three sprites 33 apart, the middle one where a lone one would be.
    assert_eq!(d.quads.iter().map(|q| q.corners[0][0]).collect::<Vec<_>>(), [15.0, 48.0, 81.0]);
    assert_eq!(d.quads[2].writes, [
        (global::COLOUR_A, 0),
        (global::COLOUR_A + 4, 0),
        (global::COLOUR_A + 8, (255.0 * f32::from_bits(0x3b80_8081)).to_bits()),
        (global::INPUTS, 2.0f32.to_bits()),
    ]);
    assert!(d.quads[0].writes.contains(&(global::COLOUR_B, 0.5f32.to_bits())));
    put_u32(&mut r, 0x22c, 0);
    assert!(draw(&w, &r, &WHOLE, &art()).quads.is_empty());
}

#[test]
fn an_emblem_widget_and_a_projected_anchor_are_refused() {
    let mut w = bitmap(2, 1, 0);
    put_u16(&mut w, 0x60, 1);
    assert_eq!(bitmap_widget(&w, &[0.0; 4], &record(), &FULL, None, &WHOLE, &art(), &no_effects), Err(Refusal::Emblem));
    let w = bitmap(6, 1, 0);
    assert_eq!(bitmap_widget(&w, &[0.0; 4], &record(), &FULL, None, &WHOLE, &art(), &no_effects), Err(Refusal::Anchor));
    assert_eq!(Refusal::Emblem.cause(), "emblem");
}

#[test]
fn the_team_colour_types_write_the_records_colour_twice() {
    let mut w = bitmap(2, 1, 0);
    put_u16(&mut w, 0x60, 4);
    let mut r = record();
    put_f32(&mut r, 0x1d8, 0.5);
    let d = draw(&w, &r, &WHOLE, &art());
    assert_eq!(d.quads.len(), 1);
    assert!(d.quads[0].writes.contains(&(global::COLOUR_A, 0.5f32.to_bits())));
    assert!(d.quads[0].writes.contains(&(global::COLOUR_B, 0.5f32.to_bits())));
}

#[test]
fn a_quads_vertices_take_texel_coordinates_when_the_image_asks() {
    let corners = [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]];
    let texels = Image { width: 32, height: 16, texels: true };
    let v = vertices(&corners, &[0.25, 0.5, 0.0, 1.0], &texels);
    assert_eq!(v[0], [1.0, 2.0, QUAD_DEPTH, QUAD_DEPTH, 8.0, 0.0]);
    assert_eq!(v[2], [3.0, 4.0, QUAD_DEPTH, QUAD_DEPTH, 16.0, 16.0]);
    let unit = Image { texels: false, ..texels };
    assert_eq!(vertices(&corners, &[0.25, 0.5, 0.0, 1.0], &unit)[3][4..], [0.25, 1.0]);
}

#[test]
fn a_text_widget_is_placed_at_its_anchor_plus_its_offset() {
    let mut w = widget(TEXT_SIZE, 1, 0, [0; 4]);
    put_u16(&mut w, 0x1c, 3);
    put_u32(&mut w, 0x24, 0xe200_0009);
    put_u32(&mut w, 0x28, 0x0500_0abc);
    put_u16(&mut w, 0x2c, 2);
    w[0x30] = 1;
    put_i16(&mut w, 0x40, -12);
    put_i16(&mut w, 0x42, 6);
    let t = text_widget(&w, &[0.0; 4], &FULL, None, &no_effects).unwrap().unwrap();
    assert_eq!((t.font, t.justification, t.string), (Some(8), 2, 0x0500_0abc));
    assert_eq!(t.basis.origin, [580.0, 450.0]);
    put_u32(&mut w, 0x28, 0);
    assert_eq!(text_widget(&w, &[0.0; 4], &FULL, None, &no_effects), Ok(None));
}

#[test]
fn a_number_is_held_and_padded_to_its_digits() {
    assert_eq!(number_text(TEXT_NUMBER | TEXT_TWO_DIGITS, 7), "07");
    assert_eq!(number_text(TEXT_NUMBER | TEXT_TWO_DIGITS, 123), "99");
    assert_eq!(number_text(TEXT_NUMBER | TEXT_TWO_DIGITS, -4), "00");
    assert_eq!(number_text(TEXT_NUMBER | TEXT_THREE_DIGITS, 7), "007");
    assert_eq!(number_text(TEXT_NUMBER | TEXT_THREE_DIGITS, 42), "042");
    assert_eq!(number_text(TEXT_NUMBER | TEXT_THREE_DIGITS, 4200), "999");
    assert_eq!(number_text(TEXT_NUMBER, -12), "-12");
    // Two digits win over three.
    assert_eq!(number_text(TEXT_NUMBER | TEXT_TWO_DIGITS | TEXT_THREE_DIGITS, 5), "05");
}

#[test]
fn the_measure_is_the_glyphs_bounds_one_past_their_far_edges() {
    assert_eq!(text_bounds(&[]), None);
    let glyphs = [[2.0, 5.0, 9.0, 14.0], [11.0, 3.0, 18.0, 14.0]];
    assert_eq!(text_bounds(&glyphs), Some([3, 2, 15, 19]));
}

#[test]
fn the_strings_rectangle_starts_centres_or_ends_at_the_origin() {
    let window = [10, 20, 490, 660];
    let bounds = [3, 2, 15, 19];
    // 17 wide and 12 high, at 100, 200 of the window.
    let origin = [120.5, 210.75];
    assert_eq!(text_rect(0, origin, &window, &bounds), [200, 100, 212, 117]);
    assert_eq!(text_rect(1, origin, &window, &bounds), [200, 92, 212, 109]);
    assert_eq!(text_rect(2, origin, &window, &bounds), [200, 83, 212, 100]);
    assert_eq!(text_rect(3, origin, &window, &bounds), [0, 0, 1000, 1000]);
}

#[test]
fn the_shadow_takes_the_lesser_alpha_and_is_left_out_at_none() {
    let main = [0.5, 1.0, 0.25, 0.0];
    assert_eq!(text_colours(&main, None), Some((0x7fff_3f00, None)));
    assert_eq!(text_colours(&main, Some(&[1.0, 0.0, 0.0, 0.0])), Some((0x7fff_3f00, Some(0x7f00_0000))));
    assert_eq!(text_colours(&main, Some(&[0.0, 1.0, 1.0, 1.0])), Some((0x7fff_3f00, None)));
    // A component outside 0..1 is not a colour the draw takes.
    assert_eq!(text_colours(&[1.5, 0.0, 0.0, 0.0], None), None);
    assert_eq!(text_colours(&[f32::NAN, 0.0, 0.0, 0.0], None), None);
}

#[test]
fn each_glyphs_shadow_is_drawn_before_it_one_pixel_right_and_down() {
    let glyphs = [(7, [10.0, 20.0, 15.0, 30.0]), (9, [16.0, 22.0, 20.0, 30.0])];
    let quads = text_quads(&glyphs, &[], 0xffff_ffff, Some(0x8000_0000));
    assert_eq!(quads.len(), 4);
    assert_eq!(quads[0], GlyphQuad { index: 7, rect: [11.0, 21.0, 16.0, 31.0], colour: 0x8000_0000 });
    assert_eq!(quads[1], GlyphQuad { index: 7, rect: [10.0, 20.0, 15.0, 30.0], colour: 0xffff_ffff });
    assert_eq!(quads[3].index, 9);
    assert_eq!(text_quads(&glyphs, &[], 0xffff_ffff, None).len(), 2);
}

#[test]
fn a_coloured_buttons_glyph_is_white_at_the_strings_alpha_and_its_shadow_is_the_strings() {
    // The coloured buttons, not the sticks and triggers after them, nor the smileys.
    for unit in [0xe100, 0xe105, 0xe003, 0xe009, 0xe132, 0xe3ff] {
        assert!(untinted(unit), "{unit:#x}");
    }
    for unit in [0x41, 0xe000, 0xe002, 0xe004, 0xe008, 0xe106, 0xe12b, 0xe131, 0xe400, 0xe446] {
        assert!(!untinted(unit), "{unit:#x}");
    }
    let glyphs = [(7, [10.0, 20.0, 15.0, 30.0]), (9, [16.0, 22.0, 20.0, 30.0])];
    let quads = text_quads(&glyphs, &[9], 0x8040_c0ff, Some(0x8000_0000));
    assert_eq!(quads.iter().map(|q| q.colour).collect::<Vec<_>>(), [0x8000_0000, 0x8040_c0ff, 0x8000_0000, 0x80ff_ffff]);
}

#[test]
fn a_text_widget_leaves_its_inputs_and_the_huds_colour() {
    let writes = text_globals(&[1.0, 0.5, 0.0, 0.25], 0x00ff_0080);
    assert_eq!(writes.len(), 7);
    assert_eq!(writes[1], (global::INPUTS + 4, 0.5f32.to_bits()));
    assert_eq!(writes[4], (TEXT_COLOUR, 1.0f32.to_bits()));
    assert_eq!(writes[5], (TEXT_COLOUR + 4, 0.0f32.to_bits()));
}

#[test]
fn the_definitions_walk_in_the_records_order() {
    let mut record = record();
    for (word, datum) in [0xe000_0001u32, 0xe000_0002, 0xe000_0003, u32::MAX, 0xe000_0005, 0xe000_0006, u32::MAX].into_iter().enumerate() {
        put_u32(&mut record, 4 * word, datum);
    }
    // Word 5 twice with slot 3, the weapons' words 2 and 4, then words 0
    // and 1 with slot 0; words 3 and 6 name nothing.
    let order: Vec<(usize, u32, u32)> = definitions(&record).unwrap().iter().map(|w| (w.step, w.datum, w.slot)).collect();
    assert_eq!(
        order,
        [(0, 0xe000_0006, 3), (1, 0xe000_0003, 4), (3, 0xe000_0005, 2), (4, 0xe000_0006, 3), (6, 0xe000_0001, 0), (7, 0xe000_0002, 0)]
    );
    assert!(definitions(&record[..20]).is_none());
}
