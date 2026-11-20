//! Two versions of the same diptych. Each game's art stays on its side
//! as focus changes. The selected tile gets more space and full colour.
use super::{Card, cover};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tags::rgba_bitmap::Template;

pub const CE: &str = "@picker/halo1";
pub const TWO: &str = "@picker/halo2";
const CAIRO: &str = r"ui\code_global_bitmaps\campaign2";

fn tiles(ce: &Card, disc: &Path) -> Result<[Card; 2]> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc)?))?;
    let menu = iso
        .find("/maps/mainmenu.map")
        .context("Halo 2 disc has no /maps/mainmenu.map")?;
    let mut map = map::Map::from_region(
        BufReader::new(File::open(disc)?),
        iso.partition_offset() + u64::from(menu.sector) * xiso::SECTOR,
        menu.size.into(),
    )?;
    let id = map
        .tag(map::TagQuery::NameInGroup(CAIRO, map::FourCC(*b"bitm")))?
        .id;
    let bitmap = Template::read(&mut map, id)?;
    let (w, h) = bitmap.size();
    let pixels = bitmap.rgba(&mut map)?;
    // Trim the baked rim and the cut corner before composing. The VFS
    // gives the completed diptych the same outer frame as a retail card.
    anyhow::ensure!(
        w > 32 && h > 32,
        "{CAIRO} is too small for its retail frame"
    );
    let interior: Vec<u8> = pixels
        .chunks_exact(usize::from(w) * 4)
        .skip(16)
        .take(usize::from(h - 32))
        .flat_map(|row| row[16 * 4..row.len() - 16 * 4].iter().copied())
        .collect();
    Ok(compose(
        ce,
        (u32::from(w - 32), u32::from(h - 32), &interior),
    ))
}

fn compose(ce: &Card, two: (u32, u32, &[u8])) -> [Card; 2] {
    let (w, h) = (220u32, 207u32);
    [true, false].map(|focus_ce| {
        let split = if focus_ce { 145 } else { 75 };
        let left = cover(ce.width, ce.height, &ce.rgba, split + 14, h);
        let right = cover(two.0, two.1, two.2, w - split + 14, h);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let edge = split + 14 - 28 * y / (h - 1);
            for x in 0..w {
                let on_left = x < edge;
                let (src, i) = if on_left {
                    (&left, y * (split + 14) + x)
                } else {
                    (&right, y * (w - split + 14) + x - (split - 14))
                };
                for &v in &src[i as usize * 4..i as usize * 4 + 3] {
                    rgba.push(if focus_ce == on_left {
                        v
                    } else {
                        (u16::from(v) * 2 / 3) as u8
                    });
                }
                rgba.push(255);
            }
        }
        let key = if focus_ce { "halo1" } else { "halo2" };
        Card {
            map: if focus_ce { CE } else { TWO }.into(),
            name: super::tag_name("game_picker", key),
            width: w,
            height: h,
            rgba,
        }
    })
}

/// The two tiles from the Halo 1 import's first card in play order and
/// Cairo Station on the Halo 2 disc. The Halo 1 picture comes only from
/// the import (`h2 import halo1` writes the cards): the game reads no
/// Halo 1 disc.
pub fn prepare(data: &Path, disc: &Path, cards: &mut Vec<Card>) -> Result<()> {
    // An import can contain only later missions.
    let ce = h1_contract::campaign::CAMPAIGN_ORDER
        .iter()
        .find_map(|level| {
            let name = super::tag_name("halo1", level);
            cards.iter().find(|c| c.name == name)
        })
        .context("the Halo 1 import has no level cards (run `h2 import halo1` again)")?;
    let pair = tiles(ce, disc)?;
    for (key, tile) in ["halo1", "halo2"].into_iter().zip(&pair) {
        let path = data
            .join("campaigns/game-picker")
            .join(format!("{key}.png"));
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, tile.width, tile.height, &tile.rgba)?;
        if std::fs::read(&path).ok().as_ref() != Some(&png) {
            content::store::write_atomic(&path, &png)?;
        }
    }
    cards.extend(pair);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_moves_the_seam_but_keeps_both_games_on_their_side() {
        let ce = Card {
            map: "a10".into(),
            name: String::new(),
            width: 1,
            height: 1,
            rgba: vec![240, 0, 0, 255],
        };
        let [a, b] = compose(&ce, (1, 1, &[0, 0, 240, 255]));
        let pixel = |c: &Card, x: usize| -> [u8; 4] {
            let at = (103 * 220 + x) * 4;
            c.rgba[at..at + 4].try_into().unwrap()
        };
        assert_eq!((a.map.as_str(), b.map.as_str()), (CE, TWO));
        assert_eq!(pixel(&a, 0), [240, 0, 0, 255]);
        assert_eq!(pixel(&a, 219), [0, 0, 160, 255]);
        assert_eq!(pixel(&b, 0), [160, 0, 0, 255]);
        assert_eq!(pixel(&a, 110), [240, 0, 0, 255]);
        assert_eq!(pixel(&b, 110), [0, 0, 240, 255]);
    }
    #[test]
    fn without_an_import_card_the_picker_names_the_import_and_reads_no_disc() {
        let missing = Path::new("/nonexistent/h2-picker/halo2.iso");
        let other = Card {
            map: "x10".into(),
            name: super::super::tag_name("mine", "x10"),
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let mut cards = vec![other];
        let e = prepare(Path::new("/nonexistent/h2-picker"), missing, &mut cards).unwrap_err();
        assert!(format!("{e:#}").contains("h2 import halo1"), "{e:#}");
        assert_eq!(cards.len(), 1);
    }
    #[test]
    #[ignore = "reads the owner's Halo 2 disc"]
    fn picker_collage_from_an_import_card_and_the_disc() {
        let disc = std::env::var_os("H2_DISC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
            });
        // An invented Halo 1 card, as the import writes one: a gradient
        // the size of Halo 1's level picture.
        let (w, h) = (139u32, 113u32);
        let rgba = (0..w * h)
            .flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 96, 255])
            .collect();
        let mut cards = vec![Card {
            map: "a30".into(),
            name: super::super::tag_name("halo1", "a30"),
            width: w,
            height: h,
            rgba,
        }];
        let serial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data = std::env::temp_dir().join(format!("h2-picker-{}-{serial}", std::process::id()));
        prepare(&data, &disc, &mut cards).unwrap();
        assert_eq!(cards.len(), 3);
        let pair = &cards[1..];
        for (i, name) in ["halo1", "halo2"].iter().enumerate() {
            let (w, h, rgba) =
                super::super::read_png(&data.join(format!("campaigns/game-picker/{name}.png")))
                    .unwrap();
            assert_eq!((w, h), (220, 207));
            assert_eq!(rgba, pair[i].rgba);
        }
        assert_eq!((pair[0].map.as_str(), pair[1].map.as_str()), (CE, TWO));
        assert_ne!(pair[0].rgba, pair[1].rgba);
        std::fs::remove_dir_all(data).unwrap();
    }
}
