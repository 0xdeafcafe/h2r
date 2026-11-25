use super::*;

/// An invented bitmap payload at `address`: header, one sequence with one
/// sprite, two images, a padding word of zeros between the sprites and
/// the images.
fn payload(address: u32) -> Vec<u8> {
    let seq_at = HEADER_LEN;
    let sprite_at = seq_at + SEQUENCE_LEN;
    let images_at = sprite_at + SPRITE_LEN + 4;
    let mut b = vec![0u8; images_at + 2 * IMAGE_LEN];
    b[0] = 7; // kind
    put(&mut b, at::SEQUENCES, 1);
    put(&mut b, at::SEQUENCES + 4, address + seq_at as u32);
    put(&mut b, at::IMAGES, 2);
    put(&mut b, at::IMAGES + 4, address + images_at as u32);
    b[seq_at..seq_at + 5].copy_from_slice(b"icons");
    b[seq_at + at::IMAGE_COUNT] = 2;
    put(&mut b, seq_at + at::SPRITES, 1);
    put(&mut b, seq_at + at::SPRITES + 4, address + sprite_at as u32);
    b[sprite_at + 8..sprite_at + 12].copy_from_slice(&1.0f32.to_le_bytes());
    for i in 0..2 {
        let r = images_at + i * IMAGE_LEN;
        b[r..r + 4].copy_from_slice(b"mtib");
        b[r + 4] = 40 + i as u8;
        b[r + 0x70] = 0xa0 + i as u8;
    }
    b
}

#[test]
fn split_then_layout_at_the_same_address_keeps_every_record() {
    let at_ = 0x9000_1000;
    let src = payload(at_);
    let parts = Parts::split(&src, at_).unwrap();
    assert_eq!(parts.images.len(), 2);
    assert_eq!(parts.sequences.len(), 1);
    assert_eq!(parts.sequences[0].label(), "icons");
    assert_eq!(parts.sequences[0].image_count(), 2);
    assert_eq!(parts.sequences[0].sprites.len(), 1);
    // Laid out again it is the same records in a new order, with no
    // padding; split at its new address it gives the same parts.
    let layout = parts.layout();
    assert_eq!(layout.bytes.len(), src.len() - 4);
    assert_eq!(layout.images_at as usize, HEADER_LEN);
    let moved = layout.at(0x9000_8000);
    let again = Parts::split(&moved, 0x9000_8000).unwrap();
    assert_eq!(again.images, parts.images);
    assert_eq!(again.sequences, parts.sequences);
    // The header differs only in its block addresses.
    for i in 0..HEADER_LEN {
        let pointer = [at::SEQUENCES + 4, at::IMAGES + 4]
            .iter()
            .any(|p| (*p..p + 4).contains(&i));
        if !pointer {
            assert_eq!(again.header[i], parts.header[i], "header byte {i:#x}");
        }
    }
}

#[test]
fn an_added_image_and_sequence_lay_out_after_the_old_ones() {
    let at_ = 0x9000_1000;
    let mut parts = Parts::split(&payload(at_), at_).unwrap();
    let new = parts.images[1].clone();
    parts.images.push(new);
    let like = parts.sequences[0].clone();
    parts.sequences.push(sequence(Some(&like), "new icon", 2, 1));
    let layout = parts.layout();
    let bytes = layout.at(0x9000_8000);
    let again = Parts::split(&bytes, 0x9000_8000).unwrap();
    assert_eq!(again.images.len(), 3);
    assert_eq!(&again.images[..2], &parts.images[..2]);
    assert_eq!(again.sequences.len(), 2);
    assert_eq!(again.sequences[0], like);
    let s = &again.sequences[1];
    assert_eq!((s.label(), s.first_image(), s.image_count()), ("new icon".into(), 2, 1));
    // Its sprite is the first one's with the image index set.
    assert_eq!(s.sprites.len(), 1);
    assert_eq!(i16::from_le_bytes([s.sprites[0][0], s.sprites[0][1]]), 2);
    assert_eq!(s.sprites[0][2..], like.sprites[0][2..]);
    // With nothing to copy, a sequence has no sprites.
    let bare = sequence(None, "x", 0, 1);
    assert!(bare.sprites.is_empty());
    assert_eq!(bare.entry.len(), SEQUENCE_LEN);
}

#[test]
fn a_payload_with_unknown_bytes_or_stray_blocks_is_refused() {
    let at_ = 0x9000_1000;
    // A nonzero byte in the padding no record holds.
    let mut b = payload(at_);
    let pad = HEADER_LEN + SEQUENCE_LEN + SPRITE_LEN;
    b[pad] = 1;
    let e = Parts::split(&b, at_).unwrap_err();
    assert!(e.contains("1 nonzero bytes"), "{e}");
    // A block that runs past the payload.
    let mut b = payload(at_);
    put(&mut b, at::IMAGES, 3);
    assert!(Parts::split(&b, at_).unwrap_err().contains("past the tag"));
    // A block before the tag.
    let mut b = payload(at_);
    put(&mut b, at::IMAGES + 4, at_ - 0x100);
    assert!(Parts::split(&b, at_).unwrap_err().contains("outside the tag"));
    // Two blocks over the same bytes.
    let mut b = payload(at_);
    let seq = word(&b, at::SEQUENCES + 4);
    put(&mut b, at::IMAGES + 4, seq);
    put(&mut b, at::IMAGES, 1);
    assert!(Parts::split(&b, at_).is_err());
    // Too short for a header.
    assert!(Parts::split(&[0; 8], at_).is_err());
}

#[test]
fn a_sprite_record_round_trips_its_fields_and_keeps_its_other_bytes() {
    let mut r = new_sprite(3);
    r[4] = 9; // a byte the view does not name
    let v = SpriteView::read(&r);
    assert_eq!((v.image, v.left, v.right, v.top, v.bottom), (3, 0.0, 1.0, 0.0, 1.0));
    let w = SpriteView { image: 5, left: 0.25, right: 0.5, top: 0.125, bottom: 0.75, registration: [0.5, 0.25] };
    w.write(&mut r);
    assert_eq!(SpriteView::read(&r), w);
    assert_eq!(r[4], 9);
    assert_eq!(r.len(), SPRITE_LEN);
}

#[test]
fn a_sprite_that_is_inverted_or_not_finite_is_refused() {
    let ok = SpriteView::read(&new_sprite(0));
    assert!(ok.check().is_ok());
    assert!(SpriteView { left: 0.6, right: 0.5, ..ok }.check().is_err());
    assert!(SpriteView { top: 2.0, ..ok }.check().is_err());
    assert!(SpriteView { bottom: f32::NAN, ..ok }.check().is_err());
    assert!(SpriteView { registration: [f32::INFINITY, 0.0], ..ok }.check().is_err());
}
